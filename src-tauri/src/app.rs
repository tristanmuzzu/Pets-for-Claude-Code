//! The overlay: a transparent, always-on-top window that polls session state
//! and animates the pet.

use crate::attention::{self, Attention};
use crate::chats;
use crate::desktop;
use crate::doctor;
use crate::hotkey::{self, Binding};
use crate::install;
use crate::log;
use crate::narration;
use crate::state::{
    self, codex_pets_dir, load_config, pets_dir, read_sessions, root, sessions_dir, Config, Session,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// How long the frontend may go quiet before we assume the page is dead.
///
/// It reports its clickable regions at least once a second. Generous anyway,
/// because Windows throttles timers hard for an occluded window and a needless
/// reload is still a visible flicker.
const FRONTEND_SILENT_MS: u64 = 45_000;
/// How long a silent page gets to answer a ping before it is declared dead.
///
/// Timers are throttled hard for an occluded window — down to once a minute —
/// so silence alone only proves the page is not *rendering*. Event delivery is
/// not throttled: a live page answers the ping immediately, and one that does
/// not is actually gone. Checked on the sweep cadence, so the effective grace
/// is one sweep interval.
const PING_GRACE_MS: u64 = 3_000;
/// A rescue that fails leaves the page just as dead, so retrying in a tight
/// loop only fills the log.
const RESCUE_INTERVAL_MS: u64 = 60_000;
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

/// When the frontend last spoke to us.
///
/// It reports its clickable regions on every render, and it renders at least
/// once a second, so silence means the page has stopped running. A window can
/// be present, visible and correctly placed while painting nothing, and this
/// is the only thing that can tell the difference.
#[derive(Default)]
struct Frontend {
    last_seen_ms: AtomicU64,
    last_ping_ms: AtomicU64,
    last_rescue_ms: AtomicU64,
}

/// The last session list the poller emitted, so the frontend can ask for a
/// re-send by clearing it. An emit that fires before the page's listeners
/// attach (first boot, any reload) is otherwise lost until the next change.
#[derive(Default)]
struct Emitted(Mutex<String>);

/// The tray's two checkboxes, so a config change made anywhere else can move
/// them. Set once at build time they drifted: toggle quiet from the pet menu
/// and the tray kept claiming the old value.
#[derive(Default)]
struct TrayToggles(Mutex<Option<(CheckMenuItem<tauri::Wry>, CheckMenuItem<tauri::Wry>)>>);

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
    sync_tray_toggles(&app, &config);
    state::save_config(&config).map_err(|e| e.to_string())
}

/// Moves the tray checkboxes to match a config changed anywhere else.
fn sync_tray_toggles(app: &AppHandle, config: &Config) {
    if let Ok(guard) = app.state::<TrayToggles>().0.lock() {
        if let Some((click_through, quiet)) = guard.as_ref() {
            let _ = click_through.set_checked(config.click_through);
            let _ = quiet.set_checked(config.quiet);
        }
    }
}

/// The page answering a ping: alive, whatever its throttled timers are doing.
#[tauri::command]
fn frontend_pong(frontend: State<'_, Frontend>) {
    frontend
        .last_seen_ms
        .store(state::now_ms(), Ordering::Relaxed);
}

/// The page has (re)attached its listeners. Anything emitted before this
/// moment was shouted into an empty room, so forget what was last sent and
/// let the poller send it again.
#[tauri::command]
fn frontend_ready(frontend: State<'_, Frontend>, emitted: State<'_, Emitted>) {
    frontend
        .last_seen_ms
        .store(state::now_ms(), Ordering::Relaxed);
    if let Ok(mut last) = emitted.0.lock() {
        last.clear();
    }
}

#[tauri::command]
fn set_hit_rects(
    rects: Vec<Rect>,
    interactive: State<'_, Interactive>,
    frontend: State<'_, Frontend>,
) {
    frontend
        .last_seen_ms
        .store(state::now_ms(), Ordering::Relaxed);
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
            description:
                "A CRT-headed bot. Its screen shows the state, so the pet reads on its own.".into(),
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
    // Turning it off is a decision, and `ensure_autostart` must not helpfully
    // switch it back on at the next launch.
    let mut config = load_config();
    if !config.autostart_initialised {
        config.autostart_initialised = true;
        let _ = state::save_config(&config);
    }
    desktop::set_autostart(enabled)
}

#[tauri::command]
fn quit(app: AppHandle) {
    shutdown(&app);
}

