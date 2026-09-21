use super::*;

#[test]
fn effort_translation_preserves_each_resolved_phase() {
    assert_eq!(
        effort_strings(&ResolvedEffort {
            plan: Some(Effort::High),
            exec: Some(Effort::Low),
        }),
        (Some("high".into()), Some("low".into()))
    );
    assert_eq!(
        effort_strings(&ResolvedEffort {
            plan: None,
            exec: None,
        }),
        (None, None)
    );
}

#[test]
fn claude_arm_passes_each_translated_effort_to_the_adapter() {
    let source = include_str!("../wiring.rs");
    let arm = source
        .split_once("CliAgent::Claude =>")
        .expect("Claude arm")
        .1
        .split_once("CliAgent::Codex =>")
        .expect("Codex arm follows Claude")
        .0;
    assert!(arm.contains("let (plan_effort, exec_effort) = effort_strings(effort);"));
    assert!(arm.contains("ClaudeAgent::new("));
    assert!(arm.contains("plan_effort, run_dir"));
    assert!(arm.contains("exec_effort,"));
}

#[test]
fn codex_arm_passes_each_translated_effort_to_the_adapter() {
    let source = include_str!("../wiring.rs");
    let arm = source
        .split_once("CliAgent::Codex =>")
        .expect("Codex arm")
        .1
        .split_once("CliAgent::Copilot =>")
        .expect("Copilot arm follows Codex")
        .0;
    assert!(arm.contains("let (plan_effort, exec_effort) = effort_strings(effort);"));
    assert!(arm.contains(".with_plan_effort(plan_effort)"));
    assert!(arm.contains(".with_exec_effort(exec_effort)"));
}

#[test]
fn copilot_arm_merges_resolved_and_persisted_effort() {
    let source = include_str!("../wiring.rs");
    let arm = source
        .split_once("CliAgent::Copilot =>")
        .expect("Copilot arm")
        .1
        .split_once("CliAgent::Cursor =>")
        .expect("Cursor arm follows Copilot")
        .0;
    assert!(arm.contains("let (plan_effort, exec_effort) = effort_strings(effort);"));
    assert!(arm.contains(".with_plan_effort(plan_effort.or_else(|| copilot.plan_effort.clone()))"));
    assert!(arm.contains(".with_exec_effort(exec_effort.or_else(|| copilot.exec_effort.clone()))"));
}

#[test]
fn kimi_arm_passes_each_translated_effort_to_the_adapter() {
    let source = include_str!("../wiring.rs");
    let arm = source
        .split_once("CliAgent::Kimi =>")
        .expect("Kimi arm")
        .1
        .split_once("CliAgent::OpenCode =>")
        .expect("OpenCode arm follows Kimi")
        .0;
    assert!(arm.contains("let (plan_effort, exec_effort) = effort_strings(effort);"));
    assert!(arm.contains(".with_plan_effort(plan_effort)"));
    assert!(arm.contains(".with_exec_effort(exec_effort)"));
}

#[test]
fn gemini_arm_passes_each_translated_effort_to_the_adapter() {
    let source = include_str!("../wiring.rs");
    let arm = source
        .split_once("CliAgent::Gemini =>")
        .expect("Gemini arm")
        .1
        .split_once("CliAgent::Kimi =>")
        .expect("Kimi arm follows Gemini")
        .0;
    assert!(arm.contains("let (plan_effort, exec_effort) = effort_strings(effort);"));
    assert!(arm.contains(".with_plan_effort(plan_effort)"));
    assert!(arm.contains(".with_exec_effort(exec_effort)"));
}

#[test]
fn opencode_arm_passes_each_translated_effort_to_the_adapter() {
    let source = include_str!("../wiring.rs");
    let arm = source
        .split_once("CliAgent::OpenCode =>")
        .expect("OpenCode arm")
        .1
        .split_once("fn resolve_plan_agent")
        .expect("resolve_plan_agent follows the match")
        .0;
    assert!(arm.contains("let (plan_effort, exec_effort) = effort_strings(effort);"));
    assert!(arm.contains(".with_plan_effort(plan_effort)"));
    assert!(arm.contains(".with_exec_effort(exec_effort)"));
    assert!(arm.contains(".with_variant("));
}

