//! server.rs
//! ---------
//! Accepts connections on the Unix domain socket and speaks the
//! newline-delimited JSON protocol defined in ipc.rs. One task per client
//! connection; the Dispatcher and shared state are behind Arc so multiple
//! frontends (GTK UI + a CLI, say) could connect at once if needed, though
//! the common case is exactly one UI instance.
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use protogen_launcher::AppIndex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::dispatcher::{DispatchOutcome, Dispatcher};
use crate::ipc::{ClientMessage, ServerMessage, StepOutcome};
use crate::voice::VoiceBridge;

pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("protogenos").join("daemon.sock")
}

pub async fn run(dispatcher: Arc<Dispatcher>, voice: Option<Arc<VoiceBridge>>) -> Result<()> {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path)?;
    info!("listening on {}", path.display());

    loop {
        let (stream, _) = listener.accept().await?;
        let dispatcher = dispatcher.clone();
        let voice = voice.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, dispatcher, voice).await {
                warn!("client connection ended: {e}");
            }
        });
    }
}

async fn handle_client(
    stream: UnixStream,
    dispatcher: Arc<Dispatcher>,
    voice: Option<Arc<VoiceBridge>>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let msg: ClientMessage = match serde_json::from_str(trimmed) {
            Ok(m) => m,
            Err(e) => {
                send(&mut write_half, &ServerMessage::Error { message: format!("bad message: {e}") }).await?;
                continue;
            }
        };

        handle_message(msg, &dispatcher, &voice, &mut write_half).await?;
    }
}

fn handle_message<'a>(
    msg: ClientMessage,
    dispatcher: &'a Arc<Dispatcher>,
    voice: &'a Option<Arc<VoiceBridge>>,
    out: &'a mut OwnedWriteHalf,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(handle_message_inner(msg, dispatcher, voice, out))
}

