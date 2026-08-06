//! What the session is doing, in its own words.
//!
//! Hooks fire at tool boundaries, so the best they can say is "Editing
//! render.js" — the category of the thing, never the point of it. Claude says
//! the point of it out loud, in the sentence before it reaches for the tool
//! ("Now the frontend: chat titles, the done-card lifecycle"), and Claude Code
//! appends that to the session transcript as it happens.
//!
//! So this reads the transcript. Only the bytes that arrived since last time,
//! only the newest line worth showing, and only for sessions that are on
//! screen. Nothing is sent anywhere: the file is already on this disk, and the
//! line goes to a card two inches from the cursor.

use crate::state::Session;
use crate::text::{summary_within, truncate};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// How much of a transcript to read the first time one is seen.
///
/// Far more than the last thing said needs, because the other question asked
/// of this file reaches further back: a monitor or a subagent started twenty
/// minutes ago is still running, and its launch is only in the transcript at
/// the point it happened. Missing one under-counts, which fails towards the
/// old behaviour rather than towards a card stuck on "still working".
const FIRST_READ: u64 = 2 * 1024 * 1024;
/// A single appended chunk this large means something unusual happened (a
/// resumed session, a pasted file); read the tail of it rather than all of it.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;
/// The card is one line tall.
const LINE_LIMIT: usize = 110;

/// How much of itself a session is allowed to say.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// No narration at all; the card keeps the tool line.
    Off,
    /// Only what Claude actually said to you.
    Speech,
    /// What it said, and the first line of what it was thinking. Thoughts
    /// arrive roughly every twenty seconds against speech's ninety, which is
    /// the difference between a pet that narrates and one that occasionally
    /// remembers you are there.
    Thoughts,
}

impl Mode {
    pub fn parse(value: &str) -> Self {
        match value {
            "off" => Mode::Off,
            "speech" => Mode::Speech,
            _ => Mode::Thoughts,
        }
    }

    fn wants_thoughts(self) -> bool {
        self == Mode::Thoughts
    }
}

/// The live setting, so the poller does not read the config file three times a
/// second to ask a question whose answer changes twice a year.
static MODE: AtomicU8 = AtomicU8::new(2);

pub fn set_mode(mode: Mode) {
    MODE.store(
        match mode {
            Mode::Off => 0,
            Mode::Speech => 1,
            Mode::Thoughts => 2,
        },
        Ordering::Relaxed,
    );
}

pub fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        0 => Mode::Off,
        1 => Mode::Speech,
        _ => Mode::Thoughts,
    }
}

