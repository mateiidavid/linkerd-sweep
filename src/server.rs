use core::task;
use std::convert::{TryFrom, TryInto};
use std::{net::SocketAddr, path::PathBuf};

use crate::tls;
use anyhow::{anyhow, bail, Context, Result};
use futures::{future, TryFutureExt};
use hyper::{http, Body, Request};
use hyper::{server::conn::Http, service::Service, Response};
use k8s_openapi::api::core::v1::PodTemplateSpec;
use k8s_openapi::Resource;
use kube::core::admission::{AdmissionRequest, AdmissionResponse};
use kube::core::ObjectMeta;
use kube::core::{admission::AdmissionReview, DynamicObject};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, debug_span, info_span, trace, Instrument};

pub struct AdmissionServer {
    bind_addr: SocketAddr,
    cert_path: PathBuf,
    key_path: PathBuf,
}

const JOB_KIND: &'static str = k8s_openapi::api::batch::v1::Job::KIND;
const JOB_GROUP: &'static str = k8s_openapi::api::batch::v1::Job::GROUP;

impl AdmissionServer {
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            cert_path: PathBuf::from("/var/run/sweep/tls.crt"),
            key_path: PathBuf::from("/var/run/sweep/tls.key"),
        }
    }

    pub async fn run(self) {
        tracing::info!("running, boss");
        let listener = TcpListener::bind(&self.bind_addr)
            .await
            .expect("listener should be created successfully");
        let _local_addr = listener.local_addr().expect("can't get local addr");

        loop {
            tracing::info!("loopin, boss");
            let socket = match listener.accept().await {
                Ok((socket, _)) => socket,
                Err(err) => {
                    tracing::error!(%err, "Failed to accept connection");
                    continue;
                }
            };

            let peer_addr = match socket.peer_addr() {
                Ok(addr) => addr,
                Err(err) => {
                    tracing::error!(%err, "Failed to get peer addr");
                    continue;
                }
            };

            tokio::spawn(
                Self::handle_conn(socket, self.cert_path.clone(), self.key_path.clone())
                    .map_err(|err| tracing::error!(%err))
                    .instrument(info_span!("connection", peer.addr = %peer_addr)),
            );
        }
    }

    async fn handle_conn(socket: TcpStream, cert_path: PathBuf, key_path: PathBuf) -> Result<()> {
        tracing::info!("buildin a conn");
        // Build TLS Connector
        let tls = match tls::mk_tls_connector(&cert_path, &key_path).await {
            Ok(tls) => tls,
            Err(err) => {
                tracing::error!(%err, "failed to build tls connector");
                return Err(anyhow!("could not establish TLS connection"));
            }
        };
        // Build TLS conn
        let stream = tls.accept(socket).await.with_context(|| "TLS Error")?;
        match Http::new()
            .serve_connection(stream, Handler::new())
            .instrument(info_span!("admission.request"))
            .await
        {
            Ok(c) => tracing::info!("connection successful"),
            Err(err) => tracing::error!(%err, "failed to process admission request"),
        }

        Ok(())
    }
}

#[derive(Clone)]
struct Handler;

impl Service<Request<Body>> for Handler {
    type Response = Response<Body>;

    type Error = anyhow::Error;

    type Future = future::BoxFuture<'static, anyhow::Result<Response<Body>>>;

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
        let handler = self.clone();
        Box::pin(async {
            let review: AdmissionReview<DynamicObject> = {
                let bytes = hyper::body::to_bytes(req.into_body())
                    .await
                    .with_context(|| "Failed to convert request body to bytes")?;
                serde_json::from_slice(&bytes)
                    .with_context(|| "Failed to deserialize request body bytes to json")?
            };
            tracing::trace!("Admission Review: {:?}", review);
            let rsp = match review.try_into() {
                Ok(req) => handler.process_request(req).into_review(),
                Err(err) => return Err(anyhow!("invalid admission request: {}", err)),
            };
            //  * quick validation: does main container match a container in the
            //  template spec? if not err out
            // JSONPATCH: add emptydir volume with init container for linkerd-await command: curl
            // binary or smth
            // JSONPATCH: add entrypoint to main container

            let bytez = serde_json::to_vec(&rsp)?;
            Ok(Response::builder()
                .status(http::StatusCode::OK)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(hyper::Body::from(bytez))
                .unwrap())
        })
    }
}

