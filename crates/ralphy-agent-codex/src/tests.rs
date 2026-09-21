use super::*;
use ralphy_adapter_support::HeadlessRun;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn clean_run() -> HeadlessRun {
    HeadlessRun {
        stdout: String::new(),
        log: String::new(),
        exited_cleanly: true,
        timed_out: false,
        idle_killed: false,
        stopped: false,
        exit_code: Some(0),
    }
}

fn rollout(model_tokens: u64) -> String {
    format!(
            "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{model_tokens},\"cached_input_tokens\":0,\"output_tokens\":0}}}}}}}}"
        )
}

fn serialized_ledger_model(usage: &ralphy_core::Usage) -> String {
    let record = ralphy_core::ledger::LedgerRecord {
        project: "owner/repo".into(),
        actor_email: "dev@example.com".into(),
        actor_name: "Dev".into(),
        ralphy_version: "test".into(),
        issue: 356,
        phase: "test".into(),
        agent: "codex".into(),
        model: usage.model.clone().unwrap_or_else(|| "unknown".into()),
        session_id: Some("session".into()),
        outcome: "done".into(),
        tokens: usage.clone(),
        ts: "2026-07-29T00:00:00Z".into(),
    };
    let value: serde_json::Value =
        serde_json::from_str(&ralphy_core::ledger::record_line(&record).unwrap()).unwrap();
    value["model"].as_str().unwrap().to_string()
}

#[test]
fn plan_and_execute_agent_paths_serialize_the_resolved_model() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let restore = std::env::var_os("CODEX_HOME");
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let codex_home = temp.path().join("codex");
    let sessions = codex_home.join("sessions");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&sessions).unwrap();
    std::env::set_var("CODEX_HOME", &codex_home);
    let ws = Workspace::new(&repo);
    let plan_path = ws.plan_path();
    let plan_rollout = sessions.join("rollout-plan.jsonl");
    let plan_agent = CodexAgent::new(Some("gpt-5-codex".into()), temp.path().join("plan-run"))
        .with_run_hook(move || {
            std::fs::write(
                &plan_path,
                "# Plan\n## Execution model: high\n## Steps\n- [ ] implement\n",
            )?;
            std::fs::write(&plan_rollout, rollout(10))?;
            Ok(clean_run())
        });
    let issue = Issue {
        number: 356,
        title: "model recovery".into(),
        body: String::new(),
        labels: Vec::new(),
        comments: Vec::new(),
    };

    let plan = Agent::plan(&plan_agent, &issue, &ws).unwrap();
    assert_eq!(plan.usage.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(serialized_ledger_model(&plan.usage), "gpt-5-codex");

    let execute_rollout = sessions.join("rollout-execute.jsonl");
    let out_path = ws.ralphy_dir().join("codex-last.txt");
    let execute_agent =
        CodexAgent::new(Some("gpt-5-codex".into()), temp.path().join("execute-run")).with_run_hook(
            move || {
                std::fs::write(&execute_rollout, rollout(20))?;
                std::fs::write(&out_path, "RALPHY_DONE_EXIT\n")?;
                Ok(clean_run())
            },
        );

    let execution = Agent::execute(&execute_agent, &plan, &ws).unwrap();
    assert_eq!(execution.usage.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(serialized_ledger_model(&execution.usage), "gpt-5-codex");

    match restore {
        Some(value) => std::env::set_var("CODEX_HOME", value),
        None => std::env::remove_var("CODEX_HOME"),
    }
}

/// The log shape that threw away a finished #356 execute: a repo grep echoed
/// this adapter's own `auth.rs` — [`CODEX_AUTH_ERROR_MSG`] and its literal
/// `401 Unauthorized` — into the child's combined stdout+stderr.
fn log_echoing_this_crates_auth_source() -> String {
    format!(
        "crates/ralphy-agent-codex/src\\auth.rs-6-pub(crate) const CODEX_AUTH_ERROR_MSG: &str =\n\
             crates/ralphy-agent-codex/src\\auth.rs-7-    \"{CODEX_AUTH_ERROR_MSG}\";\n"
    )
}

fn done_plan(ws: &Workspace) -> Plan {
    Plan {
        path: ws.plan_path(),
        open_steps: 1,
        recommended_model: Some("high".into()),
        usage: ralphy_core::Usage::default(),
        session_id: None,
    }
}

/// A green execution must survive the agent reading source that documents the
/// auth banner — and a genuinely failed one must still say `codex login`.
#[test]
fn a_clean_execute_outlives_its_own_auth_message_in_the_log() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let restore = std::env::var_os("CODEX_HOME");
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let codex_home = temp.path().join("codex");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(codex_home.join("sessions")).unwrap();
    std::env::set_var("CODEX_HOME", &codex_home);
    let ws = Workspace::new(&repo);
    let log = log_echoing_this_crates_auth_source();
    // The detector DOES fire on this text: what protects the run is the
    // failed-run gate in `run_exec_session`, not the needle.
    assert!(is_codex_auth_error(&log));

    let sentinel = ws.ralphy_dir().join("codex-last.txt");
    let clean_log = log.clone();
    let green = CodexAgent::new(Some("gpt-5-codex".into()), temp.path().join("green-run"))
        .with_run_hook(move || {
            std::fs::write(&sentinel, "RALPHY_DONE_EXIT\n")?;
            Ok(HeadlessRun {
                log: clean_log.clone(),
                ..clean_run()
            })
        });
    let execution = Agent::execute(&green, &done_plan(&ws), &ws)
        .expect("a clean run that merely read auth.rs is not an auth failure");
    assert_eq!(execution.outcome, ralphy_core::Outcome::Done);

    // Negative control: the same text from a run the CLI failed is the real
    // signal, and still bails with the actionable message.
    let signed_out = CodexAgent::new(
        Some("gpt-5-codex".into()),
        temp.path().join("signed-out-run"),
    )
    .with_run_hook(move || {
        Ok(HeadlessRun {
            log: log.clone(),
            exited_cleanly: false,
            exit_code: Some(1),
            ..clean_run()
        })
    });
    let err = Agent::execute(&signed_out, &done_plan(&ws), &ws).unwrap_err();
    assert!(err.to_string().contains(CODEX_AUTH_ERROR_MSG), "got: {err}");

    match restore {
        Some(value) => std::env::set_var("CODEX_HOME", value),
        None => std::env::remove_var("CODEX_HOME"),
    }
}

