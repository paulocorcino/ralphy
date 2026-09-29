//! `POST /api/release/update`: the workbench asks for the update, and the daemon
//! hands over to `ralphy update --handoff` (ADR-0056 §11).
//!
//! The daemon downloads nothing itself. It starts the child, reads the child's
//! output until the child says the new binary is in place, answers the page,
//! and then shuts down by itself. The child starts the new daemon. Nothing here
//! kills a process tree: the child is in this daemon's tree, so a tree kill
//! would end the update half-way.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Form;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::api_security::step_up;
use super::now_unix;
use crate::{auth, dispatch, release, serve};

/// How long the page's answer gets to leave before the listener stops.
const ANSWER_GRACE: Duration = Duration::from_millis(250);
/// A graceful shutdown waits for open connections. If one never closes, the
/// process exits anyway, well inside the time the child waits for it.
const EXIT_FALLBACK: Duration = Duration::from_secs(20);
/// How many of the child's last lines a failure reports.
const FAILURE_LINES: usize = 8;

/// One update at a time. A second click while the first downloads must not
/// start a second child that replaces the same binary.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// The `POST /api/release/update` body: the current TOTP code, when a seed is
/// armed.
#[derive(serde::Deserialize)]
pub(crate) struct UpdateForm {
    pub(crate) code: Option<String>,
}

/// Whether the workbench can offer the update. Only a build that is behind has
/// something to take. Under systemd the child would be ended with the daemon.
pub(crate) fn updatable(standing: &str, under_systemd: bool) -> bool {
    standing == "behind" && !under_systemd
}

pub(crate) async fn release_update_route(
    state: Arc<auth::AuthState>,
    release_store: Option<PathBuf>,
    headers: HeaderMap,
    Form(form): Form<UpdateForm>,
) -> Response {
    // A bearer token is how a machine client authenticates. The update ends
    // every console, so it is an operator action.
    if headers.contains_key(header::AUTHORIZATION) {
        return (
            StatusCode::FORBIDDEN,
            "only the operator can start the update, not a machine client",
        )
            .into_response();
    }
    let dir = match auth::store_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(error = %e, "failed to resolve the daemon store");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store unavailable").into_response();
        }
    };
    // Before the step-up, so a refusal here does not spend the operator's code.
    let standing = standing_in(release_store.as_deref());
    if !updatable(&standing, release::under_systemd()) {
        return (
            StatusCode::CONFLICT,
            "this daemon cannot update from the workbench; run `ralphy update` in a terminal",
        )
            .into_response();
    }
    if let Err(refused) =
        step_up::require_fresh_totp(&state, &dir, form.code.as_deref(), now_unix())
    {
        return refused.into_response();
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return (StatusCode::CONFLICT, "an update is already running").into_response();
    }

    let program = dispatch::ralphy_exe();
    let pid = std::process::id();
    let outcome = tokio::task::spawn_blocking(move || {
        hand_over(&dispatch::ProcessSpawner, &program, &dir, pid)
    })
    .await;
    match outcome {
        Ok(Ok(())) => {
            tokio::spawn(async {
                tokio::time::sleep(ANSWER_GRACE).await;
                serve::request_stop();
                tokio::time::sleep(EXIT_FALLBACK).await;
                tracing::warn!("a connection held the shutdown open; exiting for the update");
                std::process::exit(0);
            });
            Json(serde_json::json!({ "restarting": true })).into_response()
        }
        Ok(Err(why)) => {
            RUNNING.store(false, Ordering::SeqCst);
            tracing::warn!(error = %why, "the update did not start");
            (StatusCode::INTERNAL_SERVER_ERROR, why).into_response()
        }
        Err(e) => {
            RUNNING.store(false, Ordering::SeqCst);
            tracing::warn!(error = %e, "the update task did not complete");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the update task did not complete",
            )
                .into_response()
        }
    }
}

/// Where this build stands against the cached releases.
fn standing_in(store: Option<&Path>) -> String {
    let releases = match store {
        Some(dir) => ralphy_release::fetch::load(&release::cache_path_in(dir)),
        None => Vec::new(),
    };
    release::view(
        env!("RALPHY_VERSION"),
        &releases,
        ralphy_release::Channel::Rc,
        false,
    )
    .standing
}

