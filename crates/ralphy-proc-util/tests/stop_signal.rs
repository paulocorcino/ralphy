//! The stop signals of `ralphy run`, sent to a real helper child by its own pid:
//! the first one is a stop the child reads, a second one ends it.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use ralphy_proc_util::{send_stop_signal, stoppable_group};

/// A spawned helper child, killed when the test ends early, and its stdout lines.
struct TestChild {
    child: Child,
    lines: Receiver<String>,
}

impl TestChild {
    fn spawn(hold: bool) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stop_signal_test_child"));
        if hold {
            cmd.arg("--hold");
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        stoppable_group(&mut cmd);
        let mut child = cmd.spawn().expect("the helper child starts");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let child = Self { child, lines };
        child.expect_line("ready");
        child
    }

    fn expect_line(&self, want: &str) {
        let line = self.lines.recv_timeout(Duration::from_secs(20));
        assert_eq!(line.as_deref(), Ok(want), "the child's next line");
    }

    /// The exit status, or `None` when the child still runs after `within`.
    fn wait_exit(&mut self, within: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("the child can be polled") {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn rest_of_output(&self) -> Vec<String> {
        self.lines
            .recv_timeout(Duration::from_secs(2))
            .into_iter()
            .chain(self.lines.try_iter())
            .collect()
    }
}

impl Drop for TestChild {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // The test failed with the child still running: end it by its handle.
            // A child that ended in between fails nothing.
            match self.child.kill() {
                Ok(()) | Err(_) => {}
            }
        }
    }
}

#[test]
fn a_stop_signal_is_received_not_fatal() {
    let mut child = TestChild::spawn(false);
    send_stop_signal(&child.child).expect("the stop signal is sent");
    child.expect_line("stopped");
    let status = child
        .wait_exit(Duration::from_secs(5))
        .expect("the child exits after its stop");
    assert!(
        status.success(),
        "the child ended on its own, not by the signal: {status:?}"
    );
}

#[test]
fn a_second_stop_signal_ends_the_process() {
    let mut child = TestChild::spawn(true);
    send_stop_signal(&child.child).expect("the first stop signal is sent");
    child.expect_line("received");
    assert!(
        child.wait_exit(Duration::from_millis(300)).is_none(),
        "the first stop signal must not end the process"
    );
    send_stop_signal(&child.child).expect("the second stop signal is sent");
    let status = child
        .wait_exit(Duration::from_secs(5))
        .expect("a second stop signal ends the process within 5 s");
    assert!(
        !status.success(),
        "the process ended by the signal: {status:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(libc::SIGTERM),
            "the default action of SIGTERM"
        );
    }
    let rest = child.rest_of_output();
    assert!(
        !rest.iter().any(|l| l == "stopped"),
        "no `stopped` line: {rest:?}"
    );
}

#[cfg(unix)]
#[test]
#[expect(
    unsafe_code,
    reason = "FFI: libc::kill sends one signal to the helper child"
)]
fn sigint_and_sighup_are_stop_signals() {
    for signal in [libc::SIGINT, libc::SIGHUP] {
        let mut child = TestChild::spawn(false);
        let pid = libc::pid_t::try_from(child.child.id()).expect("a pid fits in pid_t");
        // SAFETY: kill(2) takes plain integers; the pid is our own live child.
        let sent = unsafe { libc::kill(pid, signal) };
        assert_eq!(sent, 0, "signal {signal} is sent");
        child.expect_line("stopped");
        let status = child
            .wait_exit(Duration::from_secs(5))
            .expect("the child exits after its stop");
        assert!(
            status.success(),
            "signal {signal} is a stop, not fatal: {status:?}"
        );
    }
}
