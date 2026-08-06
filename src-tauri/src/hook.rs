//! The hook side: read one Claude Code hook payload from stdin, translate it
//! into a pet state plus a human-readable activity line, persist it.
//!
//! Invoked as `pipsqueak hook <EventName>` once per hook event, so it must be
//! cheap, silent, and incapable of failing loudly. Anything printed to stdout
//! would be parsed by Claude Code as hook output.

use crate::process;
use crate::project;
use crate::state::{now_ms, sanitize, sessions_dir, write_atomic, FileLock, Session};
use crate::text::{basename_or, first_line, summary_of, truncate};
use serde_json::Value;
use std::fs;
use std::io::Read;

/// Events that mean the session moved forward, and so cannot still be blocked
/// on a human or finished.
/// Subagent events are deliberately absent. They say something about work the
/// *turn* delegated, not about the turn itself, so a subagent finishing must
/// never clear an outcome or resolve a prompt — which is what used to relabel
/// a finished card "Delegating" with nothing running.
const PROGRESS_EVENTS: [&str; 7] = [
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionDenied",
    "PreCompact",
    "PostCompact",
];

pub fn run(fallback_event: Option<String>) {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    capture(&raw);
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
        // Hold the same lock every hook holds for its read-modify-write, so an
        // in-flight event cannot rename the file back into existence after
        // this delete. The lock file itself is left alone: removing it out
        // from under a live holder lets a third writer into the critical
        // section, and an orphaned lock expires on its own.
        let _lock = FileLock::acquire(&path);
        let _ = fs::remove_file(&path);
        return;
    }

    // Held across the read and the write. Claude Code runs the hooks matching
    // one event in parallel, and without this two of them read the same
    // "before" and whichever renames last discards the other's event.
    let _lock = FileLock::acquire(&path);

    let mut session: Session = match fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
        Some(session) => session,
        None => {
            // Only the events that genuinely begin work may create the file.
            // Hooks run detached and in parallel, so a straggler can arrive
            // after `SessionEnd` deleted the session; recreating it from
            // nothing resurrects a zombie card that sits there until the
            // sweep notices.
            if event != "SessionStart" && event != "UserPromptSubmit" {
                return;
            }
            Session::default()
        }
    };

    let Some(mut update) = classify(&event, &payload) else {
        return;
    };

    // A subagent reporting in after the turn ended is bookkeeping, not new
    // work. Taking it at face value cleared the outcome and relabelled a
    // finished card "Delegating", so the turn that had just been announced as
    // done went back to looking busy with nothing running.
    let trailing_subagent = is_trailing_subagent(&event, &session);
    if trailing_subagent {
        update.state = "";
        update.kind.clear();
        update.activity.clear();
        update.headline = None;
        update.silent = true;
    }

    let now = now_ms();
    if session.started_ms == 0 {
        session.started_ms = now;
    }
    if session.agent_pid == 0 {
        // Once per session: walking the process table is not free, and the
        // answer cannot change while the session lives.
        if let Some((pid, created)) = process::owner() {
            session.agent_pid = pid;
            session.agent_created = created;
        }
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
    // Where the overlay can read what this session is saying while it works.
    if let Some(transcript) = payload.get("transcript_path").and_then(Value::as_str) {
        if !transcript.is_empty() {
            session.transcript = transcript.to_string();
        }
    }
    if event == "PreToolUse" {
        session.tools += 1;
        session.turn_tools += 1;
    }
    if event == "UserPromptSubmit" {
        session.turn_started_ms = now;
        session.turn_tools = 0;
        session.blocked = 0;
        session.subagents = 0;
    }

    // `Stop` and `StopFailure` clear it too: the turn ending resolves whatever
    // it was blocked on. Without this, a denied permission followed by the
    // turn ending left "Needs you" on the card for hours — the outcome the
    // same event carries is applied just below, after the slate is clean.
    let resolves = PROGRESS_EVENTS.contains(&event.as_str())
        || event == "Stop"
        || event == "StopFailure";
    if resolves && !trailing_subagent {
        session.clear_pending();
    }
    // Anything other than the prompt itself means the prompt is over.
    //
    // Including a `PreToolUse` for the very tool being asked about, which took
    // a wrong turn to arrive at. Claude Code runs `PreToolUse` first, where it
    // may answer the permission itself, and only then raises
    // `PermissionRequest`, so a `PreToolUse` *after* a pending prompt means
    // the tool is running and the prompt was answered. Trying to protect the
    // pending state from it instead left the card saying "needs you" about a
    // command that had already been approved and run.
    //
    // It is also the rule that survives being wrong about that order: if
    // `PermissionRequest` ever came first, the `PreToolUse` would clear it and
    // the next `PermissionRequest` would simply set it again.
    if !session.pending_tool.is_empty() && event != "PermissionRequest" && event != "Notification" {
        session.clear_permission();
    }
    // Compare the detail too: two consecutive prompts for the same tool but
    // different commands must not leave the previous command's text, risk
    // label, and an already-elapsed debounce clock on the new prompt.
    if !update.permission.is_empty()
        && (session.pending_tool != update.permission
            || session.pending_detail != update.permission_detail)
    {
        session.pending_tool = update.permission.clone();
        session.pending_detail = update.permission_detail.clone();
        session.pending_risk = update.permission_risk.to_string();
        session.pending_since = now;
    }
    if !update.state.is_empty() {
        session.state = update.state.to_string();
    }
    if !update.outcome.is_empty() {
        session.outcome = update.outcome.to_string();
        session.outcome_ms = now;
        session.settles_ms = now + update.settle_ms;
        // The subagent count deliberately survives the outcome. Zeroing it
        // here destroyed the one piece of evidence that contradicts "Done":
        // the turn yielded the floor, but the work it delegated is still
        // running, and the card went green anyway. It drains on the
        // `SubagentStop` events that are already arriving.
    }
    if !update.waiting.is_empty() {
        // Keep the first timestamp. A second Notification about the same
        // prompt must not restart the debounce and hide the card again.
        if session.waiting_since == 0 {
            session.waiting_since = now;
        }
        session.waiting_reason = update.waiting.clone();
    }
    if update.blocked {
        session.blocked += 1;
    }
    session.subagents = session
        .subagents
        .saturating_add_signed(update.subagents)
        .min(64);

    // Any event at all is proof of life, so whatever the sweep concluded is
    // no longer true.
    session.stalled = false;
    session.session_id = session_id;
    if !update.silent {
        // An event that does not describe work leaves the last word about work
        // standing, rather than blanking it or replacing it with its own.
        if !update.kind.is_empty() {
            session.kind = update.kind;
        }
        session.activity = update.activity.clone();
        session.detail = update.detail;
        if let Some(headline) = update.headline {
            session.headline = headline;
        }
    }
    session.event = event;
    session.event_ms = now;
    session.updated_ms = now;
    if !update.silent && !update.activity.is_empty() {
        let tag = if !update.outcome.is_empty() {
            update.outcome
        } else if !update.waiting.is_empty() {
            "waiting"
        } else if update.blocked {
            "blocked"
        } else if update.hiccup {
            "failed"
        } else if update.state.is_empty() {
            "running"
        } else {
            update.state
        };
        session.push_recent(tag, &update.activity);
    }

    if let Ok(bytes) = serde_json::to_vec(&session) {
        let _ = write_atomic(&path, &bytes);
    }
}