impl Handler {
    fn new() -> Self {
        Self
    }

    async fn process_request(self, req: AdmissionRequest<DynamicObject>) -> AdmissionResponse {
        if let Err(err) = check_request_kind(&req) {
            tracing::error!("invalid AdmissionRequest: {}", err);
            return AdmissionResponse::invalid(format!("invalid AdmissionRequest: {}", err));
        }

        let rsp = AdmissionResponse::from(&req);
        let (job_metadata, pod_template, id) = match parse_request(req)
            .instrument(debug_span!("parse_admission_req"))
            .await
        {
            Ok(v) => v,
            Err(err) => {
                tracing::error!("invalid AdmissionRequest: {}", err);
                return AdmissionResponse::invalid(format!(
                    "Error parsing AdmissionRequest: {}",
                    err
                ));
            }
        };

        let pod_metadata = pod_template.metadata.unwrap_or_default();
        let sweep_name = match process_annotations(pod_metadata, job_metadata)
            .instrument(debug_span!("process_annotations", job_id = &id))
            .await
        {
            Ok(v) => v,
            Err(err) => {
                tracing::debug!("skipping mutation on Job [{}]: {}", &id, err);
                return rsp;
            }
        };

        /*
        let patch = {
            let patches = create_patch().expect("failed to construct patches");
            tracing::info!(?patches, "patches");
            json_patch::Patch(patches)
        };
        */
        rsp.with_patch(patch).expect("failed to patch")
    }
}

// Check whether we are dealing with a Job
fn check_request_kind(req: &AdmissionRequest<DynamicObject>) -> Result<()> {
    if req.kind.kind != JOB_KIND || req.kind.group != JOB_GROUP {
        return Err(anyhow!(
            "AdmissionRequest group or kind is invalid: {:?}.{:?}",
            req.kind.group,
            req.kind.kind
        ));
    }

    Ok(())
}

// Inject annotation should be on Template Spec, not responsible for just one
// thing but whatever, it's good for now.
async fn process_annotations(pod_meta: ObjectMeta, job_meta: ObjectMeta) -> Result<String> {
    trace!(%pod_meta, %job_meta, "metadata");
    let pod_annotations = pod_meta.annotations.unwrap_or_else(|| {
        debug!("PodTemplateSpec does not have any annotations");
        Default::default
    });

    // TODO: check spec not container for this annotation
    let value = pod_annotations
        .get("linkerd.io/inject")
        .map(|str| str.to_owned())
        .ok_or_else(|| anyhow!("Inject annotation is missing from ObjectMetadata"))?;

    if value != "enabled" {
        bail!("linkerd injection is disabled");
    }

    let job_annotations = job_meta.annotations.unwrap_or_else(|| {
        debug!("Job does not have any annotations");
        Default::default
    });

    let container_name = {
        let name = job_annotations
            .get("extensions.linkerd.io/sweep-container")
            .map(|str| str.to_owned())
            .ok_or_else(|| anyhow!("Sweep annotation is missing from ObjectMetadata"))?;

        if name.is_empty() {
            debug!("sweep annotation present but value is missing");
            bail!("no container nominated for sweeping");
        }
        name
    };

    Ok(container_name)
}

async fn parse_request(
    req: AdmissionRequest<DynamicObject>,
) -> Result<(ObjectMeta, PodTemplateSpec, JobID)> {
    let obj = req
        .object
        .ok_or_else(|| anyhow!("AdmissionRequest missing 'object' field"))?;

    let meta = obj.metadata;
    let pod_spec = {
        let json_spec = obj
            .data
            .get("spec")
            .cloned()
            .ok_or_else(|| anyhow!("AdmissionRequest object missing 'spec' field"))?;
        let job_spec = serde_json::from_value(json_spec)
            .map_err(|err| anyhow!("Error deserializing object 'spec' to 'Job': {}", err))?;
        job_spec.template
    };

    let id = JobID::try_from(&meta)?;
    Ok((meta, pod_spec, id))
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
