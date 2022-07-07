use core::task;

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use hyper::body::Buf;
use hyper::{http, service::Service, Body, Request, Response};
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::PodSpec;
use k8s_openapi::Resource;
use kube::core::ObjectMeta;
use kube::{
    api::DynamicObject,
    core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview},
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::convert::{TryFrom, TryInto};
use tracing::{debug, debug_span, trace, Instrument};

#[derive(Clone)]
pub struct Admission;

impl Service<Request<Body>> for Admission {
    type Response = Response<Body>;

    type Error = anyhow::Error;

    type Future = BoxFuture<'static, anyhow::Result<Response<Body>>>;

    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        task::Poll::Ready(Ok(()))
    }

    // TODO: if we can't deserialize from bytez to json, return bad admission
    // response (i.e denied). Also do it if review.try_into doesn't work. For
    // now, we return an err
    fn call(&mut self, req: Request<Body>) -> Self::Future {
        trace!(admission.request = ?req);
        // TODO: check methods
        let handler = self.clone();
        Box::pin(async {
            let review: AdmissionReview<DynamicObject> = {
                let body = hyper::body::aggregate(req.into_body()).await?;
                serde_json::from_reader(body.reader())?
            };

            trace!(admission.review = ?review);
            let rsp = match review.try_into() {
                Ok(req) => handler.admit(req).await.into_review(),
                Err(err) => bail!("AdmissionRequest is invalid: {}", err),
            };
            //  * quick validation: does main container match a container in the
            //  template spec? if not err out
            // JSONPATCH: add emptydir volume with init container for linkerd-await command: curl
            // binary or smth
            // JSONPATCH: add entrypoint to main container

            let bytes = serde_json::to_vec(&rsp)?;
            Ok(Response::builder()
                .status(http::StatusCode::OK)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(hyper::Body::from(bytes))
                .unwrap())
        })
    }
}

impl Admission {
    pub fn new() -> Self {
        Admission
    }

    // Admit resources:
    // * Check if DynamicObject is Job. If it is, parse spec and then mutate
    // * Else, resource is unsupported, so admit without mutating
    // * (Low Priority) emit an event whenever an admission is skipped; include
    //  err message
    //  * (Low Priority) Log why admission has been skipped
    async fn admit(self, req: AdmissionRequest<DynamicObject>) -> AdmissionResponse {
        if is_kind::<Job>(&req) {
            let resp = AdmissionResponse::from(&req);
            let (job_meta, job_spec) = match parse_spec::<JobSpec>(req) {
                Ok(v) => v,
                Err(error) => {
                    debug!(%error, "Error parsing Job spec");
                    return resp.deny(error);
                }
            };

            let job_id = match JobID::try_from(&job_meta) {
                Ok(id) => id,
                Err(error) => {
                    debug!(%error, "Failed to parse Job metadata");
                    return resp.deny(error);
                }
            };

            let comm = match extract_annotation(job_meta, "extensions.linkerd.io/sweep-comm") {
                Ok(comm) => comm,
                Err(error) => {
                    debug!(%job_id, %error, "Failed to extract annotation from Job metadata");
                    return resp;
                }
            };

            // Get pod spec, and pod meta
            let (pod_meta, pod_spec) = match parse_template_spec(job_spec) {
                Ok(v) => v,
                Err(error) => {
                    debug!(%error, "Error parsing PodTemplateSpec");
                    return resp.deny(error);
                }
            };

            let injectable = match extract_annotation(pod_meta, "linkerd.io/inject") {
                Ok(inj) => inj == "enabled",
                Err(error) => {
                    debug!(%job_id, %error, "Failed to extract annotation from Pod metadata");
                    return resp;
                }
            };

            if !injectable {
                debug!(%job_id, "Skipping mutation; pod is not injected with linkerd-proxy");
                return resp;
            }

            self.mutate::<PodSpec>(pod_spec, comm)
                .instrument(debug_span!("admission.mutate", %job_id))
                .await

            /*
                 *
            let patch = {
                let spec = pod_template.spec.unwrap();
                let patcher = MakePatch::new(sweep_name, spec)
                    .add_await_volume()
                    .add_init_container();
                //.add_await_volume();
                //.add_volume_to_container()
                patcher.build_patch()
            };
            */
            // proccess annotations
            // skip with reason or continue
            // continue? return self.mutate
        } else {
            // Unsupported resource
            // admit without mutating
            // print gvk in debug
            AdmissionResponse::from(&req)
        }
    }

    async fn mutate<T: Serialize>(self, spec: T, comm: String) -> AdmissionResponse {
        todo!();
    }
}

fn is_kind<T>(req: &AdmissionRequest<DynamicObject>) -> bool
where
    T: kube::core::Resource,
    T::DynamicType: Default,
{
    let dt = Default::default();
    *req.kind.group == *T::group(&dt) && *req.kind.kind == *T::kind(&dt)
}

// READ: https://serde.rs/lifetimes.html
// and re-read through Serde docs
// DeserializeOwned basically means T can be deserialized from any lifetime, the
// assumption being that we do away with the DynamicObject after we're done.
fn parse_spec<T>(req: AdmissionRequest<DynamicObject>) -> Result<(ObjectMeta, T)>
where
    T: DeserializeOwned,
{
    let obj = req
        .object
        .ok_or_else(|| anyhow!("AdmissionRequest missing 'object' field"))?;

    let meta = obj.metadata;
    let spec = {
        let json_spec = obj
            .data
            .get("spec")
            .cloned()
            .ok_or_else(|| anyhow!("AdmissionRequest object missing 'spec' field"))?;
        serde_json::from_value(json_spec)?
    };

    Ok((meta, spec))
}

fn parse_template_spec(spec: JobSpec) -> Result<(ObjectMeta, PodSpec)> {
    let template_spec = spec.template;

    let meta = template_spec
        .metadata
        .ok_or_else(|| anyhow!("JobSpec missing 'metadata' in PodTemplateSpec"))?;

    let pod_spec = template_spec
        .spec
        .ok_or_else(|| anyhow!("JobSpec missing 'spec' in PodTemplateSpec"))?;

    Ok((meta, pod_spec))
}

fn extract_annotation(meta: ObjectMeta, key: &str) -> Result<String> {
    let annotations = if let Some(annotations) = meta.annotations {
        annotations
    } else {
        bail!("ObjectMetadata does not contain any annotations");
    };

    let value = annotations
        .get(key)
        .map(|str| str.to_owned())
        .ok_or_else(|| anyhow!("Annotation '{}' is missing from ObjectMetadata", key));
    match value {
        Ok(v) => {
            if v.is_empty() {
                bail!("Annotation '{}' does not contain a value", key);
            }
            return Ok(v);
        }
        Err(error) => bail!(error),
    }
}

struct JobID(String, String);

impl std::fmt::Display for JobID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.0, self.1)
    }
}

impl TryFrom<&ObjectMeta> for JobID {
    type Error = anyhow::Error;

    fn try_from(value: &ObjectMeta) -> Result<Self, Self::Error> {
        let name = value
            .name
            .as_ref()
            .ok_or_else(|| anyhow!("ObjectMetadata is missing its 'name' field"))?;
        let ns = value
            .namespace
            .as_ref()
            .ok_or_else(|| anyhow!("ObjectMetadata is missing its 'namespace' field"))?;
        Ok(JobID(ns.to_string(), name.to_string()))
    }
}
