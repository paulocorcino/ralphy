//! The two phases of a run, `plan` and `execute`, behind the core [`Agent`] contract.

use std::fs;

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{run_exec_session, run_plan_session, ExecCfg, PlanCfg};
use ralphy_core::{git, plan, Agent, Execution, Issue, Plan, Workspace};
use ralphy_proc_util::opencode::OpenCodeMajor;
use regex::Regex;
use tracing::info;

use crate::events::{
    is_opencode_auth_error, no_route_message, usage_limit_regex, OpencodeLimit,
    OPENCODE_AUTH_ERROR_MSG,
};
use crate::outcome::{classify_opencode_outcome, CallRead};
use crate::skills::materialize_opencode_skills;
use crate::usage::{opencode_usage, resolved_model_label, session_id_from_stream};
use crate::{OpenCodeAgent, PROMPT_PLAN_OPENCODE};

impl Agent for OpenCodeAgent {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        let log_path = self.run_dir.join("opencode.log");

        let run = || {
            let skills_dir = materialize_opencode_skills(ws)?;
            // ADR-0044 D4 No-op: resolved `--plan-effort` accepted at the CLI,
            // discarded here — must not alter argv; emit effort "". `--variant`
            // stays `--exec-variant`-only (D8).
            let _: Option<&str> = self.plan_effort.as_deref();
            ralphy_core::emit::planning(
                "opencode run",
                self.model.as_deref().unwrap_or(""),
                "",
                self.variant.as_deref().unwrap_or(""),
            );
            // No early-kill on plan: plan surfaces no usage limit today (the detector
            // below is `|_log| None`), so there is nothing for a kill to hand off to.
            // The call still streams the log live, like execute.
            let major = self.located()?.version.major;
            let r = self.run_watched(
                &skills_dir,
                ws.repo_root(),
                ralphy_adapter_support::PLAN_CHARTER,
                |_| false,
            )?;
            stop_on_no_route(major, &r.stdout)?;
            let stdout = r.stdout.clone();
            Ok((r, stdout))
        };

        let ralphy_dir = ws.ralphy_dir();
        let charter_path = ws.plan_charter_path();
        let session = run_plan_session(
            PlanCfg {
                issue_number: issue.number,
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                plan_path: &plan_path,
                plan_charter_path: &charter_path,
                charter_body: PROMPT_PLAN_OPENCODE,
                log_path: &log_path,
                auth_msg: OPENCODE_AUTH_ERROR_MSG,
                no_plan_msg: "opencode produced no plan",
            },
            run,
            is_opencode_auth_error,
            // No plan-time usage limit is surfaced for OpenCode today (the current
            // ladder is auth-then-generic); the limit path lives in execute (D9).
            |_log| None,
        )?;

        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        // None = resumed (finalized plan kept, no vendor run): no stdout to parse,
        // so report zero planning tokens.
        let (usage, session_id) = match session {
            Some((_, stdout_text)) => (
                opencode_usage(&stdout_text),
                session_id_from_stream(&stdout_text),
            ),
            None => (ralphy_core::Usage::default(), None),
        };
        info!(
            model = resolved_model_label(&usage),
            "opencode plan resolved model"
        );
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // OpenCode drops complexity routing (ADR-0005 D3), so there is no tier.
            recommended_model: None,
            path: plan_path,
            usage,
            session_id,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let log_path = self.run_dir.join("opencode.log");
        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();

        let run = || {
            let skills_dir = materialize_opencode_skills(ws)?;
            // ADR-0044 D4 No-op: resolved `--exec-effort` accepted at the CLI,
            // discarded here — must not alter argv; emit effort "". `--variant`
            // stays `--exec-variant`-only (D8).
            let _: Option<&str> = self.exec_effort.as_deref();
            ralphy_core::emit::executing(
                "opencode run",
                0,
                self.model.as_deref().unwrap_or(""),
                "",
                self.variant.as_deref().unwrap_or(""),
            );
            let major = self.located()?.version.major;
            let early_kill = stderr_early_kill(major);
            let r = self.run_watched(
                &skills_dir,
                ws.repo_root(),
                ralphy_adapter_support::PROMPT_EXECUTE,
                move |line| early_kill.as_ref().is_some_and(|re| re.is_match(line)),
            )?;
            stop_on_no_route(major, &r.stdout)?;
            let stdout = r.stdout.clone();
            Ok((r, (stdout, major)))
        };

