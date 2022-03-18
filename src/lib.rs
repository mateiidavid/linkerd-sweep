use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use futures::prelude::*;
use hyper::{client, http};
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use kube::{api::ResourceExt, runtime::watcher::Event};
use tokio::sync::mpsc;

type PodStore = Arc<Mutex<HashSet<PodID>>>;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct PodID(String, String);

pub async fn process_pods<S>(events: S, store: PodStore, sender: mpsc::Sender<(PodID, String)>)
where
    S: Stream<Item = Event<Pod>>,
{
    tokio::pin!(events);
    while let Some(ev) = events.next().await {
        handle_pod(ev, store.clone(), sender.clone()).await;
    }
}

async fn handle_pod(ev: Event<Pod>, store: PodStore, tx: mpsc::Sender<(PodID, String)>) {
    match ev {
        Event::Applied(pod) => {
            let pod_id = {
                let namespace = pod.namespace().unwrap();
                let name = pod.name();
                PodID(namespace, name)
            };

            let injected = pod
                .annotations()
                .get("linkerd.io/inject")
                .and_then(|v| Some(v == "enabled"))
                .is_some();

            let cached_pods = store.lock().unwrap();
            if cached_pods.contains(&pod_id) || !injected {
                tracing::debug!(%pod_id, "skipping pod update");
                return;
            } else {
                drop(cached_pods)
            }

            tracing::info!(%pod_id, "handling pod");
            let pod_ip = pod.status.and_then(|status| {
                let has_terminated = status
                    .container_statuses
                    .as_ref()
                    .and_then(|s| check_container_terminated(s));
                has_terminated.and(status.pod_ip)
            });

            if let Some(ip) = pod_ip {
                // TODO: add some details here in the trace. we might want to instrument
                // this whole span to see it clearly
                tracing::info!(?pod_id, %ip, "sending pod over to sweeper");
                match tx.send((pod_id.clone(), ip)).await {
                    Ok(_) => {
                        tracing::info!("sent event");
                        let mut cached_pods = store.lock().unwrap();
                        cached_pods.insert(pod_id);
                        drop(cached_pods);
                    }
                    Err(e) => tracing::error!(%e, "could not send event to sweeper"),
                }
                // send over mpsc
            }
        }
        _ => {}
    }
}

fn check_container_terminated(containers: &Vec<ContainerStatus>) -> Option<()> {
    for container in containers {
        if container.name == "linkerd-proxy" {
            continue;
        }

        let state = container.state.as_ref().unwrap();
        if let Some(terminated) = &state.terminated {
            let exit_code = terminated.exit_code;
            tracing::info!(%exit_code, name = %container.name, "found terminated container");
            // Ignore failed containers?
            if exit_code != 0 {
                return None;
            }
            return Some(());
        };
    }

    None
}

pub struct Sweeper {
    client: hyper::Client<client::HttpConnector>,
    rx: mpsc::Receiver<(PodID, String)>,
    store: PodStore,
}

impl Sweeper {
    fn new(
        client: hyper::Client<client::HttpConnector>,
        rx: mpsc::Receiver<(PodID, String)>,
        store: PodStore,
    ) -> Self {
        Self { client, rx, store }
    }

    async fn run(mut self, port: u16) -> Result<()> {
        while let Some(job) = self.rx.recv().await {
            let (id, ip) = job;
            let shutdown_endpoint = format!("{}:{}", ip, &port);
            let client = self.client.clone();
            tokio::spawn(async move {
                let req = {
                    let uri = hyper::Uri::builder()
                        .scheme(http::uri::Scheme::HTTP)
                        .authority(shutdown_endpoint)
                        .path_and_query("/shutdown")
                        .build()
                        .unwrap();
                    http::Request::builder()
                        .method(http::Method::POST)
                        .uri(uri)
                        .body(Default::default())
                        .expect("shutdown request must be valid")
                };

                tracing::info!(%id, %ip, "sending shutdown request");
                let resp = client.request(req).await.expect("failed");
                tracing::info!(%ip, "shutdown sent");
                let status = resp.status();
                tracing::info!(%status, "status");
            });
        }
        Ok(())
    }
}

impl std::fmt::Display for PodID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.0, self.1)
    }
}
