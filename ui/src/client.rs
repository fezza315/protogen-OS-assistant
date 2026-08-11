//! client.rs
//! ---------
//! Owns the Unix socket connection to protogen-daemon. Reads come in on a
//! background tokio task and are marshalled onto the GTK main loop via a
//! glib channel (GTK widgets are not thread-safe to touch from a tokio
//! task directly). Writes go out through a plain blocking std
//! UnixStream::try_clone'd handle wrapped for simplicity -- message volume
//! here is tiny (one line per user utterance / daemon event) so this
//! doesn't need to be fully async on the write side.
use std::cell::RefCell;
use std::io::Write;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::ButtonExt;
use serde::{Deserialize, Serialize};

use crate::avatar::{AvatarState, AvatarWidget};
use crate::chat_log::ChatLog;
use protogen_plan::Plan;

fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("protogenos").join("daemon.sock")
}

// Mirrors daemon/src/ipc.rs -- kept independent (not a shared crate) since
// the UI should still build even if daemon internals shift shape slightly;
// any mismatch just fails message parsing gracefully rather than failing
// the whole build.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    UserText { text: String },
    ApprovePlan { plan_id: String, trigger_phrase: String },
    RejectPlan { plan_id: String },
    StartListening,
    StopListening,
    #[allow(dead_code)]
    Forget { phrase: String },
    #[allow(dead_code)]
    Ping,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Reply { text: String },
    PlanProposed { plan: Plan, #[allow(dead_code)] from_memory: bool },
    Researching,
    ExecutionResult {
        #[allow(dead_code)]
        plan_id: String,
        #[allow(dead_code)]
        success: bool,
        steps: Vec<StepOutcome>,
    },
    #[allow(dead_code)]
    Transcript { text: String },
    #[allow(dead_code)]
    Listening { active: bool },
    Error { message: String },
    #[allow(dead_code)]
    Pong,
}

#[derive(Debug, Clone, Deserialize)]
struct StepOutcome {
    description: String,
    success: bool,
    detail: String,
}

pub struct DaemonClient {
    writer: Option<StdUnixStream>,
    last_user_text: Rc<RefCell<String>>,
}

impl DaemonClient {
    pub fn new() -> Self {
        Self { writer: None, last_user_text: Rc::new(RefCell::new(String::new())) }
    }

    fn send(&self, msg: &ClientMessage) {
        if let Some(mut w) = self.writer.as_ref().and_then(|w| w.try_clone().ok()) {
            if let Ok(mut line) = serde_json::to_string(msg) {
                line.push('\n');
                let _ = w.write_all(line.as_bytes());
            }
        } else {
            tracing_log_fallback("not connected to protogen-daemon yet");
        }
    }

    pub fn send_text(&self, text: &str) {
        *self.last_user_text.borrow_mut() = text.to_string();
        self.send(&ClientMessage::UserText { text: text.to_string() });
    }

    pub fn start_listening(&self) {
        self.send(&ClientMessage::StartListening);
    }

    pub fn stop_listening(&self) {
        self.send(&ClientMessage::StopListening);
    }

    pub fn approve_plan(&self, plan_id: &str) {
        let trigger_phrase = self.last_user_text.borrow().clone();
        self.send(&ClientMessage::ApprovePlan { plan_id: plan_id.to_string(), trigger_phrase });
    }

    pub fn reject_plan(&self, plan_id: &str) {
        self.send(&ClientMessage::RejectPlan { plan_id: plan_id.to_string() });
    }
}

fn tracing_log_fallback(msg: &str) {
    eprintln!("[protogen-ui] {msg}");
}

