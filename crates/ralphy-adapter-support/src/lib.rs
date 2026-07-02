//! Adapter support: the shared, vendor-neutral machinery every Ralphy **adapter**
//! leans on. This crate owns the mechanical plumbing that is identical by nature
//! across vendors: the **OS-level headless runner** (spawn a child `Command`,
//! drain stdout/stderr without deadlocking, poll to completion-or-timeout, kill on
//! the deadline, collect the captured output), the **one-shot JSON session runner**
//! ([`run_json_session`]), **auth-error** and **usage-limit** detection scaffolds
//! ([`auth_error`], [`detect_limit`], [`scan_json_lines`]), and the
//! **session-file snapshot-diff** helpers ([`session_files_appeared`],
//! [`list_session_files`]). Every one of these takes the vendor-specific part —
//! markers, formats, extensions, schema closures — as a parameter.
//!
//! ## Why this does NOT reopen ADR-0004
//!
//! ADR-0004 states there is "deliberately no shared 'headless runner' that both
//! bend to fit." That prohibition is about a shared **`Outcome`-detection**
//! runner — the semantic completion protocol each vendor must shape itself. This
//! crate extracts **only mechanical plumbing**, which is identical by nature, not
//! by imposition. It owns **no** completion protocol and produces **no**
//! `Outcome`: the headless runner hands back raw, still-separate stdout and
//! stderr; the JSON runner returns whatever the adapter's own validation closure
//! parses; the auth/limit scaffolds return a `bool`/`Option`, never an `Outcome`.
//! Each adapter's `classify_*` function still maps captured output onto its own
//! `Outcome`, and every vendor-specific decision (which markers signal auth, which
//! reset-string format to parse) stays in the adapter. This extraction is the
//! mechanical floor *beneath* the seam ADR-0004 protects, not a violation of it.
//! (This rationale is recorded here so a future architecture review does not
//! re-flag the shared crate as an ADR-0004 violation.)
//!
//! The public surface speaks only `std` types ([`Command`], [`Duration`],
//! [`ExitStatus`], [`String`]) — no `portable-pty`, no vendor names. Building the
//! `Command` (binary, flags, env scrub) stays in each adapter, as does slicing the
//! returned [`HeadlessOutput`] into the adapter's own local return shape.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

mod detect;
pub use detect::{auth_error, detect_limit, scan_json_lines};

mod json_session;
pub use json_session::{run_json_session, JsonSession};

