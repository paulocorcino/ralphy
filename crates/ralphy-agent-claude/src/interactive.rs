//! Interactive execution over a PTY: the live `claude` session driver
//! (`execute_outcome` → `drive_session`), its poll loop and the binary resolver.

use std::io::Write;
use std::path::Path;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{IdleWatch, ProgressBeat};
use ralphy_core::{Outcome, Plan, Workspace};
use ralphy_pty::{PtyCommand, PtySession, CURSOR_POSITION_REPLY};
use tracing::info;

use crate::api_watch::{ApiWatch, ApiWatchAction};
use crate::auth::{is_claude_auth_error, transcript_limit, CLAUDE_AUTH_ERROR_MSG};
use crate::headless::classify_outcome;
use crate::plan::materialize_plugin;
use crate::usage::{latest_transcript_text, latest_transcript_text_since};
use crate::ClaudeAgent;
use ralphy_adapter_support::remove_stale_file;

mod first_run;
mod proof;
mod terminal;
use first_run::ensure_interactive_session_ready;
use proof::{status_len, ProofWait};
pub(crate) use terminal::strip_pty_escapes;
use terminal::{scan_dsr_request, LoginTuiWatch};
#[cfg(test)]
use {
    crate::EXEC_CHARTER,
    first_run::{with_onboarding_completed, with_workspace_trusted},
    proof::{charter_keys, GateAction, ProofGate},
    ralphy_pty::CURSOR_POSITION_REQUEST,
    terminal::is_login_tui_output,
};

