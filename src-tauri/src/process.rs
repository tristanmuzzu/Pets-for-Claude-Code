//! Answering one question: is the agent that owns this session still running?
//!
//! A session file is written by a hook process that exits immediately, so the
//! file alone can never tell us whether Claude Code is still there. If Claude
//! Code crashes or is force-quit it sends no `SessionEnd`, and the card would
//! otherwise keep claiming work is happening.
//!
//! Identity is `(pid, creation time)`, never the pid alone. Every OS reuses
//! pids, and a reused pid is the case that would resurrect a dead session as
//! alive again. The creation time is opaque and platform-specific: 100ns
//! ticks on Windows, clock ticks since boot on Linux, seconds since the epoch
//! on macOS. It is only ever compared for equality against a value this same
//! machine recorded.
//!
//! Until this file had a Linux arm the pet could not tell, there, that a
//! session had died. Every session ran with `pid == 0`, "unknown" read as
//! alive, and a card that had asked a question kept asking it for twelve hours
//! after a reboot had taken the process it was asking for.
//!
//! Dependency-free, in the same spirit as `desktop.rs`.

/// The agent process that owns the current hook invocation, as
/// `(pid, creation time)`.
///
/// Claude Code may spawn command hooks through a shell (`powershell.exe` on
/// Windows, `sh -c` wherever a hook is written as a command line), so the
/// direct parent is not always the agent. Walks up past known shells to the
/// first process that could plausibly be the agent itself.
pub fn owner() -> Option<(u32, u64)> {
    #[cfg(windows)]
    {
        windows_impl::owner()
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::owner()
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::owner()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
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
    #[cfg(target_os = "linux")]
    {
        linux_impl::is_alive(pid, created)
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::is_alive(pid, created)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = created;
        true
    }
}

/// When this machine last booted, as milliseconds since the epoch.
///
/// A session file updated before that moment was written by a process that
/// cannot exist any more, whatever the file says and whether or not the hook
/// ever learned its pid. This is the one liveness fact that needs no process
/// table at all, so it holds on every platform and for files written by
/// builds that recorded no identity. `None` when the platform cannot say,
/// which the caller treats as "no opinion", never as "nothing has booted".
pub fn boot_time_ms() -> Option<u64> {
    #[cfg(windows)]
    {
        windows_impl::boot_time_ms()
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::boot_time_ms()
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::boot_time_ms()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Shells and launchers that can sit between the agent and its hook without
/// being the agent. Bare names, as the kernel reports them.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn is_passthrough_unix(name: &str) -> bool {
    matches!(
        name,
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "ksh"
            | "busybox"
            | "env"
            | "timeout"
            | "pipsqueak"
            | "start-pipsqueak"
    )
}

/// Claude Code is a few levels up when a shell sits in between; deeper than
/// this and we are walking into the terminal, the desktop session, or init.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const MAX_DEPTH_UNIX: usize = 8;

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{is_passthrough_unix, MAX_DEPTH_UNIX};
    use std::fs;
    use std::path::Path;

    fn read(pid: u32, file: &str) -> Option<String> {
        fs::read_to_string(format!("/proc/{pid}/{file}")).ok()
    }

    fn parent_of(pid: u32) -> Option<u32> {
        read(pid, "status")?
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .and_then(|value| value.trim().parse().ok())
    }

    fn comm_of(pid: u32) -> Option<String> {
        read(pid, "comm").map(|name| name.trim().to_string())
    }

    /// `(state, start time)` out of one `/proc/<pid>/stat` line.
    ///
    /// Field 2 is the command name in parentheses and may itself contain
    /// spaces and parentheses, so the fields are counted from the *last*
    /// closing parenthesis rather than split from the front. After it, field
    /// 3 is the state and field 22 is the start time in clock ticks since
    /// boot — a value that identifies this incarnation of the pid for as long
    /// as the machine stays up, which is exactly the span a pid can be reused
    /// within.
    pub(super) fn parse_stat(stat: &str) -> Option<(char, u64)> {
        let after = &stat[stat.rfind(')')? + 1..];
        let mut fields = after.split_whitespace();
        let state = fields.next()?.chars().next()?;
        let started = fields.nth(22 - 4)?.parse().ok()?;
        Some((state, started))
    }

    fn stat_of(pid: u32) -> Option<(char, u64)> {
        parse_stat(&read(pid, "stat")?)
    }

    pub fn owner() -> Option<(u32, u64)> {
        let mut pid = std::process::id();
        for _ in 0..MAX_DEPTH_UNIX {
            let parent = parent_of(pid)?;
            // pid 1 owns every orphan; a hook whose chain reaches it has lost
            // its agent, and recording init would make the session immortal.
            if parent <= 1 || parent == pid {
                return None;
            }
            if !is_passthrough_unix(&comm_of(parent)?) {
                return stat_of(parent).map(|(_, started)| (parent, started));
            }
            pid = parent;
        }
        None
    }

    pub fn is_alive(pid: u32, created: u64) -> bool {
        match stat_of(pid) {
            // A zombie is a process that has exited and not been reaped. It
            // runs nothing, and it will never write another hook.
            Some(('Z' | 'X' | 'x', _)) => false,
            Some((_, started)) => created == 0 || started == created,
            // Not readable is two different answers. No directory at all is
            // gone. A directory we may not read (a hardened `hidepid` mount)
            // is not knowing, and not knowing is not knowing it is gone.
            None => Path::new(&format!("/proc/{pid}")).exists(),
        }
    }

    pub fn boot_time_ms() -> Option<u64> {
        fs::read_to_string("/proc/stat")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("btime "))
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|seconds| seconds * 1000)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The command name is user-controlled and may contain the separator.
        #[test]
        fn stat_fields_are_counted_from_the_last_parenthesis() {
            let line = "4242 (a (weird) name) S 1 4242 4242 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 179528 1000 200 18446744073709551615";
            assert_eq!(parse_stat(line), Some(('S', 179528)));
            assert_eq!(parse_stat("garbage"), None);
        }

        /// The process running this test is alive under its own identity and
        /// dead under any other, which is the whole contract.
        #[test]
        fn a_process_is_alive_only_as_the_incarnation_recorded() {
            let me = std::process::id();
            let (_, started) = stat_of(me).expect("/proc/self/stat is readable");
            assert!(is_alive(me, started));
            assert!(
                is_alive(me, 0),
                "unknown creation time falls back to the pid"
            );
            assert!(
                !is_alive(me, started + 1),
                "same pid, different incarnation"
            );
            // The largest pid Linux hands out is 2^22; this one never exists.
            assert!(!is_alive(u32::MAX - 1, 0));
        }

        /// A test binary has a parent (the cargo test harness or a shell), so
        /// the walk must find something and it must be a live process.
        #[test]
        fn the_owner_walk_finds_a_live_ancestor() {
            let Some((pid, started)) = owner() else {
                // Sandboxed builds can run as a direct child of init.
                return;
            };
            assert_ne!(pid, std::process::id());
            assert!(is_alive(pid, started));
        }

        #[test]
        fn boot_time_is_in_the_past_and_after_the_epoch() {
            let boot = boot_time_ms().expect("/proc/stat has btime");
            let now = crate::state::now_ms();
            assert!(
                boot > 946_684_800_000,
                "boot {boot} is before the year 2000"
            );
            assert!(boot <= now, "boot {boot} is after now {now}");
        }
    }
}

