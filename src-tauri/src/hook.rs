//! The hook side: read one Claude Code hook payload from stdin, translate it
//! into a pet state plus a human-readable activity line, persist it.
//!
//! Invoked as `pipsqueak hook <EventName>` once per hook event, so it must be
//! cheap, silent, and incapable of failing loudly — anything printed to stdout
//! would be parsed by Claude Code as hook output.

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

    let Some((state, activity, detail)) = classify(&event, &payload) else {
        return;
    };

    if session.started_ms == 0 {
        session.started_ms = now_ms();
    }
    if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
        if !cwd.is_empty() {
            session.cwd = cwd.to_string();
            session.project = basename(cwd);
        }
    }
    if event == "PreToolUse" {
        session.tools += 1;
    }
    session.session_id = session_id;
    session.state = state.to_string();
    session.activity = activity.clone();
    session.detail = detail;
    session.event = event;
    session.updated_ms = now_ms();
    session.push_recent(state, &activity);

    if let Ok(bytes) = serde_json::to_vec(&session) {
        let _ = write_atomic(&path, &bytes);
    }
}

/// Returns `None` for events the pet deliberately ignores.
fn classify(event: &str, payload: &Value) -> Option<(&'static str, String, String)> {
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
        "SessionStart" => ("idle", "Session started".into(), String::new()),
        "UserPromptSubmit" => (
            "thinking",
            "Thinking…".into(),
            truncate(&text("prompt"), 400),
        ),
        "PreToolUse" => ("running", describe_tool(&tool, input), String::new()),
        "PostToolUse" => (
            "running",
            format!("Done: {}", describe_tool(&tool, input)),
            String::new(),
        ),
        "PostToolUseFailure" => (
            "failed",
            format!("{} failed", pretty_tool(&tool)),
            truncate(&first_line(&text("error")), 400),
        ),
        "PermissionRequest" => (
            "waiting",
            format!("Needs permission: {}", describe_tool(&tool, input)),
            String::new(),
        ),
        "PermissionDenied" => (
            "failed",
            format!("Denied: {}", pretty_tool(&tool)),
            String::new(),
        ),
        "Notification" => {
            let kind = text("notification_type");
            let message = truncate(&text("message"), 200);
            match kind.as_str() {
                "permission_prompt" => (
                    "waiting",
                    if message.is_empty() {
                        "Needs your permission".into()
                    } else {
                        message
                    },
                    String::new(),
                ),
                "idle_prompt" => ("waiting", "Waiting for you".into(), String::new()),
                _ => return None,
            }
        }
        "Stop" => {
            let message = text("last_assistant_message");
            let headline = first_line(&message);
            (
                "done",
                if headline.is_empty() {
                    "Finished".into()
                } else {
                    truncate(&headline, 120)
                },
                truncate(&message, 600),
            )
        }
        "StopFailure" => {
            let kind = text("error_type");
            (
                "failed",
                if kind.is_empty() {
                    "Turn failed".into()
                } else {
                    format!("Turn failed: {}", kind.replace('_', " "))
                },
                truncate(&first_line(&text("error")), 400),
            )
        }
        "SubagentStart" => (
            "running",
            format!("Subagent: {}", short(&text("agent_type"), "agent")),
            String::new(),
        ),
        "SubagentStop" => ("running", "Subagent finished".into(), String::new()),
        "PreCompact" => ("compacting", "Compacting context".into(), String::new()),
        "PostCompact" => ("thinking", "Context compacted".into(), String::new()),
        _ => return None,
    })
}

fn describe_tool(tool: &str, input: Option<&Value>) -> String {
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
        let name = parts.next().unwrap_or("call");
        return format!("{}: {}", server, name.replace('_', " "));
    }

    match tool {
        "Bash" | "PowerShell" => {
            let description = field("description");
            let command = field("command");
            let shown = if description.is_empty() { command } else { description };
            format!("Running: {}", truncate(&shown, 64))
        }
        "Read" => format!("Reading {}", basename_or(&field("file_path"), "a file")),
        "Edit" => format!("Editing {}", basename_or(&field("file_path"), "a file")),
        "Write" => format!("Writing {}", basename_or(&field("file_path"), "a file")),
        "NotebookEdit" => format!("Editing {}", basename_or(&field("notebook_path"), "a notebook")),
        "Glob" => format!("Finding {}", short(&field("pattern"), "files")),
        "Grep" => format!("Searching for {}", truncate(&field("pattern"), 40)),
        "WebFetch" => format!("Fetching {}", host_of(&field("url"))),
        "WebSearch" => format!("Searching the web for {}", truncate(&field("query"), 40)),
        "Task" | "Agent" => format!(
            "Delegating to {}",
            short(&field("subagent_type"), "a subagent")
        ),
        "Skill" => format!("Running skill {}", short(&field("skill"), "")),
        "TodoWrite" | "TaskCreate" | "TaskUpdate" => "Updating tasks".to_string(),
        "" => "Working".to_string(),
        other => pretty_tool(other),
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
        classify(event, &payload).expect("event should be classified").1
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
    fn mcp_tools_are_split_into_server_and_call() {
        let payload = json!({ "tool_name": "mcp__github__create_issue" });
        assert_eq!(activity_of("PreToolUse", payload), "github: create issue");
    }

    #[test]
    fn permission_requests_ask_for_attention() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf build" }
        });
        let (state, activity, _) = classify("PermissionRequest", &payload).unwrap();
        assert_eq!(state, "waiting");
        assert!(activity.starts_with("Needs permission: Running: rm -rf build"));
    }

    #[test]
    fn stop_reports_the_first_line_of_the_answer() {
        let payload = json!({
            "last_assistant_message": "\n\nFixed the off-by-one.\nDetails follow."
        });
        let (state, activity, detail) = classify("Stop", &payload).unwrap();
        assert_eq!(state, "done");
        assert_eq!(activity, "Fixed the off-by-one.");
        assert!(detail.contains("Details follow."));
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
