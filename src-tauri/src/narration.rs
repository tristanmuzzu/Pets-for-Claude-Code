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
    /// The last authoritative correction taken from a hook payload, so it is
    /// applied once instead of on every one of the three polls a second.
    synced_ms: u64,
    /// The session's `updated_ms` at the last read, so a session that has
    /// had no event since is not reopened.
    seen_updated_ms: u64,
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
        // Claude Code's own answer wins over anything inferred from the file.
        // Applied before the read so a launch that happened after the snapshot
        // still counts.
        if session.tasks_ms > watch.synced_ms {
            watch.synced_ms = session.tasks_ms;
            watch.outstanding = session.tasks.iter().cloned().collect();
        }
        // A transcript only grows while something is happening: a turn fires
        // hook events, and background work reports into the file before the
        // hook that follows it. So a session that is not running, has nothing
        // outstanding, and has had no event since the last read cannot have
        // appended a byte — and opening its file three times a second, for
        // every idle chat on the machine, was three quarters of the poller's
        // file traffic. First reads always happen, so the card of a finished
        // chat still gets its last line.
        let quiet = watch.offset > 0
            && !session.is_running()
            && watch.outstanding.is_empty()
            && watch.seen_updated_ms == session.updated_ms;
        if !quiet {
            follow(&path, watch, mode);
            watch.seen_updated_ms = session.updated_ms;
        }
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

/// The statuses that mean a task is over, however it went.
///
/// `running` is deliberately absent: that notification is a progress report,
/// not an ending. Only `completed` used to be here, and one agent that failed
/// or one background command killed left the count permanently one too high,
/// so every later turn read "Finishing · 1 running" and never went green.
const OVER: [&str; 4] = ["completed", "failed", "stopped", "killed"];

