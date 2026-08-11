//! backend.rs
//! ----------
//! Owns the lifecycle of every backing process the assistant needs so the
//! user never has to start anything by hand:
//!   - `jan-server` (Jan.ai's local inference server, headless mode)
//!   - the Python STT/TTS bridge (voice_bridge.py) as a persistent
//!     subprocess with a small line-delimited JSON protocol on stdin/stdout
//!
//! Both are spawned when protogen-daemon starts, monitored, and restarted
//! on unexpected exit (bounded retries, backoff). Both are terminated
//! cleanly on daemon shutdown. Nothing here is meant to be started manually
//! by the user -- the systemd --user unit installed by install_assistant.sh
//! runs protogen-daemon, and this module handles the rest.
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct BackendConfig {
    pub jan_binary: PathBuf,
    pub jan_port: u16,
    pub jan_model: String,
    pub voice_bridge_script: PathBuf,
    pub voice_bridge_python: PathBuf,
}

/// Held for a possible future graceful-shutdown path (e.g. reacting to
/// SIGTERM by cleanly killing Jan.ai/voice bridge children); not wired
/// into main.rs yet since the daemon currently relies on kill_on_drop.
#[allow(dead_code)]
pub struct BackendHandle {
    pub jan_child: Option<Child>,
    pub voice_child: Option<Child>,
}

impl BackendHandle {
    /// Not called yet -- see struct-level doc comment above.
    #[allow(dead_code)]
    pub async fn shutdown(&mut self) {
        if let Some(mut c) = self.jan_child.take() {
            let _ = c.kill().await;
        }
        if let Some(mut c) = self.voice_child.take() {
            let _ = c.kill().await;
        }
    }
}

/// Spawns Jan.ai's CLI-served local model as a supervised child process.
/// Per Jan's actual CLI (https://www.jan.ai/docs/desktop/cli, "jan serve"),
/// the invocation is `jan serve [MODEL_ID] [OPTIONS]` -- the model id is a
/// positional argument, not a flag, there is no --host or --data-dir flag
/// (Jan manages its own data folder itself), and the server listens on
/// 127.0.0.1 by default. We deliberately do NOT pass --detach: this
/// process is meant to stay attached as protogen-daemon's child so
/// kill_on_drop and the supervise() restart loop below both work correctly
/// against it.
pub async fn start_jan_server(cfg: &BackendConfig) -> Result<Child> {
    info!("starting jan serve on port {} with model {}", cfg.jan_port, cfg.jan_model);
    let child = Command::new(&cfg.jan_binary)
        .arg("serve")
        .arg(&cfg.jan_model)
        .arg("--port").arg(cfg.jan_port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawning jan serve -- is Jan.ai installed and on PATH? see docs/JAN_SETUP.md")?;
    Ok(child)
}

/// Spawns the Python STT/TTS bridge as a long-lived subprocess. It reads
/// newline-delimited JSON commands on stdin ({"op":"listen"} /
/// {"op":"speak","text":"..."}) and writes newline-delimited JSON results
/// on stdout, so the daemon never has to re-launch a Python interpreter
/// per utterance (which is what made the old --voice/--speak flags slow to
/// turn around).
pub async fn start_voice_bridge(cfg: &BackendConfig) -> Result<Child> {
    info!("starting voice bridge: {} {}", cfg.voice_bridge_python.display(), cfg.voice_bridge_script.display());
    let child = Command::new(&cfg.voice_bridge_python)
        .arg(&cfg.voice_bridge_script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawning voice bridge python process")?;
    Ok(child)
}

/// Poll Jan.ai's /v1/models endpoint until it responds or we give up.
pub async fn wait_for_jan_ready(base_url: &str, timeout: Duration) -> bool {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if let Ok(resp) = client.get(format!("{base_url}/v1/models")).send().await {
            if resp.status().is_success() {
                return true;
            }
        }
        sleep(Duration::from_millis(500)).await;
    }
    false
}

/// Supervisor loop: restarts a backing process if it dies, with capped
/// retries so a genuinely broken install doesn't spin forever burning CPU.
pub async fn supervise<F, Fut>(name: &'static str, mut spawn_fn: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Child>>,
{
    let mut retries = 0u32;
    const MAX_RETRIES: u32 = 5;
    loop {
        match spawn_fn().await {
            Ok(mut child) => {
                retries = 0;
                match child.wait().await {
                    Ok(status) => warn!("{name} exited with {status}, restarting"),
                    Err(e) => error!("{name} wait() failed: {e}"),
                }
            }
            Err(e) => {
                error!("{name} failed to start: {e}");
            }
        }
        retries += 1;
        if retries > MAX_RETRIES {
            error!("{name} exceeded max restart attempts ({MAX_RETRIES}), giving up");
            return;
        }
        sleep(Duration::from_secs(2u64.pow(retries.min(5)))).await;
    }
}
