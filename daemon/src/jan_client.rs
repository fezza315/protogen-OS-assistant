//! jan_client.rs
//! -------------
//! Thin client for Jan.ai's local server (https://jan.ai), which exposes an
//! OpenAI-compatible /v1/chat/completions endpoint on localhost once a model
//! is loaded. We point it at a DeepSeek model (configurable; default
//! "deepseek-v4" per the model name Jan.ai lists it under -- Jan resolves
//! the actual GGUF/model files itself).
//!
//! This module ONLY ever asks the model for JSON matching `LlmPlanResponse`
//! and hands that to plan::Plan::sanitize() before it's trusted anywhere
//! else. It never executes anything itself.
//!
//! Jan.ai is expected to already be running as a backing process (see
//! backend.rs / process_manager.rs) before this client is used -- the
//! daemon starts `jan-server` itself on launch so the user never has to.
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;

use protogen_plan::{PackageManager, Plan, PowerActionKind, Step, StepKind, SystemctlVerb};

#[derive(Debug, Clone)]
pub struct JanClient {
    pub base_url: String,
    pub model: String,
    http: reqwest::Client,
}

/// The ONLY shape we ask the model to produce. Deliberately mirrors
/// plan::Plan/Step but as a permissive intermediate type -- untrusted until
/// converted + sanitized. `raw_steps` are loosely-typed JSON values so a
/// malformed/hallucinated step doesn't fail the whole response; each one is
/// individually attempted against StepKind and dropped on failure.
#[derive(Debug, Deserialize)]
struct LlmPlanResponse {
    /// Short, narrated "what I'm figuring out / about to do" -- shown to
    /// the user as a distinct thinking/status line before the plan itself,
    /// separate from `reply` (which is the in-character summary once the
    /// plan is settled). Optional because trivial requests (direct app
    /// launch, memory-bank hits) never reach the model at all and so never
    /// need one.
    #[serde(default)]
    thinking: String,
    reply: String,
    #[serde(default)]
    steps: Vec<serde_json::Value>,
}

impl JanClient {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("building reqwest client"),
        }
    }

    /// Not called yet -- reserved for a future startup health-check gate
    /// before accepting socket connections.
    #[allow(dead_code)]
    pub async fn health(&self) -> bool {
        self.http
            .get(format!("{}/v1/models", self.base_url))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Ask the model to research an unknown request and propose a plan.
    /// `known_apps` and `known_utilities` are given so the model prefers
    /// LaunchOrFocus/Utility steps over package installs when possible.
    /// `web_context` is optional pre-fetched research text (see
    /// research.rs) the daemon gathered before calling this, so the model
    /// isn't relying purely on training data for package/unit names.
    pub async fn research_plan(
        &self,
        user_request: &str,
        system_prompt: &str,
        known_apps: &[String],
        known_utilities: &[String],
        web_context: Option<&str>,
        history: &[(String, String)],
    ) -> Result<Plan> {
        let schema_note = r#"
Respond with ONLY a single JSON object, no prose outside it, in exactly this shape:
{
  "thinking": "<1-3 short sentences, in character, narrating what you're figuring out -- what the request needs, which approach you're picking and why, anything you're unsure about. This is shown to the user as your visible thought process, separate from the final reply, so actually think out loud here rather than leaving it generic.>",
  "reply": "<what you say out loud to the user once you've decided, in character, briefly explaining the plan>",
  "steps": [
    {"kind": "launch_or_focus", "app_id": "<desktop file id or binary name>"},
    {"kind": "close_window", "app_id": "<desktop file id or binary name of the app to close>"},
    {"kind": "run_in_terminal", "binary": "<binary name or absolute path>", "args": ["arg1", "arg2"]},
    {"kind": "install_package", "manager": "pacman|aur|apt|dnf|zypper|flatpak", "packages": ["pkgname"]},
    {"kind": "remove_package", "manager": "pacman|aur|apt|dnf|zypper|flatpak", "packages": ["pkgname"]},
    {"kind": "systemctl", "verb": "start|stop|restart|enable|disable|enable_now|disable_now", "unit": "name.service", "user_scope": false},
    {"kind": "power_action", "action": "reboot|shutdown|suspend"},
    {"kind": "utility", "name": "<one of the known utility names>"},
    {"kind": "set_config", "file": "kwinrc", "group": "GroupName", "key": "keyname", "value": "value"}
  ]
}
Use ONLY these step kinds and ONLY the enum values shown. If a request needs
something outside these kinds, explain what's missing in "reply" and return
an empty steps list. Never include shell syntax, pipes, or explanations
inside a field value beyond the literal package/unit/binary/config value
itself -- "args" is a plain argument list, not a shell command line, so
never put flags-and-a-pipe or "&&" chains in a single arg string.

For app_id fields (launch_or_focus / close_window), the user will often use
a short, casual, or partial name instead of the exact app name -- "libre
office" for LibreOffice Writer, "the browser" for whichever browser is
installed, "gimp" for GNU Image Manipulation Program. Match against the
closest entry in "Known launchable apps" below by meaning, not exact
string; put whatever name you land on in app_id and the launcher will do
its own fuzzy resolution against the real desktop entry on top of that.
"#;

        let apps_block = if known_apps.is_empty() {
            "(none indexed yet)".to_string()
        } else {
            known_apps.join(", ")
        };
        let util_block = if known_utilities.is_empty() {
            "(none defined)".to_string()
        } else {
            known_utilities.join(", ")
        };

        let convo: String = history
            .iter()
            .rev()
            .take(6)
            .rev()
            .map(|(role, text)| format!("{role}: {text}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut prompt = format!(
            "{system_prompt}\n\nKnown launchable apps: {apps_block}\nKnown utility actions: {util_block}\n{schema_note}"
        );
        if let Some(ctx) = web_context {
            prompt.push_str(&format!(
                "\n\nResearch context gathered from the web for this request \
                 (use it for correct package/unit names, but still only emit \
                 the allowed step kinds above):\n{ctx}\n"
            ));
        }
        if !convo.is_empty() {
            prompt.push_str(&format!("\n\nConversation so far:\n{convo}\n"));
        }
        prompt.push_str(&format!("\nuser: {user_request}\nassistant JSON:"));

        let body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.2,
            "stream": false
        });

        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .send()
            .await
            .context("calling Jan.ai server")?;

        if !resp.status().is_success() {
            bail!("Jan.ai server returned status {}", resp.status());
        }

        let value: serde_json::Value = resp.json().await.context("parsing Jan.ai response")?;
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();

        let parsed: LlmPlanResponse = extract_json(content).unwrap_or(LlmPlanResponse {
            thinking: String::new(),
            reply: content.trim().to_string(),
            steps: vec![],
        });

        let mut steps = Vec::new();
        for raw in parsed.steps {
            if let Some(kind) = try_parse_step(&raw) {
                steps.push(Step { description: kind.describe(), kind });
            }
        }

        Ok(Plan {
            id: uuid::Uuid::new_v4().to_string(),
            thinking: parsed.thinking,
            reply: parsed.reply,
            steps,
            requires_confirmation: true, // recomputed properly in finalize()
        }
        .finalize())
    }
}

