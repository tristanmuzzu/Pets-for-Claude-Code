//! The overlay: a transparent, always-on-top window that polls session state
//! and animates the pet.

use crate::attention::{self, Attention};
use crate::chats;
use crate::desktop;
use crate::doctor;
use crate::hotkey::{self, Binding};
use crate::install;
use crate::narration;
use crate::state::{
    self, codex_pets_dir, load_config, pets_dir, read_sessions, root, sessions_dir, Config, Session,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State};

const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// Hit-test rates, by how close the cursor is to something interactive. Only
/// the first one has to be quick, and only while the cursor is right there.
/// Polling this fast all the time would keep a core busy for an overlay you
/// are not even pointing at.
const HIT_TEST_NEAR: Duration = Duration::from_millis(12);
const HIT_TEST_APPROACHING: Duration = Duration::from_millis(50);
const HIT_TEST_FAR: Duration = Duration::from_millis(220);
/// Roughly a card's own height: far enough out that a fast flick is caught
/// before it lands.
const NEAR_PX: f64 = 90.0;
const APPROACHING_PX: f64 = 320.0;
/// How far outside the window a cursor is still worth telling the pet about,
/// and how often to say so. A pet that can look at things wants this often
/// enough to track a moving cursor and rarely enough to be free.
const LOOK_RADIUS_PX: f64 = 420.0;
const LOOK_INTERVAL: Duration = Duration::from_millis(60);
/// Often enough that a crashed session disappears while you are still looking
/// at the card, rare enough that it costs nothing.
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
const WINDOW_LABEL: &str = "pet";

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    /// Distance from the point to the nearest edge; zero inside.
    fn distance_to(&self, x: f64, y: f64) -> f64 {
        let dx = (self.x - x).max(0.0).max(x - (self.x + self.w));
        let dy = (self.y - y).max(0.0).max(y - (self.y + self.h));
        (dx * dx + dy * dy).sqrt()
    }
}

#[derive(Default)]
struct Interactive(Mutex<Vec<Rect>>);

/// Mirrors `config.click_through` so the hit-test loop never touches the disk.
#[derive(Default)]
struct ClickThrough(AtomicBool);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct PetInfo {
    id: String,
    display_name: String,
    description: String,
    source: &'static str,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct PetPayload {
    manifest: Value,
    /// `None` for the bundled pet, which the frontend loads from its own assets.
    image_data_url: Option<String>,
}

// --- commands ----------------------------------------------------------
#[tauri::command]
fn get_sessions() -> Vec<Session> {
    current_sessions()
}

/// Everything the overlay shows, with each session's desktop chat attached.
///
/// The hooks cannot supply the title: they run inside the session and know
/// nothing about the window it is displayed in. It is joined on here instead,
/// where a missing answer costs a nicer label and nothing else.
fn current_sessions() -> Vec<Session> {
    let mut sessions = read_sessions();
    chats::decorate(&mut sessions);
    narration::decorate(&mut sessions, narration::mode());
    sessions
}

#[tauri::command]
fn get_config() -> Config {
    load_config()
}

#[tauri::command]
fn set_config(app: AppHandle, config: Config) -> Result<(), String> {
    app.state::<ClickThrough>()
        .0
        .store(config.click_through, Ordering::Relaxed);
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_ignore_cursor_events(config.click_through);
    }
    narration::set_mode(narration::Mode::parse(&config.narrate));
    state::save_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_hit_rects(rects: Vec<Rect>, interactive: State<'_, Interactive>) {
    if let Ok(mut guard) = interactive.0.lock() {
        *guard = rects;
    }
}

