use tokio::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use std::env;

    use tracing_subscriber::{prelude::*, EnvFilter};
    let log_filter = env::var("RUST_LOG").unwrap_or_else(|_| String::from("info,kube-runtime=debug,kube=debug"))

    Ok(())
}
