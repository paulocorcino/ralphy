//! Is a pid a living process, and which program is it running? One syscall per
//! platform, injectable at every call site so tests never need a second process.
//!
//! Shared by the run lock's stale-pid classifier (`ralphy-cli::runlock`) and
//! the run-snapshot reader's orphan sweep (ADR-0047 §7/§10) — one classifier,
//! not two.
//!
//! [`exe_of_pid`] exists because liveness is not identity: a pid recorded before
//! a crash or a reboot is very likely alive again as something else, and a
//! caller about to end a process tree needs to know it is ending the right one
//! (ADR-0056 §8).

/// Production liveness predicate.
#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    // Signal 0 probes without sending: 0 = alive, EPERM = alive but not ours.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Production liveness predicate.
#[cfg(windows)]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // A process we can see but not open is still a live process —
            // conservative: never take over a lock we can't inspect.
            return std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_ACCESS_DENIED as i32);
        }
        // An exited process can still be opened while a handle to it is held
        // elsewhere; STILL_ACTIVE separates the two.
        let mut code: u32 = 0;
        let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        alive
    }
}

/// The executable a live pid is running, when the platform will say.
///
/// `None` means "cannot tell" — the process is gone, or the OS refused, or this
/// target has no cheap way to ask. A caller deciding whether to end a process
/// must treat `None` as "not proven mine", never as a match.
#[cfg(target_os = "linux")]
pub fn exe_of_pid(pid: u32) -> Option<std::path::PathBuf> {
    // `/proc/<pid>/exe` is the kernel's own answer and follows renames, so a
    // comparison against a recorded path is only meaningful up to a rename —
    // which is exactly the property the caller wants: same file, whatever it is
    // called now.
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// The executable a live pid is running, when the platform will say.
#[cfg(target_os = "macos")]
pub fn exe_of_pid(pid: u32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    // PROC_PIDPATHINFO_MAXSIZE
    let mut buf = vec![0u8; 4 * libc::PATH_MAX as usize];
    let written = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
        )
    };
    if written <= 0 {
        return None;
    }
    buf.truncate(written as usize);
    Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

/// The executable a live pid is running, when the platform will say.
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
pub fn exe_of_pid(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

/// The executable a live pid is running, when the platform will say.
#[cfg(windows)]
pub fn exe_of_pid(pid: u32) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(handle);
        if !ok {
            return None;
        }
        buf.truncate(len as usize);
        Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_is_alive_detects_own_process() {
        assert!(pid_is_alive(std::process::id()));
    }

    #[test]
    fn exe_of_pid_names_this_test_binary() {
        let seen = exe_of_pid(std::process::id()).expect("this process is running something");
        let own = std::env::current_exe().expect("current_exe");
        // Compare by file name: the two APIs can disagree on canonicalization
        // (extended-length prefixes on Windows, symlinked paths on Unix), and the
        // caller's question is "the same program", not "the same spelling".
        assert_eq!(seen.file_name(), own.file_name(), "{seen:?} vs {own:?}");
    }

    #[test]
    fn exe_of_pid_says_nothing_about_a_pid_that_is_not_running() {
        // Nothing is proven about a dead pid, and a caller must read that as
        // "not mine" rather than as a match.
        assert_eq!(exe_of_pid(424_242), None);
    }
}
