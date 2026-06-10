//! Adapter support: the shared, vendor-neutral machinery every Ralphy **adapter**
//! leans on. This crate owns the **OS-level headless plumbing** — spawn a child
//! `Command`, drain stdout/stderr without deadlocking, poll to
//! completion-or-timeout, kill on the deadline, and collect the captured output —
//! and nothing more.
//!
//! ## Why this does NOT reopen ADR-0004
//!
//! ADR-0004 states there is "deliberately no shared 'headless runner' that both
//! bend to fit." That prohibition is about a shared **`Outcome`-detection**
//! runner — the semantic completion protocol each vendor must shape itself. This
//! crate extracts **only the OS-level plumbing** (spawn / drain / poll / kill /
//! collect), which is identical by nature, not by imposition. It owns **no**
//! completion protocol and produces **no** `Outcome`: it hands back the raw,
//! still-separate stdout and stderr, and each adapter's `classify_*` function
//! still maps that captured output onto its own `Outcome`. The completion
//! semantics remain entirely per-adapter, so this extraction is the mechanical
//! floor *beneath* the seam ADR-0004 protects, not a violation of it. (This
//! rationale is recorded here so a future architecture review does not re-flag the
//! shared crate as an ADR-0004 violation.)
//!
//! The public surface speaks only `std` types ([`Command`], [`Duration`],
//! [`ExitStatus`], [`String`]) — no `portable-pty`, no vendor names. Building the
//! `Command` (binary, flags, env scrub) stays in each adapter, as does slicing the
//! returned [`HeadlessOutput`] into the adapter's own local return shape.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::Path;

/// Returns `true` when `text` contains the `RALPHY_DONE_EXIT` sentinel, as
/// defined by `assets/prompts/prompt.execute.md`.
pub fn done_sentinel(text: &str) -> bool {
    text.contains("RALPHY_DONE_EXIT")
}