#[test]
fn strip_events_token_removes_env_var() {
    // Guard the process-global env var against the other events-store tests.
    let _g = events::config::ENV_LOCK.lock().unwrap();
    std::env::set_var(events::config::TOKEN_ENV, "sekret");
    assert!(std::env::var(events::config::TOKEN_ENV).is_ok());
    strip_events_token_from_env();
    assert!(
        std::env::var(events::config::TOKEN_ENV).is_err(),
        "token must be absent after strip"
    );
}

#[test]
fn operating_branch_derives_per_mode() {
    // `new` mode cuts a fresh `afk/run-<stamp>` regardless of the current branch.
    assert_eq!(
        operating_branch(BranchMode::New, "20260703-120000", Some("feature")),
        "afk/run-20260703-120000"
    );
    // `current` mode reports the current branch verbatim.
    assert_eq!(
        operating_branch(BranchMode::Current, "20260703-120000", Some("feature")),
        "feature"
    );
    // `current` mode with no resolvable current branch degrades to empty.
    assert_eq!(
        operating_branch(BranchMode::Current, "20260703-120000", None),
        ""
    );
}

#[test]
fn plan_agent_defaults_to_the_executor_when_omitted() {
    // Omitted `--plan-agent` resolves to `--agent`, keeping single-agent runs
    // unchanged; an explicit flag overrides it (any combination allowed).
    assert_eq!(
        resolve_plan_agent(None, CliAgent::Claude),
        CliAgent::Claude,
        "absent flag equals --agent"
    );
    assert_eq!(
        resolve_plan_agent(Some(CliAgent::Claude), CliAgent::OpenCode),
        CliAgent::Claude,
        "explicit --plan-agent overrides --agent"
    );
}

#[test]
fn check_agents_present_aborts_when_executor_absent() {
    let result = check_agents_present(CliAgent::Claude, CliAgent::Claude, |_| false);
    let err = result.unwrap_err();
    assert!(
        err.contains("claude"),
        "message must name the missing cli: {err}"
    );
    assert!(
        err.contains("--agent"),
        "message must mention --agent: {err}"
    );
    assert!(
        err.contains("--plan-agent"),
        "message must mention --plan-agent: {err}"
    );
}

#[test]
fn check_agents_present_gates_planner() {
    // executor (Claude) is present; planner (Codex) is absent → Err naming codex.
    let result = check_agents_present(CliAgent::Claude, CliAgent::Codex, |a| a == CliAgent::Claude);
    let err = result.unwrap_err();
    assert!(
        err.contains("codex"),
        "message must name the absent planner: {err}"
    );
}

/// The regression the live probe caught: Cursor's SELECTOR is `cursor` but its
/// binary is `cursor-agent`/`agent` and is on `PATH` on neither platform
/// (ADR-0042 D14). A name-keyed resolver reports it absent and aborts the run
/// before the adapter — which resolves it fine — is ever reached.
#[test]
fn check_agents_present_probes_cursor_by_agent_not_by_selector_name() {
    let by_binary = |a: CliAgent| match a {
        // Stands in for `locate_cursor`, which finds the real install.
        CliAgent::Cursor => true,
        // Stands in for `locate_program`, which never finds a `cursor` binary.
        _ => false,
    };
    assert!(check_agents_present(CliAgent::Cursor, CliAgent::Cursor, by_binary).is_ok());

    // And the message still names the SELECTOR the operator typed.
    let err = check_agents_present(CliAgent::Cursor, CliAgent::Cursor, |_| false).unwrap_err();
    assert!(err.contains("cursor"), "{err}");
}

#[test]
fn check_agents_present_ok_when_all_present() {
    let result = check_agents_present(CliAgent::Claude, CliAgent::Codex, |_| true);
    assert!(result.is_ok());
}

#[test]
fn resolve_copilot_flag_wins() {
    let persisted = ralphy_agent_copilot::CopilotSettings {
        plan_model: Some("persisted".into()),
        ..Default::default()
    };
    let resolved = resolve_copilot(Some("flag".into()), None, &persisted);
    assert_eq!(resolved.plan_model, Some("flag".into()));
}

#[test]
fn resolve_copilot_uses_persisted_when_flag_absent() {
    let persisted = ralphy_agent_copilot::CopilotSettings {
        plan_model: Some("persisted".into()),
        ..Default::default()
    };
    let resolved = resolve_copilot(None, None, &persisted);
    assert_eq!(resolved.plan_model, Some("persisted".into()));
}

