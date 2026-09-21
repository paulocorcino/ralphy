use super::*;

fn argv(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn env_of(cmd: &Command, key: &str) -> Option<String> {
    cmd.get_envs()
        .find_map(|(k, v)| (k == key).then(|| v.map(|v| v.to_string_lossy().into_owned()))?)
}

/// D4: `--model` rides EVERY argv, and every flag D7 refuses is absent.
#[test]
fn argv_always_carries_a_model_and_never_the_refused_flags() {
    let unpinned = build_cursor_command("s1", None, Path::new("/repo"), Path::new("/run/cfg"));
    let args = argv(&unpinned);
    let i = args
        .iter()
        .position(|a| a == "--model")
        .unwrap_or_else(|| panic!("--model must never be omitted: {args:?}"));
    assert_eq!(args[i + 1], "auto", "argv: {args:?}");

    let pinned = build_cursor_command(
        "s1",
        Some("composer-2.5"),
        Path::new("/repo"),
        Path::new("/run/cfg"),
    );
    let args = argv(&pinned);
    let i = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(args[i + 1], "composer-2.5", "argv: {args:?}");

    // The rest of the blast radius, refused by absence (D7/D9).
    for cmd in [&unpinned, &pinned] {
        let args = argv(cmd);
        for flag in [
            "--auto-review",
            "--approve-mcps",
            "-w",
            "--worktree",
            "--worktree-base",
            "--sandbox",
            "--mode",
            "--plan",
        ] {
            assert!(
                !args.iter().any(|a| a == flag),
                "refused flag {flag} reached argv: {args:?}"
            );
        }
        // The autonomy it does need.
        assert!(args.iter().any(|a| a == "--force"), "argv: {args:?}");
        let i = args.iter().position(|a| a == "--output-format").unwrap();
        assert_eq!(args[i + 1], "stream-json", "argv: {args:?}");
        let i = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[i + 1], "s1", "argv: {args:?}");
    }
}

/// The one-shots inherit the run builder's hygiene wholesale: a `ralphy init`
/// against a repository is the same blast radius as a run, so a divergence here
/// would silently exempt four verbs from D4/D7/D17/D18.
#[test]
fn the_init_builder_matches_the_run_builders_hygiene() {
    let scratch = Path::new("/run/cfg");
    let cmd = build_cursor_init_command(None, Path::new("/repo"), scratch);
    let args = argv(&cmd);

    let i = args
        .iter()
        .position(|a| a == "--model")
        .unwrap_or_else(|| panic!("--model must never be omitted: {args:?}"));
    assert_eq!(args[i + 1], "auto", "argv: {args:?}");
    assert!(args.iter().any(|a| a == "--force"), "argv: {args:?}");
    let i = args.iter().position(|a| a == "--output-format").unwrap();
    assert_eq!(args[i + 1], "stream-json", "argv: {args:?}");
    for flag in [
        "--auto-review",
        "--approve-mcps",
        "-w",
        "--worktree",
        "--worktree-base",
        "--sandbox",
        "--mode",
        "--plan",
    ] {
        assert!(
            !args.iter().any(|a| a == flag),
            "refused flag {flag} reached a one-shot argv: {args:?}"
        );
    }

    assert_eq!(
        env_of(&cmd, "CURSOR_CONFIG_DIR").map(PathBuf::from),
        Some(scratch.to_path_buf())
    );
    assert_eq!(
        env_of(&cmd, "CURSOR_AGENT_DISABLE_DEBUG_LOG").as_deref(),
        Some("1")
    );

    // The two arguments a builder could silently ignore while still passing
    // every assertion above. `cwd` is D6's premise — the gate is evaluated on
    // the path the CHILD runs in, so a builder that dropped it would gate one
    // directory and index another. `model` dropped would pin every one-shot to
    // `auto` and discard the operator's `--model` on all four verbs.
    assert_eq!(cmd.get_current_dir(), Some(Path::new("/repo")));
    let pinned = build_cursor_init_command(Some("composer-2.5"), Path::new("/repo"), scratch);
    let args = argv(&pinned);
    let i = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(args[i + 1], "composer-2.5", "argv: {args:?}");
}

