//! dispatcher.rs
//! -------------
//! The request-handling brain, but NOT the security boundary -- that lives
//! in protogen-plan (the closed StepKind vocabulary) and protogen-cmdrunner
//! (the only place a Command actually spawns). This module just decides,
//! for a given user utterance:
//!   1. Is this a known phrase in the memory bank? -> return its cached
//!      Plan (fast path, "instant" per the original request).
//!   2. Does it match a launchable app directly ("open firefox")? -> build
//!      a trivial one-step LaunchOrFocus plan without even calling the LLM.
//!   3. Otherwise: research + ask Jan.ai for a plan, sanitize it, and if it
//!      has any system-changing steps, hold it pending confirmation instead
//!      of running it.
//! Plans with ONLY launch/utility steps run immediately without a
//! confirmation prompt (matches today's low-friction "open X" / "volume up"
//! behavior). Anything else always stops for a yes/no.
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use protogen_launcher::AppIndex;
use protogen_plan::{Plan, Step, StepKind};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;

use crate::jan_client::JanClient;
use crate::memory::MemoryBank;
use crate::personality::PROFILE;
use crate::research;

pub struct Dispatcher {
    pub memory: MemoryBank,
    pub app_index: Arc<Mutex<AppIndex>>,
    pub jan: JanClient,
    pub distro: String,
    /// Plans awaiting explicit user approval, keyed by plan id -> (plan, original request text).
    pub pending: Mutex<HashMap<String, (Plan, String)>>,
    pub history: Mutex<Vec<(String, String)>>,
}

pub enum DispatchOutcome {
    /// Ran immediately (launch/utility only), here's what happened.
    Ran { plan: Plan },
    /// Needs user approval before anything runs.
    NeedsConfirmation { plan: Plan },
    /// Just talk, nothing to do.
    Reply { text: String },
}

impl Dispatcher {
    /// `progress`, if given, receives human-readable "here's what I'm
    /// doing/finding/thinking" updates as the request is handled -- shown
    /// live in the UI so the user isn't just staring at a spinner between
    /// "Researching" and the final plan/reply. Purely observational; never
    /// used for control flow.
    pub async fn handle_text(
        &self,
        text: &str,
        progress: Option<&UnboundedSender<String>>,
    ) -> Result<DispatchOutcome> {
        let say = |msg: String| {
            if let Some(tx) = progress {
                let _ = tx.send(msg);
            }
        };

        // 1. Direct app-launch fast path -- no LLM round trip needed at all
        //    for the single most common utterance shape.
        {
            let idx = self.app_index.lock().await;
            let lower = text.to_lowercase();
            if lower.starts_with("open ") || lower.starts_with("launch ") || lower.starts_with("focus ") {
                if let Some(app) = idx.resolve(&lower) {
                    say(format!("Matched '{text}' directly to the app '{}' in the launcher index -- no need to ask the model.", app.name));
                    let step = Step {
                        kind: StepKind::LaunchOrFocus { app_id: app.id.clone() },
                        description: format!("Open or focus {}", app.name),
                    };
                    let plan = Plan {
                        id: uuid::Uuid::new_v4().to_string(),
                        thinking: String::new(),
                        reply: format!("Opening {}.", app.name),
                        steps: vec![step],
                        requires_confirmation: false,
                    }
                    .finalize();
                    return Ok(DispatchOutcome::Ran { plan });
                }
            }
        }

        // 2. Memory bank fast path.
        if let Some(plan) = self.memory.lookup_phrase(text)? {
            say("Found this exact request in memory -- skipping research.".to_string());
            return Ok(if plan.requires_confirmation {
                DispatchOutcome::NeedsConfirmation { plan }
            } else {
                DispatchOutcome::Ran { plan }
            });
        }

        // 3. Unknown -- research + ask the model for a plan.
        say("Nothing in memory for this -- looking it up and asking the model for a plan.".to_string());
        let (known_apps, known_utilities) = {
            let idx = self.app_index.lock().await;
            (idx.all_names(), utility_names())
        };

        let web_context = research::research_context(text, &self.distro).await;
        if web_context.is_empty() {
            say("Research turned up nothing useful -- relying on the model's own knowledge.".to_string());
        } else {
            say(format!("Research findings:\n{web_context}"));
        }
        let web_context = if web_context.is_empty() { None } else { Some(web_context.as_str()) };

        let history = self.history.lock().await.clone();
        let system_prompt = PROFILE.system_prompt();

        let raw_plan = self
            .jan
            .research_plan(text, &system_prompt, &known_apps, &known_utilities, web_context, &history)
            .await?;

        if !raw_plan.thinking.is_empty() {
            say(format!("Thinking: {}", raw_plan.thinking));
        }
        say(format!("Decided: {}", raw_plan.reply));
        if !raw_plan.steps.is_empty() {
            let step_list: Vec<String> = raw_plan.steps.iter().map(|s| s.description.clone()).collect();
            say(format!("Proposed steps before validation:\n- {}", step_list.join("\n- ")));
        }

        let (plan, dropped) = raw_plan.sanitize();
        if !dropped.is_empty() {
            tracing::warn!("dropped unsafe/invalid steps from LLM plan: {dropped:?}");
            say(format!(
                "Note: dropped {} step(s) that didn't pass validation: {}",
                dropped.len(),
                dropped.join(", ")
            ));
        }

        {
            let mut h = self.history.lock().await;
            h.push(("user".into(), text.to_string()));
            h.push(("assistant".into(), plan.reply.clone()));
        }

        if plan.steps.is_empty() {
            return Ok(DispatchOutcome::Reply { text: plan.reply });
        }

        if plan.requires_confirmation {
            self.pending
                .lock()
                .await
                .insert(plan.id.clone(), (plan.clone(), text.to_string()));
            Ok(DispatchOutcome::NeedsConfirmation { plan })
        } else {
            Ok(DispatchOutcome::Ran { plan })
        }
    }

    /// Called once the UI reports the user approved a pending plan.
    /// Remembers it in the memory bank under the ORIGINAL request text
    /// (not the plan id) so next time the same phrasing is instant, and
    /// returns the plan for execution by the caller. `trigger_phrase` from
    /// the client is accepted but the authoritative one is whatever text
    /// originally produced this plan, to avoid a client bug poisoning the
    /// memory bank with an arbitrary key.
    pub async fn approve(&self, plan_id: &str, _client_trigger_phrase: &str) -> Result<Option<Plan>> {
        let entry = self.pending.lock().await.remove(plan_id);
        if let Some((plan, original_text)) = &entry {
            self.memory.remember(&[original_text.clone()], plan)?;
        }
        Ok(entry.map(|(plan, _)| plan))
    }

    pub async fn reject(&self, plan_id: &str) {
        self.pending.lock().await.remove(plan_id);
    }
}

fn utility_names() -> Vec<String> {
    [
        "volume_up",
        "volume_down",
        "mute",
        "brightness_up",
        "brightness_down",
        "lock_screen",
        "screenshot",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
