//! Read-only adapter for local Codex rollouts (desktop and CLI).
//!
//! No hooks or database writes: Codex owns its JSONL logs and session index.
//! We replay complete records incrementally, and only publish after catching
//! up. In particular an assistant's final message is NOT a turn-complete event.
//! These are best-effort local formats, not a stable external API.

use crate::narration::Mode;
use crate::state::{self, Provider, Session};
use crate::text::{summary_within, truncate};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RESCAN: Duration = Duration::from_secs(3);
const RECENT_MS: u64 = 12 * 60 * 60 * 1000;
const STALE_MS: u64 = 5 * 60 * 1000;
const READ_BYTES: u64 = 256 * 1024;
const MAX_RECORD: usize = 2 * 1024 * 1024;
const MAX_SESSIONS: usize = 32;
const INITIAL_TAIL: u64 = 4 * 1024 * 1024;

pub fn home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state::home_dir().join(".codex"))
}

#[derive(Default)]
struct Watch {
    offset: u64,
    modified: Option<SystemTime>,
    pending: Vec<u8>,
    oversized: bool,
    ready: bool,
    accepted: bool,
    session: Session,
    speech: String,
    thought: String,
    thought_ms: u64,
    speech_ms: u64,
    calls: HashSet<String>,
    waiting_call: String,
}

#[derive(Default)]
struct Tracker {
    watches: HashMap<PathBuf, Watch>,
    scanned: Option<Instant>,
    titles: HashMap<String, String>,
    index_stamp: Option<(SystemTime, u64)>,
}

fn cache() -> &'static Mutex<Tracker> {
    static CACHE: OnceLock<Mutex<Tracker>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Tracker::default()))
}

pub fn sessions(mode: Mode) -> Vec<Session> {
    let Ok(mut tracker) = cache().lock() else {
        return Vec::new();
    };
    let mut sessions = tracker.read(&home(), state::now_ms(), mode);
    drop(tracker);
    crate::codex_live::decorate(&mut sessions);
    sessions
}

/// The CLI has no polling loop. Catch up the bounded initial tail before
/// reporting, without waiting for any future agent activity.
pub fn snapshot(mode: Mode) -> Vec<Session> {
    let Ok(mut tracker) = cache().lock() else {
        return Vec::new();
    };
    let root = home();
    let now = state::now_ms();
    let mut result = Vec::new();
    for _ in 0..=INITIAL_TAIL / READ_BYTES + 1 {
        result = tracker.read(&root, now, mode);
        if tracker.watches.values().all(|w| w.ready) {
            break;
        }
    }
    result
}

impl Tracker {
    fn read(&mut self, root: &Path, now: u64, mode: Mode) -> Vec<Session> {
        if self.scanned.map_or(true, |at| at.elapsed() >= RESCAN) {
            let mut paths = Vec::new();
            visit(
                &root.join("sessions"),
                4,
                now.saturating_sub(RECENT_MS),
                &mut paths,
            );
            paths.sort_by(|a, b| b.cmp(a));
            paths.truncate(MAX_SESSIONS);
            let live: HashSet<_> = paths.into_iter().map(|(_, p)| p).collect();
            self.watches.retain(|path, _| live.contains(path));
            for path in live {
                self.watches.entry(path).or_default();
            }
            self.read_titles(&root.join("session_index.jsonl"));
            self.scanned = Some(Instant::now());
        }
        let mut sessions = Vec::new();
        for (path, watch) in &mut self.watches {
            // An archived rollout moves out of sessions/, so it disappears.
            if watch.follow(path).is_err() || !watch.ready || !watch.accepted {
                continue;
            }
            let mut s = watch.snapshot(now, mode);
            if now.saturating_sub(s.updated_ms) > RECENT_MS {
                continue;
            }
            if let Some(title) = s
                .session_id
                .strip_prefix("codex:")
                .and_then(|id| self.titles.get(id))
            {
                s.chat_title = title.clone();
            }
            sessions.push(s);
        }
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        sessions
    }

