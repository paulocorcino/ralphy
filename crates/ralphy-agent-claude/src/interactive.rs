//! Interactive execution over a PTY: the live `claude` session driver
//! (`execute_outcome` → `drive_session`), the reader-thread + `mpsc` plumbing and
//! the DSR (cursor-position) handshake, the logged-out login-TUI watch, the
//! first-run gate pre-clearing (`~/.claude.json` workspace-trust + onboarding),
//! and the `claude` binary resolver.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{IdleWatch, ProgressBeat};
use ralphy_core::{Outcome, Plan, Workspace};
use ralphy_pty::{PtyCommand, PtySession, CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tracing::info;

use crate::api_watch::{ApiWatch, ApiWatchAction};
use crate::auth::{is_claude_auth_error, transcript_limit, CLAUDE_AUTH_ERROR_MSG};
use crate::headless::classify_outcome;
use crate::plan::materialize_plugin;
use crate::usage::{dirs_home, latest_transcript_text, latest_transcript_text_since};
use crate::{ClaudeAgent, EXEC_CHARTER};

/// How a `drive_session` ended: a terminal [`Outcome`], or a signal that the
/// child stayed degraded past the API watch's kill and should be re-spawned once.
pub(crate) enum DriveEnd {
    Outcome(Outcome),
    Respawn,
}

/// One turn of the respawn loop's decision: either settle on a final outcome or
/// spawn one more child.
enum RespawnStep {
    Done(Outcome),
    Again,
}

/// The respawn-budget rule (budget = exactly 1), pure so it unit-tests without a
/// real PTY: a terminal outcome settles; the FIRST `Respawn` (flips `respawned`)
/// asks for another child; any later `Respawn` settles on `Timeout`.
fn respawn_step(end: DriveEnd, respawned: &mut bool) -> RespawnStep {
    match end {
        DriveEnd::Outcome(o) => RespawnStep::Done(o),
        DriveEnd::Respawn if !*respawned => {
            *respawned = true;
            RespawnStep::Again
        }
        DriveEnd::Respawn => RespawnStep::Done(Outcome::Timeout),
    }
}

impl ClaudeAgent {
    /// Drive the execution session (headless `-p` loop or interactive PTY) to a
    /// core [`Outcome`]. The token snapshot/wrap lives in [`crate::ClaudeAgent`]'s
    /// `Agent::execute`; this keeps the completion-classification logic exactly as
    /// it was.
    pub(crate) fn execute_outcome(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        if self.exec.headless_exec {
            return self.execute_headless(plan, ws);
        }

        std::fs::create_dir_all(&self.run_dir).ok();
        std::fs::create_dir_all(ws.ralphy_dir()).ok();

        // The live session reads the charter from disk (the headless copy keeps
        // the binary self-contained).
        std::fs::write(
            ws.ralphy_dir().join("exec.md"),
            ralphy_adapter_support::PROMPT_EXECUTE,
        )
        .context("writing .ralphy/exec.md")?;

        // Pre-clear Claude's first-run interactive gates (workspace trust AND the
        // theme/onboarding wizard) so the live session doesn't stall on a keypress.
        ensure_interactive_session_ready(ws.repo_root());

        let settings_path = self.write_exec_settings()?;
        let plugin_dir = materialize_plugin(ws)?;
        let exec_model = self.resolve_exec_model(plan);
        let flag_file = self.run_dir.join("status.flag");
        let _ = std::fs::remove_file(&flag_file);

        // The Stop hook writes the flag; it learns the path from this env var,
        // inherited by claude through the PTY child.
        let rc_name = self
            .run_dir
            .file_name()
            .map(|s| format!("ralphy-{}", s.to_string_lossy()))
            .unwrap_or_else(|| "ralphy".into());

        // Build the claude argv: settings, skip-permissions, model, effort,
        // optional remote-control, then the charter as the positional prompt.
        // A closure so the respawn loop can rebuild an IDENTICAL command (the
        // second child resumes from the on-disk `plan.md`, untouched between
        // spawns — see `prompt.execute.md` resume instruction).
        let build_cmd = || {
            let mut cmd = PtyCommand::new(resolve_claude_binary())
                .cwd(ws.repo_root())
                .env("RALPHY_FLAG_FILE", &flag_file)
                .arg("--dangerously-skip-permissions")
                .arg("--settings")
                .arg(settings_path.as_os_str())
                .arg("--plugin-dir")
                .arg(plugin_dir.as_os_str());
            cmd = cmd.arg("--model").arg(&exec_model);
            if let Some(e) = &self.exec.exec_effort {
                cmd = cmd.arg("--effort").arg(e);
            }
            if self.exec.remote_control {
                cmd = cmd.arg("--remote-control").arg(&rc_name);
            }
            cmd.arg(EXEC_CHARTER)
        };

        // budget_min field consumed by the telegram notifier / presenter — keep stable
        ralphy_core::emit::executing(
            if self.exec.remote_control {
                "interactive claude over the PTY --remote-control"
            } else {
                "interactive claude over the PTY"
            },
            self.exec.max_minutes_per_issue,
            &exec_model,
            self.exec.exec_effort.as_deref().unwrap_or("medium"),
        );

        let transcript_dir = self.transcript_dir(ws);
        let transcript_since = SystemTime::now()
            .checked_sub(Duration::from_secs(2))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        // Respawn budget = exactly 1: a child stuck degraded past the API watch's
        // kill is re-spawned once against `plan.md`; a second degradation returns
        // `Timeout`. Kill-before-return is invariant on EVERY iteration — the
        // `session.kill()` sits before `match end?` so an `Err` from
        // `drive_session` still reclaims the child, and there is no early
        // `return`/`?` between `spawn` and `kill`.
        let mut respawned = false;
        let outcome = loop {
            let _ = std::fs::remove_file(&flag_file);
            let mut session =
                PtySession::spawn(build_cmd()).context("spawning the claude execution session")?;
            let end = self.drive_session(
                &mut session,
                &flag_file,
                transcript_dir.as_deref(),
                transcript_since,
            );
            // Reclaim: kill the tree and drop the session (closes the ConPTY).
            // Unconditional so the child never outlives us on error paths.
            let _ = session.kill();
            match respawn_step(end?, &mut respawned) {
                RespawnStep::Done(o) => break o,
                RespawnStep::Again => {
                    info!("api degraded past kill — re-spawning child once against plan.md");
                    continue;
                }
            }
        };
        Ok(outcome)
    }

    /// Drain the PTY (tee to `exec.log`, answer DSR queries) while polling for the
    /// flag file, the child's own exit, and the per-issue wall timeout. Classifies
    /// the result into an [`Outcome`].
    fn drive_session(
        &self,
        session: &mut PtySession,
        flag_file: &Path,
        transcript_dir: Option<&Path>,
        transcript_since: SystemTime,
    ) -> Result<DriveEnd> {
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

        let mut log = std::fs::File::create(self.run_dir.join("exec.log")).ok();
        let deadline = self.issue_deadline();

        let mut timed_out = false;
        let mut child_exited = false;
        let mut limit_transcript: Option<String> = None;
        let mut next_transcript_poll = Instant::now();
        let mut dsr_carry: Vec<u8> = Vec::new();
        let mut login_watch = LoginTuiWatch::new();
        let mut api_watch = ApiWatch::new();
        // Idle watchdog (docs/adr/0038). The progress signal is transcript growth
        // and ONLY transcript growth: the TUI redraws its spinner forever, so PTY
        // bytes keep arriving from a child that is thoroughly wedged and would
        // make this watchdog permanently blind. That coarser signal is why the
        // interactive window is the larger default — a legitimate long tool call
        // advances no transcript while it runs.
        let idle_watch = IdleWatch::from_minutes(self.exec.idle_minutes_for(true));
        let idle_beat = ProgressBeat::new(Instant::now());
        // Last observed transcript length; a growth between polls is the
        // "activity resumed" signal that clears a degraded state.
        let mut last_len: usize = 0;
        loop {
            // Act as the terminal: tee output and answer cursor-position queries.
            while let Ok(chunk) = rx.try_recv() {
                if scan_dsr_request(&mut dsr_carry, &chunk) {
                    let _ = session.write_all(CURSOR_POSITION_REPLY);
                }
                login_watch.feed(&chunk);
                api_watch.feed(&chunk);
                if let Some(f) = log.as_mut() {
                    let _ = f.write_all(&chunk);
                }
            }

            if flag_file.exists() {
                break;
            }
            if Instant::now() >= next_transcript_poll {
                let advanced;
                if let Some(t) = latest_transcript_text_since(transcript_dir, transcript_since) {
                    // Any transcript activity proves the model loop started —
                    // a logged-out session never produces one (see LoginTuiWatch).
                    login_watch.disarm();
                    advanced = t.len() > last_len;
                    last_len = t.len();
                    if transcript_limit(&t).is_some() {
                        limit_transcript = Some(t);
                        break;
                    }
                } else {
                    advanced = false;
                    if login_watch.detected() {
                        // Logged-out interactive session: the login TUI stalls
                        // without exiting, so fail fast with the auth message
                        // instead of burning the wall budget into a misleading
                        // `Timeout` (issue #72). The caller kills the session.
                        bail!("{CLAUDE_AUTH_ERROR_MSG}");
                    }
                }
                if advanced {
                    idle_beat.beat(Instant::now());
                }
                match api_watch.poll(Instant::now(), advanced) {
                    ApiWatchAction::Degraded => ralphy_core::emit::api_degraded(),
                    ApiWatchAction::Recovered => ralphy_core::emit::api_recovered(),
                    ApiWatchAction::Respawn => return Ok(DriveEnd::Respawn),
                    ApiWatchAction::None => {}
                }
                // Deliberately no re-spawn here, unlike the API-banner path above:
                // a visible retry banner is evidence the child is trying and may
                // recover, whereas silence is evidence of nothing. End the drive as
                // the timeout it already is rather than spend a respawn on a guess.
                if idle_watch.expired(&idle_beat, Instant::now()) {
                    // The same canonical helper the headless path calls, so the
                    // operator-facing event does not depend on which child shape
                    // happened to be driving (docs/adr/0038).
                    ralphy_core::emit::idle_reaped(
                        idle_watch.window().map(|w| w.as_secs() / 60).unwrap_or(0),
                    );
                    timed_out = true;
                    break;
                }
                next_transcript_poll = Instant::now() + Duration::from_secs(2);
            }
            if session.try_wait()?.is_some() {
                child_exited = true;
                break;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }

        let flag = std::fs::read_to_string(flag_file).ok();
        // A transcript read is needed to spot a usage limit when the session
        // ends without a sentinel, and the live loop above also watches for the
        // Claude CLI's subagent/tool-result rate-limit shape while the PTY stays
        // alive.
        let transcript = if flag.is_none() {
            limit_transcript.or_else(|| {
                (child_exited || timed_out)
                    .then(|| latest_transcript_text_since(transcript_dir, transcript_since))
                    .flatten()
                    .or_else(|| {
                        (child_exited || timed_out)
                            .then(latest_transcript_text)
                            .flatten()
                    })
            })
        } else {
            None
        };

        // An auth failure in the transcript takes precedence over classification:
        // it won't self-heal (unlike a usage limit), so surface it immediately.
        if child_exited && flag.is_none() {
            if let Some(ref t) = transcript {
                if is_claude_auth_error(t) {
                    bail!("{CLAUDE_AUTH_ERROR_MSG}");
                }
            }
        }

        let outcome = classify_outcome(flag.as_deref(), timed_out, transcript.as_deref());
        info!(?outcome, child_exited, timed_out, "execution session ended");
        Ok(DriveEnd::Outcome(outcome))
    }
}

/// Flatten raw PTY bytes into matchable text: ANSI escape sequences are
/// dropped, and the ones that *position* text (CSI cursor-forward `ESC[nC`,
/// cursor-position `ESC[r;cH`/`f`) become a single space — the interactive TUI
/// separates the words of one visual line with cursor moves instead of spaces
/// (`Not<ESC[1C>logged<ESC[1C>in`), so without this no substring can match.
/// CR/LF also become spaces so a phrase split across writes still joins.
pub(crate) fn strip_pty_escapes(raw: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            0x1b => {
                i += 1;
                match raw.get(i) {
                    // CSI: params/intermediates until a final byte in 0x40-0x7e.
                    Some(b'[') => {
                        i += 1;
                        while i < raw.len() && !(0x40..=0x7e).contains(&raw[i]) {
                            i += 1;
                        }
                        if matches!(raw.get(i), Some(b'C') | Some(b'H') | Some(b'f')) {
                            out.push(b' ');
                        }
                        i += 1;
                    }
                    // OSC: swallow until BEL or ST (ESC \).
                    Some(b']') => {
                        i += 1;
                        while i < raw.len() {
                            if raw[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if raw[i] == 0x1b && raw.get(i + 1) == Some(&b'\\') {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    // Two-byte escape (ESC c, ESC =, ...): drop the pair.
                    Some(_) => i += 1,
                    None => {}
                }
            }
            b'\r' | b'\n' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Return `true` when raw interactive PTY output shows the logged-out REPL.
/// The signature is the status-line pair `Not logged in · Run /login`
/// (captured from CLI v2.1.198 — see tests/fixtures/login_tui_exec.log); the
/// headless banner says `Please run /login`, so `run /login` matches both.
/// Both substrings are required, mirroring [`is_claude_auth_error`]'s AND rule.
fn is_login_tui_output(raw: &[u8]) -> bool {
    let text = strip_pty_escapes(raw).to_lowercase();
    text.contains("not logged in") && text.contains("run /login")
}

/// Rolling watch over the live PTY stream for the logged-out login TUI.
///
/// A logged-out *interactive* session renders the login TUI and stalls without
/// exiting, so the `child_exited` auth check never runs and the session used
/// to burn its whole wall budget and surface as a misleading `Timeout`
/// (issue #72). The watch accumulates a bounded tail of the raw output and is
/// consulted on the session's poll cadence.
///
/// Once the JSONL transcript shows any activity the watch disarms for the rest
/// of the session: a live transcript proves the model loop started (a
/// logged-out session never produces one), and from then on agent output that
/// merely *echoes* the signature — reading this source, for instance — must
/// not trip it.
struct LoginTuiWatch {
    buf: Vec<u8>,
    disarmed: bool,
}

impl LoginTuiWatch {
    /// Plenty for the login screen; the TUI redraws, so the signature recurs.
    const MAX_BUF: usize = 32 * 1024;

    fn new() -> Self {
        Self {
            buf: Vec::new(),
            disarmed: false,
        }
    }

    /// Accumulate a PTY chunk (keeps the most recent [`Self::MAX_BUF`] bytes).
    fn feed(&mut self, chunk: &[u8]) {
        if self.disarmed {
            return;
        }
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > Self::MAX_BUF {
            let cut = self.buf.len() - Self::MAX_BUF;
            self.buf.drain(..cut);
        }
    }

    /// Transcript activity observed — stop watching and drop the buffer.
    fn disarm(&mut self) {
        self.disarmed = true;
        self.buf = Vec::new();
    }

    fn detected(&self) -> bool {
        !self.disarmed && is_login_tui_output(&self.buf)
    }
}

/// Pre-clear the first-run gates that block an *interactive* Claude session for
/// `repo_root`: the workspace-trust dialog AND the theme/onboarding wizard. The
/// headless `-p` planning path is exempt from both, but a live session stalls on
/// either forever waiting for a keypress — so an autonomous orchestrator must
/// grant up front what the operator would otherwise click. (Observed in the wild:
/// on a profile with `hasCompletedOnboarding=false`, every live exec hung at
/// "Choose the text style…" and silently burned the whole budget.) Best-effort:
/// reads `~/.claude.json`, sets the flags, and writes it back, preserving
/// everything else. A failure here just means the live session may stall
/// (surfaced as a Timeout), never a crash.
fn ensure_interactive_session_ready(repo_root: &Path) {
    let Some(home) = dirs_home() else {
        return;
    };
    let cfg_path = home.join(".claude.json");
    let root = std::fs::read_to_string(&cfg_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    // Claude keys projects by the cwd it is launched with; we launch it at
    // `repo_root`, whose display form uses forward slashes on every platform.
    let key = repo_root.to_string_lossy().replace('\\', "/");
    let updated = with_onboarding_completed(with_workspace_trusted(root, &key));
    if let Ok(s) = serde_json::to_string_pretty(&updated) {
        let _ = std::fs::write(&cfg_path, s);
    }
}

/// Set `projects[key].hasTrustDialogAccepted = true` on a parsed `~/.claude.json`,
/// creating the `projects` map and the per-project entry as needed and leaving
/// all other content untouched. Pure, so it unit-tests without the filesystem.
fn with_workspace_trusted(mut root: serde_json::Value, key: &str) -> serde_json::Value {
    use serde_json::{json, Value};
    if let Some(obj) = root.as_object_mut() {
        let projects = obj.entry("projects").or_insert_with(|| json!({}));
        if let Some(projects) = projects.as_object_mut() {
            let entry = projects.entry(key.to_string()).or_insert_with(|| json!({}));
            if let Some(entry) = entry.as_object_mut() {
                entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
            }
        }
    }
    root
}

/// Mark Claude Code's first-run onboarding wizard complete on a parsed
/// `~/.claude.json`, so an interactive session boots straight into the prompt
/// instead of the "Let's get started" / theme picker. Sets the top-level
/// `hasCompletedOnboarding` flag and seeds a `theme` only when one is absent (so
/// a user's chosen theme is never overwritten). Leaves all other content intact.
/// Pure, so it unit-tests without the filesystem.
fn with_onboarding_completed(mut root: serde_json::Value) -> serde_json::Value {
    use serde_json::{json, Value};
    if let Some(obj) = root.as_object_mut() {
        obj.insert("hasCompletedOnboarding".into(), Value::Bool(true));
        obj.entry("theme").or_insert_with(|| json!("dark"));
    }
    root
}

/// Resolve the `claude` executable to an absolute path, mirroring the ps1
/// oracle's `$Claude` resolution. This matters because the PTY backend rebuilds
/// `PATH` from the Windows registry and ignores runtime `PATH` edits, so a bare
/// `"claude"` fails wherever the install dir isn't on the *persistent* PATH.
/// Falls back to `~/.local/bin/claude[.exe]`, then to the bare name so the spawn
/// error still names it. Delegates to [`ralphy_adapter_support::resolve_program`]
/// so detection (the `ralphy init` env gate) and execution share one resolver and
/// can never disagree about where (or whether) `claude` is installed.
pub(crate) fn resolve_claude_binary() -> std::ffi::OsString {
    ralphy_adapter_support::resolve_program("claude")
}

/// Index of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Rolling-tail DSR scanner. Appends `chunk` to `carry`, searches the combined
/// buffer for `CURSOR_POSITION_REQUEST`, then truncates `carry` to the last
/// `CURSOR_POSITION_REQUEST.len() - 1` bytes so a split sequence spanning the
/// next chunk can still match. Returns `true` if the sequence was found.
fn scan_dsr_request(carry: &mut Vec<u8>, chunk: &[u8]) -> bool {
    carry.extend_from_slice(chunk);
    let found = find_subslice(carry, CURSOR_POSITION_REQUEST).is_some();
    let keep = CURSOR_POSITION_REQUEST.len().saturating_sub(1);
    if carry.len() > keep {
        carry.drain(..carry.len() - keep);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_dsr_request_detects_split_sequence() {
        // Sequence split across two chunks: first call must return false, second true.
        let mut carry = Vec::new();
        assert!(
            !scan_dsr_request(&mut carry, b"\x1b["),
            "partial prefix should not fire"
        );
        assert!(
            scan_dsr_request(&mut carry, b"6n"),
            "completing the sequence should fire"
        );

        // Unsplit: a single chunk containing the full sequence fires immediately.
        let mut carry2 = Vec::new();
        assert!(
            scan_dsr_request(&mut carry2, CURSOR_POSITION_REQUEST),
            "full sequence in one chunk should fire"
        );

        // No sequence at all: never fires.
        let mut carry3 = Vec::new();
        assert!(
            !scan_dsr_request(&mut carry3, b"hello world"),
            "unrelated bytes should not fire"
        );
    }

    #[test]
    fn respawn_budget_is_exactly_one() {
        // A terminal outcome settles immediately, untouched.
        let mut respawned = false;
        assert!(matches!(
            respawn_step(DriveEnd::Outcome(Outcome::Done), &mut respawned),
            RespawnStep::Done(Outcome::Done)
        ));
        assert!(!respawned);

        // First Respawn asks for one more child and spends the budget…
        let mut respawned = false;
        assert!(matches!(
            respawn_step(DriveEnd::Respawn, &mut respawned),
            RespawnStep::Again
        ));
        assert!(respawned);
        // …a second Respawn settles on Timeout (budget = 1).
        assert!(matches!(
            respawn_step(DriveEnd::Respawn, &mut respawned),
            RespawnStep::Done(Outcome::Timeout)
        ));
    }

    #[test]
    fn workspace_trust_sets_flag_and_preserves_other_content() {
        use serde_json::json;

        // Existing config with an unrelated project and a top-level key.
        let root = json!({
            "numStartups": 7,
            "projects": { "C:/other": { "hasTrustDialogAccepted": false, "keep": 1 } }
        });
        let out = with_workspace_trusted(root, "C:/ws");

        // The new workspace is trusted...
        assert_eq!(out["projects"]["C:/ws"]["hasTrustDialogAccepted"], true);
        // ...and nothing else was disturbed.
        assert_eq!(out["numStartups"], 7);
        assert_eq!(out["projects"]["C:/other"]["hasTrustDialogAccepted"], false);
        assert_eq!(out["projects"]["C:/other"]["keep"], 1);
    }

    #[test]
    fn workspace_trust_bootstraps_empty_config() {
        let out = with_workspace_trusted(serde_json::json!({}), "C:/ws");
        assert_eq!(out["projects"]["C:/ws"]["hasTrustDialogAccepted"], true);
    }

    #[test]
    fn onboarding_completed_sets_flag_and_seeds_theme_once() {
        use serde_json::json;

        // No theme yet → flag set and a default theme seeded.
        let out = with_onboarding_completed(json!({ "numStartups": 7 }));
        assert_eq!(out["hasCompletedOnboarding"], true);
        assert_eq!(out["theme"], "dark");
        assert_eq!(out["numStartups"], 7);

        // An existing theme is never overwritten.
        let out = with_onboarding_completed(json!({ "theme": "light" }));
        assert_eq!(out["hasCompletedOnboarding"], true);
        assert_eq!(out["theme"], "light");
    }

    /// Raw PTY bytes of a logged-out interactive session (CLI v2.1.198),
    /// captured on Windows ConPTY: the REPL renders with a
    /// `Not logged in · Run /login` status line whose words are separated by
    /// cursor-forward escapes, not spaces.
    const LOGIN_TUI_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/login_tui_exec.log");

    #[test]
    fn login_tui_fixture_detected() {
        assert!(is_login_tui_output(LOGIN_TUI_FIXTURE));
    }

    #[test]
    fn normal_pty_output_not_detected() {
        // ANSI-heavy healthy-session shapes: a working REPL status line and
        // agent prose that mentions login without the logged-out signature.
        let healthy = b"\x1b[38;2;153;153;153m\x1b[17;3H?\x1b[1Cfor\x1b[1Cshortcuts\
            \x1b[18;83H\x1b[1Chigh\x1b[1C\xc2\xb7\x1b[1C/effort\x1b[m\r\n\
            Running\x1b[1Ccargo\x1b[1Ctest...\r\n";
        assert!(!is_login_tui_output(healthy));
        assert!(!is_login_tui_output(
            b"the user is logged in \xc2\xb7 no action"
        ));
    }

    #[test]
    fn strip_pty_escapes_turns_cursor_moves_into_spaces() {
        // The fixture's exact word-separation shape: `ESC[1C` between words.
        let raw = b"\x1b[38;2;255;107;128mNot\x1b[1Clogged\x1b[1Cin\x1b[1C\xc2\xb7\x1b[1CRun\x1b[1C/login\x1b[38;2;153;153;153m";
        assert_eq!(strip_pty_escapes(raw), "Not logged in \u{b7} Run /login");
    }

    #[test]
    fn strip_pty_escapes_drops_osc_and_csi() {
        let raw = b"\x1b]0;claude\x07plain\x1b[2mtext\x1b[m";
        assert_eq!(strip_pty_escapes(raw), "plaintext");
    }

    /// Live end-to-end proof for issue #72: spawn a real logged-out `claude`
    /// in a PTY (isolated `CLAUDE_CONFIG_DIR`, onboarding pre-completed,
    /// workspace pre-trusted, no credentials) and assert the watch flags it on
    /// the same poll cadence `drive_session` uses. Needs the `claude` binary
    /// and ~15s, so it is opt-in: `cargo test -p ralphy-agent-claude -- --ignored`.
    #[test]
    #[ignore = "spawns the real claude CLI; run manually with -- --ignored"]
    fn live_logged_out_interactive_session_is_detected() {
        use std::io::Read as _;
        use std::sync::mpsc;

        let base = std::env::temp_dir().join(format!("ralphy-login-e2e-{}", std::process::id()));
        let cfg_dir = base.join("cfg");
        let work_dir = base.join("ws");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        let key = work_dir.to_string_lossy().replace('\\', "/");
        std::fs::write(
            cfg_dir.join(".claude.json"),
            serde_json::json!({
                "hasCompletedOnboarding": true,
                "theme": "dark",
                "projects": { key: { "hasTrustDialogAccepted": true } },
            })
            .to_string(),
        )
        .unwrap();

        let cmd = PtyCommand::new(resolve_claude_binary())
            .cwd(&work_dir)
            .env("CLAUDE_CONFIG_DIR", cfg_dir.as_os_str())
            .size(30, 100);
        let mut session = PtySession::spawn(cmd).expect("spawning claude");
        let mut reader = session.reader().unwrap();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut watch = LoginTuiWatch::new();
        let mut dsr_carry: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        let detected = loop {
            while let Ok(chunk) = rx.try_recv() {
                if scan_dsr_request(&mut dsr_carry, &chunk) {
                    let _ = session.write_all(CURSOR_POSITION_REPLY);
                }
                watch.feed(&chunk);
            }
            if watch.detected() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(500));
        };
        let _ = session.kill();
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            detected,
            "a logged-out interactive claude session must be flagged as an auth failure"
        );
    }

    #[test]
    fn login_watch_detects_across_chunks_and_disarms_on_transcript() {
        // The signature arrives split across PTY chunks.
        let mut watch = LoginTuiWatch::new();
        let mid = LOGIN_TUI_FIXTURE.len() / 2;
        watch.feed(&LOGIN_TUI_FIXTURE[..mid]);
        watch.feed(&LOGIN_TUI_FIXTURE[mid..]);
        assert!(watch.detected());

        // Once the transcript shows activity the watch must stay quiet even if
        // the signature bytes appear again (agent echoing this source).
        let mut watch = LoginTuiWatch::new();
        watch.disarm();
        watch.feed(LOGIN_TUI_FIXTURE);
        assert!(!watch.detected());
    }
}
