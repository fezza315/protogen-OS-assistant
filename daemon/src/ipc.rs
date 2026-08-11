//! ipc.rs
//! ------
//! Newline-delimited JSON protocol over a Unix domain socket
//! ($XDG_RUNTIME_DIR/protogenos/daemon.sock) between protogen-daemon and the
//! GTK UI (or any other frontend, including a CLI). Kept intentionally
//! small and textual so it's easy to `socat -` into for debugging.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Free text (from typed input or STT transcript) the user wants
    /// acted on or chatted about.
    UserText { text: String },
    /// User approved a previously-shown plan by id. `trigger_phrase` is the
    /// original request text, stored alongside the plan in the memory bank
    /// so the same phrasing is instant next time.
    ApprovePlan { plan_id: String, trigger_phrase: String },
    /// User rejected a previously-shown plan by id.
    RejectPlan { plan_id: String },
    /// Ask the daemon to start/stop listening via the voice bridge.
    StartListening,
    StopListening,
    /// Remove a remembered phrase from the memory bank.
    Forget { phrase: String },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Plain conversational reply, nothing to run (spoken + shown).
    Reply { text: String },
    /// A plan awaiting user confirmation before anything executes.
    PlanProposed { plan: protogen_plan::Plan, from_memory: bool },
    /// Daemon is researching an unknown request (so the UI can show a
    /// "thinking / searching" state instead of looking frozen).
    Researching,
    /// Result of executing an approved plan, step by step.
    ExecutionResult { plan_id: String, success: bool, steps: Vec<StepOutcome> },
    Transcript { text: String },
    Listening { active: bool },
    Error { message: String },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepOutcome {
    pub description: String,
    pub success: bool,
    pub detail: String,
}
