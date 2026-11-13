//! Test helper child for `stop_signal`: it installs the handlers (twice, as
//! `install` must allow), prints `ready`, then waits for a stop signal.
//!
//! With no argument it prints `stopped` and exits 0 once a signal arrived.
//! With `--hold` it prints `received` once a signal arrived and never exits on
//! its own, so only a second signal can end it.
//! With `--ignore-hup` (Unix) it ignores SIGHUP before the install, as `nohup`
//! does, then acts as with no argument.

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use ralphy_proc_util::stop_signal;

fn main() -> ExitCode {
    let mode = std::env::args().nth(1);
    let hold = mode.as_deref() == Some("--hold");
    #[cfg(unix)]
    if mode.as_deref() == Some("--ignore-hup") {
        ignore_sighup();
    }
    for _ in 0..2 {
        if let Err(e) = stop_signal::install() {
            say(&format!("install failed: {e}"));
            return ExitCode::FAILURE;
        }
    }
    say("ready");
    while !stop_signal::received() {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !hold {
        say("stopped");
        return ExitCode::SUCCESS;
    }
    say("received");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(unix)]
#[expect(unsafe_code, reason = "FFI: signal(2) sets SIGHUP to SIG_IGN")]
fn ignore_sighup() {
    // SAFETY: signal(2) takes plain integers; no handler runs.
    let previous = unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
    assert_ne!(previous, libc::SIG_ERR, "the test child can ignore SIGHUP");
}

fn say(line: &str) {
    writeln!(std::io::stdout(), "{line}").expect("the test child can write stdout");
}
