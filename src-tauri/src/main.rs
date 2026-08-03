#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod desktop;
mod hook;
mod install;
mod project;
mod state;

use std::fs;

const HELP: &str = "\
Pipsqueak — a desktop pet that shows what Claude Code is doing.

USAGE:
  pipsqueak                 Run the overlay (default)
  pipsqueak install         Register Claude Code hooks in ~/.claude/settings.json
  pipsqueak uninstall       Remove them again
  pipsqueak hook <Event>    Internal: consume one hook payload on stdin
  pipsqueak --version       Print version
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") => hook::run(args.get(1).cloned()),
        Some("install") | Some("--install") => report(install::install()),
        Some("uninstall") | Some("--uninstall") => report(install::uninstall()),
        Some("--version") | Some("-v") => println!("pipsqueak {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") => println!("{HELP}"),
        _ => app::run(),
    }
}

/// The Windows build is a GUI-subsystem binary, so stdout is usually a black
/// hole. Mirror CLI results into the data directory so they can be read back.
fn report(result: Result<String, String>) {
    let (message, code) = match result {
        Ok(message) => (message, 0),
        Err(message) => (format!("error: {message}"), 1),
    };
    println!("{message}");
    let _ = fs::create_dir_all(state::root());
    let _ = fs::write(state::root().join("last-cli-result.txt"), &message);
    std::process::exit(code);
}
