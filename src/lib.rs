use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use futures::prelude::*;
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ResourceExt, runtime::watcher::Event};
use tokio::sync::mpsc;

type PodStore = Arc<Mutex<HashSet<(String, String)>>>;
struct PodID(String, String);

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
                (namespace, name)
            };

            let store = store.lock().unwrap();
            if store.contains(&pod_id) {
                tracing::debug!(?pod_id, "skipping pod update");
                return;
            } else {
                drop(store)
            }

            tracing::info!(?pod_id, "handling pod");
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
                match tx.send((pod_id, ip)).await {
                    Ok(_) => tracing::info!("sent event"),
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
        if let Some(_) = &state.terminated {
            tracing::info!(name = %container.name, "found terminated contaienr");
            return Some(());
        };
    }

    None
}

impl std::fmt::Display for PodID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.0, self.1)
    }
}
