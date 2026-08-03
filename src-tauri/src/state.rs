//! Shared paths, on-disk types, and small filesystem helpers.
//!
//! The hook writer and the overlay never talk to each other directly: hooks
//! write one small JSON file per Claude Code session, the overlay polls the
//! directory. That keeps the hook a fire-and-forget process with no port to
//! collide with and no daemon to be running first.

use crate::process;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RECENT_LIMIT: usize = 24;

/// A session nobody has touched in this long is gone, whatever it last claimed.
/// Generous on purpose: a real prompt left waiting overnight is still true.
const STALE_AFTER_MS: u64 = 12 * 60 * 60 * 1000;

/// Work that has produced no event in this long is not happening. Claude Code
/// fires a hook for every tool call, so silence this long means the turn died
/// without a `Stop`, and the card must stop claiming otherwise.
const WORKING_STALE_MS: u64 = 5 * 60 * 1000;

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

/// One-shot instruction for a running overlay, written by `pipsqueak control`.
pub fn command_path() -> PathBuf {
    root().join("command.json")
}

/// Written every time the global hotkey fires.
///
/// "The shortcut does nothing" is otherwise unanswerable: registering the
/// chord and receiving the keypress are separate things, and from outside the
/// process they look identical. This makes the second one observable.
pub fn hotkey_log_path() -> PathBuf {
    root().join("hotkey.json")
}

