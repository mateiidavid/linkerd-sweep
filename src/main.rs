use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, Api, Client};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    let lp = ListParams::default().labels("linkerd.io/inject=enabled,proxy-sweep.io/enabled=true");

    Ok(())
}

/* TODO:
 - get up and running with kube
 - watch pod resources... or jobs?
*/
