use super::*;

mod classic_screen;
mod console_name;

#[test]
fn console_cwd_prefers_chosen_repo_then_falls_back_to_home() {
    assert_eq!(
        console_cwd(Some(PathBuf::from("x"))),
        PathBuf::from("x"),
        "a chosen repo path wins over the home-dir default"
    );
    assert_eq!(
        console_cwd(None),
        ralphy_proc_util::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        "no chosen repo falls back to the home directory (or '.' if unresolvable)"
    );
}

#[test]
fn peer_console_spec_preserves_typed_wsl_arguments() {
    let path = PathBuf::from("/home/owner/shared");
    let spec = peer_console_spec("wsl.exe".into(), "Ubuntu-22.04", &path, (24, 80), None);
    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(
        spec.args,
        vec![
            OsString::from("-d"),
            OsString::from("Ubuntu-22.04"),
            OsString::from("--cd"),
            path.into_os_string(),
        ]
    );
}

#[test]
fn peer_console_spec_runs_a_startup_command_through_a_login_sh() {
    let path = PathBuf::from("/home/owner/shared");
    let spec = peer_console_spec(
        "wsl.exe".into(),
        "Ubuntu",
        &path,
        (24, 80),
        Some("htop -d 5"),
    );
    assert_eq!(
        spec.args,
        vec![
            OsString::from("-d"),
            OsString::from("Ubuntu"),
            OsString::from("--cd"),
            path.into_os_string(),
            OsString::from("--"),
            OsString::from("sh"),
            OsString::from("-lc"),
            OsString::from("htop -d 5"),
        ],
        "the command is ONE argv element after `--`, never re-split by wsl.exe"
    );
}

#[test]
fn shell_command_args_dispatch_on_the_shell_stem_not_the_platform() {
    let posix = vec![OsString::from("-lc"), OsString::from("btop --utf-force")];
    for shell in ["/bin/bash", "/usr/bin/zsh", "/bin/sh", "fish"] {
        assert_eq!(
            shell_command_args(Path::new(shell), "btop --utf-force"),
            posix,
            "{shell} takes the POSIX login form"
        );
    }
    let ps = vec![
        OsString::from("-NoLogo"),
        OsString::from("-Command"),
        OsString::from("btop"),
    ];
    for shell in [
        r"C:\Program Files\PowerShell\pwsh.exe",
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        "pwsh",
    ] {
        assert_eq!(
            shell_command_args(Path::new(shell), "btop"),
            ps,
            "{shell} is PowerShell"
        );
    }
    assert_eq!(
        shell_command_args(Path::new(r"C:\Windows\system32\cmd.exe"), "btop"),
        vec![OsString::from("/c"), OsString::from("btop")]
    );
}

#[test]
fn console_spec_without_a_command_is_the_bare_shell() {
    let spec = console_spec(PathBuf::from("."), 24, 80, None);
    assert!(
        spec.args.is_empty(),
        "a bare console passes the shell no argv"
    );
    let spec = console_spec(PathBuf::from("."), 24, 80, Some("htop"));
    assert_eq!(
        spec.args.last(),
        Some(&OsString::from("htop")),
        "the startup command is the last argv element, whatever the shell"
    );
}

#[test]
fn every_agent_parses_from_its_query_value_and_names_a_program() {
    // The daemon's enum is hand-kept in step with the CLI's `--agent` values
    // (ADR-0040 Tier 4). A vendor missing here is invisible to the compiler —
    // `from_query` just returns `None` and the daemon refuses the spawn, which
    // is exactly how Kimi went unreachable from the workbench (issue #228).
    for (value, agent, program) in [
        ("claude", Agent::Claude, "claude"),
        ("codex", Agent::Codex, "codex"),
        ("copilot", Agent::Copilot, "copilot"),
        ("cursor", Agent::Cursor, "cursor-agent"),
        ("kimi", Agent::Kimi, "kimi"),
        ("opencode", Agent::OpenCode, "opencode"),
    ] {
        assert_eq!(
            Agent::from_query(value),
            Some(agent),
            "`agent={value}` must parse — an unparsed vendor cannot be launched"
        );
        assert_eq!(
            agent.program_name(),
            program,
            "{value} must resolve the program the adapter itself shells"
        );
    }
    assert_eq!(
        Agent::from_query("bash"),
        None,
        "an unknown value stays unparsed rather than launching a surprise program"
    );
}

