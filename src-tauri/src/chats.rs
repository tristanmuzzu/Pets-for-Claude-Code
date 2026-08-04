//! Where a session lives inside the Claude Code desktop app.
//!
//! The app keeps one JSON record per chat under its own data directory, and
//! that record holds the two things nothing else has: the chat's title, and the
//! id the app routes by. Both are keyed by `cliSessionId`, which is the same id
//! the hooks already write, so this is a join rather than a guess.
//!
//! Read-only, and best-effort by design. A different app version, a different
//! layout, or no desktop app at all costs a title and a deep link, never a
//! card.

use crate::state::Session;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The app writes `title` near the top of the record and megabytes of MCP tool
/// schemas after it, so the head is all this needs.
const HEAD_BYTES: usize = 4096;
/// Rescanning on every poll would stat several hundred files three times a
/// second for an answer that changes when a chat is created.
const RESCAN_AFTER: Duration = Duration::from_secs(3);
/// A record untouched for this long cannot belong to a session the pet is
/// showing, and skipping it keeps the scan proportional to real work.
const RECENT_MS: u64 = 14 * 24 * 60 * 60 * 1000;
/// The app's own prefix on a local chat id: the file is `local_<uuid>.json`.
const LOCAL_PREFIX: &str = "local_";

#[derive(Clone, Debug, Default)]
pub struct Chat {
    /// The `<uuid>` half of `local_<uuid>`, which is what the app's `resume`
    /// deep link expects.
    pub id: String,
    /// The app's own name for the chat. Empty if it has not titled it yet.
    pub title: String,
}

/// `%APPDATA%/Claude/claude-code-sessions`, or the platform equivalent.
fn store_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = Some(
        crate::state::home_dir()
            .join("Library")
            .join("Application Support"),
    );
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(crate::state::home_dir().join(".config")));

    base.map(|base| base.join("Claude").join("claude-code-sessions"))
}

struct Cached {
    /// Keyed by path, so an unchanged record is never read twice.
    files: HashMap<PathBuf, (u64, String, Chat)>,
    scanned: Option<Instant>,
}

fn cache() -> &'static Mutex<Cached> {
    static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cached {
            files: HashMap::new(),
            scanned: None,
        })
    })
}

/// The chat a Claude Code session belongs to, if the desktop app knows it.
pub fn lookup(session_id: &str) -> Option<Chat> {
    if session_id.is_empty() {
        return None;
    }
    let guard = refreshed()?;
    guard
        .files
        .values()
        .find(|(_, cli, _)| cli == session_id)
        .map(|(_, _, chat)| chat.clone())
}

/// Fills in the chat title and id on everything the overlay is about to show.
///
/// One scan for the whole list: looking each session up on its own would walk
/// the same directory once per card.
pub fn decorate(sessions: &mut [Session]) {
    let Some(guard) = refreshed() else {
        return;
    };
    let mut by_session: HashMap<&str, &Chat> = HashMap::new();
    for (_, cli, chat) in guard.files.values() {
        by_session.insert(cli.as_str(), chat);
    }
    for session in sessions.iter_mut() {
        if let Some(chat) = by_session.get(session.session_id.as_str()) {
            session.chat_id = chat.id.clone();
            session.chat_title = chat.title.clone();
        }
    }
}

/// Asks the desktop app to show a chat.
///
/// `claude://resume` with an id the app already has is a navigation: it finds
/// the existing chat, opens it, and raises the window. The same link with an id
/// the app has *not* seen would import a CLI transcript instead, which is why
/// only ids read back out of the app's own records are ever passed here.
pub fn open(chat_id: &str) -> bool {
    if !is_plain_id(chat_id) {
        return false;
    }
    crate::desktop::open_url(&format!("claude://resume?session={chat_id}"))
}

/// Ids come off disk and end up in a URL handed to the shell, so anything that
/// is not the shape the app writes is refused rather than escaped.
fn is_plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn refreshed() -> Option<std::sync::MutexGuard<'static, Cached>> {
    let mut guard = cache().lock().ok()?;
    let fresh = guard
        .scanned
        .map(|at| at.elapsed() < RESCAN_AFTER)
        .unwrap_or(false);
    if !fresh {
        scan(&mut guard);
        guard.scanned = Some(Instant::now());
    }
    Some(guard)
}

