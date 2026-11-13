//! Test helper child for `stop_signal`: it installs the handlers (twice, as
//! `install` must allow), prints `ready`, then waits for a stop signal.
//!
//! With no argument it prints `stopped` and exits 0 once a signal arrived.
//! With `--hold` it prints `received` once a signal arrived and never exits on
//! its own, so only a second signal can end it.

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use ralphy_proc_util::stop_signal;

fn main() -> ExitCode {
    let hold = std::env::args().nth(1).as_deref() == Some("--hold");
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

fn say(line: &str) {
    writeln!(std::io::stdout(), "{line}").expect("the test child can write stdout");
}
