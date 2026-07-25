//! Is a pid a living process? One syscall per platform, injectable at every
//! call site so tests never need a second process.
//!
//! Shared by the run lock's stale-pid classifier (`ralphy-cli::runlock`) and
//! the run-snapshot reader's orphan sweep (ADR-0047 §7/§10) — one classifier,
//! not two.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_is_alive_detects_own_process() {
        assert!(pid_is_alive(std::process::id()));
    }
}
