//! Small platform integrations: bring another window forward, make a sound,
//! and start with the machine.
//!
//! Deliberately dependency-free — a handful of `user32` calls and the `reg`
//! command are cheaper than pulling a Windows crate graph into an 8 MB app.

/// Builds a `Command` that never flashes a console window.
///
/// Without this, every `reg`/`explorer` call from the tray menu pops a black
/// rectangle on screen for a frame — which is exactly the thing a quiet desktop
/// overlay must not do.
pub fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Focuses the first visible window whose title contains one of `fragments`
/// (matched case-insensitively, in order of preference).
///
/// Best-effort: Windows refuses foreground changes from background processes
/// under some conditions, so a `false` return is normal and not an error.
pub fn focus_window_titled(fragments: &[String]) -> bool {
    #[cfg(windows)]
    {
        windows_impl::focus(fragments)
    }
    #[cfg(not(windows))]
    {
        let _ = fragments;
        false
    }
}

pub fn alert() {
    #[cfg(windows)]
    unsafe {
        // MB_ICONASTERISK — the quiet "something happened" sound.
        windows_impl::MessageBeep(0x00000040);
    }
}

pub fn autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        quiet_command("reg")
            .args(["query", RUN_KEY, "/v", "Pipsqueak"])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        let status = if enabled {
            quiet_command("reg")
                .args([
                    "add",
                    RUN_KEY,
                    "/v",
                    "Pipsqueak",
                    "/t",
                    "REG_SZ",
                    "/d",
                    &exe,
                    "/f",
                ])
                .status()
        } else {
            quiet_command("reg")
                .args(["delete", RUN_KEY, "/v", "Pipsqueak", "/f"])
                .status()
        };
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("reg exited with {status}")),
            Err(err) => Err(err.to_string()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("autostart is only implemented on Windows so far".to_string())
    }
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;

    type Hwnd = *mut c_void;
    type Bool = i32;

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(callback: extern "system" fn(Hwnd, isize) -> Bool, data: isize) -> Bool;
        fn GetWindowTextW(window: Hwnd, buffer: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(window: Hwnd) -> Bool;
        fn SetForegroundWindow(window: Hwnd) -> Bool;
        fn ShowWindow(window: Hwnd, command: i32) -> Bool;
        pub fn MessageBeep(kind: u32) -> Bool;
    }

    const SW_RESTORE: i32 = 9;

    struct Search {
        fragments: Vec<String>,
        /// Index of the matched fragment, so an earlier (more specific)
        /// fragment always wins over a later one.
        best_rank: usize,
        best: Hwnd,
    }

    extern "system" fn visit(window: Hwnd, data: isize) -> Bool {
        // Safety: `data` is the pointer we handed to EnumWindows below, which
        // outlives the enumeration.
        let search = unsafe { &mut *(data as *mut Search) };
        if unsafe { IsWindowVisible(window) } == 0 {
            return 1;
        }
        let mut buffer = [0u16; 512];
        let len = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
        if len <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&buffer[..len as usize]).to_lowercase();
        for (rank, fragment) in search.fragments.iter().enumerate() {
            if rank >= search.best_rank {
                break;
            }
            if title.contains(fragment.as_str()) {
                search.best_rank = rank;
                search.best = window;
                break;
            }
        }
        // Stop early only on a first-choice match.
        if search.best_rank == 0 {
            0
        } else {
            1
        }
    }

    pub fn focus(fragments: &[String]) -> bool {
        let mut search = Search {
            fragments: fragments
                .iter()
                .map(|f| f.to_lowercase())
                .filter(|f| !f.is_empty())
                .collect(),
            best_rank: usize::MAX,
            best: std::ptr::null_mut(),
        };
        if search.fragments.is_empty() {
            return false;
        }
        unsafe { EnumWindows(visit, &mut search as *mut Search as isize) };
        if search.best.is_null() {
            return false;
        }
        unsafe {
            ShowWindow(search.best, SW_RESTORE);
            SetForegroundWindow(search.best) != 0
        }
    }
}