    fn read_titles(&mut self, path: &Path) {
        let Ok(meta) = path.metadata() else {
            self.titles.clear();
            self.index_stamp = None;
            return;
        };
        let Ok(modified) = meta.modified() else {
            return;
        };
        let stamp = (modified, meta.len());
        if self.index_stamp == Some(stamp) {
            return;
        }
        let Ok(mut file) = File::open(path) else {
            return;
        };
        // The append-only index can grow for years. Recent names are at the end.
        let start = meta.len().saturating_sub(2 * 1024 * 1024);
        if file.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        let mut bytes = Vec::new();
        if file.take(2 * 1024 * 1024).read_to_end(&mut bytes).is_err() {
            return;
        }
        let mut titles = HashMap::new();
        for (i, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if (start > 0 && i == 0) || !line.ends_with(b"\n") {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if let (Some(id), Some(name)) = (v["id"].as_str(), v["thread_name"].as_str()) {
                titles.insert(id.to_string(), truncate(name, 180));
            }
        }
        self.titles = titles;
        self.index_stamp = Some(stamp);
    }
}

fn visit(root: &Path, depth: usize, cutoff: u64, paths: &mut Vec<(u64, PathBuf)>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth > 0 {
            visit(&entry.path(), depth - 1, cutoff, paths);
        } else if kind.is_file()
            && entry.file_name().to_string_lossy().starts_with("rollout-")
            && entry.path().extension().is_some_and(|e| e == "jsonl")
        {
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if modified >= cutoff {
                paths.push((modified, entry.path()));
            }
        }
    }
}

fn stamp(v: &Value) -> u64 {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
        .unwrap_or(0)
}

fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn content(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    v.as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

impl Watch {
    fn follow(&mut self, path: &Path) -> std::io::Result<()> {
        let meta = path.metadata()?;
        let modified = meta.modified().ok();
        if meta.len() < self.offset || (meta.len() == self.offset && self.modified != modified) {
            *self = Self::default();
        }
        if self.offset == 0 {
            let mut head = Vec::new();
            BufReader::new(File::open(path)?.take(MAX_RECORD as u64))
                .read_until(b'\n', &mut head)?;
            if !head.ends_with(b"\n") {
                return Ok(());
            }
            self.feed(&head);
            self.offset = head.len() as u64;
            if self.accepted && meta.len() > INITIAL_TAIL + self.offset {
                self.offset = meta.len() - INITIAL_TAIL;
                self.oversized = true; // discard the partial record at the tail boundary
                self.session.turn_tools_partial = true;
                self.session.state = "thinking".into();
            }
        }
        if !self.accepted {
            self.offset = meta.len();
            self.modified = modified;
            self.ready = true;
            return Ok(());
        }
        if meta.len() == self.offset {
            self.ready = true;
            return Ok(());
        }
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(READ_BYTES).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        self.modified = modified;
        self.feed(&bytes);
        self.ready = self.offset >= meta.len();
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8]) {
        for chunk in bytes.split_inclusive(|b| *b == b'\n') {
            if !self.oversized {
                if self.pending.len() + chunk.len() > MAX_RECORD {
                    self.pending.clear();
                    self.oversized = true;
                } else {
                    self.pending.extend_from_slice(chunk);
                }
            }
            if chunk.ends_with(b"\n") {
                if !self.oversized {
                    if let Ok(v) = serde_json::from_slice::<Value>(&self.pending) {
                        self.apply(&v);
                    }
                }
                self.pending.clear();
                self.oversized = false;
            }
        }
    }

    fn apply(&mut self, v: &Value) {
        let p = &v["payload"];
        let at = stamp(&v["timestamp"]);
        if v["type"] == "session_meta" {
            let id = p["id"]
                .as_str()
                .or_else(|| p["session_id"].as_str())
                .unwrap_or_default();
            self.accepted = plain_id(id)
                && !p["source"].is_object()
                && !matches!(
                    p["thread_source"].as_str(),
                    Some("guardian_review" | "subagent")
                );
            if !self.accepted {
                return;
            }
            let cwd = string(p, "cwd");
            let project = crate::project::resolve(&cwd);
            self.session = Session {
                session_id: format!("codex:{id}"),
                provider: Provider::Codex,
                chat_id: if p["originator"]
                    .as_str()
                    .is_some_and(|o| o.to_lowercase().contains("desktop"))
                {
                    id.into()
                } else {
                    String::new()
                },
                cwd,
                project: project.name,
                project_root: project.root.to_string_lossy().into(),
                workspace: project.workspace,
                scratch: project.scratch,
                state: "idle".into(),
                started_ms: at,
                updated_ms: at,
                ..Session::default()
            };
            return;
        }
        if !self.accepted || at == 0 {
            return;
        }
        match v["type"].as_str() {
            Some("event_msg") => self.event(p, at),
            Some("response_item") => self.response(p, at),
            _ => {}
        }
    }

    fn progress(&mut self, at: u64) {
        self.session.updated_ms = at;
        self.session.stalled = false;
    }

    fn say(&mut self, text: &str, at: u64, thought: bool) {
        let Some(line) = summary_within(text, 110) else {
            return;
        };
        if thought {
            self.thought = line;
            self.thought_ms = at;
        } else {
            self.speech = line;
            self.speech_ms = at;
        }
        self.progress(at);
    }

    fn start(&mut self, p: &Value, at: u64) {
        let id = string(p, "turn_id");
        if !id.is_empty() && self.session.prompt_id == id {
            return;
        }
        self.session.clear_pending();
        self.session.clear_permission();
        self.session.prompt_id = id;
        self.session.turn_started_ms = at;
        self.session.turn_ended_ms = 0;
        self.session.turn_tools = 0;
        self.session.turn_tools_partial = false;
        self.session.state = "thinking".into();
        self.session.kind = "Thinking".into();
        self.session.activity = "Starting a turn".into();
        self.session.headline.clear();
        self.session.detail.clear();
        self.speech.clear();
        self.thought.clear();
        self.calls.clear();
        self.waiting_call.clear();
        self.session.recent.clear();
        self.progress(at);
    }

    fn finish(&mut self, p: &Value, at: u64, failed: bool) {
        // A delayed completion from the previous turn must not finish this one.
        if p["turn_id"]
            .as_str()
            .is_some_and(|id| !self.session.prompt_id.is_empty() && id != self.session.prompt_id)
        {
            return;
        }
        self.session.clear_pending();
        self.session.clear_permission();
        self.waiting_call.clear();
        self.session.state = "idle".into();
        self.session.outcome = if failed { "failed" } else { "done" }.into();
        self.session.outcome_ms = at;
        self.session.settles_ms = at + 1500;
        self.session.turn_ended_ms = at;
        if self.session.turn_started_ms == 0 {
            // A completion can restore the start lost outside the initial
            // tail. Its action count remains explicitly a lower bound.
            self.session.turn_started_ms = p["started_at"]
                .as_u64()
                .and_then(|seconds| seconds.checked_mul(1000))
                .filter(|start| *start <= at)
                .unwrap_or(0);
        }
        self.session.activity = if failed {
            "Turn interrupted"
        } else {
            "Turn complete"
        }
        .into();
        if let Some(text) = p["last_agent_message"].as_str().filter(|s| !s.is_empty()) {
            self.say(text, at, false);
            self.session.detail = truncate(text, 4000);
        }
        if failed {
            self.speech.clear();
            self.thought.clear();
        }
        self.progress(at);
    }

    fn event(&mut self, p: &Value, at: u64) {
        let kind = p["type"].as_str().unwrap_or_default();
        match kind {
            "task_started" | "turn_started" => self.start(p, at),
            "task_complete" | "turn_complete" => self.finish(p, at, false),
            "turn_aborted" => self.finish(p, at, true),
            "error" if p["will_retry"] != true => {
                self.finish(p, at, true);
                self.session.activity = summary_within(&string(p, "message"), 110)
                    .unwrap_or_else(|| "Turn failed".into());
            }
            "user_message" => {
                self.session.headline =
                    summary_within(&string(p, "message"), 110).unwrap_or_default();
                self.progress(at);
            }
            "agent_message" => self.say(&string(p, "message"), at, false),
            "agent_reasoning" => self.say(&string(p, "text"), at, true),
            "token_count" if self.session.is_running() => self.progress(at),
            "item_completed" => {
                let item = &p["item"];
                match item["type"].as_str() {
                    Some("AgentMessage") => self.say(&content(&item["content"]), at, false),
                    Some("Reasoning") => {
                        let text = item["summary_text"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default();
                        self.say(&text, at, true);
                    }
                    _ => {}
                }
            }
            // Some versions persist these requests. Silence alone is never a
            // permission prompt: versions that omit them just keep Working.
            "exec_approval_request" | "apply_patch_approval_request" | "request_user_input" => {
                if !self.session.is_running() {
                    return;
                }
                self.session.waiting_since = at;
                self.session.waiting_reason = if kind == "request_user_input" {
                    "Codex has a question for you"
                } else {
                    "Codex needs your approval"
                }
                .into();
                self.progress(at);
            }
            _ => return,
        }
        self.session.event = kind.into();
    }

    fn response(&mut self, p: &Value, at: u64) {
        match p["type"].as_str() {
            Some("message") if p["role"] == "assistant" => {
                let text = content(&p["content"]);
                self.say(&text, at, false);
                if p["phase"] == "final_answer" {
                    self.session.detail = truncate(&text, 4000);
                }
            }
            Some("reasoning") => self.say(&content(&p["summary"]), at, true),
            Some("function_call" | "custom_tool_call") => {
                if !self.session.is_running() {
                    return;
                }
                let id = string(p, "call_id");
                if !id.is_empty() && !self.calls.insert(id.clone()) {
                    return;
                }
                self.session.turn_tools += 1;
                self.session.tools += 1;
                self.session.clear_pending();
                self.session.clear_permission();
                self.waiting_call.clear();
                let name = string(p, "name");
                let kind = tool_kind(&name);
                self.session.state = "running".into();
                self.session.kind = kind.into();
                self.session.activity = format!("{kind}: {}", truncate(&name, 70));
                self.session
                    .push_recent("running", &self.session.activity.clone(), at);
                if name.ends_with("request_user_input") {
                    self.waiting_call = id;
                    self.session.waiting_since = at;
                    let args = p["arguments"]
                        .as_str()
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .unwrap_or(Value::Null);
                    self.session.waiting_reason = args["questions"][0]["question"]
                        .as_str()
                        .and_then(|s| summary_within(s, 180))
                        .unwrap_or_else(|| "Codex has a question for you".into());
                }
                self.progress(at);
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                if !self.session.is_running() {
                    return;
                }
                if self.waiting_call.is_empty() || p["call_id"] == self.waiting_call {
                    self.session.waiting_since = 0;
                    self.session.waiting_reason.clear();
                    self.waiting_call.clear();
                }
                self.progress(at);
            }
            _ => {}
        }
    }

    fn snapshot(&self, now: u64, mode: Mode) -> Session {
        let mut s = self.session.clone();
        s.narration = match mode {
            Mode::Off => String::new(),
            Mode::Thoughts
                if self.thought_ms > self.speech_ms
                    && !self.thought.is_empty()
                    && s.outcome.is_empty() =>
            {
                self.thought.clone()
            }
            _ => self.speech.clone(),
        };
        // A missing terminal event means unknown, never a successful finish.
        // Work can be silent (a long command), so this is explicitly a timeout.
        if s.is_running() && s.waiting_since == 0 && now.saturating_sub(s.updated_ms) > STALE_MS {
            s.state = "idle".into();
            s.stalled = true;
            s.activity.clear();
            s.narration.clear();
        }
        s
    }
}

fn tool_kind(name: &str) -> &'static str {
    if name.contains("patch") || name.contains("edit_file") {
        "Editing"
    } else if name.contains("search") || name.contains("web") {
        "Searching"
    } else if name.contains("read") || name.contains("view") {
        "Reading"
    } else if name.contains("spawn_agent") {
        "Delegating"
    } else if name.contains("wait") {
        "Waiting"
    } else {
        "Running"
    }
}

fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn open(app: &tauri::AppHandle, session_id: &str) -> bool {
    let Some(id) = session_id.strip_prefix("codex:").filter(|id| plain_id(id)) else {
        return false;
    };
    let known = sessions(Mode::Off).iter().any(|s| s.chat_id == id);
    known && crate::desktop::open_url(app, &format!("codex://threads/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    const AT: u64 = 1788770600000;

    fn record(kind: &str, payload: Value) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(&json!({
            "timestamp": "2026-09-07T08:43:20Z", "type": kind, "payload": payload
        }))
        .unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn meta(id: &str) -> Vec<u8> {
        record(
            "session_meta",
            json!({"id": id, "cwd": "/projects/demo", "source": "vscode", "originator": "Codex Desktop"}),
        )
    }

    fn watch() -> Watch {
        let mut w = Watch::default();
        w.feed(&meta("same-id"));
        w.feed(&record(
            "event_msg",
            json!({"type": "task_started", "turn_id": "turn-1"}),
        ));
        w
    }

    #[test]
    fn complete_is_explicit_and_new_turn_clears_everything() {
        let mut w = watch();
        w.feed(&record(
            "response_item",
            json!({"type": "function_call", "name": "apply_patch", "call_id": "call-1"}),
        ));
        w.feed(&record("response_item", json!({"type": "message", "role": "assistant", "phase": "final_answer", "content": [{"text": "The fix is ready."}]})));
        assert!(w.session.outcome.is_empty());
        assert_eq!(w.session.turn_tools, 1);
        w.feed(&record("event_msg", json!({"type": "task_complete", "turn_id": "turn-1", "last_agent_message": "The fix is ready."})));
        assert_eq!(w.session.outcome, "done");
        assert!(w.session.turn_ended_ms > 0);
        w.feed(&record(
            "event_msg",
            json!({"type": "task_started", "turn_id": "turn-2"}),
        ));
        let s = w.snapshot(AT, Mode::Thoughts);
        assert!(s.outcome.is_empty() && s.narration.is_empty() && s.detail.is_empty());
        assert_eq!(s.turn_tools, 0);
        assert_eq!(s.turn_ended_ms, 0);
        w.feed(&record(
            "event_msg",
            json!({"type": "task_complete", "turn_id": "turn-1"}),
        ));
        assert!(w.session.outcome.is_empty());
    }

    #[test]
    fn incomplete_and_malformed_records_do_not_change_state() {
        let mut w = watch();
        let complete = record(
            "event_msg",
            json!({"type": "task_complete", "turn_id": "turn-1"}),
        );
        w.feed(b"invalid json\n");
        w.feed(&complete[..complete.len() - 2]);
        assert!(w.session.outcome.is_empty());
        w.feed(&complete[complete.len() - 2..]);
        assert_eq!(w.session.outcome, "done");
    }

    #[test]
    fn raw_and_paginated_messages_narrate_without_double_counting_tools() {
        let mut w = watch();
        let call = record(
            "response_item",
            json!({"type": "custom_tool_call", "name": "exec", "call_id": "a"}),
        );
        w.feed(&call);
        w.feed(&call);
        w.feed(&record(
            "event_msg",
            json!({"type": "item_completed", "item": {"type": "CommandExecution", "id": "a"}}),
        ));
        assert_eq!(w.session.turn_tools, 1);
        w.feed(&record("event_msg", json!({"type": "item_completed", "item": {"type": "AgentMessage", "content": [{"text": "Checking both agents."}]}})));
        assert_eq!(
            w.snapshot(AT, Mode::Speech).narration,
            "Checking both agents."
        );
        w.say("A useful public summary.", AT + 1, true);
        assert_eq!(
            w.snapshot(AT + 1, Mode::Thoughts).narration,
            "A useful public summary."
        );
        assert_eq!(
            w.snapshot(AT + 1, Mode::Speech).narration,
            "Checking both agents."
        );
        assert!(w.snapshot(AT + 1, Mode::Off).narration.is_empty());
    }

    #[test]
    fn questions_wait_for_the_matching_answer() {
        let mut w = watch();
        w.feed(&record("response_item", json!({"type": "function_call", "name": "request_user_input", "call_id": "q", "arguments": "{\"questions\":[{\"question\":\"Which folder?\"}]}"})));
        assert_eq!(w.session.waiting_reason, "Which folder?");
        w.feed(&record(
            "response_item",
            json!({"type": "function_call_output", "call_id": "unrelated"}),
        ));
        assert!(w.session.waiting_since > 0);
        assert!(!w.snapshot(AT + STALE_MS * 2, Mode::Speech).stalled);
        w.feed(&record(
            "response_item",
            json!({"type": "function_call_output", "call_id": "q"}),
        ));
        assert_eq!(w.session.waiting_since, 0);
    }

    #[test]
    fn silence_and_interruptions_never_become_done() {
        let mut w = watch();
        let at = w.session.updated_ms;
        let stale = w.snapshot(at + STALE_MS + 1, Mode::Thoughts);
        assert!(stale.stalled && stale.outcome.is_empty());
        w.feed(&record(
            "event_msg",
            json!({"type": "turn_aborted", "turn_id": "turn-1"}),
        ));
        assert_eq!(w.session.outcome, "failed");
        assert_eq!(w.session.event, "turn_aborted");
    }

    #[test]
    fn legacy_claude_ids_cannot_collide_and_internal_agents_are_excluded() {
        let w = watch();
        let legacy: Session = serde_json::from_value(json!({"session_id": "same-id"})).unwrap();
        assert!(matches!(legacy.provider, Provider::Claude));
        assert_ne!(w.session.session_id, legacy.session_id);
        assert_eq!(w.session.chat_id, "same-id");
        let mut internal = Watch::default();
        internal.feed(&record(
            "session_meta",
            json!({"id": "internal", "source": {"subagent": {"other": "guardian"}}}),
        ));
        internal.feed(&record("event_msg", json!({"type": "task_started"})));
        assert!(!internal.accepted);
        assert!(!plain_id("../../escape?new=1"));
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "pipsqueak-codex-{}-{}",
                std::process::id(),
                ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(p.join("sessions/2026/09/07")).unwrap();
            Self(p)
        }
        fn path(&self) -> PathBuf {
            self.0.join("sessions/2026/09/07/rollout-fixture.jsonl")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_updates_renames_and_retires_archived_sessions() {
        let dir = Fixture::new();
        let path = dir.path();
        let mut bytes = meta("task-one");
        bytes.extend(record(
            "event_msg",
            json!({"type": "task_started", "turn_id": "t"}),
        ));
        fs::write(&path, bytes).unwrap();
        fs::write(
            dir.0.join("session_index.jsonl"),
            "{\"id\":\"task-one\",\"thread_name\":\"Original title\"}\n",
        )
        .unwrap();
        let mut tracker = Tracker::default();
        let sessions = tracker.read(&dir.0, AT, Mode::Speech);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].chat_title, "Original title");
        let completion = record(
            "event_msg",
            json!({"type": "task_complete", "turn_id": "t"}),
        );
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&completion[..10]).unwrap();
        assert!(tracker.read(&dir.0, AT, Mode::Speech)[0].outcome.is_empty());
        file.write_all(&completion[10..]).unwrap();
        assert_eq!(tracker.read(&dir.0, AT, Mode::Speech)[0].outcome, "done");
        fs::write(
            dir.0.join("session_index.jsonl"),
            "{\"id\":\"task-one\",\"thread_name\":\"Renamed title\"}\n",
        )
        .unwrap();
        tracker.scanned = None;
        assert_eq!(
            tracker.read(&dir.0, AT, Mode::Speech)[0].chat_title,
            "Renamed title"
        );
        fs::rename(&path, dir.0.join("archived.jsonl")).unwrap();
        assert!(tracker.read(&dir.0, AT, Mode::Speech).is_empty());
    }

    #[test]
    fn large_logs_start_at_a_bounded_tail_and_recover_after_truncation() {
        let dir = Fixture::new();
        let path = dir.path();
        let mut file = File::create(&path).unwrap();
        file.write_all(&meta("large")).unwrap();
        file.write_all(&vec![b'x'; INITIAL_TAIL as usize + 1024])
            .unwrap();
        file.write_all(b"\n").unwrap();
        file.write_all(&record(
            "response_item",
            json!({"type": "function_call", "name": "exec", "call_id": "one"}),
        ))
        .unwrap();
        let mut w = Watch::default();
        for _ in 0..20 {
            w.follow(&path).unwrap();
        }
        assert!(w.ready && w.session.turn_tools_partial);
        assert_eq!(w.session.turn_tools, 1);
        assert!(w.pending.len() <= MAX_RECORD);
        w.feed(&record(
            "event_msg",
            json!({"type": "task_complete", "started_at": AT / 1000 - 120}),
        ));
        assert_eq!(w.session.turn_started_ms, AT - 120_000);
        assert!(w.session.turn_tools_partial);
        let mut bytes = meta("large");
        bytes.extend(record(
            "event_msg",
            json!({"type": "task_started", "turn_id": "new"}),
        ));
        fs::write(&path, bytes).unwrap();
        w.follow(&path).unwrap();
        assert!(w.ready && !w.session.turn_tools_partial);
        assert_eq!(w.session.turn_tools, 0);
        assert_eq!(w.session.prompt_id, "new");
    }
}