mod session_files;
pub use session_files::{list_session_files, session_files_appeared};

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

    /// Mark a freshly-written fixture executable so `find_program`'s Unix
    /// execute-bit check accepts it. A no-op off Unix (Windows gates on PATHEXT).
    fn mark_executable(p: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(p).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(p, perms).unwrap();
        }
        #[cfg(not(unix))]
        let _ = p;
    }

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
    fn find_program_locates_a_file_on_the_search_path() {
        let tmp = std::env::temp_dir().join(format!("ralphy-find-prog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        // A file with the searched extension (".EXE" via PATHEXT on Windows; on
        // non-Windows the bare name is what is matched).
        let bare = tmp.join("tool");
        let exe = tmp.join("tool.exe");
        let target = if cfg!(windows) { &exe } else { &bare };
        fs::write(target, b"x").unwrap();
        mark_executable(target);

        let path_var = tmp.clone().into_os_string();
        let got = find_program("tool", Some(path_var), Some(".EXE".into()))
            .expect("must locate the file on PATH");
        // The resolved path must point at a real file whose stem is `tool` (the
        // extension casing may follow PATHEXT, which is harmless on Windows'
        // case-insensitive filesystem).
        assert!(got.is_file(), "resolved path must exist: {got:?}");
        assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("tool"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_program_resolves_a_windows_cmd_shim() {
        // The defect this guards: an npm CLI present only as `name.cmd` (no
        // `.exe`) must still resolve, since `Command::new("name")` would not find
        // it. On non-Windows there is no PATHEXT, so this asserts the bare-name
        // branch instead.
        let tmp = std::env::temp_dir().join(format!("ralphy-find-cmd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path_var = tmp.clone().into_os_string();

        if cfg!(windows) {
            let shim = tmp.join("opencode.cmd");
            fs::write(&shim, b"@echo off").unwrap();
            let got = find_program("opencode", Some(path_var), Some(".EXE;.CMD".into()))
                .expect("must resolve the .cmd shim");
            assert!(got.is_file(), "resolved shim must exist: {got:?}");
            assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("opencode"));
            assert!(
                got.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("cmd")),
                "must resolve the .cmd extension, not .exe: {got:?}"
            );
        } else {
            let bare = tmp.join("opencode");
            fs::write(&bare, b"#!/bin/sh").unwrap();
            mark_executable(&bare);
            let got = find_program("opencode", Some(path_var), None);
            assert_eq!(got.as_deref(), Some(bare.as_path()));
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    #[cfg(windows)]
    fn find_program_skips_extensionless_shim_when_cmd_present() {
        // The exact npm-on-Windows layout: a bare `opencode` shell shim sits next
        // to `opencode.cmd`. The bare file is not a valid Win32 application
        // (os error 193), so the resolver must return the `.cmd`, not the shim.
        let tmp = std::env::temp_dir().join(format!("ralphy-find-pair-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("opencode"), b"#!/bin/sh\n").unwrap();
        fs::write(tmp.join("opencode.cmd"), b"@echo off\n").unwrap();

        let got = find_program(
            "opencode",
            Some(tmp.clone().into_os_string()),
            Some(".EXE;.CMD".into()),
        )
        .expect("must resolve a runnable candidate");
        assert!(
            got.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cmd")),
            "must return the .cmd, not the extensionless shim: {got:?}"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_program_returns_none_when_absent() {
        let path_var = std::env::temp_dir().into_os_string();
        assert!(find_program(
            "definitely-not-a-real-prog-xyz",
            Some(path_var),
            Some(".EXE".into())
        )
        .is_none());
    }

    #[test]
    fn locate_program_falls_back_to_local_bin_when_off_path() {
        // A program absent from PATH but present in ~/.local/bin must still be
        // located — this is the gate/execution unification: the env gate would
        // otherwise miss it, while the adapter (resolve_program) would still run it.
        let home = std::env::temp_dir().join(format!("ralphy-locate-home-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let bin = home.join(".local").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let name = "myagent";
        let file = if cfg!(windows) {
            bin.join("myagent.exe")
        } else {
            bin.join("myagent")
        };
        fs::write(&file, b"x").unwrap();
        mark_executable(&file);

        // Empty PATH so only the ~/.local/bin fallback can match.
        let got = locate_program_with(
            name,
            Some(std::ffi::OsString::new()),
            Some(".EXE".into()),
            Some(home.clone()),
        );
        assert_eq!(got.as_deref(), Some(file.as_path()));

        // Without a home, the fallback can't fire → None.
        assert!(locate_program_with(
            name,
            Some(std::ffi::OsString::new()),
            Some(".EXE".into()),
            None
        )
        .is_none());

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn locate_program_prefers_path_over_local_bin() {
        // When the program is on PATH, that wins — the fallback is only consulted
        // when PATH has nothing.
        let tmp = std::env::temp_dir().join(format!("ralphy-locate-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let on_path = if cfg!(windows) {
            tmp.join("tool.exe")
        } else {
            tmp.join("tool")
        };
        fs::write(&on_path, b"x").unwrap();
        mark_executable(&on_path);

        let got = locate_program_with(
            "tool",
            Some(tmp.clone().into_os_string()),
            Some(".EXE".into()),
            // A bogus home whose ~/.local/bin doesn't exist — PATH must win anyway.
            Some(tmp.join("nonexistent-home")),
        )
        .expect("PATH hit must win");
        // Compare by parent + stem: on Windows the resolved extension casing follows
        // PATHEXT (`.EXE`) rather than the file's `.exe`, which is harmless.
        assert_eq!(got.parent(), on_path.parent());
        assert_eq!(got.file_stem(), on_path.file_stem());
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
use std::process::{Command, ExitStatus, Stdio};
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
    // Extract into a sibling staging dir first, then swap it over `dest_dir`. A
    // failed extract (disk full, permission) leaves the previous good copy
    // untouched instead of a half-populated tree — the slow, failure-prone step
    // happens off to the side, and only the fast remove+rename touches `dest_dir`.
    let staging = dest_dir.with_file_name(format!(
        "{}.tmp-{}",
        dest_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("asset"),
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&staging); // clear any leftover from a crashed run
    fs::create_dir_all(&staging).context("creating the asset staging directory")?;
    if let Err(e) = asset.extract(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e).context("extracting the embedded asset tree");
    }
    if dest_dir.exists() {
        fs::remove_dir_all(dest_dir).context("clearing the stale materialized asset directory")?;
    }
    fs::rename(&staging, dest_dir)
        .context("swapping the materialized asset directory into place")?;
    if let Some(dir) = gitignore_dir {
        fs::write(dir.join(".gitignore"), "*\n").context("writing .gitignore")?;
    }
    Ok(())
}

/// Resolve `name` to the path of the executable a headless `Command` should
/// spawn, searching `PATH` and — on Windows — every `PATHEXT` extension. Falls
/// back to the bare `name` so the spawn error still names the missing program.
///
/// This exists because Windows ships many agent CLIs as npm shims with **no**
/// `.exe` (e.g. `opencode.cmd`): `Command::new("opencode")` only ever tries
/// `opencode` and `opencode.exe`, so it reports "program not found" even though
/// `opencode.cmd` is on `PATH`. Resolving the full path (including the `.cmd`
/// extension) lets `std`'s `Command` launch the batch shim directly — modern
/// `std` routes `.bat`/`.cmd` through the command processor with safe argument
/// escaping. A native `.exe` (the common case off Windows, and for Codex) is
/// found first and returned unchanged.
pub fn resolve_program(name: &str) -> std::ffi::OsString {
    locate_program(name)
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| name.into())
}

/// Locate `name` as an executable: search `PATH` (+`PATHEXT` on Windows), then
/// fall back to the XDG-conventional `~/.local/bin` where user-installed CLIs (and
/// Ralphy itself, see the cli's `install` module) land but which a non-login shell
/// often omits from `PATH`. Returns the resolved path, or `None` when not found
/// anywhere.
///
/// This is the single source of truth for "is this program available, and where" —
/// `resolve_program` (what every adapter spawns) and `ralphy init`'s environment
/// gate both go through it, so **detection and execution can never disagree**: a
/// CLI under `~/.local/bin` that the gate would otherwise miss on a bare `PATH`
/// probe is both reported present and actually spawned.
pub fn locate_program(name: &str) -> Option<PathBuf> {
    locate_program_with(
        name,
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        home_dir(),
    )
}

/// Pure core of [`locate_program`] over its inputs, so the `~/.local/bin` fallback
/// unit-tests against a temp home without touching the real environment.
pub fn locate_program_with(
    name: &str,
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(found) = find_program(name, path_var, pathext) {
        return Some(found);
    }
    let mut cand = home?.join(".local").join("bin").join(name);
    if cfg!(windows) {
        cand.set_extension("exe");
    }
    is_executable_file(&cand).then_some(cand)
}

/// The home directory, from the platform's usual env var (`USERPROFILE` on
/// Windows, else `HOME`). Exported so every adapter shares one definition instead
/// of re-deriving the `USERPROFILE`-or-`HOME` dance.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// True when `p` is a regular file the OS would actually run. On Unix this
/// requires an execute bit, so a non-executable data file sharing the program's
/// name on `PATH` can't shadow a real binary later in the search; off Unix, being
/// a regular file is enough (Windows gates executability on `PATHEXT` separately).
#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Search `path_var` for `name`, trying each `PATHEXT` extension on Windows. Pure
/// over its inputs so it unit-tests against a temp dir. Mirrors the Claude
/// adapter's private resolver; lives here so every headless adapter shares it.
pub fn find_program(
    name: &str,
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let path_var = path_var?;
    let exts: Vec<String> = if cfg!(windows) {
        pathext
            .and_then(|p| p.into_string().ok())
            .unwrap_or_else(|| ".EXE".into())
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    for dir in std::env::split_paths(&path_var) {
        let direct = dir.join(name);
        // On Windows a file is only executable when its extension is in PATHEXT,
        // so a bare extensionless `direct` must be skipped — npm ships agent CLIs
        // as a pair (`opencode` shell shim + `opencode.cmd`), and returning the
        // extensionless shim yields "not a valid Win32 application" (os error 193).
        // Off Windows, any existing file on PATH is a candidate as-is.
        let direct_ok = if cfg!(windows) {
            direct.is_file()
                && direct
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| {
                        exts.iter()
                            .any(|x| x.trim_start_matches('.').eq_ignore_ascii_case(e))
                    })
        } else {
            is_executable_file(&direct)
        };
        if direct_ok {
            return Some(direct);
        }
        for ext in &exts {
            let cand = dir.join(name).with_extension(ext.trim_start_matches('.'));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
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
    // On Unix, run the child in its own process group so a timeout can signal the
    // whole tree, not just the direct child. An agent CLI that spawned helpers
    // would otherwise leave a grandchild holding the stdout pipe open, blocking the
    // reader forever and forcing the collect grace to return empty — silently
    // dropping the very output the limit/auth detectors scan.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .context("failed to spawn the headless child process")?;

    // Spawn the stdout/stderr reader threads *before* writing stdin, so a prompt
    // larger than the pipe buffer (~64KB) can't deadlock against a child that
    // starts emitting output before it finishes draining stdin. A misconfigured
    // `Command` (no piped stdio) degrades to a run error, not a panic.
    let mut stdin = child
        .stdin
        .take()
        .context("headless child stdin was not piped")?;
    let stdout = child
        .stdout
        .take()
        .context("headless child stdout was not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("headless child stderr was not piped")?;

    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    let out_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stdout).read_to_end(&mut buf);
        let _ = tx_out.send(buf);
    });
    let err_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut buf);
        let _ = tx_err.send(buf);
    });

    // A broken pipe here means the child exited before draining stdin — its own
    // signal, not a fatal error. Warn and fall through to the poll loop, which
    // reaps the child, rather than `?`-returning with the child still unreaped.
    let stdin_result = stdin.write_all(prompt.as_bytes());
    drop(stdin); // close stdin so the child sees EOF
    if let Err(e) = stdin_result {
        tracing::warn!(error = %e, "writing the prompt to the headless child failed (it likely exited early)");
    }

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit = loop {
        if let Some(s) = child.try_wait().context("polling the headless child")? {
            break Some(s);
        }
        if Instant::now() >= deadline {
            kill_tree(&mut child);
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(500));
    };

    // Collect with a bounded grace so a child that flushed late is still captured.
    // After a natural exit (or `kill_tree`) the pipes reach EOF and the readers
    // finish, so this normally returns the full buffer; on the rare stuck reader we
    // warn (a truncated capture is observable) and leak that one thread rather than
    // block the whole run on it.
    let collect = Duration::from_secs(5);
    let stdout_bytes = recv_and_join(&rx_out, out_handle, collect, "stdout");
    let stderr_bytes = recv_and_join(&rx_err, err_handle, collect, "stderr");
    Ok(HeadlessOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        timed_out,
        exit,
    })
}

/// Await one reader thread's captured bytes within `grace`, then join it. On a
/// natural exit or after [`kill_tree`] the pipe hits EOF and the thread sends
/// promptly, so the join is immediate; if the grace elapses the thread is still
/// blocked (a descendant survived) — warn that the capture may be truncated and
/// leak that one thread instead of blocking the run on a join that would hang.
fn recv_and_join(
    rx: &mpsc::Receiver<Vec<u8>>,
    handle: thread::JoinHandle<()>,
    grace: Duration,
    stream: &str,
) -> Vec<u8> {
    match rx.recv_timeout(grace) {
        Ok(buf) => {
            let _ = handle.join();
            buf
        }
        Err(_) => {
            tracing::warn!(
                stream,
                "headless reader did not finish within the collect grace — output may be truncated"
            );
            Vec::new()
        }
    }
}

/// Kill the child and every descendant it spawned. `child.kill()` signals only the
/// direct child, so a helper process started by an agent CLI would survive and
/// hold the stdout pipe open. Best-effort on every arm; always reaps the child.
fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        // `taskkill /T` terminates the whole tree rooted at PID.
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // The child leads its own process group (set at spawn), so a negative pgid
        // signals the whole tree. Dependency-free via the `kill` utility.
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill(); // direct child / fallback
    let _ = child.wait(); // reap so no zombie lingers
}