/// macOS, by inspection only: no hardware has run this. Every answer is
/// checked against something the kernel echoes back (the pid it was asked
/// about), so a wrong guess about a struct layout degrades to "unknown"
/// rather than to a wrong process.
#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{is_passthrough_unix, MAX_DEPTH_UNIX};
    use std::ffi::c_void;

    const PROC_PIDTBSDINFO: i32 = 3;
    const MAXCOMLEN: usize = 16;
    /// `SZOMB` in `<sys/proc.h>`.
    const SZOMB: u32 = 5;

    /// `struct proc_bsdinfo` from `<sys/proc_info.h>`, 136 bytes.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcBsdInfo {
        flags: u32,
        status: u32,
        xstatus: u32,
        pid: u32,
        ppid: u32,
        uid: u32,
        gid: u32,
        ruid: u32,
        rgid: u32,
        svuid: u32,
        svgid: u32,
        rfu_1: u32,
        comm: [u8; MAXCOMLEN],
        name: [u8; 2 * MAXCOMLEN],
        nfiles: u32,
        pgid: u32,
        pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        nice: i32,
        start_tvsec: u64,
        start_tvusec: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Timeval {
        sec: i64,
        usec: i32,
    }

    #[link(name = "proc")]
    extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut c_void,
            buffersize: i32,
        ) -> i32;
    }

    extern "C" {
        fn sysctlbyname(
            name: *const u8,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> i32;
    }

    fn info(pid: u32) -> Option<ProcBsdInfo> {
        let mut out: ProcBsdInfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<ProcBsdInfo>() as i32;
        let got = unsafe {
            proc_pidinfo(
                pid as i32,
                PROC_PIDTBSDINFO,
                0,
                &mut out as *mut ProcBsdInfo as *mut c_void,
                size,
            )
        };
        // The kernel writes the pid it answered about; anything else means
        // the layout above is not what this OS version uses.
        (got == size && out.pid == pid).then_some(out)
    }

    fn comm(entry: &ProcBsdInfo) -> String {
        let len = entry.comm.iter().position(|b| *b == 0).unwrap_or(MAXCOMLEN);
        String::from_utf8_lossy(&entry.comm[..len]).into_owned()
    }

    pub fn owner() -> Option<(u32, u64)> {
        let mut pid = std::process::id();
        for _ in 0..MAX_DEPTH_UNIX {
            let parent = info(pid)?.ppid;
            if parent <= 1 || parent == pid {
                return None;
            }
            let entry = info(parent)?;
            if !is_passthrough_unix(&comm(&entry)) {
                return Some((parent, entry.start_tvsec));
            }
            pid = parent;
        }
        None
    }

    pub fn is_alive(pid: u32, created: u64) -> bool {
        match info(pid) {
            Some(entry) if entry.status == SZOMB => false,
            Some(entry) => created == 0 || entry.start_tvsec == created,
            // ESRCH and EPERM both land here. Ask the cheaper question: does
            // the pid exist at all? `kill(pid, 0)` says so without a layout.
            None => (unsafe { kill(pid as i32, 0) } == 0) || errno_is_eperm(),
        }
    }

    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
        fn __error() -> *mut i32;
    }

    fn errno_is_eperm() -> bool {
        const EPERM: i32 = 1;
        unsafe { *__error() == EPERM }
    }

    pub fn boot_time_ms() -> Option<u64> {
        let mut value = Timeval::default();
        let mut len = std::mem::size_of::<Timeval>();
        let name = b"kern.boottime\0";
        let ok = unsafe {
            sysctlbyname(
                name.as_ptr(),
                &mut value as *mut Timeval as *mut c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (ok == 0 && value.sec > 0).then(|| value.sec as u64 * 1000)
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
        fn GetTickCount64() -> u64;
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
    ///
    /// `None` when the snapshot could not be taken at all. That is a different
    /// answer from an empty table: "could not look" must never read as "every
    /// process is gone", because the caller deletes sessions on that verdict.
    fn snapshot() -> Option<Vec<(u32, Entry)>> {
        let mut out = Vec::new();
        let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return None;
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
        // A real process table always contains at least this process; an
        // empty walk means the enumeration itself failed.
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
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

    /// Now minus the uptime. Fast Startup makes a shutdown a hibernation, so
    /// the uptime can span what the user calls a reboot; that only makes this
    /// later than the truth, which errs on keeping a session, never on
    /// killing a live one.
    pub fn boot_time_ms() -> Option<u64> {
        let uptime = unsafe { GetTickCount64() };
        (uptime > 0).then(|| crate::state::now_ms().saturating_sub(uptime))
    }

    pub fn owner() -> Option<(u32, u64)> {
        let table = snapshot()?;
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
            // age cutoffs in `state::sweep` cover. If even the snapshot fails,
            // not knowing is not knowing it is gone.
            return match snapshot() {
                Some(table) => table.iter().any(|(id, _)| *id == pid),
                None => true,
            };
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
