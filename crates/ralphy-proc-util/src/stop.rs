//! The cooperative stop: ask a child's process group to end, give it a grace to
//! exit with its output, and leave the rest to [`crate::kill_tree`].

use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// Spawn `cmd`'s child so [`send_stop_signal`] can reach it: in its own process
/// group; on Windows also on this process's console, or on a hidden console of
/// its own when this process has none. Call it before `spawn`, after any other
/// change to the Windows creation flags: it replaces them.
pub fn stoppable_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let hidden = if has_console() { 0 } else { CREATE_NO_WINDOW };
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | hidden);
    }
}

/// Ask the process group of `child`, spawned with [`stoppable_group`], to end:
/// `SIGTERM` on Unix, `CTRL_BREAK_EVENT` on Windows.
#[cfg_attr(
    unix,
    expect(unsafe_code, reason = "FFI: libc::kill on the process group")
)]
pub fn send_stop_signal(child: &Child) -> std::io::Result<()> {
    let pid = child.id();
    #[cfg(unix)]
    {
        // A pgid of 1 or 0 would reach every process or our own group.
        if pid <= 1 {
            return Err(std::io::Error::other("refusing to signal process group 1"));
        }
        // SAFETY: kill(2) takes plain integers and touches no memory.
        if unsafe { libc::kill(-pid.cast_signed(), libc::SIGTERM) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        windows::send_ctrl_break(pid)
    }
}

/// Send the stop signal to `child`, then wait up to `grace` for it to exit.
/// `None` when the send failed (logged) or the child outlived the grace.
pub fn stop_within(child: &mut Child, grace: Duration) -> Option<ExitStatus> {
    if let Err(e) = send_stop_signal(child) {
        tracing::warn!(error = %e, "could not ask the child to stop; killing its tree now");
        return None;
    }
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => {}
            Ok(None) => return None,
            Err(e) => {
                tracing::debug!(error = %e, "polling the stopped child failed");
                return None;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Make this process exit with `code` when [`send_stop_signal`] reaches it, for
/// a helper child in a test of the stop. Call it before this process starts a
/// thread. On Windows the default stays: the process exits on
/// `CTRL_BREAK_EVENT` with a status code of its own.
pub fn exit_on_stop_signal(code: i32) {
    #[cfg(unix)]
    unix::exit_on_sigterm(code);
    #[cfg(not(unix))]
    let _: i32 = code;
}

#[cfg(unix)]
#[expect(unsafe_code, reason = "FFI: block SIGTERM and wait for it on a thread")]
mod unix {
    /// Block `SIGTERM` in this thread, which every later thread inherits, and
    /// end the process with `code` from a thread that waits for it.
    pub(super) fn exit_on_sigterm(code: i32) {
        // SAFETY: the set is zeroed, then initialised by sigemptyset before use;
        // each call takes pointers to locals that outlive it.
        let set = unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&raw mut set);
            libc::sigaddset(&raw mut set, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
            set
        };
        std::thread::spawn(move || {
            let mut signal: libc::c_int = 0;
            // SAFETY: sigwait reads the set and writes the int, both locals here;
            // _exit ends the process at once.
            unsafe {
                libc::sigwait(&raw const set, &raw mut signal);
                libc::_exit(code)
            }
        });
    }
}

#[cfg(windows)]
fn has_console() -> bool {
    windows::has_console()
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "FFI: the console API that sends a console control event"
)]
mod windows {
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleProcessList, GetStdHandle,
        SetStdHandle, CTRL_BREAK_EVENT, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    const STD_HANDLES: [STD_HANDLE; 3] = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];

    pub(super) fn has_console() -> bool {
        let mut pids = [0u32; 1];
        // SAFETY: the buffer and its length match; the call only writes into it.
        unsafe { GetConsoleProcessList(pids.as_mut_ptr(), 1) != 0 }
    }

    /// `CTRL_BREAK_EVENT` reaches only the processes on the caller's console, so
    /// a process with no console attaches to the child's for the send. Its own
    /// standard handles are put back after, because the attach can replace them.
    pub(super) fn send_ctrl_break(pid: u32) -> std::io::Result<()> {
        if has_console() {
            return generate(pid);
        }
        // SAFETY: GetStdHandle takes a constant and only returns a value.
        let saved = STD_HANDLES.map(|which| unsafe { GetStdHandle(which) });
        // SAFETY: AttachConsole takes a pid and touches no memory of ours.
        if unsafe { AttachConsole(pid) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sent = generate(pid);
        // SAFETY: FreeConsole detaches the console attached just above; SetStdHandle
        // writes back the handles this process held before the attach.
        unsafe {
            FreeConsole();
            for (which, handle) in STD_HANDLES.into_iter().zip(saved) {
                SetStdHandle(which, handle);
            }
        }
        sent
    }

    fn generate(pid: u32) -> std::io::Result<()> {
        // SAFETY: plain integers; the group is the child's, made by
        // CREATE_NEW_PROCESS_GROUP, so this process is not in it.
        if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