/// Keeps a copy of a raw hook payload, when asked to.
///
/// Off unless `~/.pipsqueak/payloads` exists, which makes turning it on a
/// `mkdir` and turning it off a delete — no setting, no restart, and nothing
/// to leave switched on by accident. It exists because the hook payload is
/// the one contract here that is not ours: fields the pet leans on are absent
/// from the published schema, and "does this event actually carry that?" is
/// otherwise unanswerable without guessing.
///
/// Payloads contain prompts and assistant text, so this is deliberately
/// awkward to enable and never on by default.
fn capture(raw: &str) {
    let dir = crate::state::root().join("payloads");
    if !dir.is_dir() {
        return;
    }
    let event = serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| {
            v.get("hook_event_name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".into());
    let name = format!("{}-{}-{}.json", now_ms(), sanitize(&event), std::process::id());
    let _ = fs::write(dir.join(name), raw);
}

/// One state change.
///
/// Every field is optional. Most events answer only one of "what is it doing",
/// "how did the turn end", and "is a human blocking it", and an event that is
/// silent on a question must leave the existing answer alone.
/// Collapsing all three into a single `state` field is what let a card get
/// stuck on an alert that nothing ever cleared.
#[derive(Default)]
struct Update {
    /// New durable state. Empty leaves it unchanged.
    state: &'static str,
    /// The word on the status line. Kept coarse.
    kind: String,
    activity: String,
    detail: String,
    /// `None` for the many events that should not disturb what the card says
    /// the turn is about.
    headline: Option<String>,
    /// How the turn ended, and how long to treat that as provisional.
    outcome: &'static str,
    settle_ms: u64,
    /// Why a human is blocking. Empty means "not waiting".
    waiting: String,
    /// Change to the live subagent count.
    subagents: i64,
    /// A tool failed mid-turn. Not a failed turn, and not counted anywhere:
    /// it only decides how this line is coloured in the log.
    hiccup: bool,
    /// The auto-mode classifier refused a tool call. Counted on the card.
    blocked: bool,
    /// A permission prompt was raised for this tool. Empty means "no change".
    permission: String,
    permission_detail: String,
    permission_risk: &'static str,
    /// Record the event, but leave everything the card displays alone.
    silent: bool,
}

/// The command a Bash-shaped tool call would run, if it is one.
fn bash_command(tool: &str, input: Option<&Value>) -> Option<String> {
    if tool != "Bash" && tool != "PowerShell" {
        return None;
    }
    input
        .and_then(|v| v.get("command"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn live(state: &'static str, kind: &str, activity: String) -> Update {
    Update {
        state,
        kind: kind.to_string(),
        activity,
        ..Default::default()
    }
}

/// Returns `None` for events the pet ignores.
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
        // Auto-compaction fires `SessionStart` again mid-session with
        // `source: "compact"` (doc-verified). Treating it as a fresh session
        // flipped a working card to an idle "Session started" and stole the
        // headline for the rest of the turn.
        "SessionStart" if text("source") == "compact" => {
            live("thinking", "Thinking", "Context compacted".into())
        }
        "SessionStart" => Update {
            state: "idle",
            kind: "Idle".into(),
            headline: Some("Session started".into()),
            ..Default::default()
        },
        "UserPromptSubmit" => {
            let prompt = text("prompt");
            Update {
                state: "thinking",
                kind: "Thinking".into(),
                activity: "Thinking…".into(),
                detail: truncate(&prompt, 400),
                headline: Some(
                    summary_of(&prompt).unwrap_or_else(|| truncate(&first_line(&prompt), 90)),
                ),
                ..Default::default()
            }
        }
        // A Task call *is* a subagent launch, and is often the only signal one
        // happened, since Claude Code does not always send a matching SubagentStart.
        "PreToolUse" if tool == "Task" || tool == "Agent" => Update {
            state: "running",
            kind: "Delegating".into(),
            activity: phrase(&tool, input).present,
            subagents: 1,
            ..Default::default()
        },
        "PreToolUse" => live("running", &kind_of(&tool), phrase(&tool, input).present),
        // The matching PostToolUse is *not* a subagent stop: the
        // dedicated SubagentStop event owns that, and counting both would
        // decrement twice.
        "PostToolUse" => live("running", &kind_of(&tool), phrase(&tool, input).past),
        // One tool failing is not the turn failing. Claude nearly always
        // tries something else. Recorded as a mark on the turn, not a state.
        "PostToolUseFailure" => Update {
            state: "running",
            kind: "Recovering".into(),
            activity: format!("{} failed", pretty_tool(&tool)),
            detail: truncate(&first_line(&text("error")), 400),
            hiccup: true,
            ..Default::default()
        },
        // PermissionRequest fires *before* anyone is asked. Auto-mode and
        // permission hooks resolve most of them in milliseconds, so believing
        // it outright made the pet cry wolf constantly. Recording it costs
        // nothing: the frontend only surfaces a prompt that outlives the
        // debounce, by which point a human really is being asked.
        //
        // Note what this hook does *not* do. It writes a file and exits 0 with
        // empty stdout. It never prints a decision, so it cannot approve or
        // deny a tool call, and Claude Code's own prompt appears exactly as it
        // would if we were not installed at all.
        "PermissionRequest" => Update {
            // Nothing visible changes yet. The card keeps showing whatever the
            // session was doing until the prompt outlives the debounce.
            silent: true,
            permission: if tool.is_empty() {
                "a tool".into()
            } else {
                tool.clone()
            },
            permission_detail: phrase(&tool, input).infinitive,
            permission_risk: bash_command(&tool, input)
                .and_then(|command| crate::risk::irreversible(&command))
                .unwrap_or_default(),
            ..Default::default()
        },
        // Auto-mode declining a call is routine policy, not a failure — the
        // documented meaning of this event is "denied by the auto mode
        // classifier", so it is exactly the thing worth counting: it says the
        // permission rules, not Claude, are what this turn is fighting.
        "PermissionDenied" => Update {
            blocked: true,
            ..live(
                "running",
                &kind_of(&tool),
                format!("Auto-mode blocked: {}", phrase(&tool, input).infinitive),
            )
        },
        "Notification" => {
            let notification = text("notification_type");
            let message = truncate(&text("message"), 200);
            // `state` stays empty: being blocked does not change what the
            // session was doing, and overwriting it here is what used to leave
            // a card stuck on "Needs you" when the answer arrived elsewhere.
            // `kind` is deliberately left alone. The card says "Needs you" off
            // the *state*, which only becomes `waiting` once the prompt has
            // outlived the debounce; writing the word here as well put it on
            // cards that were still plainly running, which is the pet claiming
            // something that was not true yet.
            let waiting = |reason: &str| Update {
                activity: reason.to_string(),
                waiting: reason.to_string(),
                ..Default::default()
            };
            match notification.as_str() {
                "permission_prompt" => waiting(if message.is_empty() {
                    "Waiting for permission"
                } else {
                    &message
                }),
                "idle_prompt" => waiting("Waiting for your reply"),
                "agent_needs_input" => waiting("A teammate needs input"),
                // `notification_type` is not in the documented payload schema,
                // so it may simply be absent. Falling back to the message text
                // keeps "needs you" working instead of silently never firing.
                "" if !message.is_empty() => {
                    let lower = message.to_lowercase();
                    if lower.contains("permission") {
                        waiting(&message)
                    } else if lower.contains("waiting for your") || lower.contains("input") {
                        waiting("Waiting for your reply")
                    } else {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        "Stop" => {
            let message = text("last_assistant_message");
            let summary = summary_of(&message);
            match stop_disposition(payload, &message) {
                // Claude is not finished; it is about to carry on. Announcing
                // "Done" here is the single easiest way for the pet to lie,
                // and the 30s the card lingers makes the lie outlast the turn.
                Stop::KeepWorking(reason) => Update {
                    state: "running",
                    kind: "Working".into(),
                    activity: reason.to_string(),
                    ..Default::default()
                },
                Stop::Finished { settle_ms } => Update {
                    // The turn is over, so the session is no longer *doing*
                    // anything. Leaving this as "running" would have the sweep
                    // decide five minutes later that a finished session had
                    // stopped responding, and say so over the answer.
                    state: "idle",
                    kind: "Done".into(),
                    // The turn is over: a live line would only show a stale
                    // tool call.
                    detail: truncate(&message, 600),
                    // Nothing worth reading in the answer leaves the headline
                    // alone, and what is already there is what the turn was
                    // asked to do, which is still true.
                    headline: summary,
                    outcome: "done",
                    settle_ms,
                    ..Default::default()
                },
            }
        }
        "StopFailure" => {
            let error_kind = text("error_type");
            Update {
                state: "idle",
                kind: "Failed".into(),
                detail: truncate(&first_line(&text("error")), 400),
                headline: Some(if error_kind.is_empty() {
                    "Turn failed".into()
                } else {
                    format!("Turn failed: {}", error_kind.replace('_', " "))
                }),
                outcome: "failed",
                ..Default::default()
            }
        }
        "SubagentStart" => Update {
            state: "running",
            kind: "Delegating".into(),
            activity: format!("Subagent: {}", short(&text("agent_type"), "agent")),
            subagents: 1,
            ..Default::default()
        },
        "SubagentStop" => Update {
            state: "running",
            kind: "Delegating".into(),
            activity: "Subagent finished".into(),
            subagents: -1,
            ..Default::default()
        },
        "PreCompact" => live("compacting", "Compacting", "Compacting context".into()),
        // Compaction finishing is not the *turn* finishing. An automatic
        // compact happens mid-turn and work resumes straight after; only a
        // manual one leaves the session genuinely idle.
        "PostCompact" => {
            if text("trigger") == "manual" {
                live("idle", "Idle", "Context compacted".into())
            } else {
                live("thinking", "Thinking", "Context compacted".into())
            }
        }
        _ => return None,
    })
}

/// What a `Stop` event actually means.
enum Stop {
    /// Claude will keep going. There is nothing to celebrate yet.
    KeepWorking(&'static str),
    /// The turn is over. `settle_ms` holds the result provisional for a
    /// moment so a follow-up event can cancel it.
    Finished { settle_ms: u64 },
}

/// `Stop` does not reliably mean "turn finished".
///
/// Claude Code fires it whenever the assistant yields the floor, which
/// includes cases where it is about to immediately continue. Three payload
/// fields say so, and taking them at face value is the difference between a
/// pet that reports completions and a pet that guesses at them.
fn stop_disposition(payload: &Value, last_message: &str) -> Stop {
    let non_empty_list = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false)
    };

    if non_empty_list("session_crons") {
        return Stop::KeepWorking("Scheduled work still running");
    }
    // Set when Claude is continuing *because* a stop hook asked it to, so a
    // further Stop is already on its way.
    if payload
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Stop::KeepWorking("A stop hook is still working");
    }
    if non_empty_list("background_tasks") {
        return if last_message.trim().is_empty() {
            Stop::KeepWorking("Background tasks still running")
        } else {
            // It said its piece, but something is still finishing behind it.
            // Hold the result briefly rather than suppress it.
            Stop::Finished { settle_ms: 2_000 }
        };
    }
    // Even a clean-looking end stays provisional for a moment. A blocking
    // stop hook (a review gate, a completion loop) can veto this stop and
    // have the turn carry straight on — `stop_hook_active` only says so on
    // the *second* Stop, after the fact. Without the hold, every vetoed stop
    // flashed a Done card that the next tool event immediately wiped.
    Stop::Finished { settle_ms: 2_000 }
}

/// The coarse category behind the status line. Several different tools map to
/// one word: it should survive a whole stretch of work, so the only thing
/// moving is the counter next to it.
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
            let shown = if description.is_empty() {
                command
            } else {
                description
            };
            forms("Running:", "Ran:", "run:", &truncate(&shown, 64))
        }
        "Read" => forms(
            "Reading",
            "Read",
            "read",
            &basename_or(&field("file_path"), "a file"),
        ),
        "Edit" => forms(
            "Editing",
            "Edited",
            "edit",
            &basename_or(&field("file_path"), "a file"),
        ),
        "Write" => forms(
            "Writing",
            "Wrote",
            "write",
            &basename_or(&field("file_path"), "a file"),
        ),
        "NotebookEdit" => forms(
            "Editing",
            "Edited",
            "edit",
            &basename_or(&field("notebook_path"), "a notebook"),
        ),
        "Glob" => forms(
            "Finding",
            "Found",
            "find",
            &short(&field("pattern"), "files"),
        ),
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

/// A subagent event that arrives after the turn it belonged to has ended.
///
/// Claude Code sends `SubagentStop` for work that outlives the answer, so this
/// is routine rather than exceptional, and it must not restart the turn.
fn is_trailing_subagent(event: &str, session: &Session) -> bool {
    match event {
        "SubagentStart" => !session.outcome.is_empty(),
        // A `SubagentStop` with nothing outstanding is also bookkeeping when
        // the next prompt has already cleared the outcome: without the count
        // check, a straggler from the previous turn relabelled a fresh
        // "Thinking…" card as "Delegating" with nothing running.
        "SubagentStop" => !session.outcome.is_empty() || session.subagents == 0,
        _ => false,
    }
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
        assert_eq!(
            activity_of("PreToolUse", payload),
            "Running: Run unit tests"
        );
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
    /// on turns that were never interrupted, so it is recorded and the card
    /// only surfaces prompts that outlive the debounce.
    #[test]
    fn permission_requests_alone_do_not_mean_blocked() {
        let update = classify(
            "PermissionRequest",
            &json!({ "tool_name": "Bash", "tool_input": { "command": "rm -rf build" } }),
        )
        .unwrap();
        assert!(update.silent, "nothing on the card may change yet");
        assert!(update.waiting.is_empty());
        assert!(update.state.is_empty());
        assert_eq!(update.permission, "Bash");
        assert_eq!(update.permission_risk, "Deletes a directory tree");
    }

    #[test]
    fn an_ordinary_prompt_carries_no_warning() {
        let update = classify(
            "PermissionRequest",
            &json!({ "tool_name": "Bash", "tool_input": { "command": "npm test" } }),
        )
        .unwrap();
        assert_eq!(update.permission, "Bash");
        assert!(update.permission_risk.is_empty());
    }

    #[test]
    fn only_a_real_prompt_means_blocked() {
        let update = classify(
            "Notification",
            &json!({ "notification_type": "permission_prompt", "message": "Allow Bash?" }),
        )
        .unwrap();
        assert_eq!(update.waiting, "Allow Bash?");
        // Not even the word: "Needs you" is the card's name for the *state*,
        // and the state is only blocked once the prompt outlives the debounce.
        // Writing it here put it on cards whose dot was still plainly running.
        assert!(update.kind.is_empty());
        // Being blocked says nothing about what the session was doing, so the
        // durable state is left alone. Overwriting it here is what used to
        // strand a card on "Needs you" when the prompt was answered elsewhere.
        assert!(update.state.is_empty());
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
        assert_eq!(update.outcome, "done");
        // Provisional even when clean: a blocking stop hook can veto this
        // stop, and zero settle time made every vetoed stop flash a Done card.
        assert_eq!(update.settle_ms, 2_000);
        // The turn is over, so the session is not running any more. Leaving it
        // running would have the staleness sweep decide five minutes later
        // that a finished session had stopped responding, and say so over the
        // answer it had already given.
        assert_eq!(update.state, "idle");
        assert_eq!(update.headline.as_deref(), Some("Fixed the off-by-one."));
        // The turn is over, so the live line clears rather than showing a stale tool.
        assert!(update.activity.is_empty());
        assert!(update.detail.contains("Details follow."));
    }

    /// The three ways `Stop` arrives while Claude is about to carry on. Each
    /// one used to produce a "Done" card that then sat there for 30 seconds
    /// being wrong.
    #[test]
    fn stop_while_still_working_is_not_a_completion() {
        let cases = [
            json!({ "session_crons": [{ "id": "nightly" }] }),
            json!({ "stop_hook_active": true }),
            json!({ "background_tasks": [{ "id": "build" }] }),
        ];
        for payload in cases {
            let update = classify("Stop", &payload).unwrap();
            assert!(
                update.outcome.is_empty(),
                "{payload} should not report a completion"
            );
            assert_eq!(update.state, "running");
        }
    }

    /// Claude said its piece but something is still finishing behind it. That
    /// is a real completion, just one worth holding briefly in case a follow
    /// up event cancels it.
    #[test]
    fn stop_with_background_work_and_an_answer_settles_late() {
        let update = classify(
            "Stop",
            &json!({
                "background_tasks": [{ "id": "build" }],
                "last_assistant_message": "Kicked off the build."
            }),
        )
        .unwrap();
        assert_eq!(update.outcome, "done");
        assert_eq!(update.settle_ms, 2_000);
    }

    /// An automatic compaction happens *inside* a turn; work resumes straight
    /// after. Reporting it as a finished turn is a false completion.
    #[test]
    fn compaction_finishing_is_not_the_turn_finishing() {
        let auto = classify("PostCompact", &json!({ "trigger": "auto" })).unwrap();
        assert_eq!(auto.state, "thinking");
        assert!(auto.outcome.is_empty());

        let manual = classify("PostCompact", &json!({ "trigger": "manual" })).unwrap();
        assert_eq!(manual.state, "idle");
        assert!(manual.outcome.is_empty());
    }

    /// Claude Code reports many subagent launches only as a `Task` tool call,
    /// so counting `SubagentStart` alone undercounts them.
    #[test]
    fn a_task_call_counts_as_a_subagent() {
        let update = classify(
            "PreToolUse",
            &json!({ "tool_name": "Task", "tool_input": { "subagent_type": "explore" } }),
        )
        .unwrap();
        assert_eq!(update.subagents, 1);
        assert_eq!(update.kind, "Delegating");

        // The paired PostToolUse must not decrement: SubagentStop owns that,
        // and counting both would take the same subagent away twice.
        let done = classify("PostToolUse", &json!({ "tool_name": "Task" })).unwrap();
        assert_eq!(done.subagents, 0);
    }

    /// One tool failing is not the turn failing. Claude nearly always tries
    /// something else, and a red card for a failed grep is noise.
    #[test]
    fn a_failed_tool_marks_the_turn_without_failing_it() {
        let update = classify(
            "PostToolUseFailure",
            &json!({ "tool_name": "Grep", "error": "no matches" }),
        )
        .unwrap();
        assert!(update.hiccup);
        assert!(update.outcome.is_empty());
        assert_eq!(update.state, "running");
        // A tool that merely failed is not a tool that was refused. Counting
        // both under one number is what made the card's red chip meaningless.
        assert!(!update.blocked);
    }

    /// The only thing the red chip counts: the auto-mode classifier refusing a
    /// call. It is policy pushing back, which is worth a number, unlike a
    /// command that simply errored.
    #[test]
    fn an_auto_mode_refusal_is_counted_and_named() {
        let update = classify(
            "PermissionDenied",
            &json!({
                "tool_name": "Bash",
                "tool_input": { "command": "git push --force" }
            }),
        )
        .unwrap();
        assert!(update.blocked);
        assert!(update.activity.starts_with("Auto-mode blocked:"));
        // Still running: a refused call is not a failed turn, and the card
        // must not go red over one.
        assert_eq!(update.state, "running");
        assert!(update.outcome.is_empty());
    }

    #[test]
    fn tool_events_never_disturb_the_headline() {
        let payload = json!({
            "tool_name": "Read",
            "tool_input": { "file_path": "/code/clock.js" }
        });
        for event in [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionDenied",
        ] {
            let update = classify(event, &payload).unwrap();
            assert!(
                update.headline.is_none(),
                "{event} should leave the headline alone"
            );
        }
    }

    /// The opening line of a reply is often a greeting, and a card that says
    /// "Tristan," has spent its only line saying nothing.
    #[test]
    fn a_greeting_is_not_a_summary() {
        let update = classify(
            "Stop",
            &json!({ "last_assistant_message": "Tristan,\n\nFixed the timezone bug." }),
        )
        .unwrap();
        assert_eq!(update.headline.as_deref(), Some("Fixed the timezone bug."));
    }

    /// An answer with nothing quotable in it leaves the headline alone rather
    /// than replacing what the turn was about with "Finished".
    #[test]
    fn an_answer_with_nothing_to_say_keeps_the_question() {
        let update = classify("Stop", &json!({ "last_assistant_message": "Tristan," })).unwrap();
        assert_eq!(update.outcome, "done");
        assert!(update.headline.is_none());
    }

    /// A dragged-in file arrives as an absolute path, and the path is the
    /// least useful part of it.
    #[test]
    fn a_dropped_file_is_named_not_pathed() {
        let update = classify(
            "UserPromptSubmit",
            &json!({ "prompt": "@\"C:\\Users\\acer\\Downloads\\CLAUDE 1.md\"" }),
        )
        .unwrap();
        assert_eq!(update.headline.as_deref(), Some("CLAUDE 1.md"));

        let mentioned = classify(
            "UserPromptSubmit",
            &json!({ "prompt": "compare @src/derive.js with ./tools/pixel.mjs please" }),
        )
        .unwrap();
        assert_eq!(
            mentioned.headline.as_deref(),
            Some("compare derive.js with pixel.mjs please")
        );
    }

    /// The `SubagentStop` for work that outlived the answer used to clear the
    /// outcome, which turned a card that had just said "Done" back into
    /// "Delegating" with nothing running.
    #[test]
    fn a_late_subagent_does_not_unfinish_a_turn() {
        let with = |outcome: &str, subagents: u64| Session {
            outcome: outcome.to_string(),
            subagents,
            ..Session::default()
        };
        assert!(is_trailing_subagent("SubagentStop", &with("done", 1)));
        assert!(is_trailing_subagent("SubagentStart", &with("failed", 0)));
        assert!(!is_trailing_subagent("SubagentStop", &with("", 2)));
        assert!(!is_trailing_subagent("PreToolUse", &with("done", 0)));
        // A new prompt cleared the outcome, but nothing is outstanding: this
        // stop belongs to the previous turn and must stay bookkeeping.
        assert!(is_trailing_subagent("SubagentStop", &with("", 0)));
        // A genuine stop for a subagent this turn started is progress.
        assert!(!is_trailing_subagent("SubagentStop", &with("", 1)));
    }

    /// A blocking stop hook can veto this stop and have the turn carry on;
    /// the payload only admits that on the *second* Stop. The hold is what
    /// keeps the Done card from flashing and vanishing when that happens.
    #[test]
    fn even_a_clean_stop_is_held_provisional() {
        let update = classify("Stop", &json!({ "last_assistant_message": "Fixed." })).unwrap();
        assert_eq!(update.outcome, "done");
        assert!(
            update.settle_ms >= 1_000,
            "settle_ms was {}",
            update.settle_ms
        );
    }

    /// Auto-compaction fires `SessionStart` again mid-session with
    /// `source: "compact"`. That is a resumption, not a fresh session: the
    /// card must not reset to idle or lose its headline mid-turn.
    #[test]
    fn a_compaction_restart_is_not_a_new_session() {
        let update = classify("SessionStart", &json!({ "source": "compact" })).unwrap();
        assert_eq!(update.state, "thinking");
        assert!(update.headline.is_none());

        let fresh = classify("SessionStart", &json!({ "source": "startup" })).unwrap();
        assert_eq!(fresh.state, "idle");
        assert_eq!(fresh.headline.as_deref(), Some("Session started"));
    }

    /// `notification_type` is not in the documented payload schema. When it is
    /// missing, the message text still has to be able to say "needs you".
    #[test]
    fn a_notification_without_a_type_still_counts_as_waiting() {
        let payload = json!({ "message": "Claude needs your permission to use Bash" });
        let update = classify("Notification", &payload).unwrap();
        assert!(!update.waiting.is_empty());

        let idle = json!({ "message": "Claude is waiting for your input" });
        assert!(!classify("Notification", &idle).unwrap().waiting.is_empty());

        // A typeless notification that reads like neither stays ignored.
        assert!(classify("Notification", &json!({ "message": "Signed in." })).is_none());
    }

    #[test]
    fn the_prompt_becomes_the_headline() {
        let payload = json!({ "prompt": "Fix the flaky timezone test\n\nIt fails on CI only." });
        let update = classify("UserPromptSubmit", &payload).unwrap();
        assert_eq!(
            update.headline.as_deref(),
            Some("Fix the flaky timezone test")
        );
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
