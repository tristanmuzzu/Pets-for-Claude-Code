//! The hook side: read one Claude Code hook payload from stdin, translate it
//! into a pet state plus a human-readable activity line, persist it.
//!
//! Invoked as `pipsqueak hook <EventName>` once per hook event, so it must be
//! cheap, silent, and incapable of failing loudly — anything printed to stdout
//! would be parsed by Claude Code as hook output.

use crate::project;
use crate::state::{
    now_ms, prune_stale, sanitize, sessions_dir, write_atomic, Session,
};
use serde_json::Value;
use std::fs;
use std::io::Read;

pub fn run(fallback_event: Option<String>) {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let payload: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);

    let event = payload
        .get("hook_event_name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(fallback_event)
        .unwrap_or_default();
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let path = sessions_dir().join(format!("{}.json", sanitize(&session_id)));

    if event == "SessionEnd" {
        let _ = fs::remove_file(&path);
        prune_stale();
        return;
    }

    let mut session: Session = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    let Some(update) = classify(&event, &payload) else {
        return;
    };

    if session.started_ms == 0 {
        session.started_ms = now_ms();
    }
    if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
        // Resolving walks the filesystem, so only redo it when the session
        // actually moves.
        if !cwd.is_empty() && cwd != session.cwd {
            let resolved = project::resolve(cwd);
            session.cwd = cwd.to_string();
            session.project = resolved.name;
            session.workspace = resolved.workspace;
            session.project_root = resolved.root.to_string_lossy().to_string();
            session.scratch = resolved.scratch;
        }
    }
    if event == "PreToolUse" {
        session.tools += 1;
        session.turn_tools += 1;
    }
    if event == "UserPromptSubmit" {
        session.turn_started_ms = now_ms();
        session.turn_tools = 0;
    }
    session.session_id = session_id;
    session.state = update.state.to_string();
    session.kind = update.kind;
    session.activity = update.activity.clone();
    session.detail = update.detail;
    if let Some(headline) = update.headline {
        session.headline = headline;
    }
    session.event = event;
    session.updated_ms = now_ms();
    if !update.activity.is_empty() {
        session.push_recent(update.state, &update.activity);
    }

    if let Ok(bytes) = serde_json::to_vec(&session) {
        let _ = write_atomic(&path, &bytes);
    }
}

/// One state change. `headline` is `None` for the many events that should not
/// disturb what the card says the turn is about.
struct Update {
    state: &'static str,
    /// The word on the status line. Deliberately coarse.
    kind: String,
    activity: String,
    detail: String,
    headline: Option<String>,
}

fn live(state: &'static str, kind: &str, activity: String) -> Update {
    Update {
        state,
        kind: kind.to_string(),
        activity,
        detail: String::new(),
        headline: None,
    }
}

/// Returns `None` for events the pet deliberately ignores.
fn classify(event: &str, payload: &Value) -> Option<Update> {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let tool = text("tool_name");
    let input = payload.get("tool_input");

    Some(match event {
        "SessionStart" => Update {
            state: "idle",
            kind: "Idle".into(),
            activity: String::new(),
            detail: String::new(),
            headline: Some("Session started".into()),
        },
        "UserPromptSubmit" => {
            let prompt = text("prompt");
            Update {
                state: "thinking",
                kind: "Thinking".into(),
                activity: "Thinking…".into(),
                detail: truncate(&prompt, 400),
                headline: Some(truncate(&first_line(&prompt), 90)),
            }
        }
        "PreToolUse" => live("running", &kind_of(&tool), phrase(&tool, input).present),
        "PostToolUse" => live("running", &kind_of(&tool), phrase(&tool, input).past),
        "PostToolUseFailure" => Update {
            state: "failed",
            kind: "Failed".into(),
            activity: format!("{} failed", pretty_tool(&tool)),
            detail: truncate(&first_line(&text("error")), 400),
            headline: None,
        },
        // PermissionRequest fires *before* anyone is asked — auto-mode and
        // permission hooks resolve most of them in milliseconds. Treating it as
        // "blocked on you" made the pet cry wolf constantly. The event that
        // means a human was actually asked is Notification/permission_prompt.
        "PermissionRequest" => return None,
        // Auto-mode declining a call is routine policy, not a failure.
        "PermissionDenied" => live(
            "running",
            &kind_of(&tool),
            format!("Auto-mode declined: {}", phrase(&tool, input).infinitive),
        ),
        "Notification" => {
            let notification = text("notification_type");
            let message = truncate(&text("message"), 200);
            match notification.as_str() {
                "permission_prompt" => live(
                    "waiting",
                    "Needs you",
                    if message.is_empty() {
                        "Waiting for permission".into()
                    } else {
                        message
                    },
                ),
                "idle_prompt" => live("waiting", "Needs you", "Waiting for your reply".into()),
                "agent_needs_input" => {
                    live("waiting", "Needs you", "A teammate needs input".into())
                }
                _ => return None,
            }
        }
        "Stop" => {
            let message = text("last_assistant_message");
            let summary = first_line(&message);
            Update {
                state: "done",
                kind: "Done".into(),
                // The turn is over: the live line would only show a stale tool.
                activity: String::new(),
                detail: truncate(&message, 600),
                headline: Some(if summary.is_empty() {
                    "Finished".into()
                } else {
                    truncate(&summary, 140)
                }),
            }
        }
        "StopFailure" => {
            let error_kind = text("error_type");
            Update {
                state: "failed",
                kind: "Failed".into(),
                activity: String::new(),
                detail: truncate(&first_line(&text("error")), 400),
                headline: Some(if error_kind.is_empty() {
                    "Turn failed".into()
                } else {
                    format!("Turn failed: {}", error_kind.replace('_', " "))
                }),
            }
        }
        "SubagentStart" => live(
            "running",
            "Delegating",
            format!("Subagent: {}", short(&text("agent_type"), "agent")),
        ),
        "SubagentStop" => live("running", "Delegating", "Subagent finished".into()),
        "PreCompact" => live("compacting", "Compacting", "Compacting context".into()),
        "PostCompact" => live("thinking", "Thinking", "Context compacted".into()),
        _ => return None,
    })
}