/// Reads every `local_*.json` under the store, skipping anything unchanged.
fn scan(cache: &mut Cached) {
    let Some(root) = store_dir() else {
        return;
    };
    let cutoff = crate::state::now_ms().saturating_sub(RECENT_MS);
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    // Today the records sit at `<store>/<account>/<org>/`. That is the app's
    // business, not ours, so the depth is a bound rather than an expectation.
    visit(&root, 3, &mut |path, modified| {
        if modified < cutoff {
            return;
        }
        let Some(id) = local_id(path) else {
            return;
        };
        seen.insert(path.to_path_buf());
        if let Some((stamp, _, _)) = cache.files.get(path) {
            if *stamp == modified {
                return;
            }
        }
        let Some(head) = read_head(path) else {
            return;
        };
        let Some(cli) = field(&head, "cliSessionId") else {
            return;
        };
        let title = field(&head, "title").unwrap_or_default();
        cache
            .files
            .insert(path.to_path_buf(), (modified, cli, Chat { id, title }));
    });

    // An archived or deleted chat must stop answering for its session.
    cache.files.retain(|path, _| seen.contains(path));
}

fn visit(dir: &Path, depth: usize, each: &mut impl FnMut(&Path, u64)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            if depth > 0 {
                visit(&path, depth - 1, each);
            }
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        each(&path, modified);
    }
}

fn local_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let id = name.strip_prefix(LOCAL_PREFIX)?.strip_suffix(".json")?;
    is_plain_id(id).then(|| id.to_string())
}

fn read_head(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut buffer = vec![0u8; HEAD_BYTES];
    let read = file.read(&mut buffer).ok()?;
    buffer.truncate(read);
    // The head can land mid-character, which is not a reason to lose the file.
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// Pulls one string field out of a JSON fragment.
///
/// A fragment, not a document: this reads the first few kilobytes of a file
/// whose tail is enormous, so a real parser has nothing to parse.
fn field(head: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after = &head[head.find(&needle)? + needle.len()..];
    let after = after.trim_start().strip_prefix(':')?;
    let start = after.find('"')?;
    // Only a string counts. Anything else means the value is a number, an
    // object, or the key was part of a longer name.
    if after[..start].chars().any(|c| !c.is_whitespace()) {
        return None;
    }

    let mut out = String::new();
    let mut chars = after[start + 1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' | 't' | 'r' => out.push(' '),
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                    out.push(
                        u32::from_str_radix(&hex, 16)
                            .ok()
                            .and_then(char::from_u32)?,
                    );
                }
                other => out.push(other),
            },
            _ => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_fields_the_app_writes_first() {
        let head = r#"{"sessionId": "local_abc", "cliSessionId": "5bb53f69-9c6c", "cwd": "C:\\code", "title": "Pet UI animations", "titleSource": "auto""#;
        assert_eq!(
            field(head, "cliSessionId").as_deref(),
            Some("5bb53f69-9c6c")
        );
        assert_eq!(field(head, "title").as_deref(), Some("Pet UI animations"));
    }

    #[test]
    fn a_truncated_record_gives_up_rather_than_guessing() {
        let head = r#"{"cliSessionId": "5bb53f69", "title": "half a titl"#;
        assert_eq!(field(head, "cliSessionId").as_deref(), Some("5bb53f69"));
        assert_eq!(field(head, "title"), None);
    }

    #[test]
    fn non_string_values_are_not_titles() {
        let head = r#"{"title": 12, "cliSessionId": "abc"}"#;
        assert_eq!(field(head, "title"), None);
    }

    #[test]
    fn escapes_survive_the_trip() {
        let head = r#"{"title": "a \"quoted\" \u2014 name"}"#;
        assert_eq!(field(head, "title").as_deref(), Some("a \"quoted\" — name"));
    }

    /// The id is spliced into a URL that the shell executes, so the only ids
    /// that are ever used are the ones that look exactly like the app's own.
    #[test]
    fn only_plain_ids_may_reach_the_shell() {
        assert!(is_plain_id("13f92b55-b7ea-43d0-8b03-810355a1c358"));
        assert!(!is_plain_id("a b"));
        assert!(!is_plain_id("x&start=calc.exe"));
        assert!(!is_plain_id(""));
    }

    #[test]
    fn a_chat_id_comes_from_the_file_name() {
        assert_eq!(
            local_id(Path::new("/store/local_13f92b55-b7ea.json")).as_deref(),
            Some("13f92b55-b7ea")
        );
        assert_eq!(local_id(Path::new("/store/other.json")), None);
    }
}