/// Stops the overlay, and stops it claiming to be running.
///
/// The heartbeat is what tells the next launch somebody is already home, and
/// it is believed for a few seconds after it was last written. Leaving it
/// behind meant quitting and immediately starting again did nothing at all,
/// silently — the single-instance guard causing the complaint it exists to
/// prevent.
fn shutdown(app: &AppHandle) {
    let _ = fs::remove_file(state::heartbeat_path());
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
    // Default: bottom-right of the work area, which already excludes the
    // taskbar wherever it is and however tall it is. The old hard-coded 72px
    // allowance undershot a scaled or stacked taskbar and the pet's feet
    // started behind it until ensure_on_screen corrected it seconds later.
    if let (Ok(Some(monitor)), Ok(size)) = (window.primary_monitor(), window.outer_size()) {
        let area = monitor.work_area();
        let x = area.position.x + (area.size.width as i32 - size.width as i32 - 24).max(0);
        let y = area.position.y + (area.size.height as i32 - size.height as i32 - 24).max(0);
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
                watch_frontend(&app);
                check_own_binary(&app);
                next_sweep = Instant::now() + SWEEP_INTERVAL;
            }
            let sessions = current_sessions();
            let encoded = serde_json::to_string(&sessions).unwrap_or_default();
            // The dedupe string lives in shared state so `frontend_ready` can
            // clear it: an emit fired before the page's listeners attached
            // needs sending again, and nothing else would change the data.
            let changed = app
                .state::<Emitted>()
                .0
                .lock()
                .map(|mut last| {
                    if *last == encoded {
                        false
                    } else {
                        *last = encoded;
                        true
                    }
                })
                .unwrap_or(false);
            if changed {
                let _ = app.emit("pipsqueak://sessions", &sessions);
            }
            beat();
            drain_commands(&app);
            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

/// Starts with Windows by default, and keeps pointing at the right program.
///
/// Two separate failures, both of which look identical from the outside — the
/// pet is simply not there, and the hotkey that would bring it back belongs to
/// a process that is not running:
///
/// 1. Nobody ever turned autostart on. It was a button in a panel shown once,
///    which is a poor way to decide something an overlay depends on entirely.
///    The first run now registers it and says so; turning it off keeps it off.
/// 2. The entry pointed at a path the program no longer lives at, after an
///    install to a different location. Present, plausible, and inert.
fn ensure_autostart(config: &Config) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = exe.to_string_lossy().to_string();
    match desktop::autostart_command() {
        // Registered, but for a program that is not this one. Rewrite it: a
        // stale entry silently stops the pet coming back after a reboot.
        Some(registered) if !same_program(&registered, &exe) => {
            log::write(&format!(
                "autostart pointed at {registered}, correcting it to {exe}"
            ));
            let _ = desktop::set_autostart(true);
        }
        Some(_) => {}
        None => {
            if config.autostart_initialised {
                // Deliberately off. Leave it alone.
                return;
            }
            let mut stored = config.clone();
            stored.autostart_initialised = true;
            // Record the decision before acting on it, so a failure here is
            // not retried on every launch forever.
            let _ = state::save_config(&stored);
            match desktop::set_autostart(true) {
                Ok(()) => log::write("first run: registered to start with Windows"),
                Err(err) => log::write(&format!("could not register autostart: {err}")),
            }
        }
    }
}

/// Windows path comparison: case and slash direction do not make two programs.
fn same_program(a: &str, b: &str) -> bool {
    let normalise = |p: &str| p.trim().trim_matches('"').replace('/', "\\").to_lowercase();
    normalise(a) == normalise(b)
}

/// Notices that the program has been deleted out from under itself.
///
/// Windows keeps a running image mapped after its file is gone, so the overlay
/// carries on looking healthy while every hook Claude Code fires is pointing
/// at a path that no longer exists. Nothing recovers from this on its own, and
/// the only honest thing to do is say so once, loudly, and keep the record.
fn check_own_binary(app: &AppHandle) {
    static REPORTED: AtomicBool = AtomicBool::new(false);
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if exe.exists() {
        REPORTED.store(false, Ordering::Relaxed);
        return;
    }
    if REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    log::write(&format!(
        "our own program is gone from {} — hooks will do nothing until it is reinstalled",
        exe.display()
    ));
    let _ = app.emit(
        "pipsqueak://notice",
        "Pipsqueak has been removed from disk. Reinstall it: the hooks now point at a program that is not there.",
    );
}

/// Notices when the page has stopped running, and reloads it.
///
/// The failure this exists for: the window is present, visible, correctly
/// positioned and painting nothing. Hiding and showing it does not help,
/// because an empty page is empty either way, and there is no error anywhere
/// to suggest what happened. The frontend reports its clickable regions on
/// every render, so silence is the signal.
fn watch_frontend(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    // A hidden window is supposed to be quiet.
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    let frontend = app.state::<Frontend>();
    let last_seen = frontend.last_seen_ms.load(Ordering::Relaxed);
    // Nothing heard yet at all: the page may still be loading.
    if last_seen == 0 {
        return;
    }
    let now = state::now_ms();
    if now.saturating_sub(last_seen) < FRONTEND_SILENT_MS {
        return;
    }
    // Silent is not dead. WebView2 throttles timers for an occluded window
    // down to about once a minute, and a page that stopped rendering because
    // of that is perfectly healthy — reloading it here is what made the
    // overlay blank and repaint every minute under a fullscreen app. Events
    // are not throttled, so ask, and only reload a page that will not answer.
    let pinged = frontend.last_ping_ms.load(Ordering::Relaxed);
    if pinged <= last_seen {
        frontend.last_ping_ms.store(now, Ordering::Relaxed);
        let _ = app.emit("pipsqueak://ping", ());
        return;
    }
    if now.saturating_sub(pinged) < PING_GRACE_MS {
        return;
    }
    if now.saturating_sub(frontend.last_rescue_ms.load(Ordering::Relaxed)) < RESCUE_INTERVAL_MS {
        return;
    }
    frontend.last_rescue_ms.store(now, Ordering::Relaxed);
    log::write(&format!(
        "frontend silent for {}s and not answering pings, reloading the page",
        now.saturating_sub(last_seen) / 1000
    ));
    reload_frontend(app);
}

