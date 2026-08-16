//! "It isn't doing anything", answered without a support thread.
//!
//! Everything this app shows arrives through Claude Code hooks, and there are
//! several ordinary ways for that chain to break silently: the hooks were
//! never registered, another tool rewrote `settings.json`, the executable
//! moved, or the overlay is not actually running. From the outside every one
//! of those looks the same: like nothing happening.
//!
//! So the checks are concrete, and the interesting one is not a check at all.
//! It watches for real hook traffic while the user goes and does something.
//! Configuration being right is not the same as it working, and only one of
//! those is worth telling someone.

use crate::install;
use crate::state::{self, claude_settings_path, now_ms, read_sessions, sessions_dir};
use serde::Serialize;
use serde_json::Value;
use std::fs;

/// A heartbeat older than this means whatever wrote it is gone.
const HEARTBEAT_STALE_MS: u64 = 6_000;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: &'static str,
    pub label: &'static str,
    /// `ok` | `warn` | `fail`
    pub status: &'static str,
    /// One plain sentence. If something is wrong, it says what to do about it.
    pub detail: String,
    /// Set when a button can plausibly fix it.
    pub fix: Option<&'static str>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub status: &'static str,
    pub checks: Vec<Check>,
    pub version: &'static str,
    pub platform: &'static str,
}

fn ok(id: &'static str, label: &'static str, detail: String) -> Check {
    Check {
        id,
        label,
        status: "ok",
        detail,
        fix: None,
    }
}

fn warn(id: &'static str, label: &'static str, detail: String) -> Check {
    Check {
        id,
        label,
        status: "warn",
        detail,
        fix: None,
    }
}

fn fail(id: &'static str, label: &'static str, detail: String, fix: Option<&'static str>) -> Check {
    Check {
        id,
        label,
        status: "fail",
        detail,
        fix,
    }
}

pub fn run(hotkey: &str) -> Report {
    let checks = vec![
        hooks_check(),
        binary_check(),
        storage_check(),
        overlay_check(),
        hotkey_check(hotkey),
        autostart_check(),
        traffic_check(),
    ];
    let status = if checks.iter().any(|c| c.status == "fail") {
        "fail"
    } else if checks.iter().any(|c| c.status == "warn") {
        "warn"
    } else {
        "ok"
    };
    Report {
        status,
        checks,
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
    }
}

fn hooks_check() -> Check {
    let path = claude_settings_path();
    if !path.exists() {
        return fail(
            "hooks",
            "Claude Code hooks",
            "No ~/.claude/settings.json. Start Claude Code once, then install the hooks.".into(),
            Some("install"),
        );
    }
    let Ok(raw) = fs::read_to_string(&path) else {
        return fail(
            "hooks",
            "Claude Code hooks",
            "~/.claude/settings.json cannot be read.".into(),
            None,
        );
    };
    if serde_json::from_str::<Value>(&raw).is_err() {
        // Never offer to repair this. Writing over a file we cannot parse is
        // how a config that was merely being edited becomes a lost one.
        return fail(
            "hooks",
            "Claude Code hooks",
            "~/.claude/settings.json is not valid JSON, so it was left alone. Fix the file first."
                .into(),
            None,
        );
    }
    let missing = install::missing_events();
    if missing.is_empty() {
        ok(
            "hooks",
            "Claude Code hooks",
            format!("All {} events registered.", install::EVENTS.len()),
        )
    } else if missing.len() == install::EVENTS.len() {
        fail(
            "hooks",
            "Claude Code hooks",
            "Not installed. Nothing will ever reach the pet until they are.".into(),
            Some("install"),
        )
    } else {
        fail(
            "hooks",
            "Claude Code hooks",
            format!(
                "Missing: {}. Something rewrote settings.json.",
                missing.join(", ")
            ),
            Some("install"),
        )
    }
}

fn binary_check() -> Check {
    let Ok(current) = crate::desktop::own_program() else {
        return warn(
            "binary",
            "Hook program",
            "Cannot determine this program's path.".into(),
        );
    };
    match install::registered_command() {
        None => ok("binary", "Hook program", "Nothing registered yet.".into()),
        Some(registered) => {
            let path = std::path::Path::new(&registered);
            // Case- and separator-insensitive: Windows paths that differ only
            // in casing or slash direction are the same file, and a raw
            // comparison warned about "a different copy" that was this one.
            let same = fs::canonicalize(path)
                .ok()
                .zip(fs::canonicalize(&current).ok())
                .map(|(a, b)| a == b)
                .unwrap_or_else(|| {
                    registered
                        .replace('/', "\\")
                        .eq_ignore_ascii_case(&current.to_string_lossy().replace('/', "\\"))
                });
            if !path.exists() {
                fail(
                    "binary",
                    "Hook program",
                    format!("The hooks point at {registered}, which no longer exists. Reinstall to update the path."),
                    Some("install"),
                )
            } else if !same {
                warn(
                    "binary",
                    "Hook program",
                    format!("The hooks run a different copy of Pipsqueak ({registered})."),
                )
            } else {
                ok(
                    "binary",
                    "Hook program",
                    "Hooks point at this executable.".into(),
                )
            }
        }
    }
}

