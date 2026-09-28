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

/// Every `build_agent` arm hands the adapter the effort it resolved for each
/// phase — through `effort_strings`, into the adapter's own setters (Copilot
/// falls back to its persisted `copilot.*_effort`; OpenCode's dialect rides
/// `with_variant`). Read from production text with whitespace removed, one
/// arm at a time, whatever the order of the arms.
#[test]
fn every_arm_passes_each_translated_effort_to_the_adapter() {
    let code: String = include_str!("../wiring.rs").split_whitespace().collect();
    let code = code.split("#[cfg(test)]mod").next().unwrap_or_default();
    let arms = [
        "Claude", "Codex", "Copilot", "Cursor", "Gemini", "Kimi", "OpenCode",
    ];
    let arm = |name: &str| -> &str {
        let head = format!("CliAgent::{name}=>");
        let start = code.find(&head).unwrap_or_else(|| panic!("no {name} arm")) + head.len();
        let rest = &code[start..];
        let end = arms
            .iter()
            .filter_map(|other| rest.find(&format!("CliAgent::{other}=>")))
            .chain(rest.find("fnresolve_plan_agent("))
            .min()
            .unwrap_or(rest.len());
        &rest[..end]
    };
    let setters: &[&str] = &[
        ".with_plan_effort(plan_effort)",
        ".with_exec_effort(exec_effort)",
    ];
    // (arm, what its body must pass on)
    let rows: [(&str, &[&str]); 6] = [
        (
            "Claude",
            &["ClaudeAgent::new(", "plan_effort,run_dir", "exec_effort,"],
        ),
        ("Codex", setters),
        (
            "Copilot",
            &[
                ".with_plan_effort(plan_effort.or_else(||copilot.plan_effort.clone()))",
                ".with_exec_effort(exec_effort.or_else(||copilot.exec_effort.clone()))",
            ],
        ),
        ("Gemini", setters),
        ("Kimi", setters),
        (
            "OpenCode",
            &[
                ".with_plan_effort(plan_effort)",
                ".with_exec_effort(exec_effort)",
                ".with_variant(",
            ],
        ),
    ];
    for (name, needles) in rows {
        let body = arm(name);
        assert!(
            body.contains("effort_strings(effort)"),
            "the {name} arm must translate the resolved effort: {body}"
        );
        for needle in needles {
            assert!(
                body.contains(needle),
                "the {name} arm must pass {needle}: {body}"
            );
        }
    }
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
/// A run whose executor or planner CLI is absent aborts, naming the missing
/// CLI and the flags that chose it; with both present it proceeds.
#[test]
fn check_agents_present_gates_executor_and_planner() {
    type Locate = fn(CliAgent) -> bool;
    // (case, executor, planner, locator, words of the refusal; `None` is Ok)
    type Row<'a> = (&'a str, CliAgent, CliAgent, Locate, Option<&'a [&'a str]>);
    let rows: [Row; 3] = [
        (
            "executor absent",
            CliAgent::Claude,
            CliAgent::Claude,
            |_| false,
            Some(&["claude", "--agent", "--plan-agent"]),
        ),
        (
            "planner absent",
            CliAgent::Claude,
            CliAgent::Codex,
            |a| a == CliAgent::Claude,
            Some(&["codex"]),
        ),
        (
            "both present",
            CliAgent::Claude,
            CliAgent::Codex,
            |_| true,
            None,
        ),
    ];
    for (case, executor, planner, locate, refusal) in rows {
        let result = check_agents_present(executor, planner, locate);
        match refusal {
            None => assert!(result.is_ok(), "{case}: {result:?}"),
            Some(words) => {
                let err = result.expect_err(case);
                for word in words {
                    assert!(err.contains(word), "{case}: must mention {word}: {err}");
                }
            }
        }
    }
}
/// `check_agents_present` asks the locator per agent, not per selector name, and
/// its message names the selector the operator typed. Cursor's selector is
/// `cursor`, but its binary is `cursor-agent`/`agent`, so a name-keyed probe
/// finds nothing (ADR-0042 D14). The locator here is the test's own; the pin
/// below covers the one `preflight_agents` passes.
#[test]
fn check_agents_present_uses_the_given_locator() {
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

/// The run probes Cursor through the adapter's own locator, so detection and
/// the spawn agree. `preflight_agents` has no locator seam, so this reads the
/// call inside its body.
#[test]
fn preflight_agents_probes_cursor_with_its_adapter_locator() {
    let source = include_str!("../wiring.rs");
    let start = source
        .find("pub(crate) fn preflight_agents(")
        .expect("preflight_agents is defined");
    let rest = &source[start..];
    // A free fn: its closing brace is the first line that starts with `}`.
    let end = rest
        .find("\n}")
        .expect("preflight_agents has a closing brace");
    let body: String = rest[..end].split_whitespace().collect();
    assert!(
        body.contains("CliAgent::Cursor=>ralphy_agent_cursor::locate_cursor().is_some(),"),
        "preflight_agents must probe Cursor with `locate_cursor`: {body}"
    );
}
/// Each phase's model: its own flag wins over the persisted `copilot.*_model`,
/// which is used when the flag is absent; nothing anywhere is `None`.
#[test]
fn resolve_copilot_models_per_phase() {
    let persisted_plan = ralphy_agent_copilot::CopilotSettings {
        plan_model: Some("persisted".into()),
        ..Default::default()
    };
    let bare = ralphy_agent_copilot::CopilotSettings::default();
    // (case, plan flag, exec flag, settings, plan model, exec model)
    let rows = [
        (
            "flag wins",
            Some("flag"),
            None,
            &persisted_plan,
            Some("flag"),
            None,
        ),
        (
            "persisted when the flag is absent",
            None,
            None,
            &persisted_plan,
            Some("persisted"),
            None,
        ),
        ("both unset", None, None, &bare, None, None),
        (
            "flags map per phase",
            Some("p"),
            Some("e"),
            &bare,
            Some("p"),
            Some("e"),
        ),
    ];
    for (case, plan, exec, settings, want_plan, want_exec) in rows {
        let resolved =
            resolve_copilot(plan.map(str::to_string), exec.map(str::to_string), settings);
        assert_eq!(resolved.plan_model.as_deref(), want_plan, "{case}: plan");
        assert_eq!(resolved.exec_model.as_deref(), want_exec, "{case}: exec");
    }
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
/// ADR-0042 has NO persisted Cursor model keys: `--model` is mandatory on this
/// vendor (D4), so there is no "unset" state to persist — the phase flags are
/// the whole model axis, and `None` becomes `--model auto` in the adapter.
/// This is the deliberate difference from `resolve_copilot`.
#[test]
fn resolve_cursor_takes_its_models_from_the_flags_only() {
    // (case, plan flag, exec flag)
    let rows = [
        ("flags", Some("composer-2.5"), Some("composer-2.5-fast")),
        ("no flags", None, None),
    ];
    for (case, plan, exec) in rows {
        let resolved = resolve_cursor(
            plan.map(str::to_string),
            exec.map(str::to_string),
            &ralphy_agent_cursor::CursorSettings::default(),
        );
        assert_eq!(resolved.plan_model.as_deref(), plan, "{case}: plan");
        assert_eq!(resolved.exec_model.as_deref(), exec, "{case}: exec");
    }
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

/// `--agent <name>` must reach that vendor's REAL adapter, not fall through to
/// another one: the composition root's match is the last place the wiring can
/// go silently wrong (ADR-0042 D1). Every agent the CLI accepts, built from a
/// parsed `run`.
#[test]
fn build_agent_builds_every_agent_the_cli_accepts() {
    use clap::{Parser, ValueEnum};
    let claude = ResolvedClaude {
        plan_model: String::new(),
        default_exec_model: String::new(),
        max_minutes_per_issue: 30,
        remote_control: false,
    };
    for agent in CliAgent::value_variants() {
        let name = agent.cli_name();
        let cli = crate::cli::Cli::try_parse_from(["ralphy", "run", "--agent", name])
            .unwrap_or_else(|e| panic!("`--agent {name}` must parse: {e}"));
        let crate::cli::Command::Run(args) = cli.command else {
            panic!("expected the run subcommand");
        };
        assert_eq!(args.agent, *agent, "{name}");
        let built = build_agent(
            *agent,
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
        assert_eq!(built.name(), name, "--agent {name} built another adapter");
    }
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