/// D2: the charter is piped. `-p` is the print-mode switch and takes no value,
/// so nothing after it may look like prompt text.
#[test]
fn argv_carries_no_prompt_word() {
    let cmd = build_cursor_command("s1", None, Path::new("/repo"), Path::new("/run/cfg"));
    let args = argv(&cmd);
    let i = args
        .iter()
        .position(|a| a == "-p")
        .expect("print mode must be requested");
    assert_eq!(
        args[i + 1],
        "--model",
        "`-p` takes no value — the charter rides stdin: {args:?}"
    );
    // Nothing on the argv is charter-sized prose.
    assert!(
        args.iter().all(|a| a.len() < 64),
        "a prompt-shaped argument reached argv: {args:?}"
    );
}

/// D17 + D18: the child never sees the operator's own config dir, and the
/// vendor's on-by-default debug log is off.
#[test]
fn the_child_runs_against_an_isolated_config_dir() {
    let run_dir = Path::new("/run/abc");
    let scratch = run_dir.join("cursor-config");
    let cmd = build_cursor_command("s1", None, Path::new("/repo"), &scratch);

    let got = env_of(&cmd, "CURSOR_CONFIG_DIR").expect("CURSOR_CONFIG_DIR must be set");
    assert_eq!(PathBuf::from(&got), scratch);
    assert!(
        PathBuf::from(&got).starts_with(run_dir),
        "the scratch dir must live under the run dir, got {got}"
    );
    assert!(
        !got.replace('\\', "/").ends_with("/.cursor"),
        "the child must never be pointed at the operator's own dir: {got}"
    );
    assert_eq!(
        env_of(&cmd, "CURSOR_AGENT_DISABLE_DEBUG_LOG").as_deref(),
        Some("1")
    );
    // D8: the credential vars are neither set nor removed.
    for key in ["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"] {
        assert!(
            !cmd.get_envs().any(|(k, _)| k == key),
            "{key} must be left exactly as the operator has it"
        );
    }
}

