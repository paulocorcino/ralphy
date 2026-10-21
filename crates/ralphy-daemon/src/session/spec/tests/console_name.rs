use super::*;

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
/// nothing answers to. A repo root that does not exist reads as un-opted
/// rather than panicking: the registry keeps an entry for a repo that has
/// moved away (`reachable()` is computed, never persisted), so the launch path
/// meets this path.
#[test]
fn a_claude_console_is_unnamed_until_the_repo_opts_in() {
    let d = tempfile::tempdir().unwrap();
    // (case, repo root)
    let rows = [
        ("un-opted repo", d.path().to_path_buf()),
        (
            "unreachable repo root",
            PathBuf::from("C:/Dev/no-such-repo-here"),
        ),
    ];
    for (case, root) in rows {
        let spec = spec_for(
            Agent::Claude,
            &root,
            root.clone(),
            "paulocorcino/ralphy",
            24,
            80,
        );
        assert!(
            spec.args.is_empty(),
            "{case}: an un-opted Claude launch carries no flags: {:?}",
            spec.args
        );
        assert!(spec.name.is_none(), "{case}: nothing may announce a name");
    }
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

/// With the opt-in on, Claude's `--name` is `wb-` + the folded console name.
/// The fold is what stands between the name and `cmd.exe` re-reading a `.cmd`
/// shim's command line, so argv must hold the folded form and nothing else.
#[test]
fn claude_name_comes_from_the_folded_console_name() {
    let d = tempfile::tempdir().unwrap();
    let repo = opted_in_repo(&d);
    let launch = |console: Option<&str>| {
        spec_with_status(
            Agent::Claude,
            &repo,
            repo.clone(),
            "owner/fincal",
            console,
            24,
            80,
            None,
        )
    };

    let spec = launch(Some("fincal #1"));
    assert_eq!(spec.name.as_deref(), Some("wb-fincal-1"));
    assert_eq!(
        spec.args,
        vec![OsString::from("--name"), OsString::from("wb-fincal-1")]
    );

    let spec = launch(Some(r#"a&b %PATH% "x""#));
    assert_eq!(
        spec.args,
        vec![OsString::from("--name"), OsString::from("wb-a-b-path-x")]
    );
    for arg in &spec.args {
        let arg = arg.to_string_lossy();
        assert!(
            !arg.contains(['&', '|', '<', '>', '^', '%', '"', ' ']),
            "a raw console name reached argv: {arg}"
        );
    }

    // Accents are not removed: each is a run of "other" characters.
    assert_eq!(launch(Some("ação")).name.as_deref(), Some("wb-a-o"));

    for console in [Some("🚀 !!"), Some(""), None] {
        let name = launch(console).name.expect("the opt-in is on");
        let hex = name
            .strip_prefix("wb-fincal-")
            .unwrap_or_else(|| panic!("{console:?} should keep the hex name, got {name}"));
        assert!(
            hex.len() == 4
                && hex
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{console:?} should end in 4 lowercase hex, got {name}"
        );
    }

    let off = tempfile::tempdir().unwrap();
    let root = off.path().to_path_buf();
    let spec = spec_with_status(
        Agent::Claude,
        &root,
        root.clone(),
        "owner/fincal",
        Some("fincal #1"),
        24,
        80,
        None,
    );
    assert!(spec.name.is_none() && spec.args.is_empty());
}