fn storage_check() -> Check {
    let dir = sessions_dir();
    if fs::create_dir_all(&dir).is_err() {
        return fail(
            "storage",
            "Session folder",
            format!("Cannot create {}.", dir.display()),
            None,
        );
    }
    let probe = dir.join(".writable");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            ok(
                "storage",
                "Session folder",
                format!("{} is writable.", dir.display()),
            )
        }
        Err(err) => fail(
            "storage",
            "Session folder",
            format!("{} is not writable: {err}", dir.display()),
            None,
        ),
    }
}

fn overlay_check() -> Check {
    // Reading our own heartbeat looks circular, but it is not: the hooks are
    // separate processes, and this is the same file they use to decide whether
    // anyone is listening.
    let fresh = fs::read_to_string(state::heartbeat_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.get("ms").and_then(Value::as_u64))
        .map(|ms| now_ms().saturating_sub(ms) < HEARTBEAT_STALE_MS)
        .unwrap_or(false);
    if fresh {
        ok("overlay", "Overlay", "Running.".into())
    } else {
        warn(
            "overlay",
            "Overlay",
            "No recent heartbeat. Session files are still recorded.".into(),
        )
    }
}

fn hotkey_check(registered: &str) -> Check {
    let wanted = state::load_config().hotkey;
    if wanted.trim().eq_ignore_ascii_case("off") || wanted.trim().is_empty() {
        return ok("hotkey", "Keyboard shortcut", "Turned off.".into());
    }
    if registered.is_empty() {
        return warn(
            "hotkey",
            "Keyboard shortcut",
            "None registered. Every candidate chord is already claimed by another program; pick a free one with \"hotkey\" in config.json."
                .into(),
        );
    }
    if registered.eq_ignore_ascii_case(wanted.trim()) {
        ok(
            "hotkey",
            "Keyboard shortcut",
            format!("{registered} shows and hides the pet. {}", fired_summary()),
        )
    } else {
        // Worth saying out loud. Pressing the chord you configured and having
        // nothing happen is indistinguishable from the app being broken.
        warn(
            "hotkey",
            "Keyboard shortcut",
            format!("{wanted} was already taken, so {registered} is being used instead."),
        )
    }
}