fn interactive_args(
    settings_path: &Path,
    plugin_dir: &Path,
    model: &str,
    effort: Option<&str>,
    remote_control: Option<&str>,
) -> Vec<std::ffi::OsString> {
    let mut args = vec![
        "--dangerously-skip-permissions".into(),
        "--settings".into(),
        settings_path.as_os_str().to_owned(),
        "--plugin-dir".into(),
        plugin_dir.as_os_str().to_owned(),
        "--model".into(),
        model.into(),
    ];
    args.extend(crate::effort_args(effort).into_iter().map(Into::into));
    if let Some(rc_name) = remote_control {
        args.extend(["--remote-control".into(), rc_name.into()]);
    }
    // No positional prompt: the pump types the charter once the session's
    // SessionStart hook has written its line (ADR-0072 D6).
    args
}

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
    /// core [`Outcome`].
    pub(crate) fn execute_outcome(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        if self.exec.headless_exec {
            return self.execute_headless(plan, ws);
        }

        std::fs::create_dir_all(&self.run_dir).context("creating the run directory")?;
        std::fs::create_dir_all(ws.ralphy_dir()).context("creating the .ralphy directory")?;

        // The live session reads the charter from disk.
        std::fs::write(
            ws.ralphy_dir().join("exec.md"),
            ralphy_adapter_support::PROMPT_EXECUTE,
        )
        .context("writing .ralphy/exec.md")?;

        // Pre-clear Claude's first-run gates (workspace trust, onboarding wizard).
        ensure_interactive_session_ready(ws.repo_root());

        let settings = self.write_exec_settings()?;
        if let Some(reason) = &settings.unproven {
            // With the waiver the charter is typed at once: a shell form
            // writes no line worth a wait.
            crate::status::refuse_or_go_on(
                &crate::status::shell_form(reason),
                self.exec.run_without_guard_proof,
            )?;
        }
        let settings_path = settings.path;
        let plugin_dir = materialize_plugin(ws)?;
        let exec_model = self.resolve_exec_model(plan);
        // Effort follows the model's precedence: operator flag, else the plan's
        // `opus-high` rung (ADR-0002, Amendment 2026-07-24).
        let exec_effort = self.resolve_exec_effort(plan);
        let flag_file = self.run_dir.join("status.flag");
        remove_stale_file(&flag_file);
        // The agent-state tail (ADR-0059 §3): the hooks append, the watcher folds.
        let status = crate::status::Watcher::start(&self.run_dir)?;

        // The Stop hook learns the flag path from an env var the PTY child inherits.
        let rc_name = self.run_dir.file_name().map_or_else(
            || "ralphy".into(),
            |s| format!("ralphy-{}", s.to_string_lossy()),
        );

        // A closure, so the respawn loop rebuilds an IDENTICAL command: the second
        // child resumes from the on-disk `plan.md` (see `prompt.execute.md`).
        let build_cmd = || {
            PtyCommand::new(resolve_claude_binary())
                .cwd(ws.repo_root())
                .env("RALPHY_FLAG_FILE", &flag_file)
                .env(crate::status::STATUS_ENV, status.path())
                .args(interactive_args(
                    &settings_path,
                    &plugin_dir,
                    &exec_model,
                    exec_effort.as_deref(),
                    self.exec.remote_control.then_some(rc_name.as_str()),
                ))
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
            exec_effort.as_deref().unwrap_or(""),
            "",
        );

        let transcript_dir = crate::usage::transcript_dir(ws);
        let transcript_since = SystemTime::now()
            .checked_sub(Duration::from_secs(2))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        // Respawn budget = exactly 1: a child stuck degraded past the API watch's
        // kill is re-spawned once; a second degradation returns `Timeout`.
        // `session.kill()` sits before `match end?`, and no `return`/`?` sits
        // between `spawn` and `kill`, so an `Err` still reclaims the child.
        let mut respawned = false;
        let outcome = loop {
            remove_stale_file(&flag_file);
            // One gate per child: it reads only the lines written after its spawn.
            let mut proof = ProofWait::new(
                status.path().to_path_buf(),
                status_len(status.path()),
                settings_path.clone(),
                ws.repo_root().to_path_buf(),
                crate::status::PROOF_WINDOW,
            );
            proof.waiver = self.exec.run_without_guard_proof;
            let mut session =
                PtySession::spawn(build_cmd()).context("spawning the claude execution session")?;
            let files = SessionFiles {
                flag_file: &flag_file,
                transcript_dir: transcript_dir.as_deref(),
                transcript_since,
            };
            let end = self.drive_session(&mut session, files, proof);
            // Kill the tree and drop the session (closes the ConPTY), on every path.
            // The child may have exited already, and the tree is reclaimed.
            if let Err(e) = session.kill() {
                info!("killing the claude session failed: {e:#}");
            }
            match respawn_step(end?, &mut respawned) {
                RespawnStep::Done(o) => break o,
                RespawnStep::Again => {
                    info!("api degraded past kill — re-spawning child once against plan.md");
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
        files: SessionFiles<'_>,
        proof: ProofWait,
    ) -> Result<DriveEnd> {
        let rx = session.output_channel()?;
        let mut pump = SessionPump::new(self, rx, files, proof);
        let exit = loop {
            if let Some(exit) = pump.step(session)? {
                break exit;
            }
            thread::sleep(Duration::from_millis(500));
        };
        if matches!(exit, SessionExit::Respawn) {
            return Ok(DriveEnd::Respawn);
        }
        let outcome = classify_exit(
            exit,
            files.flag_file,
            files.transcript_dir,
            files.transcript_since,
        )?;
        Ok(DriveEnd::Outcome(outcome))
    }
}

/// The files one session child writes and the poll loop reads.
#[derive(Clone, Copy)]
struct SessionFiles<'a> {
    flag_file: &'a Path,
    transcript_dir: Option<&'a Path>,
    transcript_since: SystemTime,
}

/// Why the poll loop of [`ClaudeAgent::drive_session`] stopped.
enum SessionExit {
    /// The Stop hook wrote the flag file.
    Flag,
    /// The operator asked to stop.
    Stopped,
    /// The transcript shows a usage limit; carries the transcript text.
    Limit(String),
    /// The idle watchdog or the per-issue wall timeout expired.
    TimedOut,
    /// The child exited on its own.
    ChildExited,
    /// The API watch asks for one more child.
    Respawn,
}

/// The state of one poll loop over a live session, and the checks it runs on
/// each turn.
struct SessionPump<'a> {
    rx: mpsc::Receiver<Vec<u8>>,
    log: Option<std::fs::File>,
    deadline: Instant,
    flag_file: &'a Path,
    transcript_dir: Option<&'a Path>,
    transcript_since: SystemTime,
    next_transcript_poll: Instant,
    dsr_carry: Vec<u8>,
    login_watch: LoginTuiWatch,
    api_watch: ApiWatch,
    // Idle watchdog (docs/adr/0038). Progress is transcript growth ONLY: the TUI
    // redraws its spinner forever, so PTY bytes would blind the watchdog. That
    // coarser signal is why the interactive window is the larger default.
    idle_watch: IdleWatch,
    idle_beat: Arc<ProgressBeat>,
    // Last transcript length; growth between polls clears a degraded state.
    last_len: usize,
    proof: ProofWait,
}