/// The coarse category behind the status line. Several different tools map to
/// one word on purpose: the word should survive a whole stretch of work so the
/// only thing moving is the counter next to it.
fn kind_of(tool: &str) -> String {
    if let Some(rest) = tool.strip_prefix("mcp__") {
        let server = rest.split("__").next().unwrap_or("MCP");
        return format!("Calling {server}");
    }
    match tool {
        "Read" | "Glob" | "Grep" | "NotebookRead" => "Reading",
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => "Editing",
        "Bash" | "PowerShell" => "Running",
        "WebFetch" | "WebSearch" => "Browsing",
        "Task" | "Agent" => "Delegating",
        "Skill" => "Running a skill",
        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet" => "Planning",
        "" => "Working",
        _ => "Working",
    }
    .to_string()
}

/// One tool call, in the three tenses the bubble needs: "Editing clock.js",
/// "Edited clock.js", "…permission to edit clock.js".
struct Phrase {
    present: String,
    past: String,
    infinitive: String,
}

fn forms(ing: &str, ed: &str, base: &str, object: &str) -> Phrase {
    let object = object.trim();
    let join = |verb: &str| {
        if object.is_empty() {
            verb.trim_end_matches(':').to_string()
        } else {
            format!("{verb} {object}")
        }
    };
    Phrase {
        present: join(ing),
        past: join(ed),
        infinitive: join(base),
    }
}