#[test]
fn resolve_copilot_none_when_both_unset() {
    let resolved = resolve_copilot(
        None,
        None,
        &ralphy_agent_copilot::CopilotSettings::default(),
    );
    assert_eq!(resolved.plan_model, None);
    assert_eq!(resolved.exec_model, None);
}

#[test]
fn resolve_copilot_maps_flags_per_phase() {
    let resolved = resolve_copilot(
        Some("p".into()),
        Some("e".into()),
        &ralphy_agent_copilot::CopilotSettings::default(),
    );
    assert_eq!(resolved.plan_model, Some("p".into()));
    assert_eq!(resolved.exec_model, Some("e".into()));
}

/// `resolve_copilot` still populates effort from settings only; flags merge
/// at `build_agent`. Model flags must not leak into the effort fields.
#[test]
fn resolve_copilot_effort_comes_from_settings_only() {
    let persisted = ralphy_agent_copilot::CopilotSettings {
        plan_effort: Some("high".into()),
        exec_effort: Some("low".into()),
        ..Default::default()
    };
    let resolved = resolve_copilot(Some("p".into()), Some("e".into()), &persisted);
    assert_eq!(resolved.plan_effort, Some("high".into()));
    assert_eq!(resolved.exec_effort, Some("low".into()));

    let bare = resolve_copilot(
        Some("p".into()),
        Some("e".into()),
        &ralphy_agent_copilot::CopilotSettings::default(),
    );
    assert_eq!(bare.plan_effort, None, "a model flag is not an effort");
    assert_eq!(bare.exec_effort, None);
}

/// D7's hatch reaches the agent only from settings.json, and defaults off.
#[test]
fn resolve_copilot_allow_builtin_mcps_comes_from_settings_only() {
    let bare = resolve_copilot(
        Some("p".into()),
        Some("e".into()),
        &ralphy_agent_copilot::CopilotSettings::default(),
    );
    assert!(!bare.allow_builtin_mcps, "the hatch defaults off");

    let persisted = ralphy_agent_copilot::CopilotSettings {
        allow_builtin_mcp_servers_i_understand_the_risk: true,
        ..Default::default()
    };
    assert!(resolve_copilot(None, None, &persisted).allow_builtin_mcps);
}

#[test]
fn resolve_cursor_flag_wins() {
    let resolved = resolve_cursor(
        Some("composer-2.5".into()),
        Some("composer-2.5-fast".into()),
        &ralphy_agent_cursor::CursorSettings::default(),
    );
    assert_eq!(resolved.plan_model, Some("composer-2.5".into()));
    assert_eq!(resolved.exec_model, Some("composer-2.5-fast".into()));
}

/// ADR-0042 has NO persisted Cursor model keys: `--model` is mandatory on this
/// vendor (D4), so there is no "unset" state to persist — the phase flags are
/// the whole model axis, and `None` becomes `--model auto` in the adapter.
/// This is the deliberate difference from `resolve_copilot`.
#[test]
fn resolve_cursor_takes_its_models_from_the_flags_only() {
    let resolved = resolve_cursor(None, None, &ralphy_agent_cursor::CursorSettings::default());
    assert_eq!(resolved.plan_model, None);
    assert_eq!(resolved.exec_model, None);
}

/// D6's hatch reaches the agent only from settings.json, and defaults off — a
/// per-run flag would make uploading the repository a one-keystroke decision.
#[test]
fn resolve_cursor_allow_indexing_comes_from_settings_only() {
    let bare = resolve_cursor(
        Some("p".into()),
        Some("e".into()),
        &ralphy_agent_cursor::CursorSettings::default(),
    );
    assert!(!bare.allow_indexing, "the hatch defaults off");

    let persisted = ralphy_agent_cursor::CursorSettings {
        allow_codebase_indexing_i_understand_the_risk: true,
    };
    assert!(resolve_cursor(None, None, &persisted).allow_indexing);
}