/// Pull the first {...} block out of a response that may have prose or
/// code fences around it (models do this constantly).
fn extract_json(text: &str) -> Option<LlmPlanResponse> {
    let text = text.trim();
    let candidate = if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            &text[start..=end]
        } else {
            text
        }
    } else {
        text
    };
    serde_json::from_str(candidate).ok()
}

/// Best-effort conversion from a loosely-typed JSON value into a real,
/// closed-vocabulary StepKind. Returns None (silently dropped) for anything
/// that doesn't match -- this is the actual enforcement point, same role as
/// the `safe_actions` filter in the original planner.py.
fn try_parse_step(v: &serde_json::Value) -> Option<StepKind> {
    let kind = v.get("kind")?.as_str()?;
    match kind {
        "launch_or_focus" => Some(StepKind::LaunchOrFocus {
            app_id: v.get("app_id")?.as_str()?.to_string(),
        }),
        "close_window" => Some(StepKind::CloseWindow {
            app_id: v.get("app_id")?.as_str()?.to_string(),
        }),
        "run_in_terminal" => Some(StepKind::RunInTerminal {
            binary: v.get("binary")?.as_str()?.to_string(),
            args: v
                .get("args")
                .and_then(|a| a.as_array())
                .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
        }),
        "install_package" => Some(StepKind::InstallPackage {
            manager: parse_manager(v.get("manager")?.as_str()?)?,
            packages: parse_string_list(v.get("packages")?)?,
        }),
        "remove_package" => Some(StepKind::RemovePackage {
            manager: parse_manager(v.get("manager")?.as_str()?)?,
            packages: parse_string_list(v.get("packages")?)?,
        }),
        "systemctl" => Some(StepKind::Systemctl {
            verb: parse_verb(v.get("verb")?.as_str()?)?,
            unit: v.get("unit").and_then(|u| u.as_str()).map(|s| s.to_string()),
            user_scope: v.get("user_scope").and_then(|b| b.as_bool()).unwrap_or(false),
        }),
        "power_action" => Some(StepKind::PowerAction {
            action: parse_power(v.get("action")?.as_str()?)?,
        }),
        "utility" => Some(StepKind::Utility {
            name: v.get("name")?.as_str()?.to_string(),
        }),
        "set_config" => Some(StepKind::SetConfig {
            file: v.get("file")?.as_str()?.to_string(),
            group: v.get("group")?.as_str()?.to_string(),
            key: v.get("key")?.as_str()?.to_string(),
            value: v.get("value")?.as_str()?.to_string(),
        }),
        _ => None,
    }
}

fn parse_manager(s: &str) -> Option<PackageManager> {
    Some(match s {
        "pacman" => PackageManager::Pacman,
        "aur" => PackageManager::Aur,
        "apt" => PackageManager::Apt,
        "dnf" => PackageManager::Dnf,
        "zypper" => PackageManager::Zypper,
        "flatpak" => PackageManager::Flatpak,
        _ => return None,
    })
}

fn parse_verb(s: &str) -> Option<SystemctlVerb> {
    Some(match s {
        "start" => SystemctlVerb::Start,
        "stop" => SystemctlVerb::Stop,
        "restart" => SystemctlVerb::Restart,
        "enable" => SystemctlVerb::Enable,
        "disable" => SystemctlVerb::Disable,
        "enable_now" => SystemctlVerb::EnableNow,
        "disable_now" => SystemctlVerb::DisableNow,
        _ => return None,
    })
}

fn parse_power(s: &str) -> Option<PowerActionKind> {
    Some(match s {
        "reboot" => PowerActionKind::Reboot,
        "shutdown" => PowerActionKind::Shutdown,
        "suspend" => PowerActionKind::Suspend,
        _ => return None,
    })
}

fn parse_string_list(v: &serde_json::Value) -> Option<Vec<String>> {
    let arr = v.as_array()?;
    let out: Vec<String> = arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