/// D17's one-way rule, proved by mutating the copy: the operator's file must be
/// byte-identical afterwards, and their policy must have arrived in the scratch.
#[test]
fn seeding_copies_cli_config_in_and_never_back() {
    const POLICY: &str = r#"{"permissions":{"deny":["Shell(git)"]}}"#;
    let operator = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let operator_file = operator.path().join("cli-config.json");
    std::fs::write(&operator_file, POLICY).unwrap();

    seed_cursor_config_dir(Some(operator.path()), scratch.path()).unwrap();

    // D7: the operator's deny list still applies under the isolation.
    let seeded = std::fs::read_to_string(scratch.path().join("cli-config.json")).unwrap();
    assert!(seeded.contains(r#""deny":["Shell(git)"]"#), "{seeded}");

    // The run mutates its copy the way `--model` does.
    std::fs::write(
        scratch.path().join("cli-config.json"),
        r#"{"model":"composer-2.5","hasChangedDefaultModel":true}"#,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&operator_file).unwrap(),
        POLICY,
        "nothing may ever be copied back"
    );
}

/// A fresh install has neither directory nor file; that is not an error.
#[test]
fn seeding_tolerates_a_missing_operator_config() {
    let scratch = tempfile::tempdir().unwrap();
    let target = scratch.path().join("cursor-config");
    seed_cursor_config_dir(None, &target).unwrap();
    assert!(target.is_dir(), "the scratch dir is created regardless");

    let empty = tempfile::tempdir().unwrap();
    seed_cursor_config_dir(Some(empty.path()), &target).unwrap();
    assert!(!target.join("cli-config.json").exists());
}

/// D14's four install shapes are covered where the search now lives
/// (`ralphy_proc_util::cursor::tests::locate_cursor_finds_each_install_shape`).
/// What stays this crate's business is that its own entry point really is that
/// search — the move's whole premise.
///
/// Comparing the two calls would be tautological (one IS the other), and
/// seeding a fake install cannot discriminate either: `%LOCALAPPDATA%\
/// cursor-agent` is itself on `PATH` on a real install, so a plain `PATH`
/// search finds the vendor by accident and agrees with the locator. What
/// discriminates deterministically on every host is the delegation ITSELF —
/// rewrite this crate's `locate_cursor` as `locate_program("cursor")` and the
/// source pin reds.
#[test]
fn locate_cursor_delegates_to_the_shared_vendor_locator() {
    let src = include_str!("../command.rs");
    let production = src.split("#[cfg(test)]").next().unwrap();
    assert!(
        production.contains("ralphy_proc_util::cursor::locate_cursor()"),
        "locate_cursor must BE the shared vendor search (ADR-0042 D19), not a \
             second implementation that can disagree with the daemon's"
    );
    // …and it is actually reached: the public entry point answers whatever the
    // shared search answers on this host, installed or not.
    assert_eq!(locate_cursor(), ralphy_proc_util::cursor::locate_cursor());
    assert_eq!(
        resolve_cursor_program(),
        locate_cursor()
            .map(PathBuf::into_os_string)
            .unwrap_or_else(|| NAMES[0].into())
    );
}

/// D20: on Windows the run pins `SHELL` to git-bash. The decision is a pure
/// function of two inputs, so every branch is asserted deterministically here;
/// the shape guarantee (the pinned path is what the vendor classifier accepts)
/// lives in `ralphy_proc_util::is_git_bash_shape` and its own tests.
#[cfg(windows)]
mod shell_pin {
    use super::*;

    #[test]
    fn no_shell_and_git_bash_found_pins_it() {
        let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        assert_eq!(git_bash_shell_pin(None, Some(bash.clone())), Some(bash));
    }

    #[test]
    fn an_operator_set_shell_is_never_overridden() {
        let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        assert_eq!(
            git_bash_shell_pin(Some("/usr/bin/fish".into()), Some(bash)),
            None,
            "a deliberate operator SHELL must pass through untouched (D8 stance)"
        );
    }

    #[test]
    fn no_git_bash_located_sets_nothing() {
        assert_eq!(
            git_bash_shell_pin(None, None),
            None,
            "SHELL must never point at a missing binary — PowerShell fallback stands"
        );
    }

    #[test]
    fn the_pinned_path_is_a_shape_the_vendor_classifier_accepts() {
        let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        let pinned = git_bash_shell_pin(None, Some(bash)).expect("pins when found");
        assert!(
            ralphy_proc_util::is_git_bash_shape(&pinned),
            "the pinned SHELL must match /git.*bash\\.exe$/i: {pinned:?}"
        );
    }

    /// The builder actually applies the pin: whatever the pure decision says for
    /// this host's real env, the `Command` reflects it. Recomputing the same
    /// inputs keeps it deterministic while still catching a wrong var name or a
    /// missing gate.
    #[test]
    fn build_cursor_command_applies_the_pin() {
        let cmd = build_cursor_command("s1", None, Path::new("."), Path::new("cfg"));
        let expected = git_bash_shell_pin(
            std::env::var_os("SHELL"),
            ralphy_proc_util::locate_git_bash(),
        );
        match expected {
            Some(p) => {
                assert_eq!(env_of(&cmd, "SHELL").map(PathBuf::from), Some(p));
                // MSYSTEM must ride along, or git-bash returns "no exit status".
                assert_eq!(
                    env_of(&cmd, "MSYSTEM").as_deref(),
                    Some("MINGW64"),
                    "pinning SHELL to git-bash without MSYSTEM breaks the shell"
                );
            }
            None => {
                assert!(
                    !cmd.get_envs().any(|(k, _)| k == "SHELL"),
                    "SHELL must not be set when the pin declines"
                );
                assert!(
                    !cmd.get_envs().any(|(k, _)| k == "MSYSTEM"),
                    "MSYSTEM must not be set when SHELL is not pinned"
                );
            }
        }
        // The one-shot builder delegates, so it inherits the same wiring.
        let init = build_cursor_init_command(None, Path::new("."), Path::new("cfg"));
        assert_eq!(env_of(&init, "SHELL"), env_of(&cmd, "SHELL"));
        assert_eq!(env_of(&init, "MSYSTEM"), env_of(&cmd, "MSYSTEM"));
    }
}

/// End-to-end validation of D20 against the **real** `cursor-agent` on a Windows
/// host. `#[ignore]` — it spawns the vendor CLI, needs a logged-in Cursor and
/// network, and costs a real model turn, so it never runs in CI. Invoke it by
/// hand in the lab:
///
/// ```text
/// cargo test -p ralphy-agent-cursor --lib -- --ignored --nocapture cursor_shell_pin_lets_a_heredoc_commit
/// ```
///
/// It drives the production builder (`build_cursor_command`) so the pin under
/// test is the shipped one, in a throwaway git repo (no trace, unlike touching
/// the lab repo), and forces the exact POSIX heredoc commit that ParserErrors
/// under PowerShell. The discriminator is the vendor's `ps-script` PowerShell
/// wrapper: under the git-bash pin it is never created; strip the pin (the
/// pre-fix state) and it reappears.
#[cfg(windows)]
mod e2e {
    use super::*;
    use std::io::{Read, Write};
    use std::thread;
    use std::time::{Duration, Instant};

    /// One instruction turn forcing the heredoc form the model reaches for out of
    /// habit — `-m "$(cat <<'EOF' … EOF)"` — the construct PowerShell rejects.
    const HEREDOC_PROMPT: &str = "\
You are in a git repository with a staged file. Make EXACTLY ONE commit by running \
this shell command VERBATIM, and nothing else — do not rewrite the quoting, do not \
use any other form:\n\n\
git commit -m \"$(cat <<'EOF'\nProbe: multi-line heredoc commit\n\nSecond paragraph proving the message survived the heredoc.\nEOF\n)\"\n\n\
After it succeeds, stop.";

    fn git(repo: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(repo)
            .args(args)
            .status()
            .expect("git must be on PATH")
            .success();
        assert!(ok, "git {args:?} failed");
    }

    /// Run `cmd` to completion (180s watchdog), feeding the prompt on stdin and
    /// returning stdout+stderr combined — the stream the fold and this probe read.
    fn run_capture(mut cmd: Command, prompt: &str) -> String {
        let mut child = cmd.spawn().expect("spawn cursor-agent");
        let pid = child.id();
        {
            let mut stdin = child.stdin.take().expect("piped stdin");
            stdin.write_all(prompt.as_bytes()).expect("write prompt");
        } // drop closes stdin → cursor's print mode processes the turn
        let mut out = child.stdout.take().expect("piped stdout");
        let mut err = child.stderr.take().expect("piped stderr");
        let ho = thread::spawn(move || {
            let mut s = String::new();
            let _ = out.read_to_string(&mut s);
            s
        });
        let he = thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        let start = Instant::now();
        loop {
            if child.try_wait().expect("try_wait").is_some() {
                break;
            }
            if start.elapsed() > Duration::from_secs(180) {
                ralphy_proc_util::kill_tree_by_pid(pid);
                let _ = child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        format!(
            "{}\n{}",
            ho.join().unwrap_or_default(),
            he.join().unwrap_or_default()
        )
    }

    /// Prepare a throwaway repo with one staged file and the indexing opt-out the
    /// runner would write, returning `(repo, scratch_config_dir)`.
    fn lab_repo() -> (tempfile::TempDir, tempfile::TempDir) {
        let repo = tempfile::tempdir().expect("tempdir");
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["config", "user.email", "probe@ralphy.test"]);
        git(repo.path(), &["config", "user.name", "ralphy probe"]);
        std::fs::write(repo.path().join("README.md"), "probe\n").expect("write file");
        // Opt out of the codebase upload exactly as the D6 gate does.
        std::fs::write(repo.path().join(".cursorindexingignore"), "*\n").expect("opt-out");
        git(repo.path(), &["add", "-A"]);
        let cfg = tempfile::tempdir().expect("config tempdir");
        (repo, cfg)
    }

    fn head_message(repo: &Path) -> String {
        let out = Command::new("git")
            .current_dir(repo)
            .args(["log", "-1", "--pretty=%B"])
            .output()
            .expect("git log");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    #[ignore = "e2e: spawns real cursor-agent, needs a logged-in Cursor + network"]
    fn cursor_shell_pin_lets_a_heredoc_commit_land_without_a_ps_script_wrapper() {
        // Precondition: this host is the production shape the fix targets — no
        // operator SHELL, and git-bash present so the pin can fire.
        assert!(
            std::env::var_os("SHELL").is_none(),
            "unset SHELL for this probe — the fix only pins when the operator has not"
        );
        let git_bash = ralphy_proc_util::locate_git_bash()
            .expect("git-bash must be installed to validate D20 on this host");

        // ---- FIX arm: the shipped builder, whose pin sets SHELL=git-bash. ----
        let (repo, cfg) = lab_repo();
        let fixed = build_cursor_command(&mint_session_id(), None, repo.path(), cfg.path());
        assert_eq!(
            env_of(&fixed, "SHELL").map(PathBuf::from),
            Some(git_bash.clone()),
            "the production builder must pin SHELL to git-bash"
        );
        assert_eq!(
            env_of(&fixed, "MSYSTEM").as_deref(),
            Some("MINGW64"),
            "MSYSTEM must ride along or git-bash returns 'no exit status'"
        );
        let stream = run_capture(fixed, HEREDOC_PROMPT);
        assert!(
            !stream.contains("no exit status"),
            "the git-bash shell must return exit codes (MSYSTEM present); stream:\n{stream}"
        );

        assert!(
                !stream.contains("ps-script"),
                "the git-bash pin must avoid the PowerShell ps-script wrapper entirely; stream:\n{stream}"
            );
        let msg = head_message(repo.path());
        assert!(
                msg.contains("multi-line heredoc commit") && msg.contains("Second paragraph"),
                "the multi-line heredoc commit must have landed; HEAD message:\n{msg}\nstream:\n{stream}"
            );

        // ---- BASELINE arm: the same builder minus the pin (the pre-fix state). ----
        // Best-effort reproduction — proof the harness would catch a regression.
        let (repo2, cfg2) = lab_repo();
        let mut baseline =
            build_cursor_command(&mint_session_id(), None, repo2.path(), cfg2.path());
        baseline.env_remove("SHELL");
        baseline.env_remove("MSYSTEM");
        let baseline_stream = run_capture(baseline, HEREDOC_PROMPT);
        eprintln!(
            "baseline (no pin) reproduced the PowerShell wrapper: {}",
            baseline_stream.contains("ps-script") || baseline_stream.contains("ParserError")
        );
    }
}

/// D20 is Windows-only: on Linux/macOS `SHELL` is already a POSIX shell and
/// Ralphy must never touch it, on either builder.
#[test]
#[cfg(not(windows))]
fn shell_is_never_touched_off_windows() {
    for cmd in [
        build_cursor_command("s1", None, Path::new("."), Path::new("cfg")),
        build_cursor_init_command(None, Path::new("."), Path::new("cfg")),
    ] {
        assert!(
            !cmd.get_envs().any(|(k, _)| k == "SHELL" || k == "MSYSTEM"),
            "ralphy must not set SHELL/MSYSTEM off Windows"
        );
    }
}

#[test]
fn mint_session_id_is_a_fresh_uuid() {
    let a = mint_session_id();
    assert_ne!(a, mint_session_id());
    assert_eq!(a.len(), 36, "not a hyphenated UUID: {a}");
    assert_eq!(a.matches('-').count(), 4, "not a hyphenated UUID: {a}");
}

/// ADR-0040 C1: naming the bare binary in a `Command` constructor fails on
/// Windows for a `.cmd` shim — and on this vendor it fails everywhere, since it
/// is on `PATH` on neither platform (D14). Fragments are assembled with
/// `concat!` so this assertion cannot match itself.
#[test]
fn no_direct_command_new() {
    // Ban a STRING-LITERAL program name outright: `cursor-agent` and `agent`
    // are both wrong here (neither is on `PATH`), so pinning one spelling would
    // miss the other.
    let production = include_str!("../command.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    assert!(
        !production.contains(concat!("Command::", "new(\"")),
        "resolve_cursor_program is the only way to name the binary"
    );
}
