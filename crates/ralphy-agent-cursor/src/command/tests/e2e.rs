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
git commit -m \"$(cat <<'EOF'\nProbe: multi-line heredoc commit\n\n\
Second paragraph proving the message survived the heredoc.\nEOF\n)\"\n\n\
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
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a read error leaves the partial output, which the test prints"
        )]
        let _ = out.read_to_string(&mut s);
        s
    });
    let he = thread::spawn(move || {
        let mut s = String::new();
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a read error leaves the partial output, which the test prints"
        )]
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
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the process tree was killed, and a failed wait only delays the reap"
            )]
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
        "the git-bash pin must avoid the PowerShell ps-script wrapper entirely; \
         stream:\n{stream}"
    );
    let msg = head_message(repo.path());
    assert!(
        msg.contains("multi-line heredoc commit") && msg.contains("Second paragraph"),
        "the multi-line heredoc commit must have landed; HEAD message:\n{msg}\n\
         stream:\n{stream}"
    );

    // ---- BASELINE arm: the same builder minus the pin (the pre-fix state). ----
    // Best-effort reproduction — proof the harness would catch a regression.
    let (repo2, cfg2) = lab_repo();
    let mut baseline = build_cursor_command(&mint_session_id(), None, repo2.path(), cfg2.path());
    baseline.env_remove("SHELL");
    baseline.env_remove("MSYSTEM");
    let baseline_stream = run_capture(baseline, HEREDOC_PROMPT);
    eprintln!(
        "baseline (no pin) reproduced the PowerShell wrapper: {}",
        baseline_stream.contains("ps-script") || baseline_stream.contains("ParserError")
    );
}
