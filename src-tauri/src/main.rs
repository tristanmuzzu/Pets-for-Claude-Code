#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod attention;
mod control;
mod desktop;
mod doctor;
mod hook;
mod install;
mod process;
mod project;
mod risk;
mod state;

use std::fs;

const HELP: &str = "\
Pipsqueak — a desktop pet that shows what Claude Code is doing.

USAGE:
  pipsqueak                 Run the overlay (default)
  pipsqueak control <what>  on | off | toggle | quit | status | <pet name>
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
        Some("install") | Some("--install") => report(install::install()),
        Some("uninstall") | Some("--uninstall") => report(install::uninstall()),
        Some("--version") | Some("-v") => println!("pipsqueak {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") => println!("{HELP}"),
        _ => app::run(),
    }
}

/// Reports a CLI result without ever taking the process down with it.
///
/// This is a GUI-subsystem binary on Windows, so depending on who launched it
/// there may be no valid stdout handle at all. `println!` panics in that case,
/// and with `panic = "abort"` that killed the process before it could record
/// anything — so write the file first, and treat printing as best-effort.
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