/// Spawns the background connection thread and wires an async-channel so
/// incoming daemon messages update chat log / avatar state on the GTK main
/// thread. (glib 0.20 removed `MainContext::channel`; `async-channel` +
/// `glib::spawn_future_local` is the current recommended replacement.)
pub fn spawn_connection(client: Rc<RefCell<DaemonClient>>, log: ChatLog, avatar: AvatarWidget) {
    let (sender, receiver) = async_channel::unbounded::<ServerMessage>();

    // Background OS thread (not a tokio task) doing blocking I/O against
    // the socket -- simplest reliable way to bridge a Unix socket into a
    // GTK app without pulling the whole app onto a tokio runtime.
    std::thread::spawn(move || loop {
        match StdUnixStream::connect(socket_path()) {
            Ok(stream) => {
                use std::io::{BufRead, BufReader};
                let read_stream = stream.try_clone().expect("clone socket for reading");
                let reader = BufReader::new(read_stream);
                for line in reader.lines() {
                    match line {
                        Ok(text) => {
                            if let Ok(msg) = serde_json::from_str::<ServerMessage>(&text) {
                                // send_blocking is fine here: this thread is
                                // a plain OS thread, not the GTK main loop,
                                // and the channel is unbounded so this never
                                // blocks waiting on the receiver.
                                let _ = sender.send_blocking(msg);
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    });

    // Separate, short-lived connection attempt loop purely to obtain the
    // write half once the socket exists. The connected StdUnixStream
    // itself is Send, so it can cross this std::sync::mpsc channel safely;
    // what we avoid is sending a closure that captures the Rc<RefCell<..>>
    // client (which is not Send) into glib::idle_add_once.
    let (writer_tx, writer_rx) = std::sync::mpsc::channel::<StdUnixStream>();
    std::thread::spawn(move || loop {
        if let Ok(stream) = StdUnixStream::connect(socket_path()) {
            let _ = writer_tx.send(stream);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    });

    // Polls the mpsc receiver from the GTK main thread (glib::timeout_add_local
    // runs its closure on the thread it was called from, so touching the
    // non-Send Rc<RefCell<DaemonClient>> here is fine) and installs the
    // writer once it arrives, then stops polling.
    {
        let client = client.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            match writer_rx.try_recv() {
                Ok(stream) => {
                    client.borrow_mut().writer = Some(stream);
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    // Runs on the GTK main context; each iteration handles one daemon
    // message and updates widgets directly, which is safe here since we're
    // on the main thread.
    glib::spawn_future_local(async move {
        while let Ok(msg) = receiver.recv().await {
            match msg {
                ServerMessage::Reply { text } => {
                    log.push_assistant(&text);
                    avatar.set_state(AvatarState::Idle);
                }
                ServerMessage::Researching => {
                    avatar.set_state(AvatarState::Thinking);
                    log.push_system_note("researching...");
                }
                ServerMessage::PlanProposed { plan, .. } => {
                    avatar.set_state(AvatarState::Idle);
                    let card = log.push_plan(&plan);
                    let client_approve = client.clone();
                    let plan_id_approve = card.plan_id.clone();
                    let card_for_disable = card.clone();
                    card.approve_btn.connect_clicked(move |_| {
                        client_approve.borrow().approve_plan(&plan_id_approve);
                        card_for_disable.disable_actions();
                    });
                    let client_reject = client.clone();
                    let plan_id_reject = card.plan_id.clone();
                    let card_for_disable2 = card.clone();
                    card.reject_btn.connect_clicked(move |_| {
                        client_reject.borrow().reject_plan(&plan_id_reject);
                        card_for_disable2.disable_actions();
                    });
                }
                ServerMessage::ExecutionResult { plan_id: _, success: _, steps } => {
                    avatar.set_state(AvatarState::Idle);
                    for step in steps {
                        let mark = if step.success { "done" } else { "failed" };
                        log.push_system_note(&format!("{} -- {} ({})", step.description, mark, step.detail));
                    }
                }
                ServerMessage::Error { message } => {
                    log.push_system_note(&format!("Error: {message}"));
                    avatar.set_state(AvatarState::Idle);
                }
                ServerMessage::Transcript { text } => {
                    log.push_user(&text);
                }
                ServerMessage::Listening { active } => {
                    avatar.set_state(if active { AvatarState::Listening } else { AvatarState::Idle });
                }
                ServerMessage::Pong => {}
            }
        }
    });
}
