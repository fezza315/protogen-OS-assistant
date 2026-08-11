//! protogen-daemon
//! ----------------
//! Entry point. Starts (or connects to) Jan.ai as a backing process, loads
//! the app launcher index and memory bank, opens the IPC socket, and runs
//! forever. This is the ONE thing meant to be started by the user/systemd
//! -- everything else (Jan.ai server, voice bridge) is started BY this
//! process so the user never manages them separately.
mod backend;
mod dispatcher;
mod ipc;
mod jan_client;
mod memory;
mod personality;
mod research;
mod server;
mod voice;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use protogen_launcher::AppIndex;
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::backend::BackendConfig;
use crate::dispatcher::Dispatcher;
use crate::jan_client::JanClient;
use crate::memory::MemoryBank;

fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("os", "ProtogenOS", "protogenos")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".protogenos"))
}

fn detect_distro() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                line.strip_prefix("ID=").map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "linux".to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    info!("ProtogenOS daemon starting");

    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;

    let jan_port: u16 = std::env::var("PROTOGEN_JAN_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1337);
    let jan_model =
        std::env::var("PROTOGEN_JAN_MODEL").unwrap_or_else(|_| "deepseek-v4".to_string());
    let jan_base_url = format!("http://127.0.0.1:{jan_port}");

    let backend_cfg = BackendConfig {
        jan_binary: PathBuf::from(std::env::var("PROTOGEN_JAN_BINARY").unwrap_or_else(|_| "jan".to_string())),
        jan_data_dir: dir.join("jan"),
        jan_port,
        jan_model: jan_model.clone(),
        voice_bridge_script: PathBuf::from(
            std::env::var("PROTOGEN_VOICE_BRIDGE").unwrap_or_else(|_| {
                "/usr/share/protogenos/voice_bridge/voice_bridge.py".to_string()
            }),
        ),
        voice_bridge_python: PathBuf::from(
            std::env::var("PROTOGEN_VOICE_PYTHON").unwrap_or_else(|_| "python3".to_string()),
        ),
    };

    // Start Jan.ai as a supervised backing process. The user never runs
    // `jan serve` themselves.
    {
        let cfg_clone_binary = backend_cfg.jan_binary.clone();
        let cfg_clone_data = backend_cfg.jan_data_dir.clone();
        let cfg_clone_port = backend_cfg.jan_port;
        let cfg_clone_model = backend_cfg.jan_model.clone();
        tokio::spawn(async move {
            let cfg = BackendConfig {
                jan_binary: cfg_clone_binary,
                jan_data_dir: cfg_clone_data,
                jan_port: cfg_clone_port,
                jan_model: cfg_clone_model,
                voice_bridge_script: PathBuf::new(),
                voice_bridge_python: PathBuf::new(),
            };
            backend::supervise("jan-server", || backend::start_jan_server(&cfg)).await;
        });
    }

    info!("waiting for Jan.ai server to become ready...");
    let ready = backend::wait_for_jan_ready(&jan_base_url, std::time::Duration::from_secs(60)).await;
    if !ready {
        tracing::warn!(
            "Jan.ai server did not become ready in time -- the assistant will retry \
             per-request, but responses will be slow/erroring until it comes up. \
             Check that Jan.ai is installed (see docs/JAN_SETUP.md)."
        );
    } else {
        info!("Jan.ai server ready at {jan_base_url}");
    }

    let app_index = Arc::new(Mutex::new(AppIndex::scan()?));
    let memory = MemoryBank::open(&dir.join("memory.sqlite3"))?;
    let jan = JanClient::new(jan_base_url, jan_model);
    let distro = detect_distro();

    let dispatcher = Arc::new(Dispatcher {
        memory,
        app_index: app_index.clone(),
        jan,
        distro,
        pending: Mutex::new(Default::default()),
        history: Mutex::new(Vec::new()),
    });

    tokio::spawn(server::refresh_app_index(app_index.clone()));

    // Voice bridge is best-effort: if the script or python deps aren't
    // installed, voice features simply aren't available and everything
    // else (text chat, plans, execution) still works.
    let voice_bridge = match backend::start_voice_bridge(&backend_cfg).await {
        Ok(mut child) => {
            let stdin = child.stdin.take();
            let stdout = child.stdout.take();
            match (stdin, stdout) {
                (Some(stdin), Some(stdout)) => {
                    info!("voice bridge process started");
                    // Detach the child so it isn't killed when this Option
                    // goes out of scope; supervision/cleanup is left as a
                    // documented follow-up (see docs/VOICE_SETUP.md).
                    std::mem::forget(child);
                    Some(Arc::new(voice::VoiceBridge::new(stdin, stdout)))
                }
                _ => {
                    tracing::warn!("voice bridge started but stdio pipes unavailable");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!("voice bridge unavailable ({e}) -- voice features disabled, text chat still works");
            None
        }
    };

    server::run(dispatcher, voice_bridge).await
}
