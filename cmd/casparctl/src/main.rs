//! Translation of `cmd/casparctl/main.go`.
//!
//! The Caspar control CLI: installs/uninstalls/starts/stops/pauses/resumes
//! the Caspar Docker container, exposes a live stats dashboard, and forwards
//! telemetry. The Rust port mirrors the Go subcommand surface but is built
//! on `clap` instead of stdlib `flag`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const NAME_FILE: &str = ".casparctl-name";
const IMAGE_FILE: &str = ".casparctl-image";

#[derive(Parser)]
#[command(name = "casparctl", version, about = "Caspar control CLI")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install the Caspar Docker container (image name optional).
    Install {
        #[arg(long, default_value = "caspar")]
        name: String,
        #[arg(long, default_value = "caspar:latest")]
        image: String,
    },
    /// Uninstall the container (keeps state directories intact).
    Uninstall,
    /// Uninstall + delete every persisted state directory.
    Purge,
    /// Start the installed container.
    Start,
    /// Pause the running container.
    Pause,
    /// Resume the paused container.
    Resume,
    /// Stop the container.
    Stop,
    /// Stream a one-shot stats snapshot to stdout.
    Stats {
        /// Refresh interval in seconds (`0` for one-shot).
        #[arg(long, default_value_t = 0)]
        interval: u64,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DockerStats {
    #[serde(rename = "Container", default)]
    container: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "CPUPerc", default)]
    cpu_perc: String,
    #[serde(rename = "MemUsage", default)]
    mem_usage: String,
    #[serde(rename = "MemPerc", default)]
    mem_perc: String,
    #[serde(rename = "NetIO", default)]
    net_io: String,
    #[serde(rename = "BlockIO", default)]
    block_io: String,
    #[serde(rename = "PIDs", default)]
    pids: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TelemetrySnapshot {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    uptime_sec: i64,
    #[serde(default)]
    node: HashMap<String, Value>,
    #[serde(default)]
    chain: HashMap<String, Value>,
    #[serde(default)]
    federation: HashMap<String, Value>,
    #[serde(default)]
    clients: HashMap<String, Value>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Install { name, image } => run_install(&name, &image),
        Cmd::Uninstall => run_uninstall(),
        Cmd::Purge => run_purge(),
        Cmd::Start => lifecycle("start"),
        Cmd::Pause => lifecycle("pause"),
        Cmd::Resume => lifecycle("unpause"),
        Cmd::Stop => lifecycle("stop"),
        Cmd::Stats { interval } => run_stats(interval),
    }
}

fn run_install(name: &str, image: &str) -> Result<()> {
    require_docker()?;
    validate_docker_name(name)?;
    ensure_storage_folders()?;

    fs::write(NAME_FILE, name)?;
    fs::write(IMAGE_FILE, image)?;

    let status = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            name,
            "--restart=unless-stopped",
            image,
        ])
        .status()?;
    expect_success(status, "docker run")?;
    println!("installed `{}` from image `{}`", name, image);
    Ok(())
}

fn run_uninstall() -> Result<()> {
    require_docker()?;
    let name = read_name()?;
    let _ = Command::new("docker").args(["stop", &name]).status();
    let status = Command::new("docker").args(["rm", "-f", &name]).status()?;
    expect_success(status, "docker rm")?;
    let _ = fs::remove_file(NAME_FILE);
    let _ = fs::remove_file(IMAGE_FILE);
    println!("uninstalled `{}`", name);
    Ok(())
}

fn run_purge() -> Result<()> {
    run_uninstall().ok();
    for dir in ["data", "logs", "snapshots", "telemetry-rocks"] {
        let _ = fs::remove_dir_all(dir);
    }
    println!("purged caspar state");
    Ok(())
}

fn lifecycle(action: &str) -> Result<()> {
    require_docker()?;
    let name = read_name()?;
    let status = Command::new("docker").args([action, &name]).status()?;
    expect_success(status, &format!("docker {}", action))?;
    println!("{} {}", action, name);
    Ok(())
}

fn run_stats(interval: u64) -> Result<()> {
    require_docker()?;
    let name = read_name()?;
    loop {
        match collect_stats(&name) {
            Ok(stats) => {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            }
            Err(e) => eprintln!("stats: {}", e),
        }
        if let Ok(snap) = fetch_telemetry() {
            println!("--- telemetry ---");
            println!("{}", serde_json::to_string_pretty(&snap)?);
        }
        if interval == 0 {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(interval));
    }
}

fn collect_stats(name: &str) -> Result<DockerStats> {
    let out = Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{json .}}",
            name,
        ])
        .output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "docker stats failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("");
    Ok(serde_json::from_str(line)?)
}

fn fetch_telemetry() -> Result<TelemetrySnapshot> {
    let url = std::env::var("CASPARCTL_TELEMETRY")
        .unwrap_or_else(|_| "http://127.0.0.1:9099/telemetry/snapshot".to_string());
    // Use curl rather than a Rust HTTP client to keep dependencies tiny —
    // curl is already a hard requirement on every Caspar host.
    let out = Command::new("curl").args(["-sS", &url]).output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "telemetry curl: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn require_docker() -> Result<()> {
    let status = Command::new("docker").arg("info").output()?;
    if !status.status.success() {
        return Err(anyhow!(
            "docker is not running: {}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    Ok(())
}

fn validate_docker_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow!("container name is empty"));
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err(anyhow!("container name must start with an alphanumeric"));
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
            return Err(anyhow!("invalid character `{}` in container name", c));
        }
    }
    Ok(())
}

fn ensure_storage_folders() -> Result<()> {
    for dir in ["data", "logs", "snapshots", "certs"] {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn expect_success(status: ExitStatus, what: &str) -> Result<()> {
    if !status.success() {
        return Err(anyhow!("{} failed (exit code {:?})", what, status.code()));
    }
    Ok(())
}

fn read_name() -> Result<String> {
    let p = Path::new(NAME_FILE);
    if !p.exists() {
        return Err(anyhow!(
            "no container installed (missing {})",
            NAME_FILE
        ));
    }
    Ok(fs::read_to_string(p)?.trim().to_string())
}

#[allow(dead_code)]
fn _silence(_: PathBuf) {}