#[derive(Clone, Default)]
struct Watch {
    /// How far into the file we have already read.
    offset: u64,
    line: String,
    /// Work the turn started and has not been told is over: background shell
    /// commands, monitors, and subagents, by the id Claude Code gave each one.
    ///
    /// This is the difference between "the assistant stopped talking" and "the
    /// work is finished", and nothing else on disk records it — there is no
    /// pending-tasks file anywhere. The transcript has both halves though: an
    /// id when the thing is launched, and the same id in the notification when
    /// it completes. Keeping the set is just subtracting one from the other.
    outstanding: BTreeSet<String>,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Watch>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Watch>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Attaches the newest line each session has said, and how much of the work it
/// started is still running, to the session itself.
///
/// Both come from the same file and the same read. Narration can be switched
/// off; the outstanding count cannot, because it is not decoration — it is
/// what stops the card saying "Done" over five running subagents.
pub fn decorate(sessions: &mut [Session], mode: Mode) {
    let Ok(mut watches) = cache().lock() else {
        return;
    };
    let mut live: Vec<PathBuf> = Vec::new();

    for session in sessions.iter_mut() {
        let Some(path) = transcript_for(session) else {
            continue;
        };
        live.push(path.clone());
        let watch = watches.entry(path.clone()).or_default();
        follow(&path, watch, mode);
        if mode != Mode::Off && !watch.line.is_empty() {
            session.narration = watch.line.clone();
        }
        session.outstanding = watch.outstanding.len() as u64;
    }

    // A transcript nobody is watching any more is just a stale offset.
    watches.retain(|path, _| live.contains(path));
}

/// Where a session's transcript is.
///
/// The hook payload carries the path, which is the only answer that is always
/// right. Sessions written by an older build have no path yet, so the layout
/// Claude Code uses is the fallback: one directory per project, named after the
/// working directory with every awkward character replaced by a dash.
fn transcript_for(session: &Session) -> Option<PathBuf> {
    if !session.transcript.is_empty() {
        let path = PathBuf::from(&session.transcript);
        if path.is_file() {
            return Some(path);
        }
    }
    if session.cwd.is_empty() || session.session_id.is_empty() {
        return None;
    }
    let slug: String = session
        .cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = crate::state::home_dir()
        .join(".claude")
        .join("projects")
        .join(slug)
        .join(format!("{}.jsonl", session.session_id));
    path.is_file().then_some(path)
}

/// Reads whatever has been appended since last time and keeps the last line
/// worth showing.
fn follow(path: &Path, watch: &mut Watch, mode: Mode) {
    let Ok(mut file) = File::open(path) else {
        return;
    };
    let Ok(length) = file.metadata().map(|m| m.len()) else {
        return;
    };
    if length == watch.offset {
        return;
    }
    // Truncated or replaced: the offset we were holding means nothing now.
    if length < watch.offset {
        *watch = Watch::default();
    }

    let start = if watch.offset == 0 {
        length.saturating_sub(FIRST_READ)
    } else {
        watch.offset.max(length.saturating_sub(MAX_CHUNK))
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut buffer = Vec::with_capacity((length - start) as usize);
    if file.take(length - start).read_to_end(&mut buffer).is_err() {
        return;
    }

    // The last line may still be half-written, so it is neither parsed nor
    // counted as read; the next poll will see the whole of it.
    let complete = match buffer.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => index + 1,
        None => return,
    };
    watch.offset = start + complete as u64;
    let text = String::from_utf8_lossy(&buffer[..complete]);

    for line in text.lines() {
        track_tasks(line, &mut watch.outstanding);
        if mode == Mode::Off {
            continue;
        }
        // A cheap reject before the expensive parse: most of a transcript is
        // tool results, which are the one thing this never shows.
        if !line.contains("\"assistant\"") {
            continue;
        }
        if let Some(said) = spoken(line, mode) {
            watch.line = said;
        }
    }
}

/// Adds work this line launched, and removes work it reported finished.
///
/// Claude Code hands every asynchronous thing an id at launch — a background
/// shell command, a monitor, a subagent — and quotes that same id back when it
/// completes. Neither half is a documented API, so this reads them as the
/// strings they are and treats absence as "nothing launched", which is the
/// answer the pet had anyway before it could see any of this.
fn track_tasks(line: &str, outstanding: &mut BTreeSet<String>) {
    // Launched. Background shells and monitors carry their id directly; a
    // subagent's launch is only distinguishable from its later mentions by
    // sitting next to the status the tool result was given.
    for key in ["\"backgroundTaskId\":\"", "\"taskId\":\""] {
        if let Some(id) = quoted_after(line, key) {
            outstanding.insert(id);
        }
    }
    if line.contains("\"async_launched\"") {
        if let Some(id) = quoted_after(line, "\"agentId\":\"") {
            outstanding.insert(id);
        }
    }

    // Finished. The completion notification names the id and says so plainly;
    // an id can be notified more than once (a resumable agent), which a set
    // handles without caring.
    if line.contains("<status>completed</status>") {
        if let Some(id) = between(line, "<task-id>", "</task-id>") {
            outstanding.remove(&id);
        }
    }
    // Stopped by hand, which is an ending like any other.
    if let Some(rest) = line.split_once("Successfully stopped task: ") {
        let id: String = rest
            .1
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !id.is_empty() {
            outstanding.remove(&id);
        }
    }
}

/// The quoted string that follows `key`, if it looks like an id.
fn quoted_after(line: &str, key: &str) -> Option<String> {
    let rest = line.split_once(key)?.1;
    let id: String = rest.chars().take_while(|c| *c != '"').collect();
    plausible_id(&id).then_some(id)
}

fn between(line: &str, open: &str, close: &str) -> Option<String> {
    let rest = line.split_once(open)?.1;
    let id = rest.split_once(close)?.0.to_string();
    plausible_id(&id).then_some(id)
}

/// Ids are short, opaque and alphanumeric. Anything else is a field that
/// happens to share a name, and counting it would strand a card on "still
/// working" forever.
fn plausible_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The line an assistant entry is worth, if any.
fn spoken(line: &str, mode: Mode) -> Option<String> {
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    // Subagent traffic lands in the same transcript, marked as a sidechain.
    // Its inner monologue on the parent card read as the live line flickering
    // between unrelated voices.
    if entry
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let content = entry.get("message")?.get("content")?.as_array()?;
    let mut best = None;
    for part in content {
        let kind = part.get("type").and_then(Value::as_str).unwrap_or_default();
        let raw = match kind {
            "text" => part.get("text").and_then(Value::as_str),
            "thinking" if mode.wants_thoughts() => part.get("thinking").and_then(Value::as_str),
            _ => None,
        };
        let Some(raw) = raw else { continue };
        // Thinking is a stream of consciousness and speech is a paragraph. Both
        // are usable one line at a time, and the last line of a thought is the
        // one that says what it is about to do.
        let line = if kind == "thinking" {
            last_thought(raw)
        } else {
            summary_within(raw, LINE_LIMIT)
        };
        if let Some(line) = line {
            best = Some(line);
        }
    }
    best
}

/// The freshest usable sentence out of a block of thinking.
///
/// Read backwards on purpose: by the end of a thought it has stopped weighing
/// and started deciding, and "now check whether the poller still runs" is worth
/// a card line in a way that "hmm, several things could be wrong here" is not.
fn last_thought(raw: &str) -> Option<String> {
    let mut lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.reverse();
    for line in lines {
        // Whole sentences only. A trailing fragment mid-thought reads as a
        // glitch, and the sentence before it says the same thing properly.
        let sentence = line
            .rsplit_once(". ")
            .map(|(_, tail)| tail)
            .unwrap_or(line)
            .trim();
        if let Some(found) = summary_within(sentence, LINE_LIMIT) {
            if found.split_whitespace().count() >= 3 {
                return Some(truncate(&found, LINE_LIMIT));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tracked(lines: &[&str]) -> Vec<String> {
        let mut out = BTreeSet::new();
        for line in lines {
            track_tasks(line, &mut out);
        }
        out.into_iter().collect()
    }

    /// The shapes below are copied from a real transcript. They are not a
    /// documented API, so if Claude Code changes them these tests are the
    /// thing that notices.
    #[test]
    fn a_background_command_is_outstanding_until_it_reports_back() {
        let launched = r#"{"type":"user","toolUseResult":{"stdout":"","backgroundTaskId":"bix46i2qi"}}"#;
        assert_eq!(tracked(&[launched]), vec!["bix46i2qi"]);

        let finished = r#"{"type":"queue-operation","content":"<task-notification><task-id>bix46i2qi</task-id><status>completed</status><summary>Background command finished</summary>"}"#;
        assert!(tracked(&[launched, finished]).is_empty());
    }

    /// A subagent's tool result arrives immediately and means "launched", not
    /// "finished" — believing it is how the pet came to say Done over five
    /// running agents.
    #[test]
    fn a_subagent_is_outstanding_from_launch_until_its_notification() {
        let launched = r#"{"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"a971514006ea69ef6"}}"#;
        assert_eq!(tracked(&[launched]), vec!["a971514006ea69ef6"]);

        let finished = r#"{"type":"queue-operation","content":"<task-notification><task-id>a971514006ea69ef6</task-id><status>completed</status>"}"#;
        assert!(tracked(&[launched, finished]).is_empty());
    }

    #[test]
    fn several_at_once_drain_one_at_a_time() {
        let a = r#"{"toolUseResult":{"status":"async_launched","agentId":"aaa111"}}"#;
        let b = r#"{"toolUseResult":{"status":"async_launched","agentId":"bbb222"}}"#;
        let monitor = r#"{"toolUseResult":{"taskId":"ba7xghdk0","timeoutMs":3000000}}"#;
        let done_a = r#"{"content":"<task-notification><task-id>aaa111</task-id><status>completed</status>"}"#;
        assert_eq!(tracked(&[a, b, monitor]).len(), 3);
        assert_eq!(tracked(&[a, b, monitor, done_a]), vec!["ba7xghdk0", "bbb222"]);
    }

    #[test]
    fn stopping_a_task_by_hand_ends_it_too() {
        let monitor = r#"{"toolUseResult":{"taskId":"ba7xghdk0","timeoutMs":3000}}"#;
        let stopped = r#"{"content":"Successfully stopped task: ba7xghdk0 (prev=…)"}"#;
        assert!(tracked(&[monitor, stopped]).is_empty());
    }

    /// An ordinary turn must not leave anything behind, or every card would
    /// eventually be stuck reporting work that never existed.
    #[test]
    fn ordinary_lines_launch_nothing() {
        let said = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"taskId is a field name"}]}}"#;
        let tool = r#"{"type":"user","toolUseResult":{"stdout":"ok","interrupted":false}}"#;
        // A mention with no id, and an id-shaped value that is not one.
        let empty = r#"{"toolUseResult":{"backgroundTaskId":""}}"#;
        let sentence = r#"{"toolUseResult":{"taskId":"not an id, but a sentence with spaces"}}"#;
        assert!(tracked(&[said, tool, empty, sentence]).is_empty());
    }