#[tauri::command]
fn save_position(app: AppHandle, x: i32, y: i32) -> Result<(), String> {
    let _ = app;
    let mut config = load_config();
    config.x = Some(x);
    config.y = Some(y);
    state::save_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_pets() -> Vec<PetInfo> {
    let mut pets = vec![
        // Byte is the default, and was missing from this list, so the menu
        // could switch away from it and never back.
        PetInfo {
            id: "byte".into(),
            display_name: "Byte".into(),
            description: "A CRT-headed bot. Its screen shows the state, so the pet reads on its own."
                .into(),
            source: "builtin",
        },
        PetInfo {
            id: "pip".into(),
            display_name: "Pip".into(),
            description: "An ember-fox with a status wisp.".into(),
            source: "builtin",
        },
        PetInfo {
            id: "ember".into(),
            display_name: "Ember".into(),
            description: "A clay pebble with a spark for a status light.".into(),
            source: "builtin",
        },
    ];
    for (dir, source) in [(pets_dir(), "user"), (codex_pets_dir(), "codex")] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("pet.json");
            let Some(manifest) = fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            else {
                continue;
            };
            let folder = entry.file_name().to_string_lossy().to_string();
            let id = manifest
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(&folder)
                .to_string();
            if pets.iter().any(|p| p.id == id) {
                continue;
            }
            pets.push(PetInfo {
                display_name: manifest
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string(),
                description: manifest
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                id,
                source,
            });
        }
    }
    pets
}

#[tauri::command]
fn load_pet(id: String) -> Result<PetPayload, String> {
    if id == "byte" || id == "ember" || id == "pip" {
        return Ok(PetPayload {
            manifest: Value::Null,
            image_data_url: None,
        });
    }
    let folder = find_pet_folder(&id).ok_or_else(|| format!("pet '{id}' not found"))?;
    let manifest: Value = fs::read_to_string(folder.join("pet.json"))
        .map_err(|e| e.to_string())
        .and_then(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))?;
    let sprite_name = manifest
        .get("spritesheetPath")
        .and_then(Value::as_str)
        .unwrap_or("spritesheet.webp");
    let sprite_path = folder.join(sprite_name);
    let bytes = fs::read(&sprite_path)
        .map_err(|e| format!("cannot read {}: {e}", sprite_path.display()))?;
    let mime = match sprite_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "gif" => "image/gif",
        _ => "image/webp",
    };
    Ok(PetPayload {
        manifest,
        image_data_url: Some(format!("data:{mime};base64,{}", base64(&bytes))),
    })
}

#[tauri::command]
fn install_hooks() -> Result<String, String> {
    install::install()
}

#[tauri::command]
fn hooks_installed() -> bool {
    install::installed()
}

#[tauri::command]
fn open_pets_dir() -> Result<(), String> {
    let dir = pets_dir();
    let _ = fs::create_dir_all(&dir);
    open_path(&dir)
}

/// Opens the chat a card is about.
///
/// The desktop app is a single window with the chats inside it, so "bring this
/// forward" is two different actions: ask the app to show *that chat*, then
/// raise the window. The first needs the id the app routes by, which only its
/// own records have; without one there is nothing to aim at but a window title,
/// and every chat shares the same one.
#[tauri::command]
fn focus_session(session_id: String, project: String, workspace: String) -> bool {
    if let Some(chat) = chats::lookup(&session_id) {
        if chats::open(&chat.id) {
            // The app raises itself when it handles the link. Matching a title
            // as well would only find the same window and risk fighting it.
            return true;
        }
    }
    focus_project(project, workspace)
}

/// Brings the window for a project forward. Matching is by window title, so it
/// is best-effort: `false` means nothing recognisable was open.
fn focus_project(project: String, workspace: String) -> bool {
    let mut fragments = Vec::new();
    if !workspace.is_empty() {
        fragments.push(workspace);
    }
    if !project.is_empty() {
        fragments.push(project);
    }
    fragments.push("claude".to_string());
    desktop::focus_window_titled(&fragments)
}

#[tauri::command]
fn alert() {
    // Do Not Disturb has to be checked here rather than in the frontend: the
    // frontend's copy of the config can be a moment stale, and a beep that
    // escapes a quiet mode is the one failure nobody forgives.
    if load_config().quiet {
        return;
    }
    desktop::alert();
}

