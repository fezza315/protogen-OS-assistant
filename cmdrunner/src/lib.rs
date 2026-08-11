//! protogen-cmdrunner
//! -------------------
//! The ONLY place in ProtogenOS that turns a Step into an actual spawned
//! process. Every arm below builds an explicit `Command` with explicit
//! `.arg()` calls -- there is no `Command::new("sh").arg("-c").arg(...)`
//! anywhere in this file, and there must never be one added. If a future
//! change needs shell features (pipes, globs, env expansion), that need
//! should be met by adding a new, narrow StepKind variant in
//! protogen-plan, not by reaching for a shell here.
//!
//! Steps are executed one at a time, in order, and execution stops at the
//! first failure (a partially-applied system change is safer to reason
//! about than silently skipping ahead).
use anyhow::{bail, Result};
use protogen_launcher::{focus_or_launch, AppIndex};
use protogen_plan::{PackageManager, Plan, PowerActionKind, Step, StepKind, SystemctlVerb};
use std::process::{Command, Output, Stdio};

#[derive(Debug, Clone)]
pub struct StepResult {
    pub description: String,
    pub success: bool,
    pub detail: String,
}

pub struct ExecutionReport {
    pub results: Vec<StepResult>,
    pub stopped_early: bool,
}

/// Fixed utility action table -- equivalent to the old commands.json
/// entries, but the argument vectors live in code (not JSON a compromised
/// config file could rewrite) since this project treats utility toggles as
/// zero-confirmation actions.
fn utility_command(name: &str) -> Option<(&'static str, Vec<&'static str>)> {
    Some(match name {
        "volume_up" => ("pactl", vec!["set-sink-volume", "@DEFAULT_SINK@", "+5%"]),
        "volume_down" => ("pactl", vec!["set-sink-volume", "@DEFAULT_SINK@", "-5%"]),
        "mute" => ("pactl", vec!["set-sink-mute", "@DEFAULT_SINK@", "toggle"]),
        "brightness_up" => ("brightnessctl", vec!["set", "+10%"]),
        "brightness_down" => ("brightnessctl", vec!["set", "10%-"]),
        "lock_screen" => ("loginctl", vec!["lock-session"]),
        "screenshot" => ("spectacle", vec!["-b", "-n"]),
        _ => return None,
    })
}

fn run(mut cmd: Command) -> Result<Output> {
    cmd.stdin(Stdio::null());
    Ok(cmd.output()?)
}

fn manager_install_cmd(manager: PackageManager, packages: &[String]) -> Command {
    match manager {
        PackageManager::Pacman => {
            let mut c = Command::new("sudo");
            c.args(["pacman", "-S", "--needed", "--noconfirm"]).args(packages);
            c
        }
        PackageManager::Aur => {
            // Prefer paru, fall back to yay -- resolved at call time, never
            // from LLM/user text.
            let helper = if which("paru") { "paru" } else { "yay" };
            let mut c = Command::new(helper);
            c.args(["-S", "--needed", "--noconfirm"]).args(packages);
            c
        }
        PackageManager::Apt => {
            let mut c = Command::new("sudo");
            c.args(["apt-get", "install", "-y"]).args(packages);
            c
        }
        PackageManager::Dnf => {
            let mut c = Command::new("sudo");
            c.args(["dnf", "install", "-y"]).args(packages);
            c
        }
        PackageManager::Zypper => {
            let mut c = Command::new("sudo");
            c.args(["zypper", "install", "-y"]).args(packages);
            c
        }
        PackageManager::Flatpak => {
            let mut c = Command::new("flatpak");
            c.args(["install", "-y", "flathub"]).args(packages);
            c
        }
    }
}

fn manager_remove_cmd(manager: PackageManager, packages: &[String]) -> Command {
    match manager {
        PackageManager::Pacman => {
            let mut c = Command::new("sudo");
            c.args(["pacman", "-R", "--noconfirm"]).args(packages);
            c
        }
        PackageManager::Aur => {
            let helper = if which("paru") { "paru" } else { "yay" };
            let mut c = Command::new(helper);
            c.args(["-R", "--noconfirm"]).args(packages);
            c
        }
        PackageManager::Apt => {
            let mut c = Command::new("sudo");
            c.args(["apt-get", "remove", "-y"]).args(packages);
            c
        }
        PackageManager::Dnf => {
            let mut c = Command::new("sudo");
            c.args(["dnf", "remove", "-y"]).args(packages);
            c
        }
        PackageManager::Zypper => {
            let mut c = Command::new("sudo");
            c.args(["zypper", "remove", "-y"]).args(packages);
            c
        }
        PackageManager::Flatpak => {
            let mut c = Command::new("flatpak");
            c.args(["uninstall", "-y"]).args(packages);
            c
        }
    }
}

