use std::{path::PathBuf, time};

use anyhow::{anyhow, Result};
use clap::Parser;
use tracing::{debug, info};
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
    /// Log level
    log_level: EnvFilter,

    /// Name of the process to watch state for
    #[clap(short, long)]
    proc_name: String,

    /// Timeout value when waiting to get pid based on process name
    #[clap(parse(try_from_str = parse_timeout), long, default_value = "300s")]
    pid_watch_timeout: time::Duration,

    /// Backoff value when retrying to get pid based on process name
    #[clap(parse(try_from_str = parse_timeout), long, default_value = "120s")]
    pid_watch_backoff: time::Duration,
}

// TODO: will need tokio here
// will have to watch for ctrl-c signal, or something similar.
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let Args {
        log_level,
        proc_name,
        pid_watch_timeout,
        pid_watch_backoff,
    } = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .try_init()
        .map_err(|err| anyhow!("Failed to initialize tracing subscriber: {}", err))?;

    let v = watch_process();
    Ok(())
}

// TODO: need a timer so we avoid using too much cpu
// TODO: use inotify to get notified when the process is deleted after we
// establish which process we're looking for
async fn run(app_comm: String, proxy_comm: String) -> Result<()> {
    unimplemented!();
}

// Check comm (command name)
fn watch_process(proxy_comm: &str, app_comm: &str) -> Vec<Proc> {
    let mut found_proxy = false;
    let mut found_app = false;
    let mut prcs = Vec::new();
    while !found_proxy && !found_app {
        debug!(%proxy_comm, %app_comm, "Watching processes");
        let mut procs = procfs::process::all_processes()
            .expect("Failed to list processes")
            .into_iter();
        while let Some(prc) = procs.next() {
            let stat = prc.stat().expect("Failed to read prc stat");
            if stat.comm == proxy_comm {
                let prc_info = ProcInfo::new(stat.pid, stat.comm);
                debug!(%prc_info, "Found proxy process stat");
                let p = Proc::Proxy(prc_info);
                found_proxy = true;
                prcs.push(p);
            } else if stat.comm == app_comm {
                let prc_info = ProcInfo::new(stat.pid, stat.comm);
                debug!(%prc_info, "Found app process stat");
                let p = Proc::App(prc_info);
                found_app = true;
                prcs.push(p);
            }
        }
    }

    prcs
}

#[derive(Debug)]
enum Proc {
    Proxy(ProcInfo),
    App(ProcInfo),
}

#[derive(Debug)]
struct ProcInfo(i32, String);

impl ProcInfo {
    fn new(pid: i32, comm: String) -> Self {
        ProcInfo(pid, comm)
    }
}

impl std::fmt::Display for ProcInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.1, self.0)
    }
}

pub fn parse_timeout(s: &str) -> Result<time::Duration> {
    let (magnitude, unit) = if let Some(offset) = s.rfind(|c: char| c.is_digit(10)) {
        let (magnitude, unit) = s.split_at(offset + 1);
        let magnitude = magnitude.parse::<u64>()?;
        (magnitude, unit)
    } else {
        anyhow::bail!("{} does not contain a timeout duration value", s);
    };

    let mul = match unit {
        "" if magnitude == 0 => 0,
        "ms" => 1,
        "s" => 1000,
        "m" => 1000 * 60,
        "h" => 1000 * 60 * 60,
        "d" => 1000 * 60 * 60 * 24,
        _ => anyhow::bail!(
            "invalid duration unit {} (expected one of 'ms', 's', 'm', 'h', or 'd')",
            unit
        ),
    };

    let ms = magnitude
        .checked_mul(mul)
        .ok_or_else(|| anyhow!("Timeout value {} overflows when converted to 'ms'", s))?;
    Ok(time::Duration::from_millis(ms))
}

#[cfg(test)]
mod tests {
    use crate::parse_timeout;
    use std::time;

    #[test]
    fn test_parse_timeout_invalid() {
        assert!(parse_timeout("120").is_err());
        assert!(parse_timeout("s").is_err());
        assert!(parse_timeout("foobars").is_err());
        assert!(parse_timeout("18446744073709551615s").is_err())
    }

    #[test]
    fn test_parse_timeout_seconds() {
        assert_eq!(time::Duration::from_secs(0), parse_timeout("0").unwrap());
        assert_eq!(time::Duration::from_secs(0), parse_timeout("0ms").unwrap());
        assert_eq!(time::Duration::from_secs(0), parse_timeout("0s").unwrap());
        assert_eq!(time::Duration::from_secs(0), parse_timeout("0m").unwrap());

        assert_eq!(
            time::Duration::from_secs(120),
            parse_timeout("120s").unwrap()
        );
        assert_eq!(
            time::Duration::from_secs(120),
            parse_timeout("120000ms").unwrap()
        );
        assert_eq!(time::Duration::from_secs(120), parse_timeout("2m").unwrap());
        assert_eq!(
            time::Duration::from_secs(7200),
            parse_timeout("2h").unwrap()
        );
        assert_eq!(
            time::Duration::from_secs(172800),
            parse_timeout("2d").unwrap()
        );
    }
}