#[tauri::command]
fn run_doctor(app: AppHandle) -> doctor::Report {
    doctor::run(&hotkey_binding(app))
}

/// Marks the start of the live connection test. The frontend tells the user to
/// go and run something, then asks what arrived.
#[tauri::command]
fn watch_start() -> u64 {
    doctor::watch_start()
}

#[tauri::command]
fn watch_result(since: u64) -> (String, String) {
    let (status, detail) = doctor::watch_result(since);
    (status.to_string(), detail)
}

#[tauri::command]
fn doctor_report(app: AppHandle) -> String {
    doctor::markdown(&doctor::run(&hotkey_binding(app)))
}

/// Blink the tray icon: a project finished while you were looking elsewhere.
#[tauri::command]
fn flash_tray(app: AppHandle) {
    attention::flash(&app);
}

/// The chord that shows and hides the pet, for the menu to display.
#[tauri::command]
fn hotkey_binding(app: AppHandle) -> String {
    app.state::<Binding>()
        .0
        .lock()
        .map(|held| held.clone())
        .unwrap_or_default()
}

/// You have seen it. Stop blinking.
#[tauri::command]
fn clear_attention(app: AppHandle) {
    attention::clear(&app);
}

#[tauri::command]
fn autostart_enabled() -> bool {
    desktop::autostart_enabled()
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    desktop::set_autostart(enabled)
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Puts the pet away without stopping it.
///
/// The distinction matters more than it sounds: the global hotkey and the tray
/// icon belong to this process, so quitting takes them with it and nothing on
/// the keyboard can bring the pet back. Hiding leaves both alive.
#[tauri::command]
fn hide_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.hide();
    }
}

// --- helpers -----------------------------------------------------------
fn find_pet_folder(id: &str) -> Option<PathBuf> {
    for dir in [pets_dir(), codex_pets_dir()] {
        let direct = dir.join(id);
        if direct.join("pet.json").exists() {
            return Some(direct);
        }
        let entries = fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let manifest = entry.path().join("pet.json");
            let matches = fs::read_to_string(&manifest)
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string))
                .map(|found| found == id)
                .unwrap_or(false);
            if matches {
                return Some(entry.path());
            }
        }
    }
    None
}

fn open_path(path: &PathBuf) -> Result<(), String> {
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";

    desktop::quiet_command(opener)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn place_window(app: &AppHandle, config: &Config) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    if let (Some(x), Some(y)) = (config.x, config.y) {
        let (x, y) = clamp_to_display(&window, x, y);
        let _ = window.set_position(PhysicalPosition::new(x, y));
        if (x, y) != (config.x.unwrap_or(x), config.y.unwrap_or(y)) {
            // Remember the corrected position, or every launch pays the same
            // correction and a drag near the edge keeps snapping back.
            let mut stored = config.clone();
            stored.x = Some(x);
            stored.y = Some(y);
            let _ = state::save_config(&stored);
        }
        return;
    }
    // Default: bottom-right, clear of the Windows taskbar. Persist it, or the
    // position stays null until the pet is dragged for the first time.
    if let (Ok(Some(monitor)), Ok(size)) = (window.primary_monitor(), window.outer_size()) {
        let area = monitor.size();
        let origin = monitor.position();
        let x = origin.x + area.width.saturating_sub(size.width + 24) as i32;
        let y = origin.y + area.height.saturating_sub(size.height + 72) as i32;
        let _ = window.set_position(PhysicalPosition::new(x, y));
        let mut stored = config.clone();
        stored.x = Some(x);
        stored.y = Some(y);
        let _ = state::save_config(&stored);
    }
}

