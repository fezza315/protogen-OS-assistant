//! plan.rs
//! --------
//! This is the single most important file in the project. It defines the
//! ONLY shapes a "system-changing action" can take. The LLM (Jan.ai /
//! DeepSeek) never gets a raw shell string handed to subprocess. It can only
//! ever produce a `Plan`, which is a list of `Step`s, and every `Step` must
//! deserialize into one of the `StepKind` variants below. Anything that
//! doesn't fit -- extra shell metacharacters, pipes, redirects, `&&`,
//! backticks, an unrecognized verb -- fails to parse and is dropped before
//! it ever reaches a process spawn.
//!
//! This mirrors the boundary that already existed in planner.py
//! ("actions can only be existing commands.json keys") but generalizes it:
//! instead of a fixed static list, the model can propose *new* structured
//! steps from an allowed vocabulary, and only those steps -- never freeform
//! text -- become a Plan. The vocabulary is closed. If you want to widen
//! what ProtogenOS can ever do, you widen it here, in code, not by
//! convincing the model.
use serde::{Deserialize, Serialize};

/// Every kind of system-affecting action ProtogenOS is capable of, ever.
/// This enum IS the capability boundary. No `Shell(String)` variant exists
/// on purpose -- see module docs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepKind {
    /// Launch a known, already-installed application (from the desktop
    /// launcher index) or focus its existing window if one is open.
    LaunchOrFocus { app_id: String },

    /// Install package(s) via the system's detected package manager.
    /// `manager` must match what the installer detected (pacman/apt/dnf/...);
    /// package names are validated against `[a-zA-Z0-9._+-]+` before use,
    /// no shell interpretation ever occurs.
    InstallPackage { manager: PackageManager, packages: Vec<String> },

    /// Remove package(s). Same validation as InstallPackage.
    RemovePackage { manager: PackageManager, packages: Vec<String> },

    /// A single systemctl verb against a single unit.
    /// verb is restricted to a closed set (see SystemctlVerb).
    Systemctl { verb: SystemctlVerb, unit: Option<String>, user_scope: bool },

    /// Reboot or shut down the machine. Always requires confirmation
    /// regardless of any other flag -- see plan.requires_confirmation().
    PowerAction { action: PowerActionKind },

    /// Close an already-open window belonging to a known app (via
    /// kdotool/wmctrl, mirroring how LaunchOrFocus finds windows).
    CloseWindow { app_id: String },

    /// Run a specific, named binary inside a new terminal window. `binary`
    /// must resolve to something on PATH or an absolute path that exists
    /// and is executable; `args` are passed through as a plain argument
    /// vector, never shell-interpreted, so no pipes/redirects/chaining are
    /// possible here even though this looks shell-adjacent. This is for
    /// requests like "run htop in a new terminal", not "run any shell
    /// command I want."
    RunInTerminal { binary: String, args: Vec<String> },

    /// A fixed, named "utility" action -- volume/brightness/lock/screenshot
    /// etc, equivalent to today's commands.json entries but resolved to a
    /// concrete arg vector at definition time, not built from LLM text.
    Utility { name: String },

    /// Write a small, size-capped text config value via a known-safe helper
    /// (kwriteconfig6-style key/value, never raw file contents from the LLM
    /// for anything under system paths). Used for things like "switch my
    /// window manager to Hyprland" style config edits.
    SetConfig { file: String, group: String, key: String, value: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    Pacman,
    Aur,
    Apt,
    Dnf,
    Zypper,
    Flatpak,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SystemctlVerb {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
    EnableNow,
    DisableNow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerActionKind {
    Reboot,
    Shutdown,
    Suspend,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub kind: StepKind,
    /// Human-readable line shown to the user in the plan preview, generated
    /// deterministically from `kind` (see `describe()`), never taken
    /// verbatim from the LLM. Kept here for UI convenience/caching only.
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    /// Narrated reasoning shown to the user as a distinct "thinking" line
    /// before the final reply/plan -- empty for trivial plans that never
    /// went through the model (direct app launch, memory-bank hits).
    #[serde(default)]
    pub thinking: String,
    /// What the assistant will say about the plan before running it.
    pub reply: String,
    pub steps: Vec<Step>,
    /// True if this plan touches packages, systemctl, power, or config --
    /// i.e. anything beyond LaunchOrFocus/Utility. These always require
    /// explicit user confirmation, no override.
    pub requires_confirmation: bool,
}

impl StepKind {
    /// Deterministic, code-generated description -- never model text --
    /// so what's shown to the user always matches what will actually run.
    pub fn describe(&self) -> String {
        match self {
            StepKind::LaunchOrFocus { app_id } => format!("Open or focus {app_id}"),
            StepKind::CloseWindow { app_id } => format!("Close {app_id}"),
            StepKind::RunInTerminal { binary, args } => {
                if args.is_empty() {
                    format!("Run '{binary}' in a new terminal")
                } else {
                    format!("Run '{binary} {}' in a new terminal", args.join(" "))
                }
            }
            StepKind::InstallPackage { manager, packages } => {
                format!("Install via {manager:?}: {}", packages.join(", "))
            }
            StepKind::RemovePackage { manager, packages } => {
                format!("Remove via {manager:?}: {}", packages.join(", "))
            }
            StepKind::Systemctl { verb, unit, user_scope } => {
                let scope = if *user_scope { " (user)" } else { "" };
                match unit {
                    Some(u) => format!("systemctl {verb:?} {u}{scope}"),
                    None => format!("systemctl {verb:?}{scope}"),
                }
            }
            StepKind::PowerAction { action } => format!("{action:?} the machine"),
            StepKind::Utility { name } => format!("Run utility action: {name}"),
            StepKind::SetConfig { file, group, key, value } => {
                format!("Set [{file}] {group}/{key} = {value}")
            }
        }
    }

    /// Any step outside plain app launching/utility toggles is treated as
    /// system-changing and forces confirmation. This is intentionally
    /// coarse -- err toward asking.
    pub fn is_system_changing(&self) -> bool {
        !matches!(
            self,
            StepKind::LaunchOrFocus { .. } | StepKind::CloseWindow { .. } | StepKind::Utility { .. }
        )
    }
}

/// Package name validator. No spaces, no shell metacharacters, no path
/// separators. Applied to every entry in InstallPackage/RemovePackage
/// before the step is accepted into a Plan.
pub fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 128
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

/// systemd unit name validator -- same idea, slightly wider charset.
pub fn is_valid_unit_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 128
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-@:".contains(c))
}

/// Binary/arg validator for RunInTerminal. Defense in depth: even though
/// these are always passed as an argv vector (never through a shell, so
/// `;`/`|`/`` ` `` etc have no special meaning to the OS here), a
/// value containing them is still a signal something's off (a
/// prompt-injected or hallucinated step trying to look like a shell
/// pipeline), so we reject on sight rather than silently execute it.
pub fn is_valid_terminal_token(s: &str) -> bool {
    !s.is_empty() && s.len() < 512 && !s.contains(['\n', '\0', ';', '|', '&', '`', '$', '<', '>'])
}

impl Plan {
    /// Re-derives requires_confirmation from the actual steps rather than
    /// trusting a flag that came from deserialized/model-influenced data.
    pub fn finalize(mut self) -> Self {
        self.requires_confirmation = self.steps.iter().any(|s| s.kind.is_system_changing());
        for step in &mut self.steps {
            step.description = step.kind.describe();
        }
        self
    }

    /// Validates every step's data against the allowlists above. Called
    /// before a Plan is ever persisted or handed to cmdrunner. Returns the
    /// list of steps that were dropped (for logging / telling the user
    /// "I couldn't safely include X").
    pub fn sanitize(mut self) -> (Self, Vec<String>) {
        let mut dropped = Vec::new();
        self.steps.retain(|step| {
            let ok = match &step.kind {
                StepKind::InstallPackage { packages, .. }
                | StepKind::RemovePackage { packages, .. } => {
                    packages.iter().all(|p| is_valid_package_name(p)) && !packages.is_empty()
                }
                StepKind::Systemctl { unit: Some(u), .. } => is_valid_unit_name(u),
                StepKind::Systemctl { unit: None, .. } => true,
                StepKind::RunInTerminal { binary, args } => {
                    is_valid_terminal_token(binary) && args.iter().all(|a| is_valid_terminal_token(a))
                }
                StepKind::SetConfig { file, group, key, value } => {
                    let sane = |s: &str| {
                        !s.is_empty() && s.len() < 256 && !s.contains(['\n', '\0', ';', '|', '`'])
                    };
                    sane(file) && sane(group) && sane(key) && sane(value)
                }
                _ => true,
            };
            if !ok {
                dropped.push(step.description.clone());
            }
            ok
        });
        (self.finalize(), dropped)
    }
}