/// Touched by the running overlay so other invocations can tell it is alive.
pub fn heartbeat_path() -> PathBuf {
    root().join("running.json")
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

/// Durable states: what the session *is* doing.
///
/// Excludes "waiting", "done" and "failed". Those answer different questions
/// (is a human blocking this, and how did the last turn end), and storing them
/// here is what lets a card get stuck on an alert forever when the resolving
/// event never arrives.
pub const RUNNING_STATES: [&str; 3] = ["thinking", "running", "compacting"];

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Session {
    pub session_id: String,
    /// idle | thinking | running | compacting. Only forward progress moves it.
    pub state: String,
    /// What this turn is *about*. Set when the turn starts and replaced when it
    /// ends, never by tool events, so it stays readable while tools churn.
    pub headline: String,
    /// The live line: "Editing render.js", "Running: npm test".
    pub activity: String,
    /// The *category* of the current work: Reading, Editing, Running… This
    /// changes far less often than `activity`, which is the point.
    pub kind: String,
    /// Longer text for the expanded panel (assistant summary, error body).
    pub detail: String,
    /// Display name of the owning project: the git repository, not the folder
    /// the session happens to be sitting in.
    pub project: String,
    /// Set only when the session runs somewhere other than the project root
    /// (a worktree, a subdirectory), for a secondary label.
    pub workspace: String,
    pub project_root: String,
    /// True when the project root is a temp directory: throwaway sessions
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

    /// How the last turn ended: "" | done | failed. Cleared the moment the
    /// next turn starts, so it can never outlive the thing it describes.
    pub outcome: String,
    pub outcome_ms: u64,
    /// A `done` is only real once the clock passes this. Claude Code sends
    /// `Stop` while background tasks are still finishing, so an immediate
    /// celebration is sometimes premature; holding it briefly lets the next
    /// event cancel it instead.
    pub settles_ms: u64,
    /// Non-zero while Claude Code is genuinely blocked on a human. Cleared by
    /// any forward progress.
    pub waiting_since: u64,
    pub waiting_reason: String,

    /// A permission prompt Claude Code raised and has not resolved.
    ///
    /// Recorded rather than believed. Auto-mode and permission hooks settle
    /// most of these in a couple of hundred milliseconds without anyone being
    /// asked, so this only becomes a visible "needs you" once it outlives the
    /// debounce in the frontend.
    pub pending_tool: String,
    pub pending_detail: String,
    pub pending_since: u64,
    /// Why the pending command looks irreversible. A hint for the card only.
    /// Nothing here approves, denies, or blocks anything.
    pub pending_risk: String,
    /// Tool failures inside the current turn. A failed grep is not a failed
    /// turn, since Claude usually just tries something else, so this is a
    /// marker rather than a state.
    pub hiccups: u64,
    /// Subagents started but not yet finished.
    pub subagents: u64,
    /// The sweep gave up on this session: running, but silent for longer than
    /// any real turn goes without firing a hook.
    ///
    /// A flag rather than a message written into the headline. The headline
    /// only changes at turn boundaries, so a sentence dropped into it by the
    /// sweep outlives the situation it describes and sits there contradicting
    /// the status line once work resumes.
    pub stalled: bool,

    /// The agent process that owns this session, as `(pid, creation time)`.
    /// The creation time is what makes it safe: pids get reused, and a reused
    /// pid would otherwise report a dead session as alive.
    pub agent_pid: u32,
    pub agent_created: u64,
    /// Timestamp of the event this file describes. Claude Code runs matching
    /// hooks in parallel, so two writers can race; the older one must lose.
    pub event_ms: u64,
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

    pub fn is_running(&self) -> bool {
        RUNNING_STATES.contains(&self.state.as_str())
    }

    /// Forward progress: whatever the session was blocked on or had finished
    /// is no longer true.
    pub fn clear_pending(&mut self) {
        self.outcome.clear();
        self.outcome_ms = 0;
        self.settles_ms = 0;
        self.waiting_since = 0;
        self.waiting_reason.clear();
    }

    pub fn clear_permission(&mut self) {
        self.pending_tool.clear();
        self.pending_detail.clear();
        self.pending_risk.clear();
        self.pending_since = 0;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Which build's idea of this file it is. See [`CONFIG_VERSION`].
    pub version: u32,
    /// False until the first run has been acknowledged. *Not* backfilled for
    /// existing installs, because an upgrade should get the welcome
    /// once too, since it is the only place the setup is explained.
    pub welcomed: bool,
    pub pet: String,
    pub scale: f64,
    pub click_through: bool,
    pub show_bubble: bool,
    /// Sessions rooted in a temp directory (scripted runs, evals) are hidden
    /// unless asked for. They are not projects and drown out real work.
    pub show_scratch: bool,
    /// Beep when a project starts waiting on you.
    pub alert_on_waiting: bool,
    /// Blink the tray icon when a project finishes. The only channel that
    /// reaches you when the overlay is behind a fullscreen window.
    pub flash_on_finish: bool,
    /// Do not disturb: no sound, no tray flash. The pet still updates.
    pub quiet: bool,
    /// The chord that shows and hides the pet, e.g. "Ctrl+Alt+P". "off"
    /// disables it. If the chord is already claimed by something else,
    /// Pipsqueak falls back to the next one it can get and says which.
    pub hotkey: String,
    /// Ask GitHub whether there is a newer release.
    ///
    /// Off until asked for. This is the only thing in the app that talks to
    /// the network at all, and a status overlay reaching out on its own
    /// without being asked is a surprise nobody signed up for.
    pub update_check: bool,
    /// A version the user has already been told about and did not want.
    pub update_dismissed: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Config {
    /// Pulls any out-of-range value back to something usable.
    ///
    /// Per field rather than all-or-nothing: one bad number in a hand-edited
    /// file should cost that one setting, not every other one alongside it.
    fn clamp(&mut self) {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            self.scale = 2.0;
        }
        self.scale = self.scale.clamp(1.0, 6.0);
        if self.pet.trim().is_empty() {
            self.pet = "byte".to_string();
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            welcomed: false,
            pet: "byte".to_string(),
            scale: 2.0,
            click_through: false,
            show_bubble: true,
            show_scratch: false,
            alert_on_waiting: false,
            flash_on_finish: true,
            quiet: false,
            hotkey: "Ctrl+Alt+P".to_string(),
            update_check: false,
            update_dismissed: String::new(),
            x: None,
            y: None,
        }
    }
}

/// The shape this build understands. Bumped when a field changes meaning,
/// not when one is merely added, which `serde(default)` already handles.
///
/// 2: the window grew to fit the setup panel, which moved the pet (drawn at
///    the bottom of it) down by the difference.
pub const CONFIG_VERSION: u32 = 2;

/// Reads the config, surviving anything that might have happened to the file.
///
/// It is a plain JSON file in the user's home directory, which means it gets
/// hand-edited, half-written by a crash, and occasionally written by a newer
/// build than this one. None of those may lose the whole config or, worse,
/// silently overwrite a newer file with older defaults.
pub fn load_config() -> Config {
    let path = config_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        // No file yet: a first run, not a problem.
        return Config::default();
    };
    match serde_json::from_str::<Config>(&raw) {
        Ok(mut config) => {
            migrate(&mut config);
            config.clamp();
            config
        }
        Err(_) => {
            // Keep whatever they had. Overwriting an unparseable config with
            // defaults destroys the only copy of a setting they may have been
            // hand-editing when something went wrong.
            let _ = fs::rename(&path, path.with_extension("json.bak"));
            Config::default()
        }
    }
}

/// Brings a config written by an older build up to date.
///
/// Each step moves one version forward and nothing else, so a config that has
/// been sitting on disk through several releases arrives correct rather than
/// having the newest step applied to the oldest shape.
fn migrate(config: &mut Config) {
    if config.version < 2 {
        // Version 2 grew the window, which pushed the pet below the bottom of
        // the screen for anyone with a saved position. Correcting the
        // coordinate here was the obvious fix and the wrong one: the saved
        // value is in physical pixels and the window's size is declared in
        // logical ones, so on a scaled display the correction was short by the
        // scale factor and the pet stayed off screen.
        //
        // `app::clamp_to_display` handles it instead, and handles every other
        // way a saved position can stop fitting: a resolution change, a
        // scaling change, a taskbar that moved. Nothing to do here but record
        // that we looked.
        config.version = 2;
    }
}

pub fn save_config(config: &Config) -> std::io::Result<()> {
    if config.version > CONFIG_VERSION {
        // Written by a newer build. Rolling it back to fields this one happens
        // to know about would quietly delete their settings.
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "config.json was written by a newer version of Pipsqueak",
        ));
    }
    let mut stored = config.clone();
    stored.clamp();
    stored.version = CONFIG_VERSION;
    let bytes = serde_json::to_vec_pretty(&stored).unwrap_or_else(|_| b"{}".to_vec());
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

