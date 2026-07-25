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