/// `--agent cursor` must reach a REAL adapter, not fall through to another
/// vendor: the composition root's match is the last place the wiring can go
/// silently wrong (ADR-0042 D1).
#[test]
fn build_agent_builds_a_cursor_agent() {
    use clap::Parser;
    let cli = crate::cli::Cli::try_parse_from(["ralphy", "run", "--agent", "cursor"])
        .expect("`--agent cursor` must parse");
    let crate::cli::Command::Run(args) = cli.command else {
        panic!("expected the run subcommand");
    };
    assert_eq!(args.agent, CliAgent::Cursor);

    let claude = ResolvedClaude {
        plan_model: String::new(),
        default_exec_model: String::new(),
        max_minutes_per_issue: 30,
        remote_control: false,
    };
    let copilot = resolve_copilot(None, None, &Default::default());
    let cursor = resolve_cursor(None, None, &Default::default());
    let agent = build_agent(
        CliAgent::Cursor,
        &args,
        PathBuf::from("/run"),
        None,
        None,
        &claude,
        &ResolvedEffort {
            plan: None,
            exec: None,
        },
        &copilot,
        &cursor,
        &resolve_gemini(None, None, &Default::default()),
        Some(0),
    );
    assert_eq!(agent.name(), "cursor");
}

/// ADR-0043 D8: each phase resolves independently — flag, then the persisted
/// `gemini.*` key, then `None` (omit `-m` and let the vendor route).
#[test]
fn gemini_models_resolve_flag_then_persisted_then_none() {
    let persisted = ralphy_agent_gemini::GeminiSettings {
        plan_model: Some("gemini-2.5-pro".into()),
        exec_model: Some("gemini-3.5-flash".into()),
    };

    // The flag beats the persisted value, per phase.
    let r = resolve_gemini(
        Some("gemini-2.5-flash".into()),
        Some("gemini-3.1-flash-lite".into()),
        &persisted,
    );
    assert_eq!(r.plan_model.as_deref(), Some("gemini-2.5-flash"));
    assert_eq!(r.exec_model.as_deref(), Some("gemini-3.1-flash-lite"));

    // Absent flags fall through to the persisted keys.
    let r = resolve_gemini(None, None, &persisted);
    assert_eq!(r.plan_model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(r.exec_model.as_deref(), Some("gemini-3.5-flash"));

    // An EMPTY flag is not a pin — it falls through rather than sending `-m ""`.
    let r = resolve_gemini(Some(String::new()), Some(String::new()), &persisted);
    assert_eq!(r.plan_model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(r.exec_model.as_deref(), Some("gemini-3.5-flash"));

    // Neither set: `None`, which omits `-m` and lets the vendor route.
    let r = resolve_gemini(None, None, &Default::default());
    assert_eq!(r.plan_model, None);
    assert_eq!(r.exec_model, None);
}

/// `--agent gemini` must reach a REAL adapter, not fall through to another
/// vendor: the composition root's match is the last place the wiring can go
/// silently wrong (ADR-0043 D1).
#[test]
fn build_agent_builds_a_gemini_agent() {
    use clap::Parser;
    let cli = crate::cli::Cli::try_parse_from(["ralphy", "run", "--agent", "gemini"])
        .expect("`--agent gemini` must parse");
    let crate::cli::Command::Run(args) = cli.command else {
        panic!("expected the run subcommand");
    };
    assert_eq!(args.agent, CliAgent::Gemini);

    let claude = ResolvedClaude {
        plan_model: String::new(),
        default_exec_model: String::new(),
        max_minutes_per_issue: 30,
        remote_control: false,
    };
    let agent = build_agent(
        CliAgent::Gemini,
        &args,
        PathBuf::from("/run"),
        None,
        None,
        &claude,
        &ResolvedEffort {
            plan: None,
            exec: None,
        },
        &resolve_copilot(None, None, &Default::default()),
        &resolve_cursor(None, None, &Default::default()),
        &resolve_gemini(None, None, &Default::default()),
        Some(0),
    );
    assert_eq!(agent.name(), "gemini");
}

/// `--plan-agent gemini` selects this vendor for the planning phase alone.
#[test]
fn plan_agent_gemini_is_accepted() {
    use clap::Parser;
    let cli = crate::cli::Cli::try_parse_from([
        "ralphy",
        "run",
        "--agent",
        "claude",
        "--plan-agent",
        "gemini",
    ])
    .expect("`--plan-agent gemini` must parse");
    let crate::cli::Command::Run(args) = cli.command else {
        panic!("expected the run subcommand");
    };
    assert_eq!(
        resolve_plan_agent(args.plan_agent, args.agent),
        CliAgent::Gemini
    );
}