/// ADR-0042 D14: `cursor-agent` is not reliably on `PATH`, and where it IS it
/// is only because the installer added `%LOCALAPPDATA%\cursor-agent` — an
/// accident the daemon must not depend on. So the Cursor arm must go through
/// the shared vendor locator, which knows the two names and three install
/// roots, not through the plain `PATH` resolver.
///
/// Deliberately mutates NO environment: blanking `PATH`/`HOME` to force the
/// resolvers apart would reach every other test in this binary (`registry.rs`
/// shells `git`; `identity.rs` resolves `HOME` under its own separate lock),
/// turning one test's setup into another's flake. Instead the wiring is pinned
/// at the source — rewriting the arm as `resolve_program` reds this on EVERY
/// host, including one where the two happen to agree — and the behaviour is
/// asserted against the locator's live answer.
#[test]
fn cursor_resolves_off_path_through_the_vendor_locator() {
    // Production text, whitespace removed, then `locate_program`'s own body.
    let code: String = include_str!("../spec.rs").split_whitespace().collect();
    let code = code.split("#[cfg(test)]mod").next().unwrap_or_default();
    let body = code
        .split_once("pubfnlocate_program(self)")
        .and_then(|(_, rest)| rest.split_once("pubfn"))
        .map(|(body, _)| body)
        .expect("Agent::locate_program");
    let arm = body
        .split_once("Agent::Cursor=>")
        .map(|(_, rest)| rest.split(",_=>").next().unwrap_or(rest))
        .expect("locate_program has a Cursor arm");
    assert!(
        arm.contains("cursor::locate_cursor()"),
        "the Cursor arm must resolve through the shared vendor locator (D14/D19): {arm}"
    );

    // Env-free and non-vacuous on a host with Cursor installed: `spec_for` must
    // yield the locator's real path, never the bare fallback name. Skipped when
    // the stand-in override is exported into the whole test run — no test here
    // sets it, but an operator's shell can.
    if std::env::var_os(AGENT_OVERRIDE_ENV).is_none() {
        assert_eq!(
            spec_for(
                Agent::Cursor,
                Path::new("."),
                PathBuf::from("."),
                "owner/repo",
                (24, 80)
            )
            .program,
            ralphy_proc_util::cursor::locate_cursor()
                .map_or_else(|| OsString::from("cursor-agent"), PathBuf::into_os_string)
        );
    }
    // The `RALPHY_DAEMON_AGENT_OVERRIDE` seam is NOT bypassed for this vendor:
    // proved end-to-end by `tests/sessions/session_ws_cursor.rs`, which launches the
    // helper bin through `agent=cursor`.
}

/// `ALL` is hand-written, so a seventh variant added to the enum and to
/// `agent_flag` — but forgotten here — would leave every pin that iterates it
/// silently vacuous, which is exactly the ADR-0040 Tier 4 drift those pins
/// exist to catch. The `match` below is exhaustive, so the compiler forces the
/// count to be revisited.
#[test]
fn all_enumerates_every_enum_variant() {
    fn tag(a: Agent) -> u8 {
        match a {
            Agent::Claude => 0,
            Agent::Codex => 1,
            Agent::Copilot => 2,
            Agent::Cursor => 3,
            Agent::Kimi => 4,
            Agent::OpenCode => 5,
            Agent::Antigravity => 6,
        }
    }
    let mut tags: Vec<u8> = Agent::ALL.iter().copied().map(tag).collect();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(
        tags,
        (0..=6).collect::<Vec<u8>>(),
        "Agent::ALL must list every variant exactly once"
    );
}

#[test]
fn antigravity_parses_from_its_query_name() {
    assert_eq!(Agent::from_query("antigravity"), Some(Agent::Antigravity));
    assert_eq!(
        Agent::Antigravity.program_name(),
        "agy",
        "the CLI installs `agy`"
    );
}

#[test]
fn the_agy_program_is_found_by_the_shared_locator() {
    assert_eq!(
        Agent::Antigravity.locate_program(),
        ralphy_proc_util::antigravity::locate_agy(),
        "the console finds agy where the adapter's runs find it"
    );
}

/// The one argument is the owned root, absolute; a console passes no autonomy flag.
#[test]
fn an_antigravity_console_gets_the_owned_root_flag() {
    let root = ralphy_proc_util::antigravity::owned_root().expect("a home folder");
    assert!(
        root.is_absolute(),
        "agy refuses a relative root: {}",
        root.display()
    );
    let spec = spec_for(
        Agent::Antigravity,
        Path::new("r"),
        PathBuf::from("r"),
        "o/r",
        (24, 80),
    );
    let mut flag = OsString::from("--gemini_dir=");
    flag.push(&root);
    assert_eq!(spec.args, vec![flag]);
    assert!(
        spec.env.is_empty(),
        "the root is a flag, not the environment"
    );
}

