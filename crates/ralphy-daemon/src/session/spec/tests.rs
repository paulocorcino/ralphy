use super::*;

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
    let spec = peer_console_spec("wsl.exe".into(), "Ubuntu-22.04", &path, 24, 80, None);
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
    let spec = peer_console_spec("wsl.exe".into(), "Ubuntu", &path, 24, 80, Some("htop -d 5"));
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
        ("gemini", Agent::Gemini, "gemini"),
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
    let src = include_str!("../spec.rs");
    assert!(
        src.contains("Agent::Cursor => ralphy_proc_util::cursor::locate_cursor()"),
        "the Cursor arm must resolve through the shared vendor locator (D14/D19)"
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
                24,
                80
            )
            .program,
            ralphy_proc_util::cursor::locate_cursor()
                .map(PathBuf::into_os_string)
                .unwrap_or_else(|| OsString::from("cursor-agent"))
        );
    }
    // The `RALPHY_DAEMON_AGENT_OVERRIDE` seam is NOT bypassed for this vendor:
    // proved end-to-end by `tests/session_ws_cursor.rs`, which launches the
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
            Agent::Gemini => 4,
            Agent::Kimi => 5,
            Agent::OpenCode => 6,
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

/// Every way of failing to read the opt-in must answer "no name". The file
/// is the operator's, so it is absent far more often than it is present, and
/// a malformed one must not be what renames a session.
#[test]
fn an_unreadable_settings_file_never_names_a_console() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path();
    assert!(!claude_console_named(repo), "no .ralphy at all");

    let dir = repo.join(".ralphy");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    for body in [
        "",
        "{",
        "{}",
        r#"{"claude":{}}"#,
        r#"{"claude":{"console_name":"true"}}"#,
        r#"{"claude":{"console_name":1}}"#,
        r#"{"claude":{"console_name":false}}"#,
        r#"{"cursor":{"console_name":true}}"#,
    ] {
        std::fs::write(&file, body).unwrap();
        assert!(
            !claude_console_named(repo),
            "settings.json = {body} must not name a console"
        );
    }

    std::fs::write(&file, r#"{"claude":{"console_name":true}}"#).unwrap();
    assert!(claude_console_named(repo), "the one shape that opts in");
}

