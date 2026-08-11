//! desktop_index.rs
//! -----------------
//! Scans the standard XDG application directories for .desktop files (the
//! same source KDE Plasma's own app launcher reads from) and builds a
//! lookup table from spoken/typed app names -> a resolved AppEntry with the
//! real Exec command and a WM class guess for focus-or-launch matching.
//!
//! This deliberately does NOT shell out to `kbuildsycoca` or read Plasma's
//! internal cache format -- parsing the .desktop files directly means this
//! works the same whether the WM is Plasma, Hyprland, or anything else that
//! honors the freedesktop.org spec, which matters given you're planning to
//! move around window managers.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    /// Desktop file id, e.g. "org.mozilla.firefox" or "firefox" -- used as
    /// the canonical app_id in StepKind::LaunchOrFocus.
    pub id: String,
    pub name: String,
    /// Exec line with %u/%f/%U/%F field codes stripped -- safe to split on
    /// whitespace and pass directly to Command::args, never through a shell.
    pub exec: Vec<String>,
    pub wm_class_guess: String,
    pub desktop_file: PathBuf,
    pub no_display: bool,
}

fn xdg_application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in data_dirs.split(':') {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }
    dirs
}

/// Strips freedesktop field codes (%f %F %u %U %d %D %n %N %i %c %k %v %m)
/// from an Exec= value and tokenizes it respecting simple quoting. This is
/// parsing, not shell evaluation -- no metacharacter (; | & $ ` ( ) < >) is
/// ever given special meaning, so a malicious/malformed .desktop file can't
/// smuggle shell syntax through this path.
fn parse_exec_line(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in exec.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
        .into_iter()
        .filter(|t| !t.starts_with('%'))
        .collect()
}

fn parse_desktop_file(path: &Path) -> Option<AppEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut in_main_group = false;
    let mut name = None;
    let mut exec_raw = None;
    let mut no_display = false;
    let mut terminal = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_main_group {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            if name.is_none() {
                name = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec_raw = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("NoDisplay=") {
            no_display = v.eq_ignore_ascii_case("true");
        } else if let Some(v) = line.strip_prefix("Terminal=") {
            terminal = v.eq_ignore_ascii_case("true");
        } else if line.starts_with("Type=") && !line.contains("Application") {
            return None; // skip Link/Directory entries
        }
    }

    let name = name?;
    let exec_raw = exec_raw?;
    let mut exec = parse_exec_line(&exec_raw);
    if exec.is_empty() {
        return None;
    }
    if terminal {
        // Run inside the user's configured terminal emulator rather than
        // attaching to the daemon's own (nonexistent) tty.
        exec = vec!["xdg-terminal-exec".to_string()]
            .into_iter()
            .chain(exec)
            .collect();
    }

    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.clone());

    let wm_class_guess = exec[0]
        .rsplit('/')
        .next()
        .unwrap_or(&exec[0])
        .to_string();

    Some(AppEntry {
        id,
        name,
        exec,
        wm_class_guess,
        desktop_file: path.to_path_buf(),
        no_display,
    })
}

pub struct AppIndex {
    pub by_id: HashMap<String, AppEntry>,
}

impl AppIndex {
    pub fn scan() -> Result<Self> {
        let mut by_id = HashMap::new();
        for dir in xdg_application_dirs() {
            if !dir.is_dir() {
                continue;
            }
            for entry in WalkDir::new(&dir).max_depth(3).into_iter().filter_map(|e| e.ok()) {
                if entry.path().extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }
                if let Some(app) = parse_desktop_file(entry.path()) {
                    by_id.entry(app.id.clone()).or_insert(app);
                }
            }
        }
        tracing::info!("indexed {} desktop applications", by_id.len());
        Ok(Self { by_id })
    }

    pub fn all_names(&self) -> Vec<String> {
        self.by_id.values().map(|a| a.name.clone()).collect()
    }

    /// Fuzzy resolve free text ("open firefox", "firefox", "the browser")
    /// to the best matching AppEntry using normalized substring match first,
    /// then string-similarity fallback -- mirrors the old keyword matcher's
    /// two-stage approach but against the live app index instead of a
    /// static phrase list.
    pub fn resolve(&self, query: &str) -> Option<&AppEntry> {
        let q = query.to_lowercase();
        let q = q
            .trim_start_matches("open ")
            .trim_start_matches("launch ")
            .trim_start_matches("start ")
            .trim();

        if let Some(app) = self.by_id.values().find(|a| a.id.to_lowercase() == q) {
            return Some(app);
        }
        if let Some(app) = self.by_id.values().find(|a| a.name.to_lowercase() == q) {
            return Some(app);
        }
        if let Some(app) = self
            .by_id
            .values()
            .find(|a| a.name.to_lowercase().contains(q) || q.contains(&a.name.to_lowercase()))
        {
            return Some(app);
        }

        self.by_id
            .values()
            .filter(|a| !a.no_display)
            .max_by(|a, b| {
                let sa = strsim::jaro_winkler(&a.name.to_lowercase(), q);
                let sb = strsim::jaro_winkler(&b.name.to_lowercase(), q);
                sa.partial_cmp(&sb).unwrap()
            })
            .filter(|a| strsim::jaro_winkler(&a.name.to_lowercase(), q) > 0.75)
    }
}