/// Pulls a position back until the whole window sits inside a display.
///
/// A midpoint check is not enough, which is how the pet ended up below the
/// bottom of the screen: the window is 640 tall, the pet is drawn at the
/// *bottom* of it, and a window whose middle is comfortably on screen can
/// still have its bottom corner hanging off. The part that matters is the part
/// you can see.
///
/// Clamping rather than re-placing also means a saved position that is merely
/// slightly wrong, after a resolution change, a scaling change, or a window
/// that grew between versions, gets nudged back instead of thrown away.
fn clamp_to_display(window: &tauri::WebviewWindow, x: i32, y: i32) -> (i32, i32) {
    let Ok(monitors) = window.available_monitors() else {
        return (x, y);
    };
    if monitors.is_empty() {
        return (x, y);
    }
    let size = window.outer_size().unwrap_or_default();
    let (w, h) = (size.width as i32, size.height as i32);
    let mid_x = x + w / 2;
    let mid_y = y + h / 2;

    // The display the window is mostly on, or the nearest one if it is adrift.
    let contains = |m: &tauri::Monitor| {
        let o = m.position();
        let s = m.size();
        mid_x >= o.x
            && mid_y >= o.y
            && mid_x < o.x + s.width as i32
            && mid_y < o.y + s.height as i32
    };
    let monitor = monitors
        .iter()
        .find(|m| contains(m))
        .or_else(|| {
            monitors.iter().min_by_key(|m| {
                let o = m.position();
                let s = m.size();
                let cx = o.x + s.width as i32 / 2;
                let cy = o.y + s.height as i32 / 2;
                (mid_x - cx).pow(2) + (mid_y - cy).pow(2)
            })
        })
        .unwrap_or(&monitors[0]);

    // Work area rather than the whole screen, so the pet does not end up
    // behind the taskbar.
    let area = monitor.work_area();
    let left = area.position.x;
    let top = area.position.y;
    let right = left + area.size.width as i32 - w;
    let bottom = top + area.size.height as i32 - h;
    // A window taller than the work area has no valid position; pin it to the
    // top-left of that display rather than inverting the range.
    (
        x.clamp(left, right.max(left)),
        y.clamp(top, bottom.max(top)),
    )
}

/// Rescues the overlay when the display it was on disappears while running.
fn ensure_on_screen(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    let Ok(position) = window.outer_position() else {
        return;
    };
    let (x, y) = clamp_to_display(&window, position.x, position.y);
    if (x, y) == (position.x, position.y) {
        return;
    }
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let mut config = load_config();
    config.x = Some(x);
    config.y = Some(y);
    let _ = state::save_config(&config);
}

