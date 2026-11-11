use super::*;

const HOOK_EXE: &str = "/bin/ralphy";
const STATUS: &str = "/run/agent-status.jsonl";

// ── build_codex_command ─────────────────────────────────────────────────

/// Every hook `-c` sits after `exec`, with `model_reasoning_effort`:
/// Codex drops a `-c` list given before `exec` when another follows it,
/// and without the trust flag it runs no hook at all.
#[test]
fn codex_command_gives_the_status_hooks_on_one_side_of_exec() {
    let cmd = build_codex_command(
        CODEX_MODEL_SOL,
        "high",
        Path::new("/repo"),
        Path::new("/repo/out.txt"),
        Path::new(HOOK_EXE),
        Path::new(STATUS),
    );
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let exec = args.iter().position(|a| a == "exec").expect("exec");
    assert_eq!(exec, 0, "exec comes first: {args:?}");
    assert!(
        args.iter().any(|a| a == "--dangerously-bypass-hook-trust"),
        "{args:?}"
    );
    let values: Vec<&String> = args
        .windows(2)
        .filter(|w| w[0] == "-c")
        .map(|w| &w[1])
        .collect();
    for event in hook_form::CODEX_STATUS_EVENTS {
        let given = values
            .iter()
            .filter(|v| v.starts_with(&format!("hooks.{event}=")))
            .count();
        assert_eq!(given, 1, "{event}: {values:?}");
    }
    assert!(
        values
            .iter()
            .filter(|v| v.starts_with("hooks."))
            .all(|v| v.contains("hook status codex")),
        "{values:?}"
    );
    assert!(
        values
            .iter()
            .any(|v| v.starts_with("model_reasoning_effort=")),
        "{values:?}"
    );
    let status = cmd
        .get_envs()
        .find(|(k, _)| *k == "RALPHY_STATUS_FILE")
        .and_then(|(_, v)| v);
    assert_eq!(status, Some(std::ffi::OsStr::new(STATUS)));
}

/// A run in a project with Codex hooks of its own gets neither the trust
/// flag nor the status hooks: the flag would run the project's hooks.
#[test]
fn a_run_in_a_project_with_its_own_hooks_gets_no_trust_flag() {
    let repo = tempfile::tempdir().expect("a temp dir");
    fs::create_dir_all(repo.path().join(".codex")).expect("the .codex dir");
    fs::write(repo.path().join(".codex").join("hooks.json"), "{}").expect("hooks.json");
    let args: Vec<String> = build_codex_command(
        CODEX_MODEL_SOL,
        "high",
        repo.path(),
        &repo.path().join("out.txt"),
        Path::new(HOOK_EXE),
        Path::new(STATUS),
    )
    .get_args()
    .map(|a| a.to_string_lossy().into_owned())
    .collect();
    assert!(
        !args.iter().any(|a| a == hook_form::CODEX_HOOK_TRUST_FLAG),
        "{args:?}"
    );
    assert!(!args.iter().any(|a| a.starts_with("hooks.")), "{args:?}");
}

#[test]
fn build_command_argv_and_env() {
    let cmd = build_codex_command(
        CODEX_MODEL_SOL,
        "high",
        Path::new("/repo"),
        Path::new("/repo/.ralphy/codex-last.txt"),
        Path::new(HOOK_EXE),
        Path::new(STATUS),
    );
    assert_eq!(cmd.get_program().to_string_lossy(), "codex");

    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"exec".to_owned()), "argv: {args:?}");
    assert!(args.contains(&"-C".to_owned()), "argv: {args:?}");
    assert!(args.contains(&"-m".to_owned()), "argv: {args:?}");
    assert!(args.contains(&"-o".to_owned()), "argv: {args:?}");
    // The effort word reaches the child as given, for each word.
    for effort in ["high", "low"] {
        let args: Vec<String> = build_codex_command(
            CODEX_MODEL_SOL,
            effort,
            Path::new("/repo"),
            Path::new("/repo/out.txt"),
            Path::new(HOOK_EXE),
            Path::new(STATUS),
        )
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        let want = format!("model_reasoning_effort=\"{effort}\"");
        assert!(
            args.contains(&want),
            "{effort}: effort arg missing: {args:?}"
        );
    }
    // Sandbox posture and the trailing stdin marker. `codex exec` defaults to
    // approval=never, so no explicit `-a` flag is passed (it is rejected by
    // codex-cli ≥0.138).
    assert!(args.contains(&"danger-full-access".to_owned()));
    assert!(args.contains(&"-".to_owned()));

    // OPENAI_API_KEY is removed on the child so an inherited key can't switch
    // the run to API billing.
    let removed = cmd
        .get_envs()
        .any(|(k, v)| k == "OPENAI_API_KEY" && v.is_none());
    assert!(removed, "OPENAI_API_KEY should be removed on the child");
}

