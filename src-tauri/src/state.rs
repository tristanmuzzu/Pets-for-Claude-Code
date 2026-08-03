//! Shared paths, on-disk types, and small filesystem helpers.
//!
//! The hook writer and the overlay never talk to each other directly: hooks
//! write one small JSON file per Claude Code session, the overlay polls the
//! directory. That keeps the hook a fire-and-forget process with no port to
//! collide with and no daemon to be running first.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RECENT_LIMIT: usize = 24;
const STALE_AFTER_MS: u64 = 12 * 60 * 60 * 1000;

pub fn home_dir() -> PathBuf {
    #[cfg(windows)]
    let raw = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let raw = std::env::var_os("HOME");
    raw.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub fn root() -> PathBuf {
    home_dir().join(".pipsqueak")
}

pub fn sessions_dir() -> PathBuf {
    root().join("sessions")
}

pub fn pets_dir() -> PathBuf {
    root().join("pets")
}

pub fn codex_pets_dir() -> PathBuf {
    home_dir().join(".codex").join("pets")
}

pub fn config_path() -> PathBuf {
    root().join("config.json")
}

pub fn claude_settings_path() -> PathBuf {
    home_dir().join(".claude").join("settings.json")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Session ids come from an external process, so never let one escape the
/// sessions directory.
pub fn sanitize(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(96)
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub ms: u64,
    pub state: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Session {
    pub session_id: String,
    /// idle | thinking | running | waiting | failed | done | compacting
    pub state: String,
    /// What this turn is *about*. Set when the turn starts and replaced when it
    /// ends — never by tool events, so it stays readable while tools churn.
    pub headline: String,
    /// The live line: "Editing render.js", "Running: npm test".
    pub activity: String,
    /// The *category* of the current work: Reading, Editing, Running… This
    /// changes far less often than `activity`, which is the point.
    pub kind: String,
    /// Longer text for the expanded panel (assistant summary, error body).
    pub detail: String,
    /// Display name of the owning project — the git repository, not the folder
    /// the session happens to be sitting in.
    pub project: String,
    /// Set only when the session runs somewhere other than the project root
    /// (a worktree, a subdirectory), for a secondary label.
    pub workspace: String,
    pub project_root: String,
    /// True when the project root is a temp directory — throwaway sessions
    /// from scripted runs, which should not compete with real work.
    pub scratch: bool,
    pub cwd: String,
    pub event: String,
    pub updated_ms: u64,
    pub started_ms: u64,
    /// Start of the current turn, for the "3m" on the status line.
    pub turn_started_ms: u64,
    /// Tool calls in the current turn.
    pub turn_tools: u64,
    pub tools: u64,
    pub recent: Vec<Entry>,
}

impl Session {
    pub fn push_recent(&mut self, state: &str, text: &str) {
        if let Some(last) = self.recent.last() {
            if last.state == state && last.text == text {
                return;
            }
        }
        self.recent.push(Entry {
            ms: now_ms(),
            state: state.to_string(),
            text: text.to_string(),
        });
        let overflow = self.recent.len().saturating_sub(RECENT_LIMIT);
        if overflow > 0 {
            self.recent.drain(0..overflow);
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub pet: String,
    pub scale: f64,
    pub click_through: bool,
    pub show_bubble: bool,
    /// Sessions rooted in a temp directory — scripted runs, evals — are hidden
    /// unless asked for. They are not projects and drown out real work.
    pub show_scratch: bool,
    /// Beep when a project starts waiting on you.
    pub alert_on_waiting: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pet: "pip".to_string(),
            scale: 2.0,
            click_through: false,
            show_bubble: true,
            show_scratch: false,
            alert_on_waiting: false,
            x: None,
            y: None,
        }
    }
}

pub fn load_config() -> Config {
    fs::read_to_string(config_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_config(config: &Config) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(config).unwrap_or_else(|_| b"{}".to_vec());
    write_atomic(&config_path(), &bytes)
}

/// Write via temp file + rename so a reader never sees a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_file_name(format!(
        "{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("out"),
        std::process::id()
    ));
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

pub fn read_sessions() -> Vec<Session> {
    let mut out: Vec<Session> = Vec::new();
    let Ok(entries) = fs::read_dir(sessions_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(session) = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Session>(&raw).ok())
        {
            out.push(session);
        }
    }
    // Anything needing a human wins, then most recently active.
    out.sort_by(|a, b| {
        attention_rank(&b.state)
            .cmp(&attention_rank(&a.state))
            .then(b.updated_ms.cmp(&a.updated_ms))
    });
    out
}

fn attention_rank(state: &str) -> u8 {
    match state {
        "waiting" => 3,
        "failed" => 2,
        _ => 0,
    }
}

/// Drop session files left behind by crashed or force-quit sessions.
pub fn prune_stale() {
    let now = now_ms();
    let Ok(entries) = fs::read_dir(sessions_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stale = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Session>(&raw).ok())
            .map(|s| now.saturating_sub(s.updated_ms) > STALE_AFTER_MS)
            .unwrap_or(true);
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}