/// Keeps the window click-through except over the pet and its cards.
///
/// A transparent overlay otherwise swallows every click inside its rectangle,
/// which would make the pet actively hostile to the thing it sits on top of.
///
/// This has to be a poll rather than ordinary pointer events, because a window
/// that is ignoring the cursor receives no events to tell it the cursor has
/// arrived. The cost of polling is the gap: a click landing inside it goes to
/// the wrong window. So the rate follows the cursor: fast when it is near
/// something interactive, idle when it is nowhere near the overlay.
///
/// (The alternative is a second, always-interactive window shaped to the hit
/// area. That works for a single small sprite; here the interactive area is a
/// stack of separate cards, and one window covering their union would swallow
/// clicks in the gaps between them.)
fn spawn_hit_test(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last: Option<bool> = None;
        let mut interval = HIT_TEST_FAR;
        let mut last_look: Option<Instant> = None;
        let mut watching = false;
        loop {
            std::thread::sleep(interval);
            let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
                continue;
            };
            let click_through = app.state::<ClickThrough>().0.load(Ordering::Relaxed);
            let (Ok(cursor), Ok(origin), Ok(scale)) = (
                window.cursor_position(),
                window.outer_position(),
                window.scale_factor(),
            ) else {
                continue;
            };
            let local_x = (cursor.x - origin.x as f64) / scale;
            let local_y = (cursor.y - origin.y as f64) / scale;

            // Where the cursor is, for a pet that was drawn able to look at it.
            // Rate-limited, and dropped entirely once the cursor is far away,
            // because this is the one message in the app that could otherwise
            // fire every frame.
            let size = window
                .inner_size()
                .map(|size| (size.width as f64 / scale, size.height as f64 / scale))
                .unwrap_or((0.0, 0.0));
            let near = local_x > -LOOK_RADIUS_PX
                && local_y > -LOOK_RADIUS_PX
                && local_x < size.0 + LOOK_RADIUS_PX
                && local_y < size.1 + LOOK_RADIUS_PX;
            if last_look
                .map(|at: Instant| at.elapsed() >= LOOK_INTERVAL)
                .unwrap_or(true)
                && (near || watching)
            {
                // `None` on the way out, once: the pet faces front again rather
                // than staring at wherever the cursor was last seen.
                let payload = near.then_some((local_x, local_y));
                let _ = app.emit("pipsqueak://cursor", payload);
                last_look = Some(Instant::now());
                watching = near;
            }

            if click_through {
                if last != Some(true) {
                    let _ = window.set_ignore_cursor_events(true);
                    last = Some(true);
                }
                interval = HIT_TEST_FAR;
                continue;
            }
            let (over, distance) = app
                .state::<Interactive>()
                .0
                .lock()
                .map(|rects| {
                    let over = rects.iter().any(|r| r.contains(local_x, local_y));
                    let distance = rects
                        .iter()
                        .map(|r| r.distance_to(local_x, local_y))
                        .fold(f64::INFINITY, f64::min);
                    (over, distance)
                })
                .unwrap_or((false, f64::INFINITY));
            // Approaching a card, or just left one: this is the window in which
            // being wrong costs a misdirected click.
            interval = if distance <= NEAR_PX {
                HIT_TEST_NEAR
            } else if distance <= APPROACHING_PX {
                HIT_TEST_APPROACHING
            } else {
                HIT_TEST_FAR
            };
            if last != Some(!over) {
                let _ = window.set_ignore_cursor_events(!over);
                last = Some(!over);
            }
        }
    });
}

