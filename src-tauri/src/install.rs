//! Registers (and removes) the Claude Code hooks that feed the pet.
//!
//! `~/.claude/settings.json` is usually hand-tuned and load-bearing, so this
//! module is deliberately conservative: it backs the file up first, only ever
//! touches entries it can prove are its own, and refuses to run at all if the
//! file is not valid JSON.

use crate::state::{claude_settings_path, now_ms, write_atomic};
use serde_json::{json, Map, Value};
use std::fs;

/// Every event the pet reacts to. Ordering is cosmetic.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];

const MARKER: &str = "pipsqueak";

fn exe_path() -> Result<String, String> {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| format!("cannot resolve own path: {e}"))
}

fn read_settings() -> Result<(Value, bool, String), String> {
    let path = claude_settings_path();
    if !path.exists() {
        return Ok((json!({}), false, String::new()));
    }
    let raw =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok((json!({}), true, raw));
    }
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "{} is not valid JSON ({e}) — refusing to touch it",
            path.display()
        )
    })?;
    if !parsed.is_object() {
        return Err(format!("{} is not a JSON object", path.display()));
    }
    Ok((parsed, true, raw))
}

fn backup(raw: &str) -> Result<String, String> {
    let path = claude_settings_path();
    let dest = path.with_file_name(format!("settings.json.pipsqueak-backup-{}", now_ms()));
    fs::write(&dest, raw).map_err(|e| format!("cannot write backup: {e}"))?;
    Ok(dest.to_string_lossy().to_string())
}

fn is_ours(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .map(|c| c.to_ascii_lowercase().contains(MARKER))
        .unwrap_or(false)
}

/// Events we expect to own but no longer do.
///
/// A partial answer is the interesting one: it means something else rewrote
/// `settings.json` and kept only some of our entries, which from the outside
/// looks exactly like working software that has quietly gone half-deaf.
pub fn missing_events() -> Vec<&'static str> {
    let Ok((root, _, _)) = read_settings() else {
        return EVENTS.to_vec();
    };
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return EVENTS.to_vec();
    };
    EVENTS
        .iter()
        .filter(|event| !event_is_ours(hooks, event))
        .copied()
        .collect()
}

fn event_is_ours(hooks: &Map<String, Value>, event: &str) -> bool {
    hooks
        .get(event)
        .and_then(Value::as_array)
        .map(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .map(|entries| entries.iter().any(is_ours))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// The program path the registered hooks actually run, if any.
///
/// Worth checking separately from "are they registered": an entry left over
/// from an install that has since been moved or uninstalled is present, valid
/// looking, and completely inert.
pub fn registered_command() -> Option<String> {
    let (root, _, _) = read_settings().ok()?;
    let hooks = root.get("hooks")?.as_object()?;
    hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .find(|entry| is_ours(entry))
        .and_then(|entry| entry.get("command").and_then(Value::as_str))
        .map(str::to_string)
}

/// Remove only our own command hooks, leaving every other hook untouched.
fn strip_ours(groups: &mut Vec<Value>) -> usize {
    let mut removed = 0;
    for group in groups.iter_mut() {
        let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        let before = list.len();
        list.retain(|entry| !is_ours(entry));
        removed += before - list.len();
    }
    groups.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .map(|list| !list.is_empty())
            .unwrap_or(true)
    });
    removed
}

fn our_group(exe: &str, event: &str) -> Value {
    json!({
        "matcher": "*",
        "hooks": [{
            "type": "command",
            "command": exe,
            "args": ["hook", event],
            "timeout": 5,
            "async": true
        }]
    })
}

fn write_settings(root: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(root).map_err(|e| e.to_string())?;
    write_atomic(&claude_settings_path(), &bytes).map_err(|e| format!("cannot write settings: {e}"))
}

fn hooks_map(root: &mut Value) -> Result<&mut Map<String, Value>, String> {
    let object = root
        .as_object_mut()
        .ok_or("settings root is not an object")?;
    let hooks = object.entry("hooks").or_insert_with(|| json!({}));
    hooks
        .as_object_mut()
        .ok_or_else(|| "settings.hooks is not an object".to_string())
}

pub fn install() -> Result<String, String> {
    let exe = exe_path()?;
    let (mut root, existed, raw) = read_settings()?;
    let backup_path = if existed && !raw.is_empty() {
        Some(backup(&raw)?)
    } else {
        None
    };

    {
        let hooks = hooks_map(&mut root)?;
        for event in EVENTS {
            let slot = hooks.entry(*event).or_insert_with(|| json!([]));
            let groups = slot
                .as_array_mut()
                .ok_or_else(|| format!("settings.hooks.{event} is not an array"))?;
            strip_ours(groups);
            groups.push(our_group(&exe, event));
        }
    }

    write_settings(&root)?;
    Ok(match backup_path {
        Some(path) => format!(
            "Installed {} hooks into {}. Backup: {}",
            EVENTS.len(),
            claude_settings_path().display(),
            path
        ),
        None => format!(
            "Installed {} hooks into {} (new file).",
            EVENTS.len(),
            claude_settings_path().display()
        ),
    })
}

pub fn uninstall() -> Result<String, String> {
    let (mut root, existed, raw) = read_settings()?;
    if !existed {
        return Ok("Nothing to remove.".to_string());
    }
    let backup_path = backup(&raw)?;

    let mut removed = 0;
    {
        let hooks = hooks_map(&mut root)?;
        let event_names: Vec<String> = hooks.keys().cloned().collect();
        for name in event_names {
            let Some(groups) = hooks.get_mut(&name).and_then(Value::as_array_mut) else {
                continue;
            };
            removed += strip_ours(groups);
            if groups.is_empty() {
                hooks.remove(&name);
            }
        }
    }

    write_settings(&root)?;
    Ok(format!(
        "Removed {removed} Pipsqueak hooks. Backup: {backup_path}"
    ))
}

pub fn installed() -> bool {
    let Ok((root, _, _)) = read_settings() else {
        return false;
    };
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    hooks.values().any(|groups| {
        groups
            .as_array()
            .map(|list| {
                list.iter().any(|group| {
                    group
                        .get("hooks")
                        .and_then(Value::as_array)
                        .map(|entries| entries.iter().any(is_ours))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ours_leaves_other_hooks_untouched() {
        let mut groups = vec![json!({
            "matcher": "*",
            "hooks": [
                { "type": "command", "command": "C:\\apps\\Pipsqueak\\pipsqueak.exe", "args": ["hook", "Stop"] },
                { "type": "command", "command": "C:\\tools\\notify.exe" }
            ]
        })];
        assert_eq!(strip_ours(&mut groups), 1);
        let remaining = groups[0]["hooks"].as_array().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0]["command"], "C:\\tools\\notify.exe");
    }

    #[test]
    fn groups_that_only_held_our_hooks_are_dropped() {
        let mut groups = vec![
            json!({ "matcher": "*", "hooks": [{ "command": "/usr/local/bin/pipsqueak" }] }),
            json!({ "matcher": "Bash", "hooks": [{ "command": "/usr/bin/lint" }] }),
        ];
        assert_eq!(strip_ours(&mut groups), 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["matcher"], "Bash");
    }

    #[test]
    fn install_is_idempotent_for_a_given_event() {
        let mut groups = vec![our_group("pipsqueak.exe", "Stop")];
        strip_ours(&mut groups);
        groups.push(our_group("pipsqueak.exe", "Stop"));
        assert_eq!(groups.len(), 1);
    }
}