/// Same drift risk as `the_optin_key_matches_the_adapters_own_schema`, one
/// layer down: the daemon duplicates the owned root's three path components
/// rather than importing `ralphy-agent-gemini` (ADR-0032 §10). Rename one in
/// the adapter and the daemon would silently point a launch — and its refusal
/// gate — at a directory the CLI never reads.
#[test]
fn the_gemini_root_layout_matches_the_adapters_own() {
    let root = include_str!("../../../../ralphy-agent-gemini/src/root.rs");
    assert!(
        root.contains(r#"ROOT_DIR_NAME: &str = "gemini-home""#),
        "the adapter renamed the owned root directory the daemon duplicates"
    );
    assert!(
        root.contains(r#"CLI_SUBDIR: &str = ".gemini""#),
        "the adapter renamed the CLI subdirectory the daemon duplicates"
    );
    let policy = include_str!("../../../../ralphy-agent-gemini/src/policy.rs");
    assert!(
        policy.contains(r#"POLICY_FILE: &str = "ralphy-policy.toml""#),
        "the adapter renamed the policy document the daemon's launch gate checks"
    );
}

/// A Gemini launch must carry BOTH halves of the containment: the env var the
/// CLI reads its root from, and the global flag the policy rides on. A spec
/// with one and not the other launches an unconstrained child.
#[test]
fn gemini_launches_under_the_owned_root_and_its_policy() {
    let repo = PathBuf::from("C:/Dev/FinCal");
    let spec = spec_for(Agent::Gemini, &repo, repo.clone(), "owner/fincal", 24, 80);
    assert_eq!(
        spec.env,
        vec![(
            OsString::from("GEMINI_CLI_HOME"),
            gemini_home(&repo).into_os_string()
        )]
    );
    assert_eq!(
        spec.args,
        vec![
            OsString::from("--policy"),
            gemini_policy_path(&repo).into_os_string()
        ]
    );
    assert!(gemini_home(&repo).ends_with("gemini-home"));
    assert!(gemini_policy_path(&repo).ends_with("ralphy-policy.toml"));

    // Gemini's containment is its OWN: no other vendor gets an env var, and
    // the one vendor that does carry args carries a different flag.
    let bare = spec_for(Agent::Codex, &repo, repo.clone(), "owner/fincal", 24, 80);
    assert!(bare.args.is_empty() && bare.env.is_empty());
}

/// The name must reach BOTH places or it is half-wired: the argv (which is
/// what the vendor's roster ends up publishing) and `spec.name` (which is
/// what the shell is told, so the operator can read the address off the
/// window). A spec with one and not the other names a console nobody can
/// find, or shows a name nothing answers to.
#[test]
fn a_claude_console_is_named_in_argv_and_on_the_spec() {
    let d = tempfile::tempdir().unwrap();
    let repo = opted_in_repo(&d);
    let spec = spec_for(
        Agent::Claude,
        &repo,
        repo.clone(),
        "paulocorcino/ralphy",
        24,
        80,
    );
    let name = spec.name.expect("a Claude launch must carry a name");
    assert_eq!(
        spec.args,
        vec![OsString::from("--name"), OsString::from(&name)],
        "the argv must carry the SAME string the shell is handed"
    );
    assert!(spec.env.is_empty(), "naming is argv-only, never env");
    assert!(
        name.starts_with("wb-ralphy-") && name.len() == "wb-ralphy-".len() + 4,
        "expected wb-<repo>-<4 hex>, got {name}"
    );
    assert!(
        name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "a name is typed back as an address: {name}"
    );

    // No other vendor takes the flag — `--name` would be an unknown argument
    // and the console would open dead. The opt-in is Claude's own key, so an
    // opted-in repo is exactly where a leak would show.
    for agent in Agent::ALL.iter().filter(|a| **a != Agent::Claude) {
        let other = spec_for(*agent, &repo, repo.clone(), "owner/ralphy", 24, 80);
        assert!(
            other.name.is_none() && !other.args.contains(&OsString::from("--name")),
            "{agent:?} must not be named"
        );
    }
}

/// The name is the operator's to ask for. Un-opted — which is every repo
/// that has never been told otherwise — the launch carries no `--name` AND
/// no `spec.name`: the vendor names the session itself, and a spec that
/// announced a name the argv never asked for would show the shell an address
/// nothing answers to.
#[test]
fn a_claude_console_is_unnamed_until_the_repo_opts_in() {
    let d = tempfile::tempdir().unwrap();
    let spec = spec_for(
        Agent::Claude,
        d.path(),
        d.path().to_path_buf(),
        "paulocorcino/ralphy",
        24,
        80,
    );
    assert!(
        spec.args.is_empty(),
        "an un-opted Claude launch carries no flags: {:?}",
        spec.args
    );
    assert!(spec.name.is_none(), "nothing may announce a name");
}

/// A repo root that does not exist reads as un-opted rather than panicking:
/// the registry keeps an entry for a repo that has moved away (`reachable()`
/// is computed, never persisted), so the launch path meets this path.
#[test]
fn an_unreachable_repo_root_is_not_a_naming_decision() {
    let gone = PathBuf::from("C:/Dev/no-such-repo-here");
    let spec = spec_for(Agent::Claude, &gone, gone.clone(), "owner/ralphy", 24, 80);
    assert!(spec.name.is_none() && spec.args.is_empty());
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
        let spec = spec_for(agent, &root, wt.clone(), "owner/ralphy", 24, 80);
        assert!(
            !spec.args.contains(&flag),
            "{agent:?} must never be given a worktree flag: {:?}",
            spec.args
        );
        assert_eq!(spec.cwd, wt, "{agent:?} runs in the checkout");
    }
    let claude = spec_for(Agent::Claude, &root, wt.clone(), "owner/ralphy", 24, 80);
    assert!(
        claude.name.is_some(),
        "the name opt-in is read from the primary"
    );
    assert_eq!(claude.args[0], OsString::from("--name"));
}

/// Gemini in a checkout stays contained by the PRIMARY tree's owned root:
/// the home and the policy are `.ralphy/`-backed, and the checkout has none.
/// The negative control pins that a regression reading `cwd` cannot pass.
#[test]
fn gemini_in_a_checkout_is_contained_by_the_primary_trees_root() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let wt = root.join(".ralphy/worktrees/wt-a");
    let spec = spec_for(Agent::Gemini, &root, wt.clone(), "owner/ralphy", 24, 80);
    assert_eq!(
        spec.env,
        vec![(
            OsString::from("GEMINI_CLI_HOME"),
            gemini_home(&root).into_os_string()
        )]
    );
    assert_eq!(
        spec.args,
        vec![
            OsString::from("--policy"),
            gemini_policy_path(&root).into_os_string()
        ]
    );
    assert_eq!(spec.cwd, wt);
    assert_ne!(gemini_home(&root), gemini_home(&wt));
    assert_ne!(gemini_policy_path(&root), gemini_policy_path(&wt));
}

/// Two consoles on the same repo are the whole point — they must not collide,
/// and a slug that is not already a legal name must be folded into one rather
/// than minted as something the operator cannot type back.
#[test]
fn console_names_are_unique_and_sanitized() {
    let a = console_name("owner/ralphy");
    let b = console_name("owner/ralphy");
    assert_ne!(a, b, "two consoles on one repo must be addressable apart");

    for (slug, want) in [
        ("owner/My.Repo", "wb-my-repo-"),
        ("owner/repo_2", "wb-repo-2-"),
        ("bare", "wb-bare-"),
        ("owner/--repo--", "wb-repo-"),
        // Nothing survives the fold, so the name still has to BE something.
        ("owner/...", "wb-repo-"),
        ("owner/", "wb-repo-"),
    ] {
        let got = console_name(slug);
        assert!(
            got.starts_with(want),
            "{slug} should mint {want}<hex>, got {got}"
        );
    }
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
