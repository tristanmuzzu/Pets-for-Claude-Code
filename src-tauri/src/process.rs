//! Answering one question: is the agent that owns this session still running?
//!
//! A session file is written by a hook process that exits immediately, so the
//! file alone can never tell us whether Claude Code is still there. If Claude
//! Code crashes or is force-quit it sends no `SessionEnd`, and the card would
//! otherwise keep claiming work is happening.
//!
//! Identity is `(pid, creation time)`, never the pid alone. Windows reuses
//! pids freely, and a reused pid is the case that would resurrect a dead
//! session as alive again.
//!
//! Dependency-free, in the same spirit as `desktop.rs`.

/// The agent process that owns the current hook invocation, as
/// `(pid, creation time in 100ns ticks)`.
///
/// Claude Code spawns command hooks through a shell on Windows, so the direct
/// parent is usually `powershell.exe`. Walks up past known shells to the first
/// process that could plausibly be the agent itself.
pub fn owner() -> Option<(u32, u64)> {
    #[cfg(windows)]
    {
        windows_impl::owner()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// True when the process is still the same one we recorded.
///
/// Unknown identities (`pid == 0`) are reported alive: not knowing is not the
/// same as knowing it is gone, and the caller falls back to an age cutoff.
pub fn is_alive(pid: u32, created: u64) -> bool {
    if pid == 0 {
        return true;
    }
    #[cfg(windows)]
    {
        windows_impl::is_alive(pid, created)
    }
    #[cfg(not(windows))]
    {
        let _ = created;
        true
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    type Bool = i32;

    const MAX_PATH: usize = 260;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    /// Claude Code is several levels up when a shell and a console host sit in
    /// between; deeper than this and we are walking into service territory.
    const MAX_DEPTH: usize = 8;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        pid: u32,
        default_heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_pid: u32,
        pri_class_base: i32,
        flags: u32,
        exe_file: [u16; MAX_PATH],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn ticks(self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
        fn OpenProcess(access: u32, inherit: Bool, pid: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, code: *mut u32) -> Bool;
        fn GetProcessTimes(
            process: Handle,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> Bool;
    }

    /// Shells, console hosts, and ourselves: processes that can sit between the
    /// agent and its hook without being the agent.
    fn is_passthrough(name: &str) -> bool {
        matches!(
            name,
            "powershell.exe"
                | "pwsh.exe"
                | "cmd.exe"
                | "conhost.exe"
                | "openconsole.exe"
                | "bash.exe"
                | "sh.exe"
                | "zsh.exe"
                | "dash.exe"
                | "busybox.exe"
                | "wsl.exe"
                | "wslhost.exe"
                | "pipsqueak.exe"
        )
    }

    struct Entry {
        parent: u32,
        name: String,
    }

    /// One snapshot of the whole process table, as `pid -> (parent, name)`.
    ///
    /// Taken once and walked in memory: asking the OS per level would let a
    /// process exit mid-walk and hand us an unrelated reused pid.
    fn snapshot() -> Vec<(u32, Entry)> {
        let mut out = Vec::new();
        let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return out;
        }
        let mut entry = ProcessEntry32W {
            size: std::mem::size_of::<ProcessEntry32W>() as u32,
            usage: 0,
            pid: 0,
            default_heap_id: 0,
            module_id: 0,
            threads: 0,
            parent_pid: 0,
            pri_class_base: 0,
            flags: 0,
            exe_file: [0; MAX_PATH],
        };
        let mut ok = unsafe { Process32FirstW(handle, &mut entry) };
        while ok != 0 {
            let len = entry
                .exe_file
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(MAX_PATH);
            out.push((
                entry.pid,
                Entry {
                    parent: entry.parent_pid,
                    name: String::from_utf16_lossy(&entry.exe_file[..len]).to_lowercase(),
                },
            ));
            ok = unsafe { Process32NextW(handle, &mut entry) };
        }
        unsafe { CloseHandle(handle) };
        out
    }

    fn created_at(pid: u32) -> Option<u64> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        let ok =
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            None
        } else {
            Some(creation.ticks())
        }
    }

    pub fn owner() -> Option<(u32, u64)> {
        let table = snapshot();
        if table.is_empty() {
            return None;
        }
        let find = |pid: u32| table.iter().find(|(id, _)| *id == pid).map(|(_, e)| e);

        let mut pid = unsafe { GetCurrentProcessId() };
        for _ in 0..MAX_DEPTH {
            let parent = find(pid)?.parent;
            if parent == 0 || parent == pid {
                return None;
            }
            let parent_entry = find(parent)?;
            if !is_passthrough(&parent_entry.name) {
                return created_at(parent).map(|created| (parent, created));
            }
            pid = parent;
        }
        None
    }

    pub fn is_alive(pid: u32, created: u64) -> bool {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            // "I may not look at it" and "it is gone" both land here, and
            // getting that wrong deletes a live session. The process table is
            // readable either way, so ask it instead of guessing. A pid still
            // listed is alive; we just cannot check it for reuse, which the
            // age cutoffs in `state::sweep` cover.
            return snapshot().iter().any(|(id, _)| *id == pid);
        }
        let mut code: u32 = 0;
        let running = unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == STILL_ACTIVE;
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        let timed =
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
                != 0;
        unsafe { CloseHandle(handle) };

        if !running {
            return false;
        }
        // A pid can be reused within the sweep interval. Same pid, different
        // start time, different process.
        if timed && created != 0 && creation.ticks() != created {
            return false;
        }
        true
    }
}