fn systemctl_cmd(verb: SystemctlVerb, unit: &Option<String>, user_scope: bool) -> Command {
    let mut c = if user_scope {
        Command::new("systemctl")
    } else {
        let mut c = Command::new("sudo");
        c.arg("systemctl");
        c
    };
    if user_scope {
        c.arg("--user");
    }
    let verb_str = match verb {
        SystemctlVerb::Start => "start",
        SystemctlVerb::Stop => "stop",
        SystemctlVerb::Restart => "restart",
        SystemctlVerb::Enable => "enable",
        SystemctlVerb::Disable => "disable",
        SystemctlVerb::EnableNow => "enable",
        SystemctlVerb::DisableNow => "disable",
    };
    c.arg(verb_str);
    if matches!(verb, SystemctlVerb::EnableNow | SystemctlVerb::DisableNow) {
        c.arg("--now");
    }
    if let Some(u) = unit {
        c.arg(u);
    }
    c
}

fn which(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn set_config(file: &str, group: &str, key: &str, value: &str) -> Result<Output> {
    // kwriteconfig6, falling back to kwriteconfig5 -- both take structured
    // args, never a shell string, and only ever touch the named config file
    // under the user's own config dir (kwriteconfig resolves relative
    // filenames against $XDG_CONFIG_HOME itself).
    let binary = if which("kwriteconfig6") { "kwriteconfig6" } else { "kwriteconfig5" };
    let mut c = Command::new(binary);
    c.args(["--file", file, "--group", group, "--key", key, value]);
    run(c)
}

/// Executes a single step. Never touches raw shell text; every branch is a
/// fixed, explicitly-argumented Command.
fn execute_step(step: &Step, app_index: &AppIndex) -> StepResult {
    let desc = step.description.clone();
    let outcome: Result<String> = (|| match &step.kind {
        StepKind::LaunchOrFocus { app_id } => {
            let app = app_index
                .by_id
                .get(app_id)
                .or_else(|| app_index.resolve(app_id))
                .ok_or_else(|| anyhow::anyhow!("app '{app_id}' not found in launcher index"))?;
            let result = focus_or_launch(app)?;
            Ok(format!("{result:?}"))
        }
        StepKind::InstallPackage { manager, packages } => {
            let out = run(manager_install_cmd(*manager, packages))?;
            check_output(out)
        }
        StepKind::RemovePackage { manager, packages } => {
            let out = run(manager_remove_cmd(*manager, packages))?;
            check_output(out)
        }
        StepKind::Systemctl { verb, unit, user_scope } => {
            let out = run(systemctl_cmd(*verb, unit, *user_scope))?;
            check_output(out)
        }
        StepKind::PowerAction { action } => {
            let verb = match action {
                PowerActionKind::Reboot => "reboot",
                PowerActionKind::Shutdown => "poweroff",
                PowerActionKind::Suspend => "suspend",
            };
            let mut c = Command::new("systemctl");
            c.arg(verb);
            let out = run(c)?;
            check_output(out)
        }
        StepKind::Utility { name } => {
            let (bin, args) = utility_command(name)
                .ok_or_else(|| anyhow::anyhow!("unknown utility action '{name}'"))?;
            let mut c = Command::new(bin);
            c.args(args);
            let out = run(c)?;
            check_output(out)
        }
        StepKind::SetConfig { file, group, key, value } => {
            let out = set_config(file, group, key, value)?;
            check_output(out)
        }
    })();

    match outcome {
        Ok(detail) => StepResult { description: desc, success: true, detail },
        Err(e) => StepResult { description: desc, success: false, detail: e.to_string() },
    }
}

fn check_output(out: Output) -> Result<String> {
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        bail!("exit {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim())
    }
}

/// Runs every step in a (already-sanitized) Plan in order, stopping at the
/// first failure. Caller is responsible for having already confirmed with
/// the user if plan.requires_confirmation was true -- this function does
/// not itself prompt for anything, it only executes.
pub fn execute_plan(plan: &Plan, app_index: &AppIndex) -> ExecutionReport {
    let mut results = Vec::new();
    let mut stopped_early = false;
    for step in &plan.steps {
        let result = execute_step(step, app_index);
        let ok = result.success;
        results.push(result);
        if !ok {
            stopped_early = true;
            break;
        }
    }
    ExecutionReport { results, stopped_early }
}
