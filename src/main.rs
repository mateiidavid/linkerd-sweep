use anyhow::Result;
use clap::Parser;
use futures::TryStreamExt;
use hyper::http;
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use kube::{api::ListParams, runtime, Api, Client};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Log level
    #[clap(long, env = "SWEEP_CONTROLLER_LOG_LEVEL", default_value = "debug")]
    log_level: kubert::LogFilter,

    /// Log format (json | plain)
    #[clap(long, default_value = "plain")]
    log_format: kubert::LogFormat,

    /// Port of the proxy where the shutdown signal should be sent
    #[clap(short, long, default_value = "4191")]
    port: u16,

    #[clap(flatten)]
    client: kubert::ClientArgs,

    #[clap(flatten)]
    admin: kubert::AdminArgs,
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        log_level,
        log_format,
        port,
        client,
        admin,
    } = Args::parse();

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

// TODO: make sure pods are only processed once, don't want to send shutdown
// signal to same pod 5 times.
async fn handle_pod(tx: mpsc::Sender<(String, String)>, pod: Pod) {
    let (name, namespace) = {
        let metadata = pod.metadata;
        (metadata.name.unwrap(), metadata.namespace.unwrap())
    };

    // if it's been cached don't process?
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
