//! The Antigravity adapter of a run: its per-phase models and efforts
//! resolved once (flag, then settings), and the agent built with the run's
//! budgets and the guard-proof waiver (ADR-0086 D4, D9).

use std::path::PathBuf;
use std::time::Instant;

use ralphy_agent_antigravity::{AntigravityAgent, AntigravitySettings};

use super::{effort_strings, ResolvedClaude, ResolvedEffort};
use crate::config;

/// The Antigravity models and efforts of a run, resolved once so the executor
/// and a split planner share one value. `None` on a model omits `--model`, and
/// `agy` uses the default of the operator's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedAntigravity {
    pub(crate) plan_model: Option<String>,
    pub(crate) exec_model: Option<String>,
    pub(crate) plan_effort: Option<String>,
    pub(crate) exec_effort: Option<String>,
    /// Persisted only: a per-run flag would make a session without the hook
    /// proof a one-keystroke decision.
    pub(crate) run_without_guard_proof: bool,
}

/// Resolve each phase: its flag, then `antigravity.*` in the settings. The
/// effort flags are the run's resolved `--plan-effort` and `--exec-effort`.
pub(crate) fn resolve_antigravity(
    plan_flag: Option<String>,
    exec_flag: Option<String>,
    effort: ResolvedEffort,
    persisted: &AntigravitySettings,
) -> ResolvedAntigravity {
    let (plan_effort, exec_effort) = effort_strings(effort);
    ResolvedAntigravity {
        plan_model: config::resolve_optional_model(plan_flag, persisted.plan_model.clone()),
        exec_model: config::resolve_optional_model(exec_flag, persisted.exec_model.clone()),
        plan_effort: plan_effort.or_else(|| persisted.plan_effort.clone()),
        exec_effort: exec_effort.or_else(|| persisted.exec_effort.clone()),
        run_without_guard_proof: persisted.run_without_guard_proof_i_understand_the_risk,
    }
}

/// The Antigravity agent of a run, writing its log into `run_dir`, with the
/// run's budgets: the issue minutes are `claude.max_minutes_per_issue`, as for
/// every headless adapter.
pub(crate) fn agent(
    resolved: &ResolvedAntigravity,
    claude: &ResolvedClaude,
    run_dir: PathBuf,
    run_deadline: Option<Instant>,
    idle_minutes: u64,
) -> AntigravityAgent {
    AntigravityAgent::new(resolved.exec_model.clone(), run_dir)
        .with_plan_model(resolved.plan_model.clone())
        .with_plan_effort(resolved.plan_effort.clone())
        .with_exec_effort(resolved.exec_effort.clone())
        .with_run_without_guard_proof(resolved.run_without_guard_proof)
        .with_run_deadline(run_deadline)
        .with_max_minutes_per_issue(claude.max_minutes_per_issue)
        .with_idle_minutes(idle_minutes)
}

/// [`agent`], boxed for the run.
pub(crate) fn boxed(
    resolved: &ResolvedAntigravity,
    claude: &ResolvedClaude,
    run_dir: PathBuf,
    run_deadline: Option<Instant>,
    idle_minutes: u64,
) -> Box<dyn ralphy_core::Agent> {
    Box::new(agent(resolved, claude, run_dir, run_deadline, idle_minutes))
}

#[cfg(test)]
mod tests {
    use ralphy_core::Effort;

    use super::*;

    fn none() -> ResolvedEffort {
        ResolvedEffort {
            plan: None,
            exec: None,
        }
    }

    /// `build_agent`'s arm is this builder; `--agent antigravity` reaching the
    /// arm is `build_agent_builds_every_agent_the_cli_accepts`.
    #[test]
    fn build_agent_builds_antigravity_with_both_phase_models() {
        use clap::Parser;
        let words = ["ralphy", "run", "--agent", "antigravity"];
        let models = ["--plan-model", "plan-id", "--exec-model", "exec-id"];
        let cli = crate::cli::Cli::try_parse_from(words.into_iter().chain(models))
            .expect("the run arguments parse");
        let crate::cli::Command::Run(args) = cli.command else {
            panic!("not a run command");
        };
        let saved = AntigravitySettings::default();
        let resolved = resolve_antigravity(args.plan_model, args.exec_model, none(), &saved);
        let claude = ResolvedClaude {
            plan_model: String::new(),
            default_exec_model: String::new(),
            max_minutes_per_issue: 30,
            remote_control: false,
            run_without_guard_proof: false,
        };
        let agent = agent(&resolved, &claude, PathBuf::from("run"), None, 20);
        assert_eq!(ralphy_core::Agent::name(&agent), "antigravity");
        let shown = format!("{agent:?}");
        for field in [
            r#"plan_model: Some("plan-id")"#,
            r#"exec_model: Some("exec-id")"#,
        ] {
            assert!(shown.contains(field), "{field} missing: {shown}");
        }
    }

    #[test]
    fn resolve_antigravity_prefers_the_flag_over_settings() {
        let saved = AntigravitySettings {
            plan_model: Some("saved-plan".into()),
            exec_model: Some("saved-exec".into()),
            plan_effort: Some("low".into()),
            exec_effort: Some("low".into()),
            run_without_guard_proof_i_understand_the_risk: true,
        };
        let flags = ResolvedEffort {
            plan: Some(Effort::High),
            exec: None,
        };
        let r = resolve_antigravity(Some("flag-plan".into()), None, flags, &saved);
        assert_eq!(r.plan_model.as_deref(), Some("flag-plan"));
        assert_eq!(r.exec_model.as_deref(), Some("saved-exec"));
        assert_eq!(r.plan_effort.as_deref(), Some("high"));
        assert_eq!(r.exec_effort.as_deref(), Some("low"));
        assert!(r.run_without_guard_proof);

        let bare = resolve_antigravity(None, None, none(), &AntigravitySettings::default());
        assert_eq!(bare.plan_model, None);
        assert_eq!(bare.exec_model, None);
        assert!(!bare.run_without_guard_proof, "the waiver defaults off");
    }
}
