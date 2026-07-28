//! Every fixture here builds its "remote" as a LOCAL directory and clones it by
//! PATH — the suite never names a network remote, so nothing in it can hang on
//! or depend on the network.

use super::*;
use crate::git::git;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-sync-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn configure(dir: &Path) {
    git(dir, &["config", "user.email", "t@example.com"]).unwrap();
    git(dir, &["config", "user.name", "Test"]).unwrap();
}

fn commit(dir: &Path, file: &str, body: &str, msg: &str) {
    std::fs::write(dir.join(file), body).unwrap();
    git(dir, &["add", "."]).unwrap();
    git(dir, &["commit", "-q", "-m", msg]).unwrap();
}

/// A repo with one commit on `main` and no remote of its own.
fn init_remote(name: &str) -> PathBuf {
    let dir = tmp(name);
    git(&dir, &["init", "-q", "-b", "main"]).unwrap();
    configure(&dir);
    commit(&dir, "a.txt", "one\n", "init");
    dir
}

/// A clone of `remote` by filesystem path.
fn clone_of(remote: &Path, name: &str) -> PathBuf {
    let dir = tmp(name);
    git(
        &std::env::temp_dir(),
        &[
            "clone",
            "-q",
            &remote.display().to_string(),
            &dir.display().to_string(),
        ],
    )
    .unwrap();
    configure(&dir);
    dir
}

fn fetch_head(repo: &Path) -> PathBuf {
    PathBuf::from(git(repo, &["rev-parse", "--absolute-git-dir"]).unwrap()).join("FETCH_HEAD")
}

fn head_sha(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"]).unwrap()
}

