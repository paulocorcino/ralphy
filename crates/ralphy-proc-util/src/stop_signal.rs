//! The stop signals of `ralphy run` (ADR-0054, amendment of 2026-11-13): the
//! first one only raises a flag that the run reads, a later one ends the process.
//!
//! No signal is ignored or blocked here: a child inherits both through exec.
//! A signal the process already ignores keeps its action.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Set by the handler on the first stop signal; the handler touches nothing else.
static RECEIVED: AtomicBool = AtomicBool::new(false);

/// Makes [`install`] idempotent: each `SetConsoleCtrlHandler` call adds one more
/// entry to the process's handler list.
static INSTALLED: Mutex<bool> = Mutex::new(false);

/// Handle the stop signals in this process: SIGTERM, SIGINT and SIGHUP on Unix,
/// the console control events on Windows. Calling it again does nothing.
pub fn install() -> std::io::Result<()> {
    let mut installed = INSTALLED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !*installed {
        platform::install()?;
        *installed = true;
    }
    Ok(())
}

/// Has a stop signal reached this process since [`install`]?
pub fn received() -> bool {
    RECEIVED.load(Ordering::Acquire)
}

/// Record a stop signal; `true` when an earlier one was recorded already.
fn record() -> bool {
    RECEIVED.swap(true, Ordering::AcqRel)
}

#[cfg(unix)]
#[expect(unsafe_code, reason = "FFI: sigaction for the stop signals")]
mod platform {
    const SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

    /// Runs in signal context: an atomic swap, then only async-signal-safe calls.
    extern "C" fn on_signal(signal: libc::c_int) {
        if super::record() {
            // SAFETY: signal(2) and raise(3) are async-signal-safe and take
            // plain integers. The signal stays blocked until this handler
            // returns, then the default action ends the process.
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
        }
    }

    pub(super) fn install() -> std::io::Result<()> {
        let handler: extern "C" fn(libc::c_int) = on_signal;
        for signal in SIGNALS {
            // A signal the parent ignores stays ignored, as under `nohup`.
            if is_ignored(signal)? {
                continue;
            }
            // SAFETY: the struct is zeroed, then each field is written before
            // sigaction reads it; `sa_sigaction` is a pointer-sized field that
            // holds a handler address. sigaction only reads the struct.
            let rc = unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                (&raw mut action.sa_sigaction)
                    .cast::<extern "C" fn(libc::c_int)>()
                    .write(handler);
                libc::sigemptyset(&raw mut action.sa_mask);
                action.sa_flags = libc::SA_RESTART;
                libc::sigaction(signal, &raw const action, std::ptr::null_mut())
            };
            if rc != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Is the action of `signal` in this process `SIG_IGN`?
    fn is_ignored(signal: libc::c_int) -> std::io::Result<bool> {
        // SAFETY: a null new action makes sigaction only write the current one
        // into the zeroed struct.
        let (rc, old) = unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            let rc = libc::sigaction(signal, std::ptr::null(), &raw mut old);
            (rc, old)
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(old.sa_sigaction == libc::SIG_IGN)
    }
}

#[cfg(windows)]
#[expect(unsafe_code, reason = "FFI: SetConsoleCtrlHandler for the stop events")]
mod platform {
    use std::time::Duration;

    use windows_sys::core::BOOL;
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };

    /// Windows ends the process about 5 s after a console close; the run's
    /// teardown gets this long, and an earlier exit of the process cuts it short.
    const TEARDOWN_WAIT: Duration = Duration::from_millis(4_500);

    /// Runs on a thread the system starts for the event. `FALSE` passes the
    /// event on to the default handler, which ends the process.
    extern "system" fn on_event(event: u32) -> BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => BOOL::from(!super::record()),
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                super::record();
                std::thread::sleep(TEARDOWN_WAIT);
                1
            }
            _ => 0,
        }
    }

    pub(super) fn install() -> std::io::Result<()> {
        // SAFETY: the handler is a function that lives for the whole process.
        if unsafe { SetConsoleCtrlHandler(Some(on_event), 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
