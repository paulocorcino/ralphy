use super::*;

/// The argv/env the tests read back out of a built `Command`.
fn argv(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// D4's no-workspace case, which the plain "join `.ralphy` onto the repo"
/// implementation gets silently wrong: `draft_issues` and
/// `consolidate_knowledge` legitimately run outside any repository, and the
/// root they land on must be the SAME `<home>/.ralphy` the login probe
/// ensures — asserted as that literal value, not merely "some path".
#[test]
fn one_shot_base_falls_back_outside_a_repository() {
    // A REAL repository: `git::is_repo` shells out to `rev-parse`, which an
    // empty `.git` directory does not satisfy.
    let repo = tempfile::tempdir().expect("tempdir");
    ralphy_core::git::init(repo.path()).expect("git init");
    assert_eq!(
        one_shot_base(repo.path()),
        repo.path().join(".ralphy"),
        "in a workspace the one-shot shares the run's root"
    );

    // `git rev-parse` walks UP, so a `TMPDIR` that happens to sit inside a
    // checkout makes the no-workspace branch unreachable. Assert the
    // precondition rather than the fallback in that case: a red here on a
    // correct implementation would be worse than a stated skip.
    let plain = tempfile::tempdir().expect("tempdir");
    if ralphy_core::git::is_repo(plain.path()) {
        assert_eq!(one_shot_base(plain.path()), plain.path().join(".ralphy"));
        return;
    }
    assert_eq!(
        one_shot_base(plain.path()),
        ralphy_proc_util::home_dir()
            .expect("this host has a home directory")
            .join(".ralphy"),
        "with no workspace the one-shot shares the login probe's root"
    );
}

/// AC2, on the one-shot path: the child is pointed at the root Ralphy owns and
/// carries Ralphy's own policy document — neither is bypassed here. No child is
/// spawned: the discovery probe lives outside `prepare_root`, which is what
/// keeps this test spawn-free.
#[test]
fn the_one_shot_command_carries_the_owned_root_and_the_policy() {
    let base = tempfile::tempdir().expect("tempdir");
    let work = tempfile::tempdir().expect("tempdir");
    let cmd = one_shot_command(base.path(), work.path(), None).expect("prepare + build");

    let owned = base.path().join("gemini-home");
    let home = cmd
        .get_envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("GEMINI_CLI_HOME"))
        .and_then(|(_, v)| v)
        .expect("GEMINI_CLI_HOME must be set on the one-shot too");
    assert_eq!(
        Path::new(home),
        owned,
        "D4: the owned root, not the operator's"
    );

    let args = argv(&cmd);
    let i = args
        .iter()
        .position(|a| a == "--policy")
        .unwrap_or_else(|| panic!("the one-shot must carry a policy document: {args:?}"));
    let policy = owned.join(".gemini").join("ralphy-policy.toml");
    assert_eq!(Path::new(&args[i + 1]), policy, "argv: {args:?}");
    let body = fs::read_to_string(&policy).expect("the policy document must be written");
    assert!(body.contains(r#"toolName = "invoke_agent""#), "{body}");
    assert!(body.contains(r#"decision = "deny""#), "{body}");

    let j = args
        .iter()
        .position(|a| a == "--approval-mode")
        .unwrap_or_else(|| panic!("autonomy must be requested: {args:?}"));
    assert_eq!(args[j + 1], "yolo", "argv: {args:?}");

    // AC5, the half an argv CAN carry: the turn is an addressable session
    // under the owned `GEMINI_CLI_HOME` asserted above, which is where the
    // vendor puts its session record. That the record actually LANDS there is
    // a live observation, not an argv property — see ADR-0043's #259 section.
    let k = args
        .iter()
        .position(|a| a == "--session-id")
        .unwrap_or_else(|| panic!("the one-shot must be an addressable session: {args:?}"));
    let id = &args[k + 1];
    assert_eq!(id.len(), 36, "a v4 uuid: {id}");
    assert_eq!(id.matches('-').count(), 4, "a v4 uuid: {id}");
}

/// AC4's ordering, with the discriminating control: a managed host prints the
/// tool-server notice in EVERY log, so a ladder that let any revocation
/// pre-empt the limit would permanently misroute quota exhaustion — and a hard
/// stop must still outrank the limit, because it will not heal on a retry.
#[test]
fn the_one_shot_ladder_orders_revocation_before_limit() {
    const BOTH: &str = "Gemini CLI is not running in a trusted directory.\n\
                        Error: quota exceeded for this project\n";
    let msg = one_shot_stop(BOTH, Some(55), false).expect("a stop is reported");
    assert!(
        msg.contains("refused the workspace as untrusted"),
        "the hard stop must win: {msg}"
    );
    assert!(
        !msg.contains("provider limit"),
        "the limit note must not be what is reported: {msg}"
    );

    // The limit alone still routes as the limit.
    let limit = one_shot_stop("Error: quota exceeded\n", Some(1), false).expect("a stop");
    assert!(limit.contains("provider limit"), "{limit}");

    // The exit taxonomy alone, verbatim — the reason this path cannot go
    // through a runner that discards the child's status.
    assert_eq!(
        one_shot_stop("", Some(54), false).as_deref(),
        outcome::classify_exit(Some(54)).actionable_stop(),
        "exit 54 must carry its own diagnosis"
    );
    assert!(one_shot_stop("", Some(54), false)
        .unwrap()
        .contains("exit 54"));

    // A clean exit with nothing to report is not an error.
    assert_eq!(one_shot_stop("", Some(0), false), None);
    // …and a wall timeout with no other signal still is.
    assert!(one_shot_stop("", None, true)
        .expect("a timeout is a stop")
        .contains("wall timeout"));
    // On a timeout the child was KILLED, so `exit` is `None` and the
    // exit-code rung is inert — a routine administrator notice in the log
    // must not be reported in the reap's place.
    let reaped = one_shot_stop("MCP server disabled by administrator\n", None, true)
        .expect("a timeout is a stop");
    assert!(reaped.contains("wall timeout"), "{reaped}");
}

/// The gate the ladder must not be consulted past (HIGH, self-review): two of
/// its rungs key on FREE TEXT, and on this vendor that text is routine. A
/// managed host prints the tool-server notice in every log, and a backlog or
/// triage thread may merely MENTION a rate limit — neither may cost a session
/// that exited 0 its artifact.
#[test]
fn a_successful_one_shot_is_never_failed_by_its_own_log() {
    // The rungs are live on these strings — which is exactly why the gate
    // must exist.
    assert!(one_shot_stop("MCP server disabled by administrator\n", Some(0), false).is_some());
    assert!(one_shot_stop("the backlog mentions a rate limit\n", Some(0), false).is_some());

    let dir = tempfile::tempdir().expect("tempdir");
    for text in [
        "MCP server disabled by administrator\n",
        "the backlog mentions a rate limit\n",
    ] {
        let log_path = dir.path().join("one-shot.log");
        let kept = judge_one_shot(finished(0, text), &log_path)
            .expect("a session that exited 0 is not failed by its own log");
        assert_eq!(kept, text);
    }

    // …and the log is persisted before ANY bail, so every error message's
    // `(see <log>)` points at a file that exists.
    let log_path = dir.path().join("failed.log");
    judge_one_shot(finished(1, "Error: quota exceeded\n"), &log_path)
        .expect_err("a failed session is an error");
    assert_eq!(
        fs::read_to_string(&log_path).expect("the log was written"),
        "Error: quota exceeded\n"
    );
}

/// A finished child with the given exit code, whose stdout is `stdout`.
fn finished(code: i32, stdout: &str) -> ralphy_adapter_support::HeadlessOutput {
    #[cfg(unix)]
    let exit = std::os::unix::process::ExitStatusExt::from_raw(code << 8);
    #[cfg(windows)]
    let exit = std::os::windows::process::ExitStatusExt::from_raw(code.cast_unsigned());
    ralphy_adapter_support::HeadlessOutput {
        stdout: stdout.to_owned(),
        stderr: String::new(),
        timed_out: false,
        idle_killed: false,
        stopped: false,
        exit: Some(exit),
    }
}

#[test]
fn a_clean_one_shot_with_auth_text_keeps_its_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_path = dir.path().join("one-shot.log");
    let text = "Please set an Auth method in your settings\n";
    let kept = judge_one_shot(finished(0, text), &log_path)
        .expect("a one-shot that exited 0 keeps its result");
    assert_eq!(kept, text);
}

#[test]
fn a_failed_one_shot_with_auth_text_is_an_auth_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_path = dir.path().join("one-shot.log");
    let err = judge_one_shot(
        finished(1, "Please set an Auth method in your settings\n"),
        &log_path,
    )
    .expect_err("a one-shot that failed with a login error is an auth failure");
    assert_eq!(
        err.to_string(),
        format!("{GEMINI_AUTH_ERROR_MSG} (see {})", log_path.display())
    );
}

/// A stale artifact is the one failure mode with a GitHub-visible blast
/// radius: a drafting session that writes nothing would otherwise hand the cli
/// the PREVIOUS run's draft to publish.
#[test]
fn a_stale_artifact_never_survives_into_a_new_session() {
    let d = tempfile::tempdir().expect("tempdir");
    let out = d.path().join("nested").join("draft.json");
    let log = d.path().join("logs").join("init-issues.log");
    fs::create_dir_all(out.parent().unwrap()).unwrap();
    fs::write(&out, r#"{"issues":["a previous run's draft"]}"#).unwrap();

    clear_stale_artifact(&out, &log).expect("clearing the artifact");
    assert!(!out.exists(), "the previous run's draft must be gone");
    assert!(
        log.parent().unwrap().is_dir(),
        "the log's parent is created"
    );
    assert!(out.parent().unwrap().is_dir(), "the artifact's parent too");
}

/// `diagnose_repo`'s child runs in the throwaway `neutral_cwd`, but its owned
/// root belongs to the TARGET: a root under a directory the caller deletes
/// would make every diagnosis a fresh installation. Pinned on the source — no
/// test here spawns a child, so nothing else would catch the swap.
#[test]
fn each_verb_roots_itself_at_the_target_not_the_scratch_cwd() {
    let src = crate::tests::production_text(include_str!("../tasks.rs"));
    let code = crate::tests::code_of(include_str!("../tasks.rs"));
    assert!(
        crate::tests::fn_body(&code, "pubfndiagnose_repo(").contains(concat!(
            "one_shot_",
            "command(&one_shot_base(repo),neutral_cwd,"
        )),
        "diagnose_repo must root at the target repo while running in the neutral cwd"
    );
    let calls = code.matches(concat!("one_shot_", "command(")).count()
        - code.matches(concat!("fnone_shot_", "command(")).count();
    assert!(
        calls > 0,
        "the verbs build their command through one_shot_command"
    );
    assert_eq!(
        code.matches(concat!("one_shot_", "command(&one_shot_base("))
            .count(),
        calls,
        "every verb derives its base through one_shot_base, none hand-rolls one"
    );
    // The artifact BOM guard is the shared one, which is why `strip_bom` was
    // promoted to `pub` rather than copied here.
    assert!(src.contains("ralphy_adapter_support::strip_bom("));
}

/// AC2: an image fetched during triage reaches the charter as an
/// `@`-reference, appended after the manifest — never in its place.
#[test]
fn triage_prompt_carries_an_at_reference_per_fetched_image() {
    let repo = tempfile::tempdir().expect("tempdir");
    ralphy_core::git::init(repo.path()).expect("git init");
    let png = PathBuf::from("/tmp/attachments/1/red.png");
    let req = TriageRequest {
        issue_numbers: &[1],
        queue_label: "triage-agent",
        thread_block: "\n## Thread (issue #1)\nthread not fetched: x\n",
        attachments_manifest: "\n\n## Attachments\n- #1: red.png\n",
        image_paths: std::slice::from_ref(&png),
    };
    let out_path = repo.path().join("triage-draft.json");

    let prompt = triage_prompt(repo.path(), &req, &out_path);
    assert!(
        prompt.contains(&command::at_reference(&png, cfg!(windows))),
        "prompt: {prompt}"
    );
    assert!(
        prompt.contains(req.attachments_manifest),
        "the manifest text must survive verbatim: {prompt}"
    );
    assert!(
        prompt.contains(&format!("{}{}", req.thread_block, req.attachments_manifest)),
        "the thread block comes before the manifest: {prompt}"
    );
}

/// AC2's argv half: the triage command widens its workspace by exactly
/// each distinct fetched-attachment directory, and no other verb does.
#[test]
fn the_triage_command_includes_every_attachment_directory() {
    let base = tempfile::tempdir().expect("tempdir");
    let work = tempfile::tempdir().expect("tempdir");
    let images = [
        PathBuf::from("/tmp/attachments/1/a.png"),
        PathBuf::from("/tmp/attachments/1/b.png"),
        PathBuf::from("/tmp/attachments/2/c.png"),
    ];

    let mut cmd = one_shot_command(base.path(), work.path(), None).expect("prepare + build");
    command::add_include_directories(&mut cmd, &command::attachment_dirs(&images));

    let args = argv(&cmd);
    let positions: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--include-directories")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(positions.len(), 2, "argv: {args:?}");
    assert_eq!(
        args[positions[0] + 1],
        "/tmp/attachments/1",
        "argv: {args:?}"
    );
    assert_eq!(
        args[positions[1] + 1],
        "/tmp/attachments/2",
        "argv: {args:?}"
    );

    // `triage_issues` must pass the request's own images, and it is the
    // only verb that may widen the workspace this way.
    let src = crate::tests::production_text(include_str!("../tasks.rs"));
    let triage = src
        .split_at(
            src.find("pub fn triage_issues(")
                .expect("triage_issues must exist"),
        )
        .1;
    let (triage, _) = triage.split_once("\n}").expect("triage_issues must end");
    assert!(
        triage.contains(concat!(
            "command::add_include_",
            "directories(&mut cmd, &command::attachment_dirs(req.image_paths));"
        )),
        "triage_issues must include the directories of its own attachments"
    );
    assert_eq!(
        src.matches(concat!("add_include_", "directories(")).count(),
        1,
        "the other three one-shot verbs must not widen their workspace"
    );
}