// ── with_max_minutes_per_issue ──────────────────────────────────────────

#[test]
fn codex_honours_max_minutes_per_issue() {
    assert_eq!(
        CodexAgent::new(None, PathBuf::from("/run"))
            .budget
            .max_minutes_per_issue,
        ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
    );
    let a = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
    assert_eq!(a.budget.max_minutes_per_issue, 120);
    let short = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
    let long = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
    assert!(long.issue_deadline() > short.issue_deadline());
    let rd = Instant::now() + Duration::from_secs(1);
    let clamped = CodexAgent::new(None, PathBuf::from("/run"))
        .with_max_minutes_per_issue(1000)
        .with_run_deadline(Some(rd));
    assert!(clamped.issue_deadline() <= rd);
}

#[test]
fn codex_zero_minutes_disables_the_per_issue_cap() {
    // `0` → no per-issue cap: the deadline sits at the far-future horizon,
    // well past any finite budget.
    let uncapped = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(0);
    let capped = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
    assert!(uncapped.issue_deadline() > capped.issue_deadline());

    // …but an uncapped issue is still bounded by the run deadline when set.
    let rd = Instant::now() + Duration::from_secs(1);
    let bounded = CodexAgent::new(None, PathBuf::from("/run"))
        .with_max_minutes_per_issue(0)
        .with_run_deadline(Some(rd));
    assert!(bounded.issue_deadline() <= rd);
}

// ── effort → model_reasoning_effort ─────────────────────────────────────

#[test]
fn exec_effort_override_wins_over_the_tier_default() {
    // An explicit `--exec-effort high` beats the tier's routed default on
    // every issue — even when the tier (here `medium`→Terra:medium) would
    // otherwise route to `medium`.
    let agent = CodexAgent::new(None, PathBuf::from("/run")).with_exec_effort(Some("high".into()));
    assert_eq!(agent.resolved_exec_effort("medium"), "high");
    let cmd = build_codex_command(
        CODEX_MODEL_SOL,
        agent.resolved_exec_effort("medium"),
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo/out.txt"),
    );
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter().any(|a| a == "model_reasoning_effort=\"high\""),
        "argv must carry the operator's high effort: {args:?}"
    );
}

#[test]
fn unset_exec_effort_falls_back_to_the_tier_default() {
    // With no `--exec-effort`, the effort is whatever the tier routed — no
    // longer a flat `medium`. `xhigh`→Sol:high proves the high rung reaches
    // the argv unaided by an operator flag.
    let agent = CodexAgent::new(None, PathBuf::from("/run"));
    let (model, tier_effort) = command::tier_to_model_effort(Some("xhigh"));
    assert_eq!(agent.resolved_exec_effort(tier_effort), "high");
    let cmd = build_codex_command(
        model,
        agent.resolved_exec_effort(tier_effort),
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo/out.txt"),
    );
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter().any(|a| a == "model_reasoning_effort=\"high\""),
        "unset effort must inherit the tier's routed effort: {args:?}"
    );
}