/// Reloads the page in the overlay window.
pub fn reload_frontend(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    match window.url() {
        Ok(url) => {
            if let Err(err) = window.navigate(url) {
                log::write(&format!("reload failed: {err}"));
            }
        }
        Err(err) => log::write(&format!("reload could not read the current url: {err}")),
    }
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
    let Some(action) = serde_json::from_str::<Value>(&raw).ok().and_then(|value| {
        value
            .get("action")
            .and_then(Value::as_str)
            .map(String::from)
    }) else {
        let _ = fs::remove_file(&path);
        return;
    };
    // A pet switch is delivered as an event, and an event emitted before the
    // page's listeners attach is dropped while the CLI has already printed
    // "Pet switched". Leave the command for a later tick instead.
    if action.starts_with("pet:")
        && app
            .state::<Frontend>()
            .last_seen_ms
            .load(Ordering::Relaxed)
            == 0
    {
        return;
    }
    let _ = fs::remove_file(&path);

    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    match action.as_str() {
        "show" => {
            let _ = window.show();
            // "off then on" is what everyone tries first when the overlay is
            // misbehaving, so it must fix a dead page — but reloading a
            // healthy one is itself a visible blank-and-repaint. Ask the page
            // instead; the watchdog reloads it if it stays silent.
            let frontend = app.state::<Frontend>();
            let last_seen = frontend.last_seen_ms.load(Ordering::Relaxed);
            let now = state::now_ms();
            if last_seen != 0 && now.saturating_sub(last_seen) >= FRONTEND_SILENT_MS {
                frontend.last_ping_ms.store(now, Ordering::Relaxed);
                let _ = app.emit("pipsqueak://ping", ());
            }
        }
        "hide" => {
            let _ = window.hide();
        }
        "toggle" => toggle_window(app),
        "quit" => shutdown(app),
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
    let reload = MenuItem::with_id(app, "reload", "Reload the overlay", true, None::<&str>)?;
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
            &reload,
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
                // Tell the frontend, or its own copy of the config silently
                // reverts this on its next save.
                let _ = app.emit("pipsqueak://config", &config);
            }
            "quiet" => {
                let mut config = load_config();
                config.quiet = !config.quiet;
                let _ = state::save_config(&config);
                if config.quiet {
                    attention::clear(app);
                }
                let _ = app.emit("pipsqueak://config", &config);
            }
            "reload" => {
                log::write("reload requested from the tray");
                reload_frontend(app);
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
            "quit" => shutdown(app),
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
    if let Ok(mut guard) = app.state::<TrayToggles>().0.lock() {
        *guard = Some((click_through, quiet));
    }
    Ok(())
}

pub fn run() {
    // One overlay at a time. A second instance stacks a second pet on the
    // first and both drain the same command file, so show/hide lands on a
    // random one. If somebody is already home, wake them instead.
    if crate::control::is_running() {
        let _ = crate::control::run(Some("show".to_string()));
        return;
    }
    let _ = fs::create_dir_all(sessions_dir());
    let _ = fs::create_dir_all(pets_dir());
    let _ = fs::create_dir_all(root());
    state::sweep();

    tauri::Builder::default()
        .manage(Interactive::default())
        .manage(Attention::default())
        .manage(Binding::default())
        .manage(Frontend::default())
        .manage(ClickThrough::default())
        .manage(Emitted::default())
        .manage(TrayToggles::default())
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
            hide_window,
            frontend_pong,
            frontend_ready
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
            log::write(&format!(
                "started {} pid {} from {}",
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
                std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "an unknown path".into())
            ));

            // Whatever chord we actually got, so the menu can show it rather
            // than the one that was asked for.
            match hotkey::start(handle.clone()) {
                Ok(chord) => {
                    log::write(&format!("hotkey registered as {chord}"));
                    if let Ok(mut held) = handle.state::<Binding>().0.lock() {
                        *held = chord;
                    }
                }
                Err(reason) => log::write(&format!("no global hotkey: {reason}")),
            }

            ensure_autostart(&config);

            // The heartbeat must exist before anything can race us: the
            // poller's first tick used to be the first beat, and a `control`
            // call landing in that gap launched a second overlay.
            beat();
            spawn_poller(handle.clone());
            spawn_hit_test(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start Pipsqueak");
}
