//! `ralphy-pty-demo` — drive an interactive shell through the PTY and print what
//! it rendered. Run it to see the crate's acceptance criteria end to end:
//! spawn → answer terminal queries → write input → capture TTY output → wait.
//!
//!     cargo run -p ralphy-pty --bin ralphy-pty-demo
//!
//! On Windows it drives `cmd.exe`; elsewhere `sh`. Either way the shell is
//! interactive (no `/c`/`-c`): we feed it `echo` and `exit` over the PTY master,
//! exactly as a person typing would. Output is drained on a background thread
//! and handed back here so the main loop can both reply to the shell's
//! cursor-position query and poll the child for exit.

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use ralphy_pty::{PtyCommand, PtySession, CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};

const MARKER: &str = "hello-from-the-pty";

fn main() -> Result<()> {
    // An interactive shell: prompt, echo of the typed line, then our output.
    #[cfg(windows)]
    let cmd = PtyCommand::new("cmd.exe");
    #[cfg(not(windows))]
    let cmd = PtyCommand::new("sh");

    let mut session = PtySession::spawn(cmd)?;

    // Drain the master on its own thread; chunks come back over a channel so the
    // main loop stays free to answer queries and poll for exit.
    let mut reader = session.reader()?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    // Type into the terminal: echo a marker, then exit so the shell closes.
    session.write_all(format!("echo {MARKER}\r\n").as_bytes())?;
    session.write_all(b"exit\r\n")?;

    let mut rendered = String::new();
    let start = Instant::now();
    loop {
        while let Ok(chunk) = rx.try_recv() {
            // Act as the terminal: answer the cursor-position query so the shell
            // unblocks at start-up.
            if find_subslice(&chunk, CURSOR_POSITION_REQUEST).is_some() {
                session.write_all(CURSOR_POSITION_REPLY)?;
            }
            rendered.push_str(&String::from_utf8_lossy(&chunk));
        }
        if session.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(15) {
            session.kill()?;
            bail!("shell did not exit within 15s");
        }
        thread::sleep(Duration::from_millis(20));
    }
    // Drain anything emitted between the last poll and exit.
    while let Ok(chunk) = rx.try_recv() {
        rendered.push_str(&String::from_utf8_lossy(&chunk));
    }
    let exit = session.wait()?;

    println!("--- captured PTY output ---");
    print!("{rendered}");
    println!("\n--- end ({} bytes) ---", rendered.len());
    println!("child exit: success={} code={}", exit.success, exit.code);

    if rendered.contains(MARKER) {
        println!("OK: captured the marker the shell echoed back");
        Ok(())
    } else {
        bail!("did not see {MARKER:?} in the captured PTY output");
    }
}

/// Index of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
