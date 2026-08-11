//! focus_or_launch.rs
//! -------------------
//! "open firefox" should tab into an already-open Firefox window instead of
//! spawning a second instance. This needs a compositor-level tool since
//! window enumeration/activation isn't something a plain process can do.
//!
//! Strategy, in order of preference:
//!   1. kdotool (https://github.com/jinliu/kdotool) -- works on both KWin
//!      X11 and KWin Wayland via KWin's scripting API, and also happens to
//!      work under wlroots compositors (Hyprland, Sway) that support the
//!      same wlr-foreign-toplevel protocol pieces kdotool relies on. This
//!      is why kdotool was picked over a KDE-only DBus call: it's the one
//!      tool with a realistic shot at working across the KDE-now,
//!      Hyprland-later path you described.
//!   2. wmctrl -- X11 only (ICCCM/EWMH), used as a fallback on any X11
//!      session where kdotool isn't installed.
//!   3. If neither is available, or no matching window is found: fall
//!      through to a plain launch.
//!
//! Every external call here uses Command::arg (never a shell string), and
//! every argument passed to it comes from the AppEntry's already-parsed
//! exec vector or a plain window-class string -- nothing here accepts free
//! text from the LLM.
use anyhow::Result;
use std::process::Command;
use std::sync::OnceLock;

use crate::desktop_index::AppEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTool {
    Kdotool,
    Wmctrl,
    None,
}

static DETECTED_TOOL: OnceLock<FocusTool> = OnceLock::new();

fn tool_available(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn detect_focus_tool() -> FocusTool {
    *DETECTED_TOOL.get_or_init(|| {
        if tool_available("kdotool") {
            FocusTool::Kdotool
        } else if tool_available("wmctrl") {
            FocusTool::Wmctrl
        } else {
            FocusTool::None
        }
    })
}

/// Attempts to find and activate a window belonging to this app. Returns
/// Ok(true) if a window was found and focused, Ok(false) if no matching
/// window exists (caller should then launch fresh).
pub fn try_focus_existing(app: &AppEntry) -> Result<bool> {
    match detect_focus_tool() {
        FocusTool::Kdotool => try_focus_kdotool(&app.wm_class_guess),
        FocusTool::Wmctrl => try_focus_wmctrl(&app.wm_class_guess),
        FocusTool::None => Ok(false),
    }
}

fn try_focus_kdotool(class_hint: &str) -> Result<bool> {
    // kdotool search --class <hint> returns window id(s), one per line.
    let search = Command::new("kdotool")
        .args(["search", "--class", class_hint])
        .output()?;
    let ids = String::from_utf8_lossy(&search.stdout);
    let first_id = ids.lines().next().map(str::trim).filter(|s| !s.is_empty());
    if let Some(id) = first_id {
        let activate = Command::new("kdotool")
            .args(["windowactivate", id])
            .status()?;
        return Ok(activate.success());
    }
    Ok(false)
}

fn try_focus_wmctrl(class_hint: &str) -> Result<bool> {
    let list = Command::new("wmctrl").arg("-lx").output()?;
    let out = String::from_utf8_lossy(&list.stdout);
    // wmctrl -lx columns: id desktop WM_CLASS host title...
    let target_line = out.lines().find(|line| {
        line.split_whitespace()
            .nth(2)
            .map(|class| class.to_lowercase().contains(&class_hint.to_lowercase()))
            .unwrap_or(false)
    });
    if let Some(line) = target_line {
        if let Some(win_id) = line.split_whitespace().next() {
            let activate = Command::new("wmctrl").args(["-i", "-a", win_id]).status()?;
            return Ok(activate.success());
        }
    }
    Ok(false)
}

/// Launches a fresh instance, fully detached from the daemon so it survives
/// independently and doesn't inherit stdio.
pub fn launch_fresh(app: &AppEntry) -> Result<()> {
    let (bin, args) = app.exec.split_first().ok_or_else(|| anyhow::anyhow!("empty exec"))?;
    Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// The full "open firefox" behavior: try to focus an existing window,
/// otherwise launch a new one.
pub fn focus_or_launch(app: &AppEntry) -> Result<FocusOrLaunchResult> {
    if try_focus_existing(app)? {
        return Ok(FocusOrLaunchResult::Focused);
    }
    launch_fresh(app)?;
    Ok(FocusOrLaunchResult::Launched)
}

/// Closes every window belonging to this app (via kdotool/wmctrl, same
/// class-hint matching as focus), used for "close firefox" style requests.
/// Returns the number of windows actually closed.
pub fn close_all_windows(app: &AppEntry) -> Result<u32> {
    match detect_focus_tool() {
        FocusTool::Kdotool => close_all_kdotool(&app.wm_class_guess),
        FocusTool::Wmctrl => close_all_wmctrl(&app.wm_class_guess),
        FocusTool::None => Ok(0),
    }
}

fn close_all_kdotool(class_hint: &str) -> Result<u32> {
    let search = Command::new("kdotool")
        .args(["search", "--class", class_hint])
        .output()?;
    let ids = String::from_utf8_lossy(&search.stdout);
    let mut closed = 0u32;
    for id in ids.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let status = Command::new("kdotool").args(["windowclose", id]).status()?;
        if status.success() {
            closed += 1;
        }
    }
    Ok(closed)
}

fn close_all_wmctrl(class_hint: &str) -> Result<u32> {
    let list = Command::new("wmctrl").arg("-lx").output()?;
    let out = String::from_utf8_lossy(&list.stdout);
    let mut closed = 0u32;
    for line in out.lines() {
        let matches = line
            .split_whitespace()
            .nth(2)
            .map(|class| class.to_lowercase().contains(&class_hint.to_lowercase()))
            .unwrap_or(false);
        if !matches {
            continue;
        }
        if let Some(win_id) = line.split_whitespace().next() {
            let status = Command::new("wmctrl").args(["-i", "-c", win_id]).status()?;
            if status.success() {
                closed += 1;
            }
        }
    }
    Ok(closed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOrLaunchResult {
    Focused,
    Launched,
}
