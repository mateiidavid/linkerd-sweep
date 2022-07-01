use std::{collections::HashSet, net::SocketAddr, sync::Arc};

use anyhow::Result;
use clap::Parser;
use futures::lock::Mutex;
use k8s_openapi::api::core::v1::Pod;
use kube::api::ListParams;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Log level
    #[clap(
        long,
        env = "LINKERD_SWEEP_LOG_LEVEL",
        default_value = "linkerd_sweep=info,warn"
    )]
    log_level: kubert::LogFilter,

    /// Log format (json | plain)
    #[clap(long, default_value = "plain")]
    log_format: kubert::LogFormat,
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        log_level,
        log_format,
        client,
        admin,
    } = Args::parse();

    log_format.try_init(log_level)?;
    let addr = SocketAddr::from(([0, 0, 0, 0], 443));
    let server = linkerd_sweep::server::AdmissionServer::new(addr);
    server.run().await;
    Ok(())
}