/// Held for the read-modify-write of one session file.
///
/// Claude Code runs every matching hook for an event in parallel, so two
/// `pipsqueak hook` processes routinely touch the same file at the same
/// moment. Without this, both read the same "before", and whichever renames
/// last silently discards the other's event.
pub struct FileLock {
    path: PathBuf,
    held: bool,
}

impl FileLock {
    /// Waits up to ~200ms, then proceeds anyway.
    ///
    /// Losing an event is worse than an interleaved write: hooks are the only
    /// source of truth here, and a hook that gives up writes nothing at all.
    pub fn acquire(target: &Path) -> Self {
        let path = target.with_extension("json.lock");
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        for attempt in 0..40 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Self { path, held: true },
                Err(_) => {
                    // A hook killed mid-write would otherwise block every later
                    // event for this session forever.
                    if attempt == 0 {
                        if let Ok(meta) = fs::metadata(&path) {
                            let expired = meta
                                .modified()
                                .ok()
                                .and_then(|t| t.elapsed().ok())
                                .map(|age| age.as_secs() >= 2)
                                .unwrap_or(true);
                            if expired {
                                let _ = fs::remove_file(&path);
                                continue;
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        Self { path, held: false }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
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
        attention_rank(b)
            .cmp(&attention_rank(a))
            .then(b.updated_ms.cmp(&a.updated_ms))
    });
    out
}

fn attention_rank(session: &Session) -> u8 {
    if session.waiting_since > 0 {
        3
    } else if session.outcome == "failed" {
        2
    } else {
        0
    }
}

/// Retires sessions that are no longer telling the truth.
///
/// Three rules, in order of how confident each is:
///
/// 1. The agent process is gone. Certain, so delete the session outright.
/// 2. Nothing has happened for [`WORKING_STALE_MS`] while the card claims work
///    is in progress. Claude Code fires a hook per tool call, so that silence
///    means the turn died without a `Stop`. Downgrade rather than delete, since
///    the session may still be resumed. A session genuinely *waiting* on a
///    human is left alone: that claim stays true no matter how long it takes.
/// 3. Nothing at all for [`STALE_AFTER_MS`]. Delete.
///
/// Returns the number of files it changed, so callers can skip a redraw.
pub fn sweep() -> usize {
    let now = now_ms();
    let Ok(entries) = fs::read_dir(sessions_dir()) else {
        return 0;
    };
    let mut changed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(mut session) = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Session>(&raw).ok())
        else {
            // Unreadable or from an incompatible build: it can only mislead.
            let _ = fs::remove_file(&path);
            changed += 1;
            continue;
        };

        let age = now.saturating_sub(session.updated_ms);

        if session.agent_pid != 0 && !process::is_alive(session.agent_pid, session.agent_created) {
            let _ = fs::remove_file(&path);
            changed += 1;
            continue;
        }
        if age > STALE_AFTER_MS {
            let _ = fs::remove_file(&path);
            changed += 1;
            continue;
        }
        // A session that reported how its turn ended is not silently stuck,
        // however long ago that was.
        if age > WORKING_STALE_MS && session.is_running() && session.outcome.is_empty() {
            session.state = "idle".to_string();
            session.kind = "Idle".to_string();
            session.activity.clear();
            session.subagents = 0;
            // Not a completion. Saying "Done" here would be the same lie in a
            // friendlier voice. The headline is left alone: it still describes
            // what the turn was about, which is true whatever happened next.
            session.stalled = true;
            if let Ok(bytes) = serde_json::to_vec(&session) {
                let _ = write_atomic(&path, &bytes);
                changed += 1;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Migration leaves the saved position alone on purpose. Correcting it
    /// here needs the display scale, which this layer does not have, and
    /// getting it wrong is what put the pet below the bottom of the screen.
    /// Placement clamps it to the work area instead.
    #[test]
    fn migration_does_not_touch_the_saved_position() {
        let mut config = Config {
            version: 1,
            x: Some(1326),
            y: Some(649),
            ..Config::default()
        };
        migrate(&mut config);
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!((config.x, config.y), (Some(1326), Some(649)));
    }

    #[test]
    fn a_hand_edited_value_costs_only_itself() {
        let mut config = Config {
            scale: -12.0,
            pet: "  ".into(),
            show_scratch: true,
            ..Config::default()
        };
        config.clamp();
        assert_eq!(config.scale, 2.0);
        assert_eq!(config.pet, "byte");
        assert!(config.show_scratch, "an unrelated setting must survive");
    }

    #[test]
    fn a_newer_config_is_never_written_over() {
        let config = Config {
            version: CONFIG_VERSION + 1,
            ..Config::default()
        };
        assert!(save_config(&config).is_err());
    }
}