#[test]
fn neutral_top_rungs_saturate_to_codex_high() {
    // `xhigh`/`max` are neutral lexicon rungs (ADR-0044) with no Codex
    // `model_reasoning_effort` value; they must clamp to `high` so the `-c`
    // override is one `codex exec` accepts, not one it rejects at startup.
    assert_eq!(codex_reasoning_effort("xhigh"), "high");
    assert_eq!(codex_reasoning_effort("max"), "high");
    // Valid words pass through untouched.
    for w in ["low", "medium", "high", "minimal"] {
        assert_eq!(codex_reasoning_effort(w), w);
    }
    for word in ["xhigh", "max"] {
        let cmd = build_codex_command(
            CODEX_MODEL_SOL,
            word,
            Path::new("/repo"),
            Path::new("/repo/out.txt"),
            Path::new(HOOK_EXE),
            Path::new(STATUS),
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == "model_reasoning_effort=\"high\""),
            "{word} should clamp to high in argv: {args:?}"
        );
    }
}

// ── build_codex_init_command ────────────────────────────────────────────

#[test]
fn build_init_command_attaches_images() {
    let cmd = build_codex_init_command(
        Some(CODEX_MODEL_TERRA),
        "medium",
        Path::new("/repo"),
        &[PathBuf::from("/t/a.png")],
    );
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let i = args.iter().position(|a| a == "-i").expect("`-i` present");
    assert_eq!(args[i + 1], "/t/a.png", "path follows -i: {args:?}");
    let dash = args.iter().position(|a| a == "-").expect("`-` present");
    assert!(i < dash, "-i must precede the stdin marker: {args:?}");
}

#[test]
fn build_init_command_no_images_emits_no_i_flag() {
    let cmd = build_codex_init_command(Some(CODEX_MODEL_TERRA), "medium", Path::new("/repo"), &[]);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !args.iter().any(|a| a == "-i"),
        "no -i for empty slice: {args:?}"
    );
}

#[test]
fn build_init_command_without_model_omits_m_flag() {
    // No override and no config model → delegate to Codex's recommended
    // default by omitting `-m` entirely (ADR-0004, Amendment 2026-07-10).
    let cmd = build_codex_init_command(None, "medium", Path::new("/repo"), &[]);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !args.iter().any(|a| a == "-m"),
        "no -m when no model is named: {args:?}"
    );
    // The rest of the invocation is unchanged.
    assert!(args.iter().any(|a| a == "--skip-git-repo-check"));
    assert!(args
        .iter()
        .any(|a| a == "model_reasoning_effort=\"medium\""));
}

// ── recommended_tier ────────────────────────────────────────────────────

#[test]
fn recommended_tier_parses_each_tier() {
    assert_eq!(
        recommended_tier("## Execution model: low\nbecause").as_deref(),
        Some("low")
    );
    assert_eq!(
        recommended_tier("## Execution model: Medium\n").as_deref(),
        Some("medium")
    );
    assert_eq!(
        recommended_tier("## Execution model: HIGH\n").as_deref(),
        Some("high")
    );
    // The fourth rung must parse as `xhigh`, not be shadowed by `high`.
    assert_eq!(
        recommended_tier("## Execution model: xhigh\n").as_deref(),
        Some("xhigh")
    );
    assert_eq!(
        recommended_tier("## Execution model: XHigh\n").as_deref(),
        Some("xhigh")
    );
}

#[test]
fn recommended_tier_none_on_no_match() {
    assert_eq!(recommended_tier("no judgment here"), None);
    // A Claude model name is not a Codex tier.
    assert_eq!(recommended_tier("## Execution model: opus"), None);
}

// ── tier_to_model_effort ────────────────────────────────────────────────

#[test]
fn tier_to_model_effort_maps_and_defaults() {
    assert_eq!(tier_to_model_effort(Some("low")), (CODEX_MODEL_LUNA, "low"));
    assert_eq!(
        tier_to_model_effort(Some("medium")),
        (CODEX_MODEL_TERRA, "medium")
    );
    // Both flagship rungs land on Sol; only the effort differs.
    assert_eq!(
        tier_to_model_effort(Some("high")),
        (CODEX_MODEL_SOL, "medium")
    );
    assert_eq!(
        tier_to_model_effort(Some("xhigh")),
        (CODEX_MODEL_SOL, "high")
    );
    // Absent or unrecognized tiers default to the everyday model at medium.
    assert_eq!(tier_to_model_effort(None), (CODEX_MODEL_TERRA, "medium"));
    assert_eq!(
        tier_to_model_effort(Some("bogus")),
        (CODEX_MODEL_TERRA, "medium")
    );
}

// ── parse_codex_config_model ────────────────────────────────────────────

#[test]
fn parse_codex_config_model_reads_root_model() {
    let toml = "model = \"gpt-5.5\"\nmodel_reasoning_effort = \"high\"\n\n[features]\nx = true\n";
    assert_eq!(parse_codex_config_model(toml).as_deref(), Some("gpt-5.5"));
}

#[test]
fn parse_codex_config_model_ignores_effort_and_non_root() {
    // model_reasoning_effort is a different key, not the model.
    assert_eq!(
        parse_codex_config_model("model_reasoning_effort = \"high\"\n"),
        None
    );
    // A `model` under a section is not the root default.
    assert_eq!(
        parse_codex_config_model("[mcp_servers.x]\nmodel = \"other\"\n"),
        None
    );
    assert_eq!(parse_codex_config_model("no keys here\n"), None);
}
