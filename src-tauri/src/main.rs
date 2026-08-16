#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod attention;
mod chats;
mod control;
mod desktop;
mod doctor;
mod hook;
mod hotkey;
mod install;
mod log;
mod narration;
mod process;
mod project;
mod risk;
mod state;
mod text;

use std::fs;

const HELP: &str = "\
Pipsqueak, a desktop pet that shows what Claude Code is doing.

USAGE:
  pipsqueak                 Run the overlay (default)
  pipsqueak control <what>  on | off | toggle | quit | status | <pet name>
  pipsqueak autostart <on>  on | off | status: start with the machine
  pipsqueak sessions        Print what the overlay would show right now, as JSON
  pipsqueak install         Register Claude Code hooks in ~/.claude/settings.json
  pipsqueak uninstall       Remove them again
  pipsqueak hook <Event>    Internal: consume one hook payload on stdin
  pipsqueak --version       Print version

`control` starts the overlay if it is not already running, so it is the only
command needed to turn the pet on. This is what the Claude Code /pet skill uses.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") => hook::run(args.get(1).cloned()),
        Some("control") => report(control::run(args.get(1).cloned())),
        Some("autostart") => report(autostart(args.get(1).map(String::as_str))),
        Some("sessions") => report(sessions()),
        Some("install") | Some("--install") => report(install::install()),
        Some("uninstall") | Some("--uninstall") => report(install::uninstall()),
        // Through `report` like everything else. A bare `println!` panics when
        // there is no usable stdout — which for a GUI-subsystem binary depends
        // entirely on who launched it — and `panic = "abort"` turns that into
        // the version command killing the process instead of answering.
        Some("--version") | Some("-v") => {
            report(Ok(format!("pipsqueak {}", env!("CARGO_PKG_VERSION"))))
        }
        Some("--help") | Some("-h") => report(Ok(HELP.to_string())),
        _ => app::run(),
    }
}

/// What the overlay would draw, without needing the overlay.
///
/// A card is a session file joined to two things that are not in it: the chat
/// the desktop app knows about, and what the transcript says is still running.
/// Both are computed rather than stored, so "why is the card saying that"
/// could only be answered by looking at the pet. Now it can be answered in a
/// terminal, and asserted in a test.
fn sessions() -> Result<String, String> {
    let mut sessions = state::read_sessions();
    chats::decorate(&mut sessions);
    narration::decorate(&mut sessions, narration::Mode::parse("thoughts"));
    serde_json::to_string_pretty(&sessions).map_err(|e| e.to_string())
}

/// `pipsqueak autostart on|off|status`.
///
/// The tray menu and the welcome panel could already do this, which is fine
/// until the pet is not running, which is exactly when somebody needs it.
fn autostart(what: Option<&str>) -> Result<String, String> {
    match what.unwrap_or("status") {
        "status" | "" => Ok(match desktop::autostart_command() {
            Some(exe) => format!("Starts {}, running {exe}", desktop::AT_LOGIN),
            None => format!("Does not start {}.", desktop::AT_LOGIN),
        }),
        "on" | "enable" | "yes" => {
            // Record the decision so the first-run default cannot undo it.
            let mut config = state::load_config();
            config.autostart_initialised = true;
            let _ = state::save_config(&config);
            desktop::set_autostart(true).map(|()| format!("Will start {}.", desktop::AT_LOGIN))
        }
        "off" | "disable" | "no" => {
            let mut config = state::load_config();
            config.autostart_initialised = true;
            let _ = state::save_config(&config);
            desktop::set_autostart(false).map(|()| format!("Will not start {}.", desktop::AT_LOGIN))
        }
        other => Err(format!("unknown autostart option: {other}")),
    }
}

/// Reports a CLI result without ever taking the process down with it.
///
/// This is a GUI-subsystem binary on Windows, so depending on who launched it
/// there may be no valid stdout handle at all. `println!` panics in that case,
/// and with `panic = "abort"` that killed the process before it could record
/// anything, so write the file first and treat printing as best-effort.
fn report(result: Result<String, String>) {
    use std::io::Write;

    let (message, code) = match result {
        Ok(message) => (message, 0),
        Err(message) => (format!("error: {message}"), 1),
    };

    let _ = fs::create_dir_all(state::root());
    let _ = fs::write(state::root().join("last-cli-result.txt"), &message);

    let mut out = std::io::stdout();
    let _ = writeln!(out, "{message}");
    // stdout is block-buffered when it is not a terminal, and process::exit
    // runs no destructors.
    let _ = out.flush();

    std::process::exit(code);
}