fn phrase(tool: &str, input: Option<&Value>) -> Phrase {
    let field = |key: &str| {
        input
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    if let Some(rest) = tool.strip_prefix("mcp__") {
        let mut parts = rest.splitn(2, "__");
        let server = parts.next().unwrap_or("mcp");
        let name = parts.next().unwrap_or("call").replace('_', " ");
        return forms("Calling", "Called", "call", &format!("{server}: {name}"));
    }

    match tool {
        "Bash" | "PowerShell" => {
            let description = field("description");
            let command = field("command");
            let shown = if description.is_empty() { command } else { description };
            forms("Running:", "Ran:", "run:", &truncate(&shown, 64))
        }
        "Read" => forms("Reading", "Read", "read", &basename_or(&field("file_path"), "a file")),
        "Edit" => forms("Editing", "Edited", "edit", &basename_or(&field("file_path"), "a file")),
        "Write" => forms("Writing", "Wrote", "write", &basename_or(&field("file_path"), "a file")),
        "NotebookEdit" => forms(
            "Editing",
            "Edited",
            "edit",
            &basename_or(&field("notebook_path"), "a notebook"),
        ),
        "Glob" => forms("Finding", "Found", "find", &short(&field("pattern"), "files")),
        "Grep" => forms(
            "Searching for",
            "Searched for",
            "search for",
            &truncate(&field("pattern"), 40),
        ),
        "WebFetch" => forms("Fetching", "Fetched", "fetch", &host_of(&field("url"))),
        "WebSearch" => forms(
            "Searching the web for",
            "Searched the web for",
            "search the web for",
            &truncate(&field("query"), 40),
        ),
        "Task" | "Agent" => forms(
            "Delegating to",
            "Heard back from",
            "delegate to",
            &short(&field("subagent_type"), "a subagent"),
        ),
        "Skill" => forms("Running skill", "Ran skill", "run skill", &field("skill")),
        "TodoWrite" | "TaskCreate" | "TaskUpdate" => {
            forms("Updating", "Updated", "update", "tasks")
        }
        "" => forms("Working", "Worked", "work", ""),
        other => Phrase {
            present: other.to_string(),
            past: format!("{other} finished"),
            infinitive: format!("use {other}"),
        },
    }
}

fn pretty_tool(tool: &str) -> String {
    if tool.is_empty() {
        "A tool".to_string()
    } else {
        tool.to_string()
    }
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn basename_or(path: &str, fallback: &str) -> String {
    if path.is_empty() {
        fallback.to_string()
    } else {
        basename(path)
    }
}

fn host_of(url: &str) -> String {
    let without_scheme = url.split("//").last().unwrap_or(url);
    let host = without_scheme.split('/').next().unwrap_or(without_scheme);
    if host.is_empty() {
        "a page".to_string()
    } else {
        host.to_string()
    }
}

fn short(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        truncate(value, 40)
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn activity_of(event: &str, payload: serde_json::Value) -> String {
        classify(event, &payload)
            .expect("event should be classified")
            .activity
    }

    #[test]
    fn bash_prefers_the_description_over_the_raw_command() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "npm test -- --run", "description": "Run unit tests" }
        });
        assert_eq!(activity_of("PreToolUse", payload), "Running: Run unit tests");
    }

    #[test]
    fn file_tools_show_only_the_basename() {
        let payload = json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": "C:\\code\\app\\src\\render.js" }
        });
        assert_eq!(activity_of("PreToolUse", payload), "Editing render.js");
    }

    #[test]
    fn finished_tools_are_reported_in_the_past_tense() {
        let payload = json!({
            "tool_name": "Edit",
            "tool_input": { "file_path": "/code/render.js" }
        });
        assert_eq!(activity_of("PostToolUse", payload), "Edited render.js");
    }

    #[test]
    fn mcp_tools_are_split_into_server_and_call() {
        let payload = json!({ "tool_name": "mcp__github__create_issue" });
        assert_eq!(
            activity_of("PreToolUse", payload),
            "Calling github: create issue"
        );
    }

    /// PermissionRequest fires before anyone is asked, and auto-mode resolves
    /// most of them instantly. Reacting to it made the pet claim to be blocked
    /// on turns that were never interrupted.
    #[test]
    fn permission_requests_alone_do_not_mean_blocked() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf build" }
        });
        assert!(classify("PermissionRequest", &payload).is_none());
    }

    #[test]
    fn only_a_real_prompt_means_blocked() {
        let update = classify(
            "Notification",
            &json!({ "notification_type": "permission_prompt", "message": "Allow Bash?" }),
        )
        .unwrap();
        assert_eq!(update.state, "waiting");
        assert_eq!(update.kind, "Needs you");
    }

    #[test]
    fn auto_mode_declining_a_call_is_not_a_failure() {
        let update = classify(
            "PermissionDenied",
            &json!({ "tool_name": "Bash", "tool_input": { "command": "curl example.com" } }),
        )
        .unwrap();
        assert_eq!(update.state, "running");
        assert!(update.activity.starts_with("Auto-mode declined"));
    }

    #[test]
    fn related_tools_share_one_status_word() {
        for tool in ["Read", "Glob", "Grep"] {
            assert_eq!(kind_of(tool), "Reading");
        }
        for tool in ["Edit", "Write", "NotebookEdit"] {
            assert_eq!(kind_of(tool), "Editing");
        }
        assert_eq!(kind_of("mcp__github__create_issue"), "Calling github");
    }

    #[test]
    fn stop_reports_the_first_line_of_the_answer_as_the_headline() {
        let payload = json!({
            "last_assistant_message": "\n\nFixed the off-by-one.\nDetails follow."
        });
        let update = classify("Stop", &payload).unwrap();
        assert_eq!(update.state, "done");
        assert_eq!(update.headline.as_deref(), Some("Fixed the off-by-one."));
        // The turn is over, so the live line clears rather than showing a stale tool.
        assert!(update.activity.is_empty());
        assert!(update.detail.contains("Details follow."));
    }

    #[test]
    fn tool_events_never_disturb_the_headline() {
        let payload = json!({
            "tool_name": "Read",
            "tool_input": { "file_path": "/code/clock.js" }
        });
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure", "PermissionDenied"] {
            let update = classify(event, &payload).unwrap();
            assert!(
                update.headline.is_none(),
                "{event} should leave the headline alone"
            );
        }
    }

    #[test]
    fn the_prompt_becomes_the_headline() {
        let payload = json!({ "prompt": "Fix the flaky timezone test\n\nIt fails on CI only." });
        let update = classify("UserPromptSubmit", &payload).unwrap();
        assert_eq!(update.headline.as_deref(), Some("Fix the flaky timezone test"));
        assert_eq!(update.activity, "Thinking…");
    }

    #[test]
    fn uninteresting_notifications_are_ignored() {
        let payload = json!({ "notification_type": "auth_success" });
        assert!(classify("Notification", &payload).is_none());
        assert!(classify("MessageDisplay", &json!({})).is_none());
    }

    #[test]
    fn truncation_never_splits_a_character() {
        let truncated = truncate("ααααα", 3);
        assert_eq!(truncated.chars().count(), 3);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn session_ids_cannot_escape_the_sessions_directory() {
        assert_eq!(crate::state::sanitize("../../etc/passwd"), "etcpasswd");
        assert_eq!(crate::state::sanitize(""), "unknown");
    }
}