/// Returns the trimmed reason from the first `RALPHY_BLOCKED_EXIT <reason>` line
/// in `text`, or `None` when no such line is present. A bare marker with no
/// trailing text yields `Some("")`.
pub fn blocked_reason(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("RALPHY_BLOCKED_EXIT"))?;
    Some(
        line.split_once("RALPHY_BLOCKED_EXIT")
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use include_dir::include_dir;

    static FIXTURE: include_dir::Dir<'_> =
        include_dir!("$CARGO_MANIFEST_DIR/tests/fixtures/sample");

    #[test]
    fn materialize_assets_clears_extracts_and_writes_gitignore() {
        let tmp = std::env::temp_dir().join(format!("ralphy-mat-assets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);

        // Destination with a pre-existing stale file.
        let dest = tmp.join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("stale.txt"), b"stale").unwrap();

        // Separate dir for the .gitignore.
        let gitignore_dir = tmp.join("gi");
        fs::create_dir_all(&gitignore_dir).unwrap();

        materialize_assets(&FIXTURE, &dest, Some(&gitignore_dir)).expect("materialize");

        // Stale file was cleared.
        assert!(
            !dest.join("stale.txt").exists(),
            "stale file must be removed before extraction"
        );
        // Top-level file extracted.
        assert!(
            dest.join("hello.txt").is_file(),
            "hello.txt must be extracted"
        );
        // Nested file extracted.
        assert!(
            dest.join("sub/nested.txt").is_file(),
            "sub/nested.txt must be extracted"
        );
        // .gitignore written at the requested location.
        let gi_path = gitignore_dir.join(".gitignore");
        assert!(gi_path.is_file(), ".gitignore must be written");
        let gi_contents = fs::read_to_string(&gi_path).unwrap();
        assert!(
            gi_contents.contains('*'),
            ".gitignore must contain '*': {gi_contents:?}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn blocked_reason_extracts_trimmed_reason() {
        assert_eq!(
            blocked_reason("RALPHY_BLOCKED_EXIT missing key"),
            Some("missing key".into())
        );
    }

    #[test]
    fn done_sentinel_detects_bare_done() {
        assert!(done_sentinel("some output\nRALPHY_DONE_EXIT\n"));
    }

    #[test]
    fn neither_sentinel_yields_none_and_false() {
        let text = "no sentinel here";
        assert_eq!(blocked_reason(text), None);
        assert!(!done_sentinel(text));
    }

    #[test]
    fn blocked_reason_with_surrounding_whitespace_is_trimmed() {
        assert_eq!(
            blocked_reason("  RALPHY_BLOCKED_EXIT   need crate X  "),
            Some("need crate X".into())
        );
    }
}
use std::process::{Command, ExitStatus};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Materialize `asset` into `dest_dir`, clearing any prior copy first, and
/// optionally write a `*` `.gitignore` at `gitignore_dir/.gitignore`.
///
/// The clear-before-extract pattern guarantees a removed file in the embedded
/// tree never lingers between runs. `gitignore_dir` is `None` for adapters that
/// own no `.gitignore` concern (Claude's plugin dir is already inside `.ralphy`
/// which carries its own ignore rules); it is `Some(dir)` for adapters that
/// materialize into a directory the executor might otherwise commit
/// (Codex → `.agents`, OpenCode → `.ralphy`).
pub fn materialize_assets(
    asset: &include_dir::Dir,
    dest_dir: &Path,
    gitignore_dir: Option<&Path>,
) -> Result<()> {
    if dest_dir.exists() {
        fs::remove_dir_all(dest_dir).context("clearing the stale materialized asset directory")?;
    }
    fs::create_dir_all(dest_dir).context("creating the asset destination directory")?;
    asset
        .extract(dest_dir)
        .context("extracting the embedded asset tree")?;
    if let Some(dir) = gitignore_dir {
        fs::write(dir.join(".gitignore"), "*\n").context("writing .gitignore")?;
    }
    Ok(())
}

/// The raw result of driving one headless child to completion or timeout.
///
/// `stdout` and `stderr` are kept **separate** (each captured as lossy-UTF-8) so
/// every adapter can combine or slice them as it needs — the OpenCode adapter
/// parses the JSON event stream from stdout alone, while Codex and Claude
/// concatenate the two. `exit` is `Some(status)` on a natural exit and `None`
/// exactly when the child was killed on the timeout deadline, letting each caller
/// recover its own `exited`/`exited_cleanly` flag from `std` types alone.
#[derive(Debug)]
pub struct HeadlessOutput {
    /// Everything the child wrote to stdout, captured complete (no truncation).
    pub stdout: String,
    /// Everything the child wrote to stderr, captured complete (no truncation).
    pub stderr: String,
    /// `true` when the child outlived `timeout` and was killed.
    pub timed_out: bool,
    /// The child's exit status, or `None` when it was killed on the deadline.
    pub exit: Option<ExitStatus>,
}

/// Spawn `cmd`, pipe `prompt` on its stdin, drain stdout/stderr to completion or
/// timeout, killing the child if it outlives `timeout`. `cmd` must already have
/// stdin/stdout/stderr set to [`Stdio::piped()`](std::process::Stdio::piped); the
/// adapter builds the rest (binary, flags, env scrub).
///
/// The reader threads start *before* the prompt is written so a prompt larger than
/// the pipe buffer (~64KB) can't deadlock against a child that begins emitting
/// output before it finishes draining stdin. The wall poll ticks every 500ms; on
/// the deadline the child is killed and reaped and `timed_out`/`exit = None` are
/// reported. Output is then collected with a 5s grace so a child that flushed late
/// is still captured complete.
pub fn run_headless(mut cmd: Command, prompt: &str, timeout: Duration) -> Result<HeadlessOutput> {
    let mut child = cmd
        .spawn()
        .context("failed to spawn the headless child process")?;

    // Spawn the stdout/stderr reader threads *before* writing stdin, so a prompt
    // larger than the pipe buffer (~64KB) can't deadlock against a child that
    // starts emitting output before it finishes draining stdin.
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stdout).read_to_end(&mut buf);
        let _ = tx_out.send(buf);
    });
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut buf);
        let _ = tx_err.send(buf);
    });

    stdin
        .write_all(prompt.as_bytes())
        .context("piping the prompt to the headless child")?;
    drop(stdin); // close stdin so the child sees EOF

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit = loop {
        if let Some(s) = child.try_wait().context("polling the headless child")? {
            break Some(s);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(500));
    };

    let collect = Duration::from_secs(5);
    let stdout_bytes = rx_out.recv_timeout(collect).unwrap_or_default();
    let stderr_bytes = rx_err.recv_timeout(collect).unwrap_or_default();
    Ok(HeadlessOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        timed_out,
        exit,
    })
}
