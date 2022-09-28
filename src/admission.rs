use core::task;

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use hyper::body::Buf;
use hyper::{http, service::Service, Body, Request, Response};
use json_patch::PatchOperation;
use k8s_openapi::api::batch::v1::JobSpec;
use k8s_openapi::api::core::v1::{Pod, PodSpec};
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
            // Turn request body into an AdmissionReview
            let review: AdmissionReview<DynamicObject> = {
                let body = hyper::body::aggregate(req.into_body()).await?;
                serde_json::from_reader(body.reader())?
            };

            trace!(admission.review = ?review);
            let rsp = match review.try_into() {
                Ok(req) => handler.admit(req).await.into_review(),
                Err(err) => bail!("AdmissionRequest is invalid: {}", err),
            };

            tracing::info!(?rsp);
            let b = serde_json::to_string(&rsp)?;
            Ok(Response::builder()
                .status(http::StatusCode::OK)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(hyper::Body::from(b))
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
    //  * Update: check if DynamicObject is Pod. If it is, parse spec, and then
    //  mutate
    async fn admit(self, req: AdmissionRequest<DynamicObject>) -> AdmissionResponse {
        if is_kind::<Pod>(&req) {
            let resp = AdmissionResponse::from(&req);
            let (pod_meta, pod_spec) = match parse_spec::<PodSpec>(req) {
                Ok(v) => v,
                Err(error) => {
                    debug!(%error, "Error parsing Pod spec");
                    return resp.deny(error);
                }
            };

            let pod_id = match PodID::try_from(&pod_meta) {
                Ok(id) => id,
                Err(error) => {
                    debug!(%error, "Failed to parse Pod metadata");
                    // TODO: pods won't have names yet unfortunately
                    PodID("banana".into(), "banana-namespace".into())
                    //return resp.deny(error);
                }
            };

            let pod_labels = if let Some(labels) = pod_meta.labels {
                labels
            } else {
                debug!("Pod does not contain any labels");
                return resp;
            };

            if !pod_labels.contains_key("extensions.linkerd.io/sweep-sidecar") {
                debug!(%pod_id, "Pod is missing 'sweep-sidecar' label");
                return resp;
            } else {
                let enabled = match pod_labels.get("extensions.linkerd.io/sweep-sidecar") {
                    Some(lv) => lv == "enabled",
                    None => false,
                };

                if !enabled {
                    debug!(%pod_id, "Skipping pod, 'linkerd-sweep' is not enabled");
                    return resp;
                }
            }

            self.mutate::<PodSpec>(resp, pod_spec)
                .instrument(debug_span!("admission.mutate", %pod_id))
                .await
        } else {
            debug!("Not pod kind");
            // Unsupported resource
            // admit without mutating
            // print gvk in debug
            AdmissionResponse::from(&req)
        }
    }

    async fn mutate<T>(self, resp: AdmissionResponse, spec: T) -> AdmissionResponse
    where
        T: Serialize + JsonPatch,
    {
        match spec.generate_patch() {
            Ok(patch) => resp
                .with_patch(patch)
                .expect("Failed to patch AdmissionResponse"),

            Err(err) => {
                debug!(%err, "Failed to generate patch");
                resp
            }
        }
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

trait JsonPatch {
    fn generate_patch(self) -> Result<json_patch::Patch>;
}

impl JsonPatch for PodSpec {
    fn generate_patch(self) -> Result<json_patch::Patch> {
        let mut patches: Vec<PatchOperation> = vec![];

        if self.init_containers.is_none() {
            patches.push(mk_add_patch("/spec/initContainers", {}));
        }
        patches.push(mk_add_patch(
            "/spec/initContainers/-",
            create_curl_container(),
        ));

        if self.volumes.is_none() {
            patches.push(mk_add_patch("/spec/volumes", {}));
        }

        patches.push(mk_add_patch(
            "/spec/volumes/-",
            k8s_openapi::api::core::v1::Volume {
                name: "linkerd-await".into(),
                empty_dir: Some(k8s_openapi::api::core::v1::EmptyDirVolumeSource::default()),
                ..Default::default()
            },
        ));

        for (i, container) in self.containers.into_iter().enumerate() {
            let name = container.name.clone();
            // skip if proxy
            if name == "linkerd-proxy" {
                continue;
            }

            let current_path = format!("{}/{}", "/spec/containers", i);
            match create_container_patches(&current_path, container) {
                Ok(mut container_patches) => {
                    patches.append(&mut container_patches);
                    debug!(container_name=%name, "Patched container");
                }
                Err(err) => {
                    debug!(container_name=%name, %err, "Skipped patch");
                }
            }
        }

        //
        Ok(json_patch::Patch(patches))
    }
}

fn create_container_patches(
    root_path: &str,
    c: k8s_openapi::api::core::v1::Container,
) -> Result<Vec<json_patch::PatchOperation>> {
    let comm = c
        .command
        .as_ref()
        .ok_or_else(|| anyhow!("container {} is missing 'command' field", c.name))?;

    let mut new_args = vec!["--shutdown".into(), "--".into()];
    for command in comm.clone().into_iter() {
        new_args.push(command);
    }

    if let Some(args) = c.args {
        for arg in args.into_iter() {
            new_args.push(arg);
        }
    }

    let mut patches = Vec::new();
    let comm_path = format!("{}/command", root_path);
    patches.push(mk_replace_patch(comm_path, vec!["/linkerd/linkerd-await"]));

    let arg_path = format!("{}/args", root_path);
    patches.push(mk_replace_patch(arg_path, new_args));

    if c.volume_mounts.is_none() {
        let volume_mount_path = format!("{}/volumeMounts", root_path);
        patches.push(mk_root_patch(volume_mount_path));
    }

    let volume_path = format!("{}/volumeMounts/-", root_path);
    patches.push(mk_add_patch(volume_path, create_volume_mount(true)));

    Ok(patches)
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

struct PodID(String, String);

impl std::fmt::Display for PodID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.0, self.1)
    }
}

