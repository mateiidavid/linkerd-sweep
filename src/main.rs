pub mod lib;

use anyhow::Result;
use clap::Parser;
use futures::prelude::*;
use hyper::http;
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use kube::{api::ListParams, runtime};
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

    let mut rt = kubert::Runtime::builder()
        .with_log(log_level, log_format)
        .with_admin(admin)
        .with_client(client)
        .build()
        .await?;

    let (tx, mut rx) = mpsc::channel(100);

    let params = ListParams::default().labels("linkerd.io/sweep-proxy=true");
    let pods = rt.watch_all::<Pod>(params);

    let run_watcher = tokio::spawn(async move {
        tokio::pin!(pods);

        let mut seen = std::collections::HashSet::<(String, String)>::new();
        while let Some(ev) = pods.next().await {
            match ev {
                runtime::watcher::Event::Applied(_) => todo!(),
                runtime::watcher::Event::Deleted(_) => todo!(),
                runtime::watcher::Event::Restarted(_) => todo!(),
            }
        }
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

/* TODO:
 - get up and running with kube
 - watch pod resources... or jobs?
*/
