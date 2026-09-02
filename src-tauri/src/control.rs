//! `pipsqueak control <action>`, the entry point used by the Claude Code
//! `/pet` command and anything else that wants to drive a running overlay.
//!
//! Commands are handed over through a file rather than a socket, for the same
//! reason session state is: no port to collide with, nothing to be running
//! first, and it works identically whether the overlay is up or not.

use crate::state::{command_path, heartbeat_path, now_ms, write_atomic};
use serde_json::json;
use std::fs;

/// A heartbeat older than this means nobody is home.
const HEARTBEAT_TIMEOUT_MS: u64 = 6_000;

pub fn run(action: Option<String>) -> Result<String, String> {
    let action = action
        .unwrap_or_else(|| "toggle".to_string())
        .to_lowercase();
    let running = is_running();

    match action.as_str() {
        "status" => Ok(if running {
            "Pipsqueak is running.".to_string()
        } else {
            "Pipsqueak is not running.".to_string()
        }),
        "off" | "hide" => {
            if !running {
                return Ok("Pipsqueak is not running.".to_string());
            }
            send("hide")?;
            Ok("Pet hidden.".to_string())
        }
        // The UI distinguishes "put it away" from "stop it"; the CLI keeps
        // the same distinction. "stop" leaving the process, hotkey and tray
        // alive was a lie by another name.
        "quit" | "exit" | "stop" => {
            if !running {
                return Ok("Pipsqueak is not running.".to_string());
            }
            send("quit")?;
            Ok("Pipsqueak closed.".to_string())
        }
        "on" | "show" | "start" => {
            if running {
                send("show")?;
                Ok("Pet shown.".to_string())
            } else {
                launch()?;
                Ok("Pipsqueak started.".to_string())
            }
        }
        "toggle" | "" => {
            if running {
                send("toggle")?;
                Ok("Pet toggled.".to_string())
            } else {
                launch()?;
                Ok("Pipsqueak started.".to_string())
            }
        }
        // Anything else is treated as a pet to switch to.
        pet => {
            send(&format!("pet:{pet}"))?;
            if !running {
                launch()?;
            }
            Ok(format!("Pet switched to {pet}."))
        }
    }
}

pub fn is_running() -> bool {
    let Some(beat) = fs::read_to_string(heartbeat_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    else {
        return false;
    };
    let fresh = beat
        .get("ms")
        .and_then(serde_json::Value::as_u64)
        .map(|ms| now_ms().saturating_sub(ms) < HEARTBEAT_TIMEOUT_MS)
        .unwrap_or(false);
    if !fresh {
        return false;
    }
    // Recent is not the same as alive. An overlay that was killed leaves its
    // last heartbeat behind, and for the next few seconds that file claims
    // somebody is home: starting the pet again in that window did nothing at
    // all, silently, which is the exact complaint the single-instance guard
    // exists to prevent rather than cause.
    match beat.get("pid").and_then(serde_json::Value::as_u64) {
        Some(pid) if pid > 0 => crate::process::is_alive(pid as u32, 0),
        // An older build's heartbeat had no pid; trust its freshness.
        _ => true,
    }
}

fn send(action: &str) -> Result<(), String> {
    let payload = json!({ "action": action, "ms": now_ms() });
    write_atomic(
        &command_path(),
        serde_json::to_vec(&payload).unwrap_or_default().as_slice(),
    )
    .map_err(|e| format!("cannot write command: {e}"))
}

/// Starts the overlay detached. The binary is a GUI-subsystem app, so this
/// never flashes a console window.
///
/// Through the login launcher when there is one that is not this binary: a
/// wrapper that sets `GDK_BACKEND=x11` or waits for the tray is the way the
/// owner wants the pet started, and starting the bare binary from here gave
/// a pet with no always-on-top and a blank tray menu. Detached on Unix as
/// well — its own process group, no inherited pipes — because the pet used
/// to die with the shell that ran `pipsqueak control on`, and held the
/// `/pet` skill's stdout open for as long as it lived.
fn launch() -> Result<(), String> {
    let exe = crate::desktop::own_program()?;
    let launcher = crate::desktop::autostart_command().filter(|registered| {
        !crate::desktop::same_program(registered, &exe.to_string_lossy())
            && crate::desktop::program_is_present(registered)
    });
    let mut command = match &launcher {
        Some(registered) => crate::desktop::quiet_command(registered),
        None => crate::desktop::quiet_command(&exe),
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot start Pipsqueak: {e}"))
}