/// Start `ralphy update --handoff <pid>` and read its output until it says the
/// new binary is in place. `Ok` means the daemon must now shut down. `Err`
/// carries the child's last lines, and the daemon keeps running on the binary
/// it has.
///
/// The child is left running on `Ok`: it waits for this process to exit. After
/// the marker it writes only to its log, so the pipe this drops is never
/// written again.
fn hand_over(
    spawner: &dyn dispatch::Spawner,
    program: &OsStr,
    store: &Path,
    daemon_pid: u32,
) -> Result<(), String> {
    let pid = daemon_pid.to_string();
    let argv = ["update", "--handoff", pid.as_str()];
    let mut child = spawner
        .spawn(program, &argv, store, None)
        .map_err(|e| format!("could not start the update: {e:#}"))?;
    let output = child
        .take_output()
        .ok_or_else(|| "the update started with no output to read".to_string())?;

    let mut last: Vec<String> = Vec::new();
    for line in BufReader::new(output).lines() {
        let line = line.map_err(|e| format!("reading the update's output: {e}"))?;
        if line.trim() == release::HANDOFF_READY {
            return Ok(());
        }
        if !line.trim().is_empty() {
            last.push(line);
            if last.len() > FAILURE_LINES {
                last.remove(0);
            }
        }
    }
    let code = child
        .wait()
        .map_err(|e| format!("waiting for the update: {e:#}"))?;
    if last.is_empty() {
        let code = code.map_or_else(|| "none".to_string(), |c| c.to_string());
        return Err(format!(
            "the update stopped with no message (exit code {code})"
        ));
    }
    Err(last.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child whose whole output is `text`, exiting with `code`.
    struct Scripted {
        output: Option<Vec<u8>>,
        code: Option<i32>,
    }

    impl dispatch::Child for Scripted {
        fn pid(&self) -> Option<u32> {
            Some(4242)
        }
        fn wait(&mut self) -> anyhow::Result<Option<i32>> {
            Ok(self.code)
        }
        fn take_output(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
            self.output
                .take()
                .map(|b| Box::new(std::io::Cursor::new(b)) as _)
        }
    }

    /// Records the argv it was asked to run and answers with `text`.
    struct Fake {
        text: &'static str,
        code: Option<i32>,
        argv: std::sync::Mutex<Vec<String>>,
    }

    impl dispatch::Spawner for Fake {
        fn spawn(
            &self,
            _program: &OsStr,
            args: &[&str],
            _cwd: &Path,
            _daemon_id: Option<&str>,
        ) -> anyhow::Result<Box<dyn dispatch::Child>> {
            let mut argv = self.argv.lock().expect("the argv lock is never poisoned");
            *argv = args.iter().map(|a| a.to_string()).collect();
            Ok(Box::new(Scripted {
                output: Some(self.text.as_bytes().to_vec()),
                code: self.code,
            }))
        }
    }

    fn fake(text: &'static str, code: Option<i32>) -> Fake {
        Fake {
            text,
            code,
            argv: std::sync::Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn the_marker_hands_over_and_names_this_daemon() {
        let text = "taking v0.1.0-rc.31\nchecksum ok\nralphy update: the new binary is in place\n";
        let spawner = fake(text, None);
        hand_over(&spawner, OsStr::new("ralphy"), Path::new("."), 777)
            .expect("the marker means the daemon must shut down");
        assert_eq!(
            *spawner.argv.lock().expect("lock"),
            ["update", "--handoff", "777"],
            "the child must be told which daemon to wait for"
        );
    }

    #[test]
    fn a_child_that_ends_without_the_marker_keeps_the_daemon_and_says_why() {
        let text = "taking v0.1.0-rc.31\nError: checksum mismatch: expected aa, got bb\n";
        let why = hand_over(
            &fake(text, Some(1)),
            OsStr::new("ralphy"),
            Path::new("."),
            1,
        )
        .expect_err("no marker means the daemon keeps running");
        assert!(
            why.ends_with("checksum mismatch: expected aa, got bb"),
            "{why}"
        );
    }

    #[test]
    fn a_silent_failure_still_names_its_exit_code() {
        let why = hand_over(&fake("", Some(3)), OsStr::new("ralphy"), Path::new("."), 1)
            .expect_err("no marker");
        assert!(why.contains("exit code 3"), "{why}");
    }

    #[test]
    fn only_a_build_that_is_behind_and_not_under_systemd_is_offered_the_update() {
        assert!(updatable("behind", false));
        assert!(
            !updatable("behind", true),
            "systemd ends the child with the daemon"
        );
        for standing in ["level", "ahead", "unknown"] {
            assert!(
                !updatable(standing, false),
                "{standing} has nothing to take"
            );
        }
    }
}