async fn handle_message_inner(
    msg: ClientMessage,
    dispatcher: &Arc<Dispatcher>,
    voice: &Option<Arc<VoiceBridge>>,
    out: &mut OwnedWriteHalf,
) -> Result<()> {
    match msg {
        ClientMessage::Ping => send(out, &ServerMessage::Pong).await,

        ClientMessage::UserText { text } => {
            send(out, &ServerMessage::Researching).await.ok();

            // Progress narration: handle_text() streams "here's what I'm
            // thinking/doing" lines over this channel while it runs. We
            // race draining that channel against the dispatch future
            // itself (via tokio::select!) so each line reaches the socket
            // as soon as it's produced, not batched up after the fact --
            // out is a single &mut handle so only one side writes to it at
            // a time, but that's fine since we're not writing concurrently,
            // just interleaving as events arrive.
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let dispatch_fut = dispatcher.handle_text(&text, Some(&progress_tx));
            tokio::pin!(dispatch_fut);

            let outcome = loop {
                tokio::select! {
                    biased;
                    line = progress_rx.recv() => {
                        if let Some(line) = line {
                            send(out, &ServerMessage::Thinking { text: line }).await.ok();
                        }
                    }
                    result = &mut dispatch_fut => {
                        // Drain any remaining buffered lines that arrived
                        // right as the dispatch future completed.
                        while let Ok(line) = progress_rx.try_recv() {
                            send(out, &ServerMessage::Thinking { text: line }).await.ok();
                        }
                        break result;
                    }
                }
            };

            match outcome {
                Ok(DispatchOutcome::Reply { text }) => {
                    speak_in_background(voice, &text);
                    send(out, &ServerMessage::Reply { text }).await
                }
                Ok(DispatchOutcome::NeedsConfirmation { plan }) => {
                    speak_in_background(voice, &plan.reply);
                    send(out, &ServerMessage::PlanProposed { plan, from_memory: false }).await
                }
                Ok(DispatchOutcome::Ran { plan }) => {
                    speak_in_background(voice, &plan.reply);
                    let report = execute_and_report(&plan, dispatcher).await;
                    send(
                        out,
                        &ServerMessage::ExecutionResult {
                            plan_id: plan.id.clone(),
                            success: !report.stopped_early,
                            steps: report
                                .results
                                .into_iter()
                                .map(|r| StepOutcome { description: r.description, success: r.success, detail: r.detail })
                                .collect(),
                        },
                    )
                    .await
                }
                Err(e) => {
                    error!("dispatch error: {e}");
                    send(out, &ServerMessage::Error { message: e.to_string() }).await
                }
            }
        }

        ClientMessage::ApprovePlan { plan_id, trigger_phrase } => {
            match dispatcher.approve(&plan_id, &trigger_phrase).await {
                Ok(Some(plan)) => {
                    let report = execute_and_report(&plan, dispatcher).await;
                    let summary = if report.stopped_early {
                        "Something in that plan failed -- check the details.".to_string()
                    } else {
                        "Done.".to_string()
                    };
                    speak_in_background(voice, &summary);
                    send(
                        out,
                        &ServerMessage::ExecutionResult {
                            plan_id: plan.id.clone(),
                            success: !report.stopped_early,
                            steps: report
                                .results
                                .into_iter()
                                .map(|r| StepOutcome { description: r.description, success: r.success, detail: r.detail })
                                .collect(),
                        },
                    )
                    .await
                }
                Ok(None) => send(out, &ServerMessage::Error { message: "plan not found or expired".into() }).await,
                Err(e) => send(out, &ServerMessage::Error { message: e.to_string() }).await,
            }
        }

        ClientMessage::RejectPlan { plan_id } => {
            dispatcher.reject(&plan_id).await;
            send(out, &ServerMessage::Reply { text: "Cancelled.".into() }).await
        }

        ClientMessage::Forget { phrase } => {
            let removed = dispatcher.memory.forget(&phrase).unwrap_or(false);
            let text = if removed {
                format!("Forgot '{phrase}'.")
            } else {
                format!("Didn't have '{phrase}' remembered.")
            };
            send(out, &ServerMessage::Reply { text }).await
        }

        // A single "listen for N seconds, transcribe, then dispatch exactly
        // like typed text" cycle. Voice input never bypasses the
        // memory/research/plan pipeline -- a transcript is just text that
        // enters handle_text() the same as anything typed, so a
        // misheard word can at worst produce a wrong (still
        // confirmation-gated) plan, never a shortcut around it.
        ClientMessage::StartListening => {
            send(out, &ServerMessage::Listening { active: true }).await?;
            let Some(bridge) = voice.as_ref() else {
                return send(
                    out,
                    &ServerMessage::Error { message: "voice bridge is not available".into() },
                )
                .await;
            };
            match bridge.listen(5).await {
                Ok(text) if !text.trim().is_empty() => {
                    send(out, &ServerMessage::Transcript { text: text.clone() }).await?;
                    send(out, &ServerMessage::Listening { active: false }).await?;
                    // Re-dispatch as if it were typed text.
                    return handle_message(ClientMessage::UserText { text }, dispatcher, voice, out).await;
                }
                Ok(_) => send(out, &ServerMessage::Listening { active: false }).await,
                Err(e) => {
                    send(out, &ServerMessage::Listening { active: false }).await?;
                    send(out, &ServerMessage::Error { message: format!("listen failed: {e}") }).await
                }
            }
        }
        ClientMessage::StopListening => send(out, &ServerMessage::Listening { active: false }).await,
    }
}

async fn execute_and_report(
    plan: &protogen_plan::Plan,
    dispatcher: &Arc<Dispatcher>,
) -> protogen_cmdrunner::ExecutionReport {
    let app_index_guard = dispatcher.app_index.lock().await;
    protogen_cmdrunner::execute_plan(plan, &app_index_guard)
}

/// Fires off text-to-speech without blocking the caller or the socket
/// response -- speaking is a nice-to-have side effect, never something the
/// UI should wait on. Best-effort: a TTS failure just means silence, it
/// never surfaces as an error to the user.
fn speak_in_background(voice: &Option<Arc<VoiceBridge>>, text: &str) {
    let Some(bridge) = voice.clone() else { return };
    let text = text.to_string();
    if text.trim().is_empty() {
        return;
    }
    tokio::spawn(async move {
        if let Err(e) = bridge.speak(&text, None).await {
            warn!("speak failed: {e}");
        }
    });
}

async fn send(out: &mut (impl AsyncWriteExt + Unpin), msg: &ServerMessage) -> Result<()> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    out.write_all(line.as_bytes()).await?;
    Ok(())
}

/// Called on startup and periodically to keep the app launcher index fresh
/// as the user installs/removes software.
pub async fn refresh_app_index(app_index: Arc<Mutex<AppIndex>>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        match AppIndex::scan() {
            Ok(fresh) => *app_index.lock().await = fresh,
            Err(e) => warn!("app index refresh failed: {e}"),
        }
    }
}