/// How many times the chord has actually reached us, and when.
///
/// Registering a hotkey and receiving one are separate things: another program
/// with a low-level keyboard hook can swallow the keypress on its way past,
/// and from inside this process that is indistinguishable from nobody pressing
/// it. Saying which one it is turns a shrug into a next step.
fn fired_summary() -> String {
    let Some(value) = fs::read_to_string(state::hotkey_log_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    else {
        return "Not pressed yet since it was installed.".to_string();
    };
    let count = value.get("fired").and_then(Value::as_u64).unwrap_or(0);
    let at = value.get("at").and_then(Value::as_u64).unwrap_or(0);
    let ago = now_ms().saturating_sub(at) / 1000;
    format!("Pressed {count} time(s), last {ago}s ago.")
}

/// Will the pet still be here after a reboot?
///
/// "It was gone this morning and the shortcut did nothing" is the same
/// complaint as "the hooks are not firing" from the outside, and this check is
/// where somebody looks for the answer. A setup that will lose the pet at the
/// next restart used to pass with every light green.
fn autostart_check() -> Check {
    let current = crate::desktop::own_program().unwrap_or_default();
    match crate::desktop::autostart_command() {
        None => warn(
            "autostart",
            "After a reboot",
            format!(
                "Will not start {}. Turn it back on from the menu, or run `pipsqueak autostart on`.",
                crate::desktop::AT_LOGIN
            ),
        ),
        Some(registered) => {
            let same = std::path::Path::new(&registered)
                .canonicalize()
                .ok()
                .zip(current.canonicalize().ok())
                .map(|(a, b)| a == b)
                .unwrap_or_else(|| {
                    crate::desktop::same_program(&registered, &current.to_string_lossy())
                });
            if same {
                ok(
                    "autostart",
                    "After a reboot",
                    format!("Starts {}.", crate::desktop::AT_LOGIN),
                )
            } else if crate::desktop::program_is_present(&registered) {
                // Left alone on purpose: this is what a wrapper script looks
                // like from here, and the pet no longer takes such an entry
                // over. Saying "reinstalling corrects it" would be advice to
                // undo a working setup.
                warn(
                    "autostart",
                    "After a reboot",
                    format!(
                        "Starts {registered}, which is not this program but does exist. \
                         Left alone in case it launches the pet for you; \
                         run `pipsqueak autostart on` to point it here instead."
                    ),
                )
            } else {
                warn(
                    "autostart",
                    "After a reboot",
                    format!("Starts {registered}, which is not there any more. Reinstalling corrects it."),
                )
            }
        }
    }
}

fn traffic_check() -> Check {
    let sessions = read_sessions();
    if sessions.is_empty() {
        return warn(
            "traffic",
            "Recent activity",
            "No sessions recorded yet. Run the connection test below.".into(),
        );
    }
    let newest = sessions.iter().map(|s| s.updated_ms).max().unwrap_or(0);
    let age_min = now_ms().saturating_sub(newest) / 60_000;
    let detail = format!(
        "{} session(s); last event {age_min} minute(s) ago.",
        sessions.len()
    );
    // Session files linger for hours; their mere existence is not a pulse.
    // Hooks that broke this morning used to read green here with the age
    // buried in the detail text.
    if age_min > 60 {
        return warn("traffic", "Recent activity", detail);
    }
    ok("traffic", "Recent activity", detail)
}

/// A timestamp to compare against later. See [`watch_result`].
pub fn watch_start() -> u64 {
    now_ms()
}

/// What happened in the session folder since `since`.
///
/// The distinction that matters is the third one. If nothing arrived at all we
/// cannot tell "you did not do anything" from "the hooks are not firing", and
/// saying the wrong one wastes someone's afternoon.
pub fn watch_result(since: u64) -> (&'static str, String) {
    let sessions = read_sessions();
    let touched: Vec<&state::Session> = sessions.iter().filter(|s| s.updated_ms >= since).collect();
    if touched.is_empty() {
        return (
            "none",
            "No hook events arrived. Either nothing ran, or the hooks are not firing. Check that the events above are registered and that Claude Code was restarted after installing them."
                .into(),
        );
    }
    let projects: Vec<String> = {
        let mut names: Vec<String> = touched
            .iter()
            .map(|s| {
                if s.project.is_empty() {
                    s.session_id.clone()
                } else {
                    s.project.clone()
                }
            })
            .collect();
        names.sort();
        names.dedup();
        names
    };
    (
        "ok",
        format!(
            "{} event(s) arrived from: {}. The chain works end to end.",
            touched.len(),
            projects.join(", ")
        ),
    )
}

/// A report you can paste into an issue.
///
/// Paths are the only interesting thing in here and they carry the user's
/// name, so the home directory is folded back to `~` before anything leaves.
pub fn markdown(report: &Report) -> String {
    let mut out = String::from("# Pipsqueak diagnostic\n\n");
    out.push_str(&format!(
        "- Version: {}\n- Platform: {}\n- Overall: {}\n\n",
        report.version, report.platform, report.status
    ));
    out.push_str("| Check | Status | Detail |\n|---|---|---|\n");
    for check in &report.checks {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            check.label,
            check.status,
            redact(&check.detail).replace('|', "\\|")
        ));
    }
    out.push_str("\nGenerated locally. Contains no session content, prompts, or file contents.\n");
    out
}

fn redact(text: &str) -> String {
    let home = state::home_dir().to_string_lossy().to_string();
    let mut out = text.to_string();
    if !home.is_empty() {
        out = out.replace(&home, "~");
        // Windows paths reach us both ways round depending on who built them.
        out = out.replace(&home.replace('\\', "/"), "~");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_never_carries_a_home_directory() {
        let home = state::home_dir().to_string_lossy().to_string();
        let report = Report {
            status: "ok",
            checks: vec![ok(
                "storage",
                "Session folder",
                format!("{home}/.pipsqueak/sessions"),
            )],
            version: "0.0.0",
            platform: "test",
        };
        let text = markdown(&report);
        assert!(
            !text.contains(&home),
            "the home path leaked into the report"
        );
        assert!(text.contains("~/.pipsqueak") || text.contains("~\\.pipsqueak"));
    }

    #[test]
    fn a_table_cell_cannot_break_the_table() {
        let report = Report {
            status: "ok",
            checks: vec![ok("x", "Pipe", "a | b".into())],
            version: "0.0.0",
            platform: "test",
        };
        assert!(markdown(&report).contains("a \\| b"));
    }
}
