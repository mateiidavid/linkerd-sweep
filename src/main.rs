use std::net::SocketAddr;

use anyhow::Result;
use clap::Parser;
use linkerd_sweep::server::AdmissionServer;
use tracing::info;

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
    #[clap(long, env = "LINKERD_SWEEP_LOG_FORMAT", default_value = "plain")]
    log_format: kubert::LogFormat,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let Args {
        log_level,
        log_format,
    } = Args::parse();

    log_format.try_init(log_level)?;

    let (_shutdown_tx, shutdown_rx) = kubert::shutdown::sigint_or_sigterm()?;

    let listen_addr = SocketAddr::from(([0, 0, 0, 0], 443));
    let server = AdmissionServer::new(listen_addr, shutdown_rx.clone());
    let server_task = tokio::spawn(server.run());
    tokio::select! {
        _ = shutdown_rx.signaled() => {
            info!("Received shutdown signal");
            return Ok(());
        }

        _ = server_task => {}
    }

    Ok(())
}
