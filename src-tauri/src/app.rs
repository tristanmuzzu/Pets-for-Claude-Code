//! The overlay: a transparent, always-on-top window that polls session state
//! and animates the pet.

use crate::install;
use crate::state::{
    self, codex_pets_dir, load_config, pets_dir, read_sessions, root, sessions_dir, Config, Session,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State};

const POLL_INTERVAL: Duration = Duration::from_millis(300);
const HIT_TEST_INTERVAL: Duration = Duration::from_millis(60);
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
}

#[derive(Default)]
struct Interactive(Mutex<Vec<Rect>>);

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
    read_sessions()
}

#[tauri::command]
fn get_config() -> Config {
    load_config()
}

#[tauri::command]
fn set_config(app: AppHandle, config: Config) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_ignore_cursor_events(config.click_through);
    }
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
    let mut pets = vec![PetInfo {
        id: "pip".into(),
        display_name: "Pip".into(),
        description: "An ember-fox with a status wisp.".into(),
        source: "builtin",
    }];
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
    if id == "pip" {
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

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
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
    let mut command = {
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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
        let _ = window.set_position(PhysicalPosition::new(x, y));
        return;
    }
    // Default: bottom-right, clear of the Windows taskbar.
    if let (Ok(Some(monitor)), Ok(size)) = (window.primary_monitor(), window.outer_size()) {
        let area = monitor.size();
        let x = area.width.saturating_sub(size.width + 24) as i32;
        let y = area.height.saturating_sub(size.height + 72) as i32;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
}

/// Keeps the window click-through except over the pet and its bubble.
///
/// A transparent overlay otherwise swallows every click inside its rectangle,
/// which would make the pet actively hostile to the thing it sits on top of.
fn spawn_hit_test(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last: Option<bool> = None;
        loop {
            std::thread::sleep(HIT_TEST_INTERVAL);
            let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
                continue;
            };
            if load_config().click_through {
                if last != Some(true) {
                    let _ = window.set_ignore_cursor_events(true);
                    last = Some(true);
                }
                continue;
            }
            let (Ok(cursor), Ok(origin), Ok(scale)) = (
                window.cursor_position(),
                window.outer_position(),
                window.scale_factor(),
            ) else {
                continue;
            };
            let local_x = (cursor.x - origin.x as f64) / scale;
            let local_y = (cursor.y - origin.y as f64) / scale;
            let over = app
                .state::<Interactive>()
                .0
                .lock()
                .map(|rects| rects.iter().any(|r| r.contains(local_x, local_y)))
                .unwrap_or(false);
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
        loop {
            let sessions = read_sessions();
            let encoded = serde_json::to_string(&sessions).unwrap_or_default();
            if encoded != last {
                last = encoded;
                let _ = app.emit("pipsqueak://sessions", &sessions);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    });
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
    let pets = MenuItem::with_id(app, "pets", "Open pets folder", true, None::<&str>)?;
    let hooks = MenuItem::with_id(app, "hooks", "Reinstall Claude Code hooks", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Pipsqueak", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle,
            &click_through,
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
                if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
                    let _ = window.set_ignore_cursor_events(config.click_through);
                }
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
    state::prune_stale();

    tauri::Builder::default()
        .manage(Interactive::default())
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
            quit
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let config = load_config();
            place_window(&handle, &config);
            if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                let _ = window.set_ignore_cursor_events(true);
                let _ = window.show();
            }
            build_tray(&handle)?;
            spawn_poller(handle.clone());
            spawn_hit_test(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start Pipsqueak");
}