#[test]
fn plan_and_execute_use_the_resolved_effort_helpers() {
    // Pins the production call sites to the same helpers the argv tests drive —
    // a plan/execute that ignores stored fields would otherwise stay green.
    let prod = include_str!("lib.rs");
    assert!(
        prod.contains("let effort = self.resolved_plan_effort();"),
        "plan must bind effort via resolved_plan_effort"
    );
    assert!(
        prod.contains("let effort = self.resolved_exec_effort(routed_effort);"),
        "execute must bind effort via resolved_exec_effort(routed_effort)"
    );
}

#[test]
fn effort_does_not_alter_the_tier_routed_model() {
    assert_eq!(
        tier_to_model_effort(Some("low")).0,
        command::CODEX_MODEL_LUNA
    );
    assert_eq!(
        tier_to_model_effort(Some("medium")).0,
        command::CODEX_MODEL_TERRA
    );
    assert_eq!(tier_to_model_effort(Some("high")).0, CODEX_MODEL_SOL);
    assert_eq!(tier_to_model_effort(Some("xhigh")).0, CODEX_MODEL_SOL);

    // A fixed model id in `-m` is unchanged when only the effort argv varies —
    // effort couples to the tier as a DEFAULT, but the `-m` column is set by
    // the model, never by the effort word.
    for effort in ["low", "high"] {
        let cmd = build_codex_command(
            command::CODEX_MODEL_TERRA,
            effort,
            std::path::Path::new("/repo"),
            std::path::Path::new("/repo/out.txt"),
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let m = args.iter().position(|a| a == "-m").expect("-m present");
        assert_eq!(
            args[m + 1],
            command::CODEX_MODEL_TERRA,
            "effort={effort} must not alter -m: {args:?}"
        );
    }
}

// ── resolve_model ───────────────────────────────────────────────────────

#[test]
fn resolve_model_override_wins() {
    // The explicit --exec-model override wins over config and the tier-routed
    // fallback, with no dependence on the machine's Codex config.
    let overridden = CodexAgent::new(Some("gpt-5".into()), PathBuf::from("/run"));
    assert_eq!(overridden.resolve_model(CODEX_MODEL_SOL), "gpt-5");
    assert_eq!(
        overridden.resolve_model(command::tier_to_model_effort(Some("low")).0),
        "gpt-5"
    );
}

// ── trait binding (compile-level) ───────────────────────────────────────

#[test]
fn codex_agent_is_a_dyn_agent() {
    // Proves `CodexAgent: Agent` and that it can be handed to the core as a
    // `&dyn Agent` (the core never learns the vendor).
    let agent = CodexAgent::new(None, PathBuf::from("/run"));
    let _as_dyn: &dyn Agent = &agent;
}

// ── PROMPT_PLAN_CODEX reviewer step ────────────────────────────────────

#[test]
fn plan_charter_file_carries_full_prompt() {
    // The full charter lands on disk (mirrors exec.md) and per-issue stdin
    // stays a one-line pointer — pins the byte reduction issue #80 delivers.
    let base = std::env::temp_dir().join(format!("ralphy-codex-charter-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let ws = Workspace::new(&base);
    fs::create_dir_all(ws.ralphy_dir()).unwrap();

    fs::write(ws.plan_charter_path(), PROMPT_PLAN_CODEX).unwrap();
    assert_eq!(
        fs::read_to_string(ws.plan_charter_path()).unwrap(),
        PROMPT_PLAN_CODEX
    );
    assert!(ralphy_adapter_support::PLAN_CHARTER.len() * 50 < PROMPT_PLAN_CODEX.len());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn prompt_plan_codex_contains_reviewer_step() {
    assert!(
        PROMPT_PLAN_CODEX.contains("reviewer"),
        "planning prompt must reference the reviewer skill"
    );
    let lower = PROMPT_PLAN_CODEX.to_lowercase();
    assert!(
        lower.contains("only") && lower.contains("commits you made"),
        "reviewer step must scope to this issue's own commits"
    );
    assert!(
        !PROMPT_PLAN_CODEX.contains("independent subagent"),
        "must not use Claude 'independent subagent' phrasing"
    );
}

#[test]
fn prompt_plan_codex_carries_finalize_trailer() {
    // Pin the FULL literal (suffix + spacing), not just the prefix: a drift to
    // `issue = <N> -->` would keep a prefix check green yet make the trailer no
    // longer match `plan_is_finalized_for`, silently disabling resume.
    assert!(
        PROMPT_PLAN_CODEX.contains("<!-- ralphy-plan: issue=<N> -->"),
        "planning prompt must instruct writing the exact finalized-plan trailer"
    );
}