fn spawn_poller(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last = String::new();
        let mut next_sweep = Instant::now();
        loop {
            // Hooks can only ever say what happened; nothing writes a file to
            // report that Claude Code died. The sweep is the only thing that
            // can retire a session, so it has to run on a clock of its own.
            if Instant::now() >= next_sweep {
                state::sweep();
                // Same cadence, different job: catch an undock or an unplugged
                // monitor that left the overlay somewhere you cannot reach it.
                ensure_on_screen(&app);
                next_sweep = Instant::now() + SWEEP_INTERVAL;
            }
            let sessions = current_sessions();
            let encoded = serde_json::to_string(&sessions).unwrap_or_default();
            if encoded != last {
                last = encoded;
                let _ = app.emit("pipsqueak://sessions", &sessions);
            }
            beat();
            drain_commands(&app);
            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

/// Tells other invocations of the binary that an overlay is already up.
fn beat() {
    let payload = serde_json::json!({ "ms": state::now_ms(), "pid": std::process::id() });
    let _ = state::write_atomic(
        &state::heartbeat_path(),
        serde_json::to_vec(&payload).unwrap_or_default().as_slice(),
    );
}

/// Applies whatever `pipsqueak control …` left for us, then clears it.
fn drain_commands(app: &AppHandle) {
    let path = state::command_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    let _ = fs::remove_file(&path);
    let Some(action) = serde_json::from_str::<Value>(&raw).ok().and_then(|value| {
        value
            .get("action")
            .and_then(Value::as_str)
            .map(String::from)
    }) else {
        return;
    };

    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    match action.as_str() {
        "show" => {
            let _ = window.show();
        }
        "hide" => {
            let _ = window.hide();
        }
        "toggle" => toggle_window(app),
        "quit" => app.exit(0),
        other => {
            if let Some(pet) = other.strip_prefix("pet:") {
                let _ = window.show();
                // The frontend owns both sprite loading and the config file, and
                // writing it from here would race its copy and lose the window
                // position.
                let _ = app.emit("pipsqueak://pet", pet.to_string());
            }
        }
    }
}

fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.show();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let config = load_config();
    let toggle = MenuItem::with_id(app, "toggle", "Show / hide pet", true, None::<&str>)?;
    let click_through = CheckMenuItem::with_id(
        app,
        "click_through",
        "Click through the pet",
        true,
        config.click_through,
        None::<&str>,
    )?;
    let quiet = CheckMenuItem::with_id(
        app,
        "quiet",
        "Do not disturb",
        true,
        config.quiet,
        None::<&str>,
    )?;
    let pets = MenuItem::with_id(app, "pets", "Open pets folder", true, None::<&str>)?;
    let hooks = MenuItem::with_id(
        app,
        "hooks",
        "Reinstall Claude Code hooks",
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Pipsqueak", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle,
            &click_through,
            &quiet,
            &PredefinedMenuItem::separator(app)?,
            &pets,
            &hooks,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("pipsqueak")
        .tooltip("Pipsqueak")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => toggle_window(app),
            "click_through" => {
                let mut config = load_config();
                config.click_through = !config.click_through;
                let _ = state::save_config(&config);
                app.state::<ClickThrough>()
                    .0
                    .store(config.click_through, Ordering::Relaxed);
                if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
                    let _ = window.set_ignore_cursor_events(config.click_through);
                }
            }
            "quiet" => {
                let mut config = load_config();
                config.quiet = !config.quiet;
                let _ = state::save_config(&config);
                if config.quiet {
                    attention::clear(app);
                }
                let _ = app.emit("pipsqueak://config", ());
            }
            "pets" => {
                let _ = open_pets_dir();
            }
            "hooks" => {
                let message = match install::install() {
                    Ok(text) => text,
                    Err(text) => text,
                };
                let _ = app.emit("pipsqueak://notice", message);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                // Clicking the tray is the user noticing it. Whatever the
                // blink was for, they have seen it.
                attention::clear(tray.app_handle());
                toggle_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

pub fn run() {
    let _ = fs::create_dir_all(sessions_dir());
    let _ = fs::create_dir_all(pets_dir());
    let _ = fs::create_dir_all(root());
    state::sweep();

    tauri::Builder::default()
        .manage(Interactive::default())
        .manage(Attention::default())
        .manage(Binding::default())
        .manage(ClickThrough::default())
        .invoke_handler(tauri::generate_handler![
            get_sessions,
            get_config,
            set_config,
            set_hit_rects,
            save_position,
            list_pets,
            load_pet,
            install_hooks,
            hooks_installed,
            open_pets_dir,
            focus_session,
            alert,
            flash_tray,
            run_doctor,
            watch_start,
            watch_result,
            doctor_report,
            clear_attention,
            hotkey_binding,
            autostart_enabled,
            set_autostart,
            quit,
            hide_window
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let config = load_config();
            handle
                .state::<ClickThrough>()
                .0
                .store(config.click_through, Ordering::Relaxed);
            narration::set_mode(narration::Mode::parse(&config.narrate));
            place_window(&handle, &config);
            if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                let _ = window.set_ignore_cursor_events(true);
                let _ = window.show();
            }
            build_tray(&handle)?;
            // Whatever chord we actually got, so the menu can show it rather
            // than the one that was asked for.
            match hotkey::start(handle.clone()) {
                Ok(chord) => {
                    if let Ok(mut held) = handle.state::<Binding>().0.lock() {
                        *held = chord;
                    }
                }
                Err(reason) => eprintln!("no global hotkey: {reason}"),
            }

            spawn_poller(handle.clone());
            spawn_hit_test(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start Pipsqueak");
}