impl<'a> SessionPump<'a> {
    fn new(
        agent: &ClaudeAgent,
        rx: mpsc::Receiver<Vec<u8>>,
        files: SessionFiles<'a>,
        proof: ProofWait,
    ) -> Self {
        Self {
            rx,
            log: std::fs::File::create(agent.run_dir.join("exec.log")).ok(),
            deadline: agent.issue_deadline(),
            flag_file: files.flag_file,
            transcript_dir: files.transcript_dir,
            transcript_since: files.transcript_since,
            next_transcript_poll: Instant::now(),
            dsr_carry: Vec::new(),
            login_watch: LoginTuiWatch::new(),
            api_watch: ApiWatch::new(),
            idle_watch: IdleWatch::from_minutes(agent.exec.idle_minutes_for(true)),
            idle_beat: ProgressBeat::new(Instant::now()),
            last_len: 0,
            proof,
        }
    }

    /// One turn of the loop: `Some` when the session is over.
    fn step(&mut self, session: &mut PtySession) -> Result<Option<SessionExit>> {
        self.drain(session);
        self.advance_proof(Instant::now(), &mut |bytes| session.write_all(bytes))?;

        if self.flag_file.exists() {
            return Ok(Some(SessionExit::Flag));
        }
        // The operator's stop (docs/adr/0054). Nothing is killed here: the caller
        // reaps the session on every exit path, and a second kill would be a
        // second owner of the child.
        if ralphy_core::stop::requested() {
            info!("operator stop — ending the interactive session");
            return Ok(Some(SessionExit::Stopped));
        }
        if Instant::now() >= self.next_transcript_poll {
            if let Some(exit) = self.poll_transcript()? {
                return Ok(Some(exit));
            }
        }
        if session.try_wait()?.is_some() {
            return Ok(Some(SessionExit::ChildExited));
        }
        if Instant::now() >= self.deadline {
            return Ok(Some(SessionExit::TimedOut));
        }
        Ok(None)
    }

    /// One turn of the proof wait: [`ProofWait::advance`].
    fn advance_proof(
        &mut self,
        now: Instant,
        keys: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        self.proof.advance(now, keys)
    }

    /// Act as the terminal: tee output and answer cursor-position queries.
    fn drain(&mut self, session: &mut PtySession) {
        while let Ok(chunk) = self.rx.try_recv() {
            if scan_dsr_request(&mut self.dsr_carry, &chunk) {
                // A closed terminal ends the session on the next poll.
                if let Err(e) = session.write_all(CURSOR_POSITION_REPLY) {
                    info!("cursor-position reply not written: {e:#}");
                }
            }
            self.login_watch.feed(&chunk);
            self.api_watch.feed(&chunk);
            tee_to_log(self.log.as_mut(), &chunk);
        }
    }

