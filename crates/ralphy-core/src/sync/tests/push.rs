use super::*;

// ---------------------------------------------------------------------------
// push (#320). The remote is a BARE local directory here — a non-bare one
// refuses an update to its own checked-out branch, which would make every
// assertion below read as a push failure for the wrong reason.
// ---------------------------------------------------------------------------

/// A bare clone of a one-commit `main`, usable as a push target.
fn init_bare_remote(name: &str) -> PathBuf {
    let source = init_remote(&format!("{name}-src"));
    let dir = tmp(name);
    remove_scratch(&dir);
    git(
        &std::env::temp_dir(),
        &[
            "clone",
            "-q",
            "--bare",
            &source.display().to_string(),
            &dir.display().to_string(),
        ],
    )
    .unwrap();
    remove_scratch(&source);
    dir
}

/// `git rev-parse` of a ref inside the bare remote, `None` when it has no such
/// ref — this is how a test asserts what actually landed on the other side.
fn remote_ref(remote: &Path, name: &str) -> Option<String> {
    let out = raw(remote, &["rev-parse", "--verify", "--quiet", name]).unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn branch_off(repo: &Path, name: &str) {
    git(repo, &["checkout", "-q", "-b", name]).unwrap();
}

#[test]
fn push_publishes_the_remotes_default_branch() {
    // ADR-0046 amendment 2026-09-16: the default branch is a branch like any
    // other. The operator named it by standing on it; a remote that must not
    // take the push says so itself (forge-side protection), not this seam.
    let remote = init_bare_remote("push-default-remote");
    let clone = clone_of(&remote, "push-default-clone");
    commit(&clone, "b.txt", "work\n", "work");
    let before = remote_ref(&remote, "refs/heads/main");

    let outcome = push(&clone).unwrap();
    assert!(
        matches!(&outcome, PushOutcome::Pushed { branch, .. } if branch == "main"),
        "got {outcome:?}"
    );
    assert_ne!(
        remote_ref(&remote, "refs/heads/main"),
        before,
        "the remote's main moved to the pushed commit"
    );

    remove_scratch(&remote);
    remove_scratch(&clone);
}

#[test]
fn push_publishes_a_branch_with_no_upstream_and_sets_one() {
    let remote = init_bare_remote("push-new-remote");
    let clone = clone_of(&remote, "push-new-clone");
    branch_off(&clone, "feat/x");
    commit(&clone, "b.txt", "work\n", "work");
    assert!(
        remote_ref(&remote, "refs/heads/feat/x").is_none(),
        "the branch is unpublished before the push"
    );

    let outcome = push(&clone).unwrap();
    assert_eq!(
        outcome,
        PushOutcome::Pushed {
            remote: "origin".to_string(),
            branch: "feat/x".to_string(),
            set_upstream: true,
        }
    );
    assert_eq!(
        outcome.reason(),
        None,
        "a push that worked is not a refusal"
    );
    assert_eq!(
        remote_ref(&remote, "refs/heads/feat/x").as_deref(),
        Some(head_sha(&clone).as_str()),
        "the remote carries exactly the commit that was pushed"
    );
    // The upstream it set is what makes the NEXT status meaningful.
    let st = status(&clone).unwrap();
    assert_eq!(
        st.tracking,
        Some(Tracking {
            upstream: "origin/feat/x".to_string(),
            ahead: 0,
            behind: 0,
        })
    );

    remove_scratch(&remote);
    remove_scratch(&clone);
}

#[test]
fn push_of_a_level_branch_is_up_to_date() {
    let remote = init_bare_remote("push-level-remote");
    let clone = clone_of(&remote, "push-level-clone");
    branch_off(&clone, "feat/y");
    commit(&clone, "b.txt", "work\n", "work");
    push(&clone).unwrap();

    let outcome = push(&clone).unwrap();
    assert_eq!(outcome, PushOutcome::UpToDate);
    assert_eq!(outcome.reason(), None, "nothing to push is not a refusal");

    remove_scratch(&remote);
    remove_scratch(&clone);
}

#[test]
fn push_refuses_a_remote_that_moved_on() {
    let remote = init_bare_remote("push-rej-remote");
    let ours = clone_of(&remote, "push-rej-ours");
    let theirs = clone_of(&remote, "push-rej-theirs");

    // Both start from the same published branch…
    branch_off(&ours, "feat/z");
    commit(&ours, "b.txt", "ours-1\n", "ours-1");
    push(&ours).unwrap();
    git(&theirs, &["fetch", "-q", "origin"]).unwrap();
    git(
        &theirs,
        &["checkout", "-q", "-b", "feat/z", "origin/feat/z"],
    )
    .unwrap();

    // …then the other side publishes a commit ours has never seen.
    commit(&theirs, "c.txt", "theirs\n", "theirs");
    push(&theirs).unwrap();
    let theirs_sha = remote_ref(&remote, "refs/heads/feat/z");

    commit(&ours, "d.txt", "ours-2\n", "ours-2");
    let outcome = push(&ours).unwrap();
    assert_eq!(
        outcome,
        PushOutcome::Rejected {
            remote: "origin".to_string()
        }
    );
    assert!(
        outcome.reason().unwrap().contains("pull first"),
        "reason: {:?}",
        outcome.reason()
    );
    assert_eq!(
        remote_ref(&remote, "refs/heads/feat/z"),
        theirs_sha,
        "the rejected push never overwrote the other side's commit"
    );

    remove_scratch(&remote);
    remove_scratch(&ours);
    remove_scratch(&theirs);
}

#[test]
fn push_from_a_detached_head_refuses() {
    let remote = init_bare_remote("push-det-remote");
    let clone = clone_of(&remote, "push-det-clone");
    let sha = head_sha(&clone);
    git(&clone, &["checkout", "-q", "--detach", &sha]).unwrap();

    let outcome = push(&clone).unwrap();
    assert_eq!(outcome, PushOutcome::DetachedHead);
    assert!(
        outcome.reason().unwrap().contains("check out a branch"),
        "reason: {:?}",
        outcome.reason()
    );

    remove_scratch(&remote);
    remove_scratch(&clone);
}

#[test]
fn push_without_a_remote_says_so() {
    let solo = init_remote("push-solo");
    branch_off(&solo, "feat/solo");
    commit(&solo, "b.txt", "work\n", "work");

    let outcome = push(&solo).unwrap();
    assert_eq!(outcome, PushOutcome::NoRemote);
    assert!(
        outcome.reason().unwrap().contains("no remote"),
        "reason: {:?}",
        outcome.reason()
    );

    remove_scratch(&solo);
}

#[test]
fn an_auth_failure_is_a_refusal_not_a_rejection() {
    // The exact shapes git produces under `GIT_TERMINAL_PROMPT=0` and from a
    // forge's HTTPS endpoint. Misfiling any of these as `Rejected` would tell
    // the operator to pull when the credential is what failed.
    for stderr in [
        "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        "remote: Authentication failed for 'https://github.com/o/r/'",
        "remote: Permission denied to user",
        "fatal: unable to access: The requested URL returned error: 403 Forbidden",
    ] {
        assert_eq!(
            classify_push_failure(stderr, "origin".to_string()),
            PushOutcome::AuthFailed {
                remote: "origin".to_string()
            },
            "stderr: {stderr}"
        );
    }
    // Anything unmodelled reads as the refusal whose advice is harmless when
    // wrong.
    assert_eq!(
        classify_push_failure(
            "! [rejected] feat -> feat (fetch first)",
            "origin".to_string()
        ),
        PushOutcome::Rejected {
            remote: "origin".to_string()
        }
    );
    assert_eq!(
        classify_push_failure("something nobody modelled", "origin".to_string()),
        PushOutcome::Rejected {
            remote: "origin".to_string()
        }
    );
}