/// The daemon reparses the adapter's settings file rather than importing the
/// core (ADR-0032 §10), so the two schemas can drift silently. This is the pin:
/// rename either name in the adapter and the daemon's gate reds here.
#[test]
fn the_optin_key_matches_the_adapters_own_schema() {
    let src = include_str!("../../../../ralphy-agent-cursor/src/settings.rs");
    assert!(
        src.contains("allow_codebase_indexing_i_understand_the_risk"),
        "the adapter renamed the opt-in key the daemon reparses"
    );
    assert!(
        src.contains(r#"SECTION: &'static str = "cursor""#),
        "the adapter renamed the settings section the daemon reparses"
    );
}

/// The same pin for the console-name opt-in, whose schema is
/// `ralphy-agent-claude`'s. The key is a `bool` there and read as one here:
/// were it to become an `Option<bool>` the JSON shape would not change, but
/// were it to become a string this gate would read `false` for every repo
/// that opted in — so the declaration, not just the name, is pinned.
#[test]
fn the_console_name_key_matches_the_adapters_own_schema() {
    let src = include_str!("../../../../ralphy-agent-claude/src/settings.rs");
    assert!(
        src.contains("pub console_name: bool,"),
        "the adapter renamed or retyped the opt-in the daemon reparses"
    );
    assert!(
        src.contains(r#"SECTION: &'static str = "claude""#),
        "the adapter renamed the settings section the daemon reparses"
    );
}

/// A repo whose `.ralphy/settings.json` opts into the console name.
fn opted_in_repo(d: &tempfile::TempDir) -> PathBuf {
    let repo = d.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".ralphy")).unwrap();
    std::fs::write(
        repo.join(".ralphy").join("settings.json"),
        r#"{"claude":{"console_name":true}}"#,
    )
    .unwrap();
    repo
}

/// No vendor is ever handed a worktree flag (ADR-0063 §3): the worktree is
/// Ralphy's and arrives as `cwd`. The opt-ins are read from `root`, not from
/// `cwd` — the checkout has no `.ralphy/`, so a read keyed on `cwd` would
/// silently turn Claude's name off inside a worktree.
#[test]
fn no_vendor_is_ever_given_a_worktree_flag_and_the_cwd_is_the_checkout() {
    let d = tempfile::tempdir().unwrap();
    let root = opted_in_repo(&d);
    let wt = root.join(".ralphy/worktrees/wt-a");
    let flag = OsString::from("--worktree");
    for agent in Agent::ALL {
        let spec = spec_for(agent, &root, wt.clone(), "owner/ralphy", (24, 80));
        assert!(
            !spec.args.contains(&flag),
            "{agent:?} must never be given a worktree flag: {:?}",
            spec.args
        );
        assert_eq!(spec.cwd, wt, "{agent:?} runs in the checkout");
    }
    let claude = spec_for(Agent::Claude, &root, wt.clone(), "owner/ralphy", (24, 80));
    assert!(
        claude.name.is_some(),
        "the name opt-in is read from the primary"
    );
    assert_eq!(claude.args[0], OsString::from("--name"));
}

/// The refusal is the safe default: only an explicit `true` opens the upload.
#[test]
fn cursor_indexing_allowed_defaults_to_false() {
    let d = tempfile::tempdir().unwrap();
    assert!(
        !cursor_indexing_allowed(d.path()),
        "no .ralphy/ at all must not opt in"
    );

    let settings = d.path().join(".ralphy").join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    for (body, want) in [
        ("{}", false),
        (
            r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":true}}"#,
            true,
        ),
        (
            r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":"yes"}}"#,
            false,
        ),
        (
            r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":false}}"#,
            false,
        ),
        ("not json", false),
    ] {
        std::fs::write(&settings, body).unwrap();
        assert_eq!(
            cursor_indexing_allowed(d.path()),
            want,
            "settings.json = {body}"
        );
    }
}

/// A Codex console in a project with Codex hooks of its own gets neither the
/// trust flag nor the status hooks, and is silent with a reason from its
/// start; in a project without them it gets both.
#[test]
fn a_codex_console_in_a_project_with_its_own_hooks_gets_no_trust_flag() {
    let flag = OsString::from(ralphy_proc_util::hook_form::CODEX_HOOK_TRUST_FLAG);
    let store = tempfile::tempdir().unwrap();
    let launch = |repo: &Path| {
        let files = crate::agent_state::StatusFiles::for_agent(Agent::Codex, store.path(), 1);
        spec_with_status(
            Agent::Codex,
            repo,
            repo.to_path_buf(),
            "owner/r",
            None,
            24,
            80,
            files,
        )
    };
    let plain = tempfile::tempdir().unwrap();
    let spec = launch(plain.path());
    assert!(spec.args.contains(&flag), "{:?}", spec.args);
    assert_eq!(spec.unreported_reason, None);

    let hooked = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(hooked.path().join(".codex")).unwrap();
    std::fs::write(hooked.path().join(".codex").join("hooks.json"), "{}").unwrap();
    let spec = launch(hooked.path());
    assert!(!spec.args.contains(&flag), "{:?}", spec.args);
    assert!(spec.status.is_none(), "no status file is tailed");
    assert_eq!(
        spec.unreported_reason.as_deref(),
        Some(crate::agent_state::CODEX_OWN_HOOKS_REASON)
    );
}