/// Adds work this line launched, and removes work it reported finished.
///
/// Read from the *shape* of the entry rather than by looking for id-shaped
/// substrings anywhere on the line, which is what this used to do and is why
/// the count was mostly fiction.
///
/// THE BUG THIS EXISTS FOR
///
/// `taskId` is overwhelmingly the *to-do list's* field, not an async one:
/// across 233 real transcripts, 694 of 723 occurrences were `TaskUpdate`
/// ticking a checkbox off, with ids like `"1"`. Every one of them added a
/// background task that could never finish, because no completion notification
/// for a checkbox exists. 39 of those sessions ended holding 302 phantom
/// runners between them — one card said "Finishing · 7 running" over a turn
/// that had been over for two minutes. Reading `toolUseResult` as an object
/// and only trusting the four shapes Claude Code actually launches work with
/// takes those 302 down to 11, every one of which is a real task that outlived
/// its session.
fn track_tasks(line: &str, outstanding: &mut BTreeSet<String>) {
    // Endings first, and by substring, because a notification is delivered as
    // an XML-ish blob inside a JSON string rather than as fields.
    if line.contains("task-notification") {
        end_notified(line, outstanding);
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

    // A cheap reject before the parse. Most of a transcript mentions none of
    // these, and parsing every line would triple the cost of a poll.
    if !(line.contains("backgroundTaskId") || line.contains("agentId") || line.contains("taskId")) {
        return;
    }
    let Ok(entry) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if let Some(result) = entry.get("toolUseResult").and_then(Value::as_object) {
        let field = |key: &str| result.get(key).and_then(Value::as_str);
        let status = field("status").unwrap_or_default();

        // A to-do list write. Same field name, nothing asynchronous about it.
        let is_todo = result.contains_key("updatedFields")
            || result.contains_key("statusChange")
            || result.contains_key("tasks");
        // Launched, in the four shapes that really mean it.
        if !is_todo {
            if let Some(id) = field("backgroundTaskId").filter(|id| plausible_id(id)) {
                outstanding.insert(id.to_string());
            }
            if status == "async_launched" {
                for key in ["agentId", "taskId"] {
                    if let Some(id) = field(key).filter(|id| plausible_id(id)) {
                        outstanding.insert(id.to_string());
                    }
                }
            }
            // A monitor: an id handed back alongside how long it may run.
            // `agentId` is deliberately not read here — a *synchronous*
            // subagent's result carries one too, and it has already finished.
            if result.contains_key("timeoutMs") || result.contains_key("persistent") {
                if let Some(id) = field("taskId").filter(|id| plausible_id(id)) {
                    outstanding.insert(id.to_string());
                }
            }
        }
        // A result that reports the work over. Rarer than the notification and
        // not a substitute for it, but a task whose status is echoed back on a
        // later line used to be *re-added* by that echo and never removed
        // again.
        if OVER.contains(&status) {
            for key in ["backgroundTaskId", "agentId", "taskId"] {
                if let Some(id) = field(key) {
                    outstanding.remove(id);
                }
            }
        }
    }
    // The same echo, in the shape Claude Code attaches it to a later turn.
    if let Some(attachment) = entry.get("attachment").and_then(Value::as_object) {
        let status = attachment
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("");
        if OVER.contains(&status) {
            for key in ["taskId", "agentId"] {
                if let Some(id) = attachment.get(key).and_then(Value::as_str) {
                    outstanding.remove(id);
                }
            }
        }
    }
}

/// Retires every task named by a finished task-notification.
///
/// Every id, not the first one: the scan Claude Code runs when a session is
/// resumed reports the previous session's abandoned work as one notification
/// listing up to ten `<task-id>` tags under a single `<status>stopped</status>`.
/// Reading only the first discarded nine endings, and those nine sat in the
/// count for the rest of the session.
fn end_notified(line: &str, outstanding: &mut BTreeSet<String>) {
    let Some(status) = between(line, "<status>", "</status>") else {
        return;
    };
    if !OVER.contains(&status.as_str()) {
        return;
    }
    let mut rest = line;
    while let Some((_, tail)) = rest.split_once("<task-id>") {
        let Some((id, next)) = tail.split_once("</task-id>") else {
            return;
        };
        // `__orphan_summary__:shell` and friends are markers the notification
        // itself describes as "not tasks".
        if !id.starts_with("__") {
            outstanding.remove(id);
        }
        rest = next;
    }
}

fn between(line: &str, open: &str, close: &str) -> Option<String> {
    let rest = line.split_once(open)?.1;
    Some(rest.split_once(close)?.0.to_string())
}

/// Ids are short, opaque and alphanumeric. Anything else is a field that
/// happens to share a name, and counting it would strand a card on "still
/// working" forever.
///
/// All-digits is rejected outright: those are to-do list rows, and nothing
/// Claude Code launches asynchronously is numbered `1`.
fn plausible_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.chars().all(|c| c.is_ascii_digit())
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
        let launched =
            r#"{"type":"user","toolUseResult":{"stdout":"","backgroundTaskId":"bix46i2qi"}}"#;
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
        assert_eq!(
            tracked(&[a, b, monitor, done_a]),
            vec!["ba7xghdk0", "bbb222"]
        );
    }

    /// Work ends four ways in a real transcript, and three of them are not
    /// "completed". Counting only completions left a card reading
    /// "Finishing · 1 running" for the rest of the session over one agent that
    /// had already failed.
    #[test]
    fn work_that_ends_badly_still_ends() {
        let launch = |id: &str| {
            format!(r#"{{"toolUseResult":{{"status":"async_launched","agentId":"{id}"}}}}"#)
        };
        let notify = |id: &str, status: &str| {
            format!(
                r#"{{"content":"<task-notification><task-id>{id}</task-id><status>{status}</status>"}}"#
            )
        };
        for status in ["completed", "failed", "stopped", "killed"] {
            let lines = [launch("aaa111"), notify("aaa111", status)];
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            assert!(tracked(&refs).is_empty(), "{status} should end the work");
        }
        // A progress report is not an ending.
        let lines = [launch("aaa111"), notify("aaa111", "running")];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(tracked(&refs), vec!["aaa111"]);
    }

    #[test]
    fn stopping_a_task_by_hand_ends_it_too() {
        let monitor = r#"{"toolUseResult":{"taskId":"ba7xghdk0","timeoutMs":3000}}"#;
        let stopped = r#"{"content":"Successfully stopped task: ba7xghdk0 (prev=…)"}"#;
        assert!(tracked(&[monitor, stopped]).is_empty());
    }

    /// The bug that made the count fiction.
    ///
    /// `taskId` is overwhelmingly the *to-do list's* field: 694 of the 723
    /// occurrences across 233 real transcripts were a checkbox being ticked,
    /// numbered `"1"`, `"2"`, `"3"`. Each one added a background task that
    /// could never finish, because no completion notification for a checkbox
    /// exists. 39 of those sessions ended holding 302 phantom runners between
    /// them; one card read "Finishing · 7 running" over a turn that had been
    /// over for two minutes.
    #[test]
    fn ticking_off_a_to_do_item_is_not_launching_a_task() {
        let created = r#"{"toolUseResult":{"tasks":[{"taskId":"1","title":"Read the code"}]}}"#;
        let updated = r#"{"toolUseResult":{"success":true,"taskId":"1","updatedFields":["status"],"statusChange":{"from":"in_progress","to":"completed"}}}"#;
        // The model *asking* for the change is not the system confirming one.
        let asked = r#"{"message":{"content":[{"type":"tool_use","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        assert!(tracked(&[created, updated, asked]).is_empty());
        // Belt and braces: nothing Claude Code launches is numbered.
        assert!(!plausible_id("1"));
        assert!(plausible_id("bix46i2qi"));
    }

    /// Resuming a session produces one notification listing every task the
    /// previous session abandoned — up to ten `<task-id>` tags under a single
    /// `<status>`. Reading only the first discarded nine endings, and those
    /// nine sat in the count for the rest of the session.
    #[test]
    fn one_notification_can_end_more_than_one_task() {
        let launch = |id: &str| format!(r#"{{"toolUseResult":{{"backgroundTaskId":"{id}"}}}}"#);
        let orphans = r#"{"content":"<task-notification><task-id>aaa111</task-id><task-id>bbb222</task-id><task-id>__orphan_summary__:shell</task-id><status>stopped</status><summary>2 background shell command task(s) from the previous session have no completion record</summary></task-notification>"}"#;
        let lines = [launch("aaa111"), launch("bbb222"), orphans.to_string()];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(tracked(&refs).is_empty());
    }

    /// A finished task whose id is echoed on a later line used to be *added
    /// back* by that echo — by the very line saying it had completed — and
    /// never removed again.
    #[test]
    fn an_echo_of_finished_work_does_not_revive_it() {
        let launched = r#"{"toolUseResult":{"status":"async_launched","agentId":"a817eb2d8"}}"#;
        let finished = r#"{"content":"<task-notification><task-id>a817eb2d8</task-id><status>completed</status></task-notification>"}"#;
        let echoed = r#"{"attachment":{"type":"task_status","taskId":"a817eb2d8","description":"Fix the sync","status":"completed"}}"#;
        assert!(tracked(&[launched, finished, echoed]).is_empty());
        // And the echo alone never counts as a launch.
        assert!(tracked(&[echoed]).is_empty());
    }

    /// A subagent called and waited for finishes before its result is written,
    /// and its result carries an `agentId` too. Counting that as a launch
    /// strands the card on work that was over before the line was written.
    #[test]
    fn a_subagent_that_already_finished_is_not_outstanding() {
        let sync = r#"{"toolUseResult":{"agentId":"ac8f6ac61","agentType":"Explore","status":"completed"}}"#;
        assert!(tracked(&[sync]).is_empty());
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