#[test]
fn tracking_reports_branch_upstream_and_counts() {
    let remote = init_remote("track-remote");
    let clone = clone_of(&remote, "track-clone");

    let st = status(&clone).unwrap();
    assert_eq!(
        st.head,
        Head::Branch {
            name: "main".to_string()
        }
    );
    assert_eq!(
        st.tracking,
        Some(Tracking {
            upstream: "origin/main".to_string(),
            ahead: 0,
            behind: 0,
        })
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn no_upstream_is_its_own_state() {
    let repo = init_remote("no-upstream");

    let st = status(&repo).unwrap();
    assert_eq!(
        st.head,
        Head::Branch {
            name: "main".to_string()
        }
    );
    assert!(
        st.tracking.is_none(),
        "no upstream must be absent, never a zeroed Tracking: {st:?}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn detached_head_is_a_state_not_an_error() {
    let remote = init_remote("detached-remote");
    let clone = clone_of(&remote, "detached-clone");
    commit(&clone, "b.txt", "two\n", "second");
    git(&clone, &["checkout", "-q", "--detach", "HEAD"]).unwrap();

    let st = status(&clone).expect("a detached HEAD is a state, not an error");
    assert!(
        matches!(st.head, Head::Detached { .. }),
        "expected a detached head: {st:?}"
    );
    assert!(st.tracking.is_none(), "detached tracks nothing: {st:?}");

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

/// The one input that trips an inverted `--left-right` reading: a swapped parse
/// still passes every symmetric fixture, and this is the negative control.
#[test]
fn behind_only_reads_one_behind_zero_ahead() {
    let remote = init_remote("behind-remote");
    let clone = clone_of(&remote, "behind-clone");
    commit(&remote, "b.txt", "two\n", "second");
    git(&clone, &["fetch", "-q"]).unwrap();

    assert_eq!(
        status(&clone).unwrap().tracking,
        Some(Tracking {
            upstream: "origin/main".to_string(),
            ahead: 0,
            behind: 1,
        })
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn ahead_only_reads_one_ahead_zero_behind() {
    let remote = init_remote("ahead-remote");
    let clone = clone_of(&remote, "ahead-clone");
    commit(&clone, "b.txt", "two\n", "local");

    assert_eq!(
        status(&clone).unwrap().tracking,
        Some(Tracking {
            upstream: "origin/main".to_string(),
            ahead: 1,
            behind: 0,
        })
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn diverged_reads_one_each_way() {
    let remote = init_remote("diverge-remote");
    let clone = clone_of(&remote, "diverge-clone");
    commit(&remote, "b.txt", "theirs\n", "theirs");
    commit(&clone, "c.txt", "ours\n", "ours");
    git(&clone, &["fetch", "-q"]).unwrap();

    assert_eq!(
        status(&clone).unwrap().tracking,
        Some(Tracking {
            upstream: "origin/main".to_string(),
            ahead: 1,
            behind: 1,
        })
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn status_makes_no_network_call() {
    let remote = init_remote("nonet-remote");
    let clone = clone_of(&remote, "nonet-clone");
    // A clone leaves no FETCH_HEAD, so its absence is an honest "never fetched"
    // AND an oracle for "status touched no remote".
    assert!(
        !fetch_head(&clone).exists(),
        "a fresh clone must leave no FETCH_HEAD"
    );

    for _ in 0..3 {
        assert!(status(&clone).unwrap().last_fetch.is_none());
    }
    assert!(
        !fetch_head(&clone).exists(),
        "status must not fetch: FETCH_HEAD appeared"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn last_fetch_is_the_fetch_head_mtime() {
    let remote = init_remote("stamp-remote");
    let clone = clone_of(&remote, "stamp-clone");
    assert!(status(&clone).unwrap().last_fetch.is_none());

    git(&clone, &["fetch", "-q"]).unwrap();

    let stamp = status(&clone)
        .unwrap()
        .last_fetch
        .expect("a fetched repo reports when");
    let parsed = chrono::DateTime::parse_from_rfc3339(&stamp)
        .unwrap_or_else(|e| panic!("last_fetch must be RFC 3339: {stamp:?} ({e})"));

    // The discriminating half: a reader that answered `now()` whenever
    // FETCH_HEAD exists would satisfy every assertion above AND every staleness
    // label downstream, so the label could always read "fetched just now" with
    // the whole gate green. The stamp must NOT move while nothing fetches…
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert_eq!(
        status(&clone).unwrap().last_fetch.as_deref(),
        Some(stamp.as_str()),
        "the stamp is the file's mtime, not the clock: it moved with no fetch"
    );

    // …and must move when something does.
    git(&clone, &["fetch", "-q"]).unwrap();
    let again = status(&clone).unwrap().last_fetch.expect("still stamped");
    let reparsed = chrono::DateTime::parse_from_rfc3339(&again).expect("RFC 3339");
    assert!(
        reparsed > parsed,
        "a second fetch must advance the stamp: {stamp} -> {again}"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

/// The absence of `FETCH_HEAD` proves "did not fetch", not "made no network
/// call" — `git ls-remote`, the natural way to freshen counts, writes no
/// FETCH_HEAD and would pass that oracle. Here the remote PATH is made
/// unreachable after the clone: the remote-tracking refs stay readable, so a
/// local-only reader answers unchanged, while anything that consults the remote
/// fails.
#[test]
fn status_consults_no_remote_at_all() {
    let remote = init_remote("unreachable-remote");
    let clone = clone_of(&remote, "unreachable-clone");
    commit(&remote, "b.txt", "two\n", "second");
    git(&clone, &["fetch", "-q"]).unwrap();
    let before = status(&clone).unwrap();

    let gone = remote.with_file_name(format!("ralphy-sync-{}-gone", std::process::id()));
    std::fs::rename(&remote, &gone).unwrap();
    // Positive control: the remote really is unreachable now.
    assert!(
        crate::git::git(&clone, &["ls-remote", "origin"]).is_err(),
        "the fixture must be unreachable, or this proves nothing"
    );

    let after = status(&clone).expect("status must answer with the remote gone");
    assert_eq!(after, before, "the read is local-only: {after:?}");

    let _ = std::fs::remove_dir_all(&gone);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn a_single_non_origin_remote_is_the_one_to_fetch() {
    let remote = init_remote("solo-remote");
    let clone = clone_of(&remote, "solo-clone");
    // A repo whose only remote is `upstream` HAS somewhere to fetch from;
    // answering `NoRemote` there would be a lie the operator cannot act on.
    git(&clone, &["remote", "rename", "origin", "upstream"]).unwrap();
    git(&clone, &["config", "--unset", "branch.main.remote"]).unwrap();
    commit(&remote, "b.txt", "two\n", "second");

    assert_eq!(
        fetch(&clone).unwrap(),
        FetchOutcome::Fetched {
            remote: "upstream".to_string()
        }
    );
    assert!(fetch_head(&clone).exists());

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn fetch_updates_the_remote_tracking_ref() {
    let remote = init_remote("fetch-remote");
    let clone = clone_of(&remote, "fetch-clone");
    commit(&remote, "b.txt", "two\n", "second");

    assert_eq!(
        fetch(&clone).unwrap(),
        FetchOutcome::Fetched {
            remote: "origin".to_string()
        }
    );
    assert!(fetch_head(&clone).exists(), "a fetch writes FETCH_HEAD");
    assert_eq!(status(&clone).unwrap().tracking.unwrap().behind, 1);

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn fetch_without_a_remote_refuses_and_touches_no_ref() {
    let repo = init_remote("fetch-no-remote");

    let outcome = fetch(&repo).unwrap();
    assert_eq!(outcome, FetchOutcome::NoRemote);
    assert!(
        outcome.reason().unwrap().contains("no remote"),
        "the refusal names the cause: {:?}",
        outcome.reason()
    );
    assert!(
        !fetch_head(&repo).exists(),
        "a refused fetch writes nothing"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn pull_fast_forwards_when_behind() {
    let remote = init_remote("ff-remote");
    let clone = clone_of(&remote, "ff-clone");
    commit(&remote, "b.txt", "two\n", "second");
    commit(&remote, "c.txt", "three\n", "third");
    fetch(&clone).unwrap();

    assert_eq!(
        pull(&clone).unwrap(),
        PullOutcome::FastForwarded { commits: 2 }
    );
    assert_eq!(
        head_sha(&clone),
        git(&clone, &["rev-parse", "origin/main"]).unwrap()
    );
    assert!(
        clone.join("c.txt").exists(),
        "the fast-forward landed on disk"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn a_diverged_branch_refuses_and_starts_no_merge() {
    let remote = init_remote("div-remote");
    let clone = clone_of(&remote, "div-clone");
    commit(&remote, "b.txt", "theirs\n", "theirs");
    commit(&clone, "c.txt", "ours\n", "ours");
    fetch(&clone).unwrap();
    let before = head_sha(&clone);

    let outcome = pull(&clone).unwrap();
    assert_eq!(
        outcome,
        PullOutcome::Diverged {
            ahead: 1,
            behind: 1
        }
    );
    assert_eq!(head_sha(&clone), before, "a refusal moves nothing");
    let merge_head = PathBuf::from(git(&clone, &["rev-parse", "--absolute-git-dir"]).unwrap())
        .join("MERGE_HEAD");
    assert!(
        !merge_head.exists(),
        "no merge is ever started: {} exists",
        merge_head.display()
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn pull_without_upstream_refuses_with_its_own_reason() {
    let repo = init_remote("pull-no-upstream");

    let outcome = pull(&repo).unwrap();
    assert_eq!(outcome, PullOutcome::NoUpstream);
    let reason = outcome.reason().unwrap();
    assert!(reason.contains("has no upstream"), "reason: {reason}");
    assert_ne!(
        Some(reason),
        PullOutcome::Diverged {
            ahead: 1,
            behind: 1
        }
        .reason(),
        "each refusal reads as its own problem"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn pull_on_a_detached_head_refuses() {
    let remote = init_remote("pull-detached-remote");
    let clone = clone_of(&remote, "pull-detached-clone");
    commit(&remote, "b.txt", "two\n", "second");
    fetch(&clone).unwrap();
    git(&clone, &["checkout", "-q", "--detach", "HEAD"]).unwrap();
    let before = head_sha(&clone);

    let outcome = pull(&clone).unwrap();
    assert_eq!(outcome, PullOutcome::DetachedHead);
    assert!(outcome.reason().unwrap().contains("detached"));
    assert_eq!(head_sha(&clone), before);

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn pull_up_to_date_and_ahead_only_are_both_up_to_date() {
    let remote = init_remote("uptodate-remote");
    let clone = clone_of(&remote, "uptodate-clone");

    assert_eq!(pull(&clone).unwrap(), PullOutcome::UpToDate);
    assert_eq!(pull(&clone).unwrap().reason(), None);

    commit(&clone, "b.txt", "ours\n", "ours");
    assert_eq!(
        pull(&clone).unwrap(),
        PullOutcome::UpToDate,
        "ahead-only has nothing to absorb"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn pull_with_a_conflicting_local_edit_is_blocked() {
    let remote = init_remote("blocked-remote");
    let clone = clone_of(&remote, "blocked-clone");
    commit(&remote, "a.txt", "theirs\n", "theirs");
    fetch(&clone).unwrap();
    std::fs::write(clone.join("a.txt"), "mine\n").unwrap();
    let before = head_sha(&clone);

    let outcome = pull(&clone).unwrap();
    assert_eq!(outcome, PullOutcome::WorkingTreeBlocked);
    assert!(
        outcome.reason().unwrap().contains("local changes"),
        "reason: {:?}",
        outcome.reason()
    );
    assert_eq!(head_sha(&clone), before, "a blocked pull moves nothing");
    assert_eq!(
        std::fs::read_to_string(clone.join("a.txt")).unwrap(),
        "mine\n",
        "the local edit survives"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

// ---------------------------------------------------------------------------
// push (#320). The remote is a BARE local directory here — a non-bare one
// refuses an update to its own checked-out branch, which would make every
// assertion below read as a push failure for the wrong reason.
// ---------------------------------------------------------------------------

/// A bare clone of a one-commit `main`, usable as a push target.
fn init_bare_remote(name: &str) -> PathBuf {
    let source = init_remote(&format!("{name}-src"));
    let dir = tmp(name);
    let _ = std::fs::remove_dir_all(&dir);
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
    let _ = std::fs::remove_dir_all(&source);
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

fn write_base_branch(repo: &Path, value: &str) {
    let ws = crate::Workspace::new(repo);
    let mut s = crate::Settings::load(&ws).unwrap();
    s.base_branch = Some(value.to_string());
    s.save(&ws).unwrap();
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

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
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

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn push_refuses_the_remotes_default_branch() {
    let remote = init_bare_remote("push-prot-remote");
    let clone = clone_of(&remote, "push-prot-clone");
    commit(&clone, "b.txt", "work\n", "work");
    let before = remote_ref(&remote, "refs/heads/main");

    let outcome = push(&clone).unwrap();
    assert_eq!(
        outcome,
        PushOutcome::ProtectedRef {
            branch: "main".to_string()
        }
    );
    assert!(
        outcome.reason().unwrap().contains("default branch"),
        "reason: {:?}",
        outcome.reason()
    );
    assert_eq!(
        remote_ref(&remote, "refs/heads/main"),
        before,
        "a refused push writes nothing to the remote"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn push_refuses_the_configured_base_branch() {
    let remote = init_bare_remote("push-base-remote");
    let clone = clone_of(&remote, "push-base-clone");
    branch_off(&clone, "dev");
    commit(&clone, "b.txt", "work\n", "work");
    // The operator's own configuration: a repo pointed at `origin/dev` protects
    // `dev` exactly as it protects the remote's default branch.
    write_base_branch(&clone, "origin/dev");

    assert_eq!(
        push(&clone).unwrap(),
        PushOutcome::ProtectedRef {
            branch: "dev".to_string()
        }
    );
    assert!(
        remote_ref(&remote, "refs/heads/dev").is_none(),
        "a refused push writes nothing to the remote"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
}

#[test]
fn push_ignores_a_base_branch_naming_another_remote() {
    let remote = init_bare_remote("push-other-remote");
    let clone = clone_of(&remote, "push-other-clone");
    branch_off(&clone, "dev");
    commit(&clone, "b.txt", "work\n", "work");
    // `upstream/dev` is a ref on a DIFFERENT remote; protecting `dev` on
    // `origin` because of it would be a name collision mistaken for a rule.
    write_base_branch(&clone, "upstream/dev");

    assert!(
        matches!(push(&clone).unwrap(), PushOutcome::Pushed { .. }),
        "a base branch on another remote protects nothing here"
    );

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
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

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&ours);
    let _ = std::fs::remove_dir_all(&theirs);
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

    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&clone);
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

    let _ = std::fs::remove_dir_all(&solo);
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

#[test]
fn strip_remote_only_strips_this_remote() {
    assert_eq!(
        strip_remote("origin/main", "origin").as_deref(),
        Some("main")
    );
    assert_eq!(
        strip_remote("origin/release/1.x", "origin").as_deref(),
        Some("release/1.x")
    );
    assert_eq!(strip_remote("upstream/main", "origin"), None);
    assert_eq!(strip_remote("main", "origin").as_deref(), Some("main"));
}
