use std::{collections::HashSet, net::SocketAddr, sync::Arc};

use crate::server::AdmissionServer;
use anyhow::Result;
use clap::Parser;
use futures::lock::Mutex;
use k8s_openapi::api::core::v1::Pod;
use kube::api::ListParams;
use tokio::sync::mpsc;
use tracing::{debug, info};

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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let Args {
        log_level,
        log_format,
    } = Args::parse();

    log_format.try_init(log_level)?;

    let (shutdown_tx, shutdown_rx) = kubert::shutdown::sigint_or_sigterm()?;

    let addr = SocketAddr::from(([0, 0, 0, 0], 443));
    let server = AdmissionServer::new(addr, shutdown_rx.clone());
    let server_task = tokio::spawn(server.run());
    tokio::select! {
        _ = shutdown_rx.signaled() => {
            info!("Received shutdown signal");
            return Ok(());
        }

        _ = server_task => {
            let result = server_task.unwrap();
            return result;
        }
    }
}
