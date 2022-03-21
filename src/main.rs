pub mod lib;

use std::{collections::HashSet, sync::Arc};

use anyhow::Result;
use clap::Parser;
use futures::lock::Mutex;
use k8s_openapi::api::core::v1::Pod;
use kube::api::ListParams;
use linkerd_sweep::Sweeper;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Log level
    #[clap(
        long,
        env = "LINKERD_SWEEP_LOG_LEVEL",
        default_value = "linkerd_sweep=debug,kubert=info,warn"
    )]
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

    let (tx, rx) = mpsc::channel(100);

    tracing::info!("Hello!");
    let params = ListParams::default().labels("linkerd.io/sweep-proxy=true");
    let pods = rt.watch_all::<Pod>(params);

    let store = Arc::new(Mutex::new(HashSet::new()));
    let sweeper = Sweeper::new(hyper::Client::default(), rx, store.clone());
    // TODO: Can't remove async block because anyhow does not impl futures. Can we
    // switch to Boxed err instead?
    let run_sweeper = tokio::spawn(async move { sweeper.run(port).await });

    let run_watcher =
        tokio::spawn(async move { linkerd_sweep::process_pods(pods, store, tx).await });

    let _ = tokio::join!(run_watcher, run_sweeper);

    Ok(())
}

/* TODO:
 - get up and running with kube
 - watch pod resources... or jobs?
*/