        let ralphy_dir = ws.ralphy_dir();
        let (r, (stdout_text, major)) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: OPENCODE_AUTH_ERROR_MSG,
            },
            run,
            is_opencode_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let read = CallRead::new(major, &stdout_text, &r.log);
        // OpenCode fronts N providers whose reset strings are unreliable: a "5 hour"
        // limit reporting a reset ~12h out (FinCal #73, glm-5.2) is internally
        // impossible, yet parses cleanly and would park the run on a bogus instant.
        // Never schedule on an OpenCode reset — keep detecting the limit but discard
        // the hint so the runner falls to the ~30-min synthetic cadence (ADR-0030 D1)
        // instead of honouring it. Trustworthiness is a per-vendor extraction decision
        // (the `classify` seam, ADR-0023); Claude/Codex are stable and keep their hints.
        if let Some(OpencodeLimit { reset: Some(reset) }) = &read.limit {
            info!(
                %reset,
                "opencode reported a usage-limit reset — discarding \
                 (OpenCode resets are unschedulable by policy)"
            );
        }

        let outcome =
            classify_opencode_outcome(r.exited_cleanly, r.timed_out, committed, read.reported());
        let usage = opencode_usage(&stdout_text);
        info!(
            ?outcome,
            model = resolved_model_label(&usage),
            exited_cleanly = r.exited_cleanly,
            timed_out = r.timed_out,
            committed,
            saw_error = read.saw_error(),
            "opencode execution ended"
        );
        Ok(Execution {
            outcome,
            usage,
            session_id: session_id_from_stream(&stdout_text),
        })
    }
}

/// The stderr early kill of an execute. On 1.x a provider quota block prints
/// only to `--print-logs` stderr, then the child idles in silent backoff until
/// the wall timeout (`FinCal` #71/#73, glm-5.2); the match is the shape the
/// post-run `parse_opencode_log_limit` keys on, so a killed run still ends
/// `Limit`. On 2.x the limit is a stdout event that ends the run, and stderr
/// holds the server's `level=INFO` lines (ADR-0094 D4g).
fn stderr_early_kill(major: OpenCodeMajor) -> Option<Regex> {
    match major {
        OpenCodeMajor::V1 => Some(usage_limit_regex()),
        OpenCodeMajor::V2 => None,
    }
}

/// Stop a 2.x call whose model has no route, before the shared classifier
/// (ADR-0094 D4d): its two causes are a missing login or a wrong model id.
fn stop_on_no_route(major: OpenCodeMajor, stdout: &str) -> Result<()> {
    if let Some(message) = no_route_message(major, stdout) {
        bail!(
            "opencode cannot run the model ({message}): log in to the model's provider \
             with `opencode auth login`, or check the model id with `opencode models`"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use ralphy_proc_util::opencode::OpenCodeVersionError;

    use super::*;

    /// An agent whose version read gives `error`, and a count of its spawns.
    fn refused(
        run_dir: std::path::PathBuf,
        error: OpenCodeVersionError,
    ) -> (OpenCodeAgent, Arc<AtomicUsize>) {
        let spawns = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&spawns);
        let agent = OpenCodeAgent::new(None, run_dir)
            .with_version(move || Err(error.clone()))
            .with_run_hook(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("the run hook must not be called")
            });
        (agent, spawns)
    }

    fn workspace(temp: &tempfile::TempDir) -> Workspace {
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("a scratch repo");
        Workspace::new(&repo)
    }

    #[test]
    fn a_third_major_stops_the_run_before_the_spawn() {
        let temp = tempfile::tempdir().expect("a scratch folder");
        let ws = workspace(&temp);
        let error = OpenCodeVersionError::Unsupported {
            text: "3.0.0".into(),
        };
        let (agent, spawns) = refused(temp.path().join("run"), error);
        let issue = Issue {
            number: 1,
            title: "t".into(),
            body: String::new(),
            labels: Vec::new(),
            comments: Vec::new(),
        };
        let err = agent
            .plan(&issue, &ws)
            .expect_err("a refused version fails the plan");
        assert!(format!("{err:#}").contains("3.0.0"), "{err:#}");
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_unreadable_version_stops_the_run_before_the_spawn() {
        let temp = tempfile::tempdir().expect("a scratch folder");
        let ws = workspace(&temp);
        let error = OpenCodeVersionError::Unreadable {
            program: "opencode".into(),
        };
        let want = error.to_string();
        let (agent, spawns) = refused(temp.path().join("run"), error);
        let plan = Plan {
            path: ws.plan_path(),
            open_steps: 1,
            recommended_model: None,
            usage: ralphy_core::Usage::default(),
            session_id: None,
        };
        let err = agent
            .execute(&plan, &ws)
            .expect_err("a refused version fails the execute");
        assert!(format!("{err:#}").contains(&want), "{err:#}");
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_stderr_early_kill_is_for_1x_only() {
        let v1 = stderr_early_kill(OpenCodeMajor::V1).expect("1.x has a stderr early kill");
        assert!(v1.is_match(r#"error.error="AI_APICallError: Usage limit reached""#));
        assert!(stderr_early_kill(OpenCodeMajor::V2).is_none());
    }
}
