use anyhow::Result;
use futures::TryStreamExt;
use hyper::http;
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use kube::{api::ListParams, runtime, Api, Client};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    // copy&pasta eliza's set-up for tracing subscriber
    use std::env;

    use tracing_subscriber::{prelude::*, EnvFilter};

    let log_filter = env::var("RUST_LOG")
        .unwrap_or_else(|_| String::from("info,kube-runtime=debug,kube=debug"))
        .parse::<EnvFilter>()?;
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(log_filter)
        .init();

    let client = Client::try_default().await?;
    let api_pod = Api::<Pod>::all(client);
    let lp = ListParams::default().labels("proxy-sweep.io/enabled=true");

    let (tx, mut rx) = mpsc::channel(100);
    let watcher = runtime::utils::try_flatten_applied(runtime::watcher(api_pod, lp));

    let run_watcher = tokio::spawn(async move {
        let tx = tx.clone();
        watcher
            // can probably turn this into a filter_map and then a for each that
            // sends data through channel?
            .try_for_each(|pod| {
                let sender = tx.clone();
                async move {
                    // TODO: Remove unwrap
                    handle_pod(sender, pod).await;
                    Ok(())
                }
            })
            .await
            .unwrap();
    });

    //TODO: (matei)
    // it works, but we process pods multiple times. how can we make sure
    // they're processed only once? caching?
    // request denied and unmeshed/untls'd requests because we send to the proxy
    // port. what do healthchecks do here?
    let run_sweeper = tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            async move {
                let (id, ip) = job;
                tracing::info!(%id, %ip, "building shutdown request for job");
                let req = {
                    let uri = hyper::Uri::builder()
                        .scheme(http::uri::Scheme::HTTP)
                        .authority(format!("{}:4191", ip))
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
                let resp = hyper::Client::default().request(req).await.expect("failed");
                tracing::info!(%ip, "shutdown sent");
                let status = resp.status();
                tracing::info!(%status, "status");
            }
            .await
        }
    });

    let _ = tokio::join!(run_watcher, run_sweeper);

    Ok(())
}

async fn handle_pod(tx: mpsc::Sender<(String, String)>, pod: Pod) {
    let (name, namespace) = {
        let metadata = pod.metadata;
        (metadata.name.unwrap(), metadata.namespace.unwrap())
    };

    tracing::info!(%namespace, %name, "handling pod");
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
        let id = format!("{}/{}", namespace, name);
        tracing::info!(%id, %ip, "sending pod over to sweeper");
        match tx.send((id, ip)).await {
            Ok(_) => tracing::info!("sent event"),
            Err(e) => tracing::error!(%e, "could not send event to sweeper"),
        }
        // send over mpsc
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

/* TODO:
 - get up and running with kube
 - watch pod resources... or jobs?
*/