    /// The transcript-paced checks: usage limit, login TUI, API watch and idle
    /// watchdog. `Some` when one of them ends the session.
    fn poll_transcript(&mut self) -> Result<Option<SessionExit>> {
        let advanced;
        if let Some(t) = latest_transcript_text_since(self.transcript_dir, self.transcript_since) {
            // Any transcript activity proves the model loop started —
            // a logged-out session never produces one (see LoginTuiWatch).
            self.login_watch.disarm();
            advanced = t.len() > self.last_len;
            self.last_len = t.len();
            if transcript_limit(&t).is_some() {
                return Ok(Some(SessionExit::Limit(t)));
            }
        } else {
            advanced = false;
            if self.login_watch.detected() {
                // The login TUI stalls without exiting: fail fast with the auth
                // message, not a misleading `Timeout` (issue #72).
                bail!("{CLAUDE_AUTH_ERROR_MSG}");
            }
        }
        if advanced {
            self.idle_beat.beat(Instant::now());
        }
        match self.api_watch.poll(Instant::now(), advanced) {
            ApiWatchAction::Degraded => ralphy_core::emit::api_degraded(),
            ApiWatchAction::Recovered => ralphy_core::emit::api_recovered(),
            ApiWatchAction::Respawn => return Ok(Some(SessionExit::Respawn)),
            ApiWatchAction::None => {}
        }
        // No re-spawn here, unlike the API-banner path: a retry banner shows the
        // child is trying, whereas silence shows nothing.
        if self.idle_watch.expired(&self.idle_beat, Instant::now()) {
            // The headless path calls the same helper (docs/adr/0038).
            ralphy_core::emit::idle_reaped(
                self.idle_watch.window().map_or(0, |w| w.as_secs() / 60),
            );
            return Ok(Some(SessionExit::TimedOut));
        }
        self.next_transcript_poll = Instant::now() + Duration::from_secs(2);
        Ok(None)
    }
}

/// Turn the end of a session (other than a respawn) into an [`Outcome`].
fn classify_exit(
    exit: SessionExit,
    flag_file: &Path,
    transcript_dir: Option<&Path>,
    transcript_since: SystemTime,
) -> Result<Outcome> {
    let child_exited = matches!(exit, SessionExit::ChildExited);
    let timed_out = matches!(exit, SessionExit::TimedOut);
    let stopped = matches!(exit, SessionExit::Stopped);
    let limit_transcript = match exit {
        SessionExit::Limit(t) => Some(t),
        SessionExit::Flag
        | SessionExit::Stopped
        | SessionExit::TimedOut
        | SessionExit::ChildExited
        | SessionExit::Respawn => None,
    };

    let flag = std::fs::read_to_string(flag_file).ok();
    // A transcript read is needed to spot a usage limit when the session
    // ends without a sentinel, and the live loop also watches for the
    // Claude CLI's subagent/tool-result rate-limit shape while the PTY stays
    // alive.
    let transcript = if flag.is_none() {
        limit_transcript.or_else(|| {
            ended_transcript(child_exited || timed_out, transcript_dir, transcript_since)
        })
    } else {
        None
    };

    // An auth failure in the transcript takes precedence over classification:
    // it won't self-heal (unlike a usage limit), so surface it immediately.
    if child_exited && flag.is_none() && transcript.as_deref().is_some_and(is_claude_auth_error) {
        bail!("{CLAUDE_AUTH_ERROR_MSG}");
    }

    let outcome = classify_outcome(flag.as_deref(), timed_out, transcript.as_deref());
    info!(
        ?outcome,
        child_exited, timed_out, stopped, "execution session ended"
    );
    Ok(outcome)
}

/// The latest transcript text of a session that has ended: this session's
/// transcript first, then the newest one anywhere.
fn ended_transcript(
    ended: bool,
    transcript_dir: Option<&Path>,
    transcript_since: SystemTime,
) -> Option<String> {
    if !ended {
        return None;
    }
    latest_transcript_text_since(transcript_dir, transcript_since).or_else(latest_transcript_text)
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

/// Copies a chunk of console output to the transcript log, when there is one.
///
/// The log is a copy for the operator, not the run, so a failed write is only logged.
fn tee_to_log(log: Option<&mut std::fs::File>, chunk: &[u8]) {
    if let Some(f) = log {
        if let Err(e) = f.write_all(chunk) {
            info!("transcript log write failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests;
