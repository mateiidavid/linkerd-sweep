use core::task;

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use hyper::body::Buf;
use hyper::{http, service::Service, Body, Request, Response};
use k8s_openapi::api::core::v1::{Pod, PodSpec};
use kube::core::ObjectMeta;
use kube::{
    api::DynamicObject,
    core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview},
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::convert::TryInto;
use tracing::{debug, debug_span, trace, Instrument};

use crate::patch::JsonPatch;

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

            /* TODO: we won't have access to a name here, should base ID of
             * parent perhaps.
            let pod_id = match PodID::try_from(&pod_meta) {
                Ok(id) => id,
                Err(error) => {
                    debug!(%error, "Failed to parse Pod metadata");
                    // TODO: pods won't have names yet unfortunately
                    PodID("banana".into(), "banana-namespace".into())
                    //return resp.deny(error);
                }
            };

            */

            let pod_labels = if let Some(labels) = pod_meta.labels {
                labels
            } else {
                debug!("Pod does not contain any labels");
                return resp;
            };

            if !pod_labels.contains_key("extensions.linkerd.io/sweep-sidecar") {
                debug!("Pod is missing 'sweep-sidecar' label");
                return resp;
            } else {
                let enabled = match pod_labels.get("extensions.linkerd.io/sweep-sidecar") {
                    Some(lv) => lv == "enabled",
                    None => false,
                };

                if !enabled {
                    debug!("Skipping pod, 'linkerd-sweep' is not enabled");
                    return resp;
                }
            }

            self.mutate::<PodSpec>(resp, pod_spec)
                .instrument(debug_span!("admission.mutate"))
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
