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
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// How much of a transcript to read the first time one is seen. Enough to find
/// the last thing said in a busy turn, small enough to be one disk read.
const FIRST_READ: u64 = 256 * 1024;
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
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Watch>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Watch>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Attaches the newest line each session has said to the session itself.
pub fn decorate(sessions: &mut [Session], mode: Mode) {
    if mode == Mode::Off {
        return;
    }
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
        if !watch.line.is_empty() {
            session.narration = watch.line.clone();
        }
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
