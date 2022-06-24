use clap::Parser;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser)]
#[clap(version)]
struct Args {
    #[clap(
         parse(try_from_str),
         long,
         env = "PROC_WATCHER_LOG_LEVEL",
         default_value = "debug"
     )]
     log_level: EnvFilter,

     #[clap(long)]
     proc_name: String,
}

fn main() -> anyhow::Result<()> {
    let Args {log_level, proc_name} = Args::parse();
    let subscriber = tracing_subscriber::fmt().with_env_filter(log_level).try_init().unwrap();

    tracing::debug!("Test");
    tracing::info!("Test");
    tracing::error!("Test");
    println!("Hello, world!");

    Ok(())
}