    fn entry(kind: &str, text: &str) -> String {
        let mut part = serde_json::Map::new();
        part.insert("type".into(), json!(kind));
        part.insert(kind.into(), json!(text));
        json!({ "type": "assistant", "message": { "content": [Value::Object(part)] } }).to_string()
    }

    /// Subagents write into the same transcript, marked `isSidechain`. Their
    /// lines on the parent card read as the narration jumping between voices.
    #[test]
    fn a_sidechain_voice_stays_off_the_card() {
        let line = json!({
            "type": "assistant",
            "isSidechain": true,
            "message": { "content": [{ "type": "text", "text": "Subagent progress report." }] }
        })
        .to_string();
        assert_eq!(spoken(&line, Mode::Speech), None);
    }

    #[test]
    fn speech_is_taken_as_written() {
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "text", "text": "Now the frontend: chat titles and the done card." }] }
        })
        .to_string();
        assert_eq!(
            spoken(&line, Mode::Speech).as_deref(),
            Some("Now the frontend: chat titles and the done card.")
        );
    }

    /// The greeting rule earns its keep here: every reply in this session opens
    /// with one, and a pet that narrates "Tristan," is worse than silent.
    #[test]
    fn a_greeting_is_never_the_narration() {
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "text", "text": "Tristan,\n\nInstalled and running." }] }
        })
        .to_string();
        assert_eq!(
            spoken(&line, Mode::Speech).as_deref(),
            Some("Installed and running.")
        );
    }

    #[test]
    fn thoughts_are_only_read_when_asked_for() {
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "thinking", "thinking": "The poller is the suspect here. Let me check whether it still runs." }] }
        })
        .to_string();
        assert_eq!(spoken(&line, Mode::Speech), None);
        assert_eq!(
            spoken(&line, Mode::Thoughts).as_deref(),
            Some("Let me check whether it still runs.")
        );
    }

    #[test]
    fn tool_calls_say_nothing() {
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "tool_use", "name": "Read", "input": {} }] }
        })
        .to_string();
        assert_eq!(spoken(&line, Mode::Thoughts), None);
        let result =
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result" }] } })
                .to_string();
        assert_eq!(spoken(&result, Mode::Thoughts), None);
    }

    #[test]
    fn a_fragment_of_a_thought_is_not_a_sentence() {
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "thinking", "thinking": "Right.\nOK\nso" }] }
        })
        .to_string();
        assert_eq!(spoken(&line, Mode::Thoughts), None);
    }

    /// The setting arrives as a string from a hand-editable config file, so
    /// the only wrong answer is one that turns narration off by accident.
    #[test]
    fn an_unreadable_setting_still_narrates() {
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse("speech"), Mode::Speech);
        assert_eq!(Mode::parse("thoughts"), Mode::Thoughts);
        assert_eq!(Mode::parse("nonsense"), Mode::Thoughts);
        assert_eq!(Mode::parse(""), Mode::Thoughts);
    }

    #[test]
    fn a_half_written_line_is_left_for_next_time() {
        let dir = std::env::temp_dir().join(format!("pipsqueak-narration-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("transcript.jsonl");
        let whole = entry("text", "First thing done.");
        std::fs::write(&path, format!("{whole}\n{{\"type\":\"assist")).unwrap();

        let mut watch = Watch::default();
        follow(&path, &mut watch, Mode::Speech);
        assert_eq!(watch.line, "First thing done.");
        assert_eq!(
            watch.offset,
            whole.len() as u64 + 1,
            "the partial line is not consumed"
        );

        // The rest of it arrives, and is read exactly once.
        let second = entry("text", "Second thing done.");
        std::fs::write(&path, format!("{whole}\n{second}\n")).unwrap();
        follow(&path, &mut watch, Mode::Speech);
        assert_eq!(watch.line, "Second thing done.");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