impl TryFrom<&ObjectMeta> for PodID {
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
        Ok(PodID(ns.to_string(), name.to_string()))
    }
}

//
//////
//////  Playing around
///

fn mk_add_patch<T: Serialize, S: Into<String>>(path: S, value: T) -> json_patch::PatchOperation {
    json_patch::PatchOperation::Add(json_patch::AddOperation {
        path: path.into(),
        value: serde_json::json!(value),
    })
}

fn mk_replace_patch<T: Serialize, S: Into<String>>(
    path: S,
    value: T,
) -> json_patch::PatchOperation {
    json_patch::PatchOperation::Replace(json_patch::ReplaceOperation {
        path: path.into(),
        value: serde_json::json!(value),
    })
}

fn mk_root_patch<S: Into<String>>(path: S) -> json_patch::PatchOperation {
    json_patch::PatchOperation::Add(json_patch::AddOperation {
        path: path.into(),
        value: serde_json::json!({}),
    })
}

fn create_volume_mount(read_only: bool) -> k8s_openapi::api::core::v1::VolumeMount {
    k8s_openapi::api::core::v1::VolumeMount {
        mount_path: "/linkerd".to_owned(),
        name: "linkerd-await".to_owned(),
        read_only: Some(read_only),
        ..Default::default()
    }
}

fn create_curl_container() -> k8s_openapi::api::core::v1::Container {
    let mut args = vec!["-c".into()];
    let comm = format!("cp {} {}", "/tmp/linkerd-await", "/linkerd/linkerd-await");
    args.push(comm);
    k8s_openapi::api::core::v1::Container {
        name: "await-init".to_owned(),
        image: Some("ghcr.io/mateiidavid/await-util:test".to_owned()),
        image_pull_policy: Some("IfNotPresent".to_owned()),
        volume_mounts: Some(vec![create_volume_mount(false)]),
        command: Some(vec!["/bin/sh".into()]),
        args: Some(args),
        ..Default::default()
    }
}
