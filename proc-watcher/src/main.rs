use anyhow::{anyhow, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

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

    #[clap(short, long)]
    proc_name: String,
}

// TODO: will need tokio here
// will have to watch for ctrl-c signal, or something similar.
fn main() -> Result<()> {
    let Args {
        log_level,
        proc_name,
    } = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .try_init()
        .map_err(|err| anyhow!("Failed to initialize tracing subscriber: {}", err))?;

    for prc in procfs::process::all_processes().unwrap() {
        let pid = prc.pid();
        let stat = prc.stat().unwrap();
        let name = stat.comm;
        info!(%pid, %name, "looking at proc");
    }

    Ok(())
}

// TODO: need a timer so we avoid using too much cpu
// TODO: use inotify to get notified when the process is deleted after we
// establish which process we're looking for
fn run(proc_name: String) -> Result<()> {
    
}
