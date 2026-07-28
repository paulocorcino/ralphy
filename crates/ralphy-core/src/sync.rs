//! The branch's relation to its upstream: which branch HEAD is on, whether it
//! tracks anything, how far ahead/behind it is, and when the remote-tracking
//! refs were last refreshed — plus the acts that change those answers: `fetch`,
//! a fast-forward-only `pull`, and a `push` that publishes the branch.
//!
//! [`status`] makes NO network call, so a UI may read it freely; the counts it
//! reports are therefore as stale as the last fetch, which is why `last_fetch`
//! travels beside them. Every act here is the operator's own — a click or a
//! typed command — never a timer's and never a run's ([`push`] in particular:
//! ADR-0046 amendment, #320).
//!
//! Scope: this module holds status/fetch/pull/push only. Staging, discarding
//! and committing are working-tree operations that live in `changes`; they do
//! not belong here just because they are also git.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::git::{git, raw, raw_env};

/// What HEAD points at. Detached is a STATE, not a failure: a repo mid-bisect
/// or on a checked-out tag reports it and the UI renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Head {
    Branch { name: String },
    Detached { sha: String },
}

/// The branch's relation to its upstream. Absent (`Option::None` on
/// [`SyncStatus`]) when there is no upstream at all — which is a different
/// answer from a zeroed `Tracking`, and the UI must not conflate the two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tracking {
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
}

/// One read of the repo's sync state. `last_fetch` is RFC 3339 (local offset),
/// `None` when the repo has never fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncStatus {
    pub head: Head,
    pub tracking: Option<Tracking>,
    pub last_fetch: Option<String>,
}

/// Read the repo's sync state. Makes no network call — every command here is
/// local plumbing, so a caller may poll it without turning the process into a
/// scheduled network client.
///
/// `repo` must be the git TOPLEVEL, as [`crate::changes::changes`] requires:
/// callers resolve with [`crate::git::resolve_toplevel`].
pub fn status(repo: &Path) -> Result<SyncStatus> {
    let head = head_of(repo)?;
    let tracking = tracking_of(repo, &head)?;
    let last_fetch = last_fetch_of(repo)?;
    Ok(SyncStatus {
        head,
        tracking,
        last_fetch,
    })
}

/// HEAD as a branch name, or the short sha when detached. `symbolic-ref` exits
/// 1 on a detached HEAD, which is the whole discrimination — an unborn branch
/// still resolves here, so a fresh `git init` reports `Branch`.
fn head_of(repo: &Path) -> Result<Head> {
    let out = raw(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if out.status.success() {
        return Ok(Head::Branch {
            name: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        });
    }
    let sha = git(repo, &["rev-parse", "--short", "HEAD"])
        .context("resolving the sha of a detached HEAD")?;
    Ok(Head::Detached { sha })
}

/// The upstream and the two counts, or `None` when there is nothing to compare
/// against: a detached HEAD, an unborn branch, or a branch with no upstream.
fn tracking_of(repo: &Path, head: &Head) -> Result<Option<Tracking>> {
    if matches!(head, Head::Detached { .. }) {
        return Ok(None);
    }
    // An unborn branch has no commit to name in a revision range; `rev-list`
    // would fail rather than answer "zero".
    if !raw(repo, &["rev-parse", "--verify", "--quiet", "HEAD"])?
        .status
        .success()
    {
        return Ok(None);
    }
    let up = raw(
        repo,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    if !up.status.success() {
        return Ok(None);
    }
    let upstream = String::from_utf8_lossy(&up.stdout).trim().to_string();
    let range = format!("{upstream}...HEAD");
    let counts = git(repo, &["rev-list", "--left-right", "--count", &range])
        .with_context(|| format!("counting commits between HEAD and {upstream}"))?;
    // `--left-right` prints LEFT then RIGHT, and the range names the upstream on
    // the left: behind, then ahead. Inverting this is silent — the orientation
    // is pinned by a one-sided test fixture.
    let (behind, ahead) = parse_counts(&counts)
        .with_context(|| format!("parsing `git rev-list --left-right --count {range}`"))?;
    Ok(Some(Tracking {
        upstream,
        ahead,
        behind,
    }))
}

/// The two whitespace-separated counts of `rev-list --left-right --count`.
fn parse_counts(out: &str) -> Result<(usize, usize)> {
    let mut fields = out.split_whitespace();
    let left = fields.next().unwrap_or_default();
    let right = fields.next().unwrap_or_default();
    let left: usize = left
        .parse()
        .with_context(|| format!("left count {left:?} is not a number"))?;
    let right: usize = right
        .parse()
        .with_context(|| format!("right count {right:?} is not a number"))?;
    Ok((left, right))
}

/// When the remote-tracking refs were last refreshed, as the mtime of
/// `FETCH_HEAD`. A repo that has never fetched has no such file — including a
/// fresh clone, which writes its refs without one.
fn last_fetch_of(repo: &Path) -> Result<Option<String>> {
    let path = git_dir(repo)?.join("FETCH_HEAD");
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mtime = meta
        .modified()
        .with_context(|| format!("reading the mtime of {}", path.display()))?;
    Ok(Some(
        chrono::DateTime::<chrono::Local>::from(mtime).to_rfc3339(),
    ))
}

/// The repo's git directory as an absolute path (a worktree's is not
/// `<repo>/.git`, so this is asked rather than assumed).
fn git_dir(repo: &Path) -> Result<std::path::PathBuf> {
    let dir = git(repo, &["rev-parse", "--absolute-git-dir"])
        .with_context(|| format!("resolving the git directory of {}", repo.display()))?;
    Ok(std::path::PathBuf::from(dir))
}

/// What a [`fetch`] did. A repo with nowhere to fetch from is an OUTCOME, not
/// an `Err`: the operator asked a reasonable question and gets a reason back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    Fetched { remote: String },
    NoRemote,
}

impl FetchOutcome {
    /// Prose for a refusal, `None` when there was none. This is the operator's
    /// message — the CLI exits non-zero carrying it, and it is composed here
    /// rather than relayed from git's stderr.
    pub fn reason(&self) -> Option<String> {
        match self {
            FetchOutcome::Fetched { .. } => None,
            FetchOutcome::NoRemote => {
                Some("cannot fetch: this branch has no remote to fetch from".to_string())
            }
        }
    }
}

/// Fetch the branch's remote — the only network call in this module, and only
/// when the operator asks for it.
///
/// A transport or auth failure stays an `Err`: it is a failure, not an outcome,
/// and the distinction is what keeps "no remote configured" legible.
///
/// `GIT_TERMINAL_PROMPT=0` is pinned because this call runs under a console-less
/// daemon child: a remote whose credential is not cached would otherwise prompt
/// on a terminal nobody can see, and the click would hang with no feedback.
pub fn fetch(repo: &Path) -> Result<FetchOutcome> {
    let Some(remote) = remote_for_head(repo)? else {
        return Ok(FetchOutcome::NoRemote);
    };
    // `--end-of-options`: the remote name comes from the repo's own config, and
    // one starting with `-` would otherwise be parsed as a git option.
    let out = raw_env(
        repo,
        &["fetch", "--quiet", "--end-of-options", &remote],
        &[("GIT_TERMINAL_PROMPT", "0")],
    )
    .with_context(|| format!("fetching {remote}"))?;
    if !out.status.success() {
        bail!(
            "fetching {remote}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(FetchOutcome::Fetched { remote })
}

/// The remote the current branch fetches from: its own `branch.<name>.remote`
/// when configured, else `origin`, else the repo's ONE remote when it has
/// exactly one — a repo whose only remote is named `upstream` has somewhere to
/// fetch from, and answering `NoRemote` there would be a lie. Ambiguity (several
/// remotes, none of them `origin`, and no branch config) is `None`: this module
/// has no way to pick, and guessing is worse than refusing.
fn remote_for_head(repo: &Path) -> Result<Option<String>> {
    if let Head::Branch { name } = head_of(repo)? {
        let key = format!("branch.{name}.remote");
        let out = raw(repo, &["config", "--get", &key])?;
        if out.status.success() {
            let configured = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !configured.is_empty() {
                return Ok(Some(configured));
            }
        }
    }
    let listed = git(repo, &["remote"]).context("listing the repo's remotes")?;
    let remotes: Vec<&str> = listed
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect();
    if remotes.contains(&"origin") {
        return Ok(Some("origin".to_string()));
    }
    Ok(match remotes.as_slice() {
        [only] => Some((*only).to_string()),
        _ => None,
    })
}

/// What a [`pull`] did — or why it refused. Every variant but the first two is
/// a refusal whose [`PullOutcome::reason`] is the operator's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullOutcome {
    UpToDate,
    FastForwarded { commits: usize },
    NoUpstream,
    DetachedHead,
    Diverged { ahead: usize, behind: usize },
    WorkingTreeBlocked,
}

impl PullOutcome {
    /// Prose for a refusal, `None` when the pull succeeded. Each refusal reads
    /// as its own reason — "no upstream" and "diverged" are different problems
    /// with different next actions, and neither relays a git error string.
    pub fn reason(&self) -> Option<String> {
        Some(match self {
            PullOutcome::UpToDate | PullOutcome::FastForwarded { .. } => return None,
            PullOutcome::NoUpstream => {
                "cannot pull: this branch has no upstream — nothing to pull from".to_string()
            }
            PullOutcome::DetachedHead => {
                "cannot pull: HEAD is detached — check out a branch first".to_string()
            }
            PullOutcome::Diverged { ahead, behind } => format!(
                "cannot fast-forward: the branch and its upstream have diverged ({ahead} ahead, {behind} behind) — merge or rebase in a terminal"
            ),
            PullOutcome::WorkingTreeBlocked => {
                "cannot fast-forward: local changes would be overwritten — commit, stash or discard them first".to_string()
            }
        })
    }
}

/// Absorb the upstream's commits, fast-forward ONLY. Nothing that needs a merge
/// or a rebase is ever started — the workbench has no conflict-resolution UI and
/// this is what keeps it from needing one.
///
/// Cross-path invariant: every refusal path performs ZERO git writes. The
/// ordering is decide-then-write — [`status`]'s counts settle divergence before
/// any `git merge` runs, so a diverged branch never sees git at all.
pub fn pull(repo: &Path) -> Result<PullOutcome> {
    let st = status(repo)?;
    if matches!(st.head, Head::Detached { .. }) {
        return Ok(PullOutcome::DetachedHead);
    }
    let Some(tracking) = st.tracking else {
        return Ok(PullOutcome::NoUpstream);
    };
    if tracking.behind == 0 {
        // Ahead-only included: there is nothing upstream to absorb.
        return Ok(PullOutcome::UpToDate);
    }
    if tracking.ahead > 0 {
        return Ok(PullOutcome::Diverged {
            ahead: tracking.ahead,
            behind: tracking.behind,
        });
    }
    // `--end-of-options`: the upstream name comes from the repo's own config.
    let out = raw(
        repo,
        &["merge", "--ff-only", "--end-of-options", &tracking.upstream],
    )
    .with_context(|| format!("fast-forwarding onto {}", tracking.upstream))?;
    if !out.status.success() {
        // git's stderr never reaches the operator (the refusal prose is this
        // module's own), but dropping it unread would erase the only account of
        // a cause this classification does not model — so it is logged.
        tracing::warn!(
            upstream = %tracking.upstream,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "`git merge --ff-only` refused"
        );
        // Re-read rather than assume: the counts that cleared divergence are
        // from BEFORE the merge, and a run (or the operator) can have committed
        // in between. Only a still-fast-forwardable branch is really blocked by
        // the working tree.
        if let Some(t) = status(repo)?.tracking {
            if t.ahead > 0 && t.behind > 0 {
                return Ok(PullOutcome::Diverged {
                    ahead: t.ahead,
                    behind: t.behind,
                });
            }
        }
        return Ok(PullOutcome::WorkingTreeBlocked);
    }
    Ok(PullOutcome::FastForwarded {
        commits: tracking.behind,
    })
}

/// What a [`push`] did — or why it refused. Like [`PullOutcome`], every variant
/// but the first two is a refusal whose [`PushOutcome::reason`] is the
/// operator's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// `set_upstream` records that this push also gave the branch its upstream —
    /// the first publication of a fresh run branch, which the CLI says out loud
    /// because it changed the repo's config, not just the remote.
    Pushed {
        remote: String,
        branch: String,
        set_upstream: bool,
    },
    UpToDate,
    NoRemote,
    DetachedHead,
    /// The branch is one this module will not publish over: the remote's own
    /// default branch, or the configured `base_branch`. See [`push`].
    ProtectedRef {
        branch: String,
    },
    /// The remote moved on. Not remediated here — a force-push is exactly the
    /// destructive act this seam must never perform on an operator's behalf.
    Rejected {
        remote: String,
    },
    /// The remote refused the credential. Reported, never remediated: there is
    /// no credential prompt and no credential UI anywhere in this path.
    AuthFailed {
        remote: String,
    },
}

impl PushOutcome {
    /// Prose for a refusal, `None` when the push succeeded. Each refusal reads
    /// as its own reason and names the operator's next action; none of them
    /// relays a git error string.
    pub fn reason(&self) -> Option<String> {
        Some(match self {
            PushOutcome::Pushed { .. } | PushOutcome::UpToDate => return None,
            PushOutcome::NoRemote => {
                "cannot push: this branch has no remote to push to".to_string()
            }
            PushOutcome::DetachedHead => {
                "cannot push: HEAD is detached — check out a branch first".to_string()
            }
            PushOutcome::ProtectedRef { branch } => format!(
                "cannot push: '{branch}' is this repo's default branch — push it from a terminal if you mean it"
            ),
            PushOutcome::Rejected { remote } => format!(
                "cannot push: {remote} has commits this branch does not — pull first, then push again"
            ),
            PushOutcome::AuthFailed { remote } => format!(
                "cannot push: {remote} refused the credential — authenticate in a terminal, this will not prompt"
            ),
        })
    }
}

/// Publish the current branch to its remote. **The operator's own act**: this is
/// called from a click or a typed command, never from a run, a timer or a
/// scheduler (ADR-0046 amendment, #320) — which is why there is no opt-in flag
/// guarding it and why the agent's own `git push` deny rule is untouched.
///
/// A branch with no upstream gets one (`--set-upstream`), so a fresh run branch
/// is publishable without walking to a terminal.
///
/// Refuses on the repo's default branch ([`protected_branches`]) — the one way
/// this feature can do real damage, refused in code rather than in prose. Never
/// force-pushes: a remote that moved on is a refusal the operator resolves.
///
/// Cross-path invariant, as [`pull`]: every refusal path performs ZERO git
/// writes. `GIT_TERMINAL_PROMPT=0` is pinned for the same reason [`fetch`] pins
/// it — this runs under a console-less daemon child, where a credential prompt
/// would hang a click against a terminal nobody can see.
pub fn push(repo: &Path) -> Result<PushOutcome> {
    let st = status(repo)?;
    let Head::Branch { name: branch } = st.head else {
        return Ok(PushOutcome::DetachedHead);
    };
    let Some(remote) = remote_for_head(repo)? else {
        return Ok(PushOutcome::NoRemote);
    };
    if protected_branches(repo, &remote)?
        .iter()
        .any(|p| p == &branch)
    {
        return Ok(PushOutcome::ProtectedRef { branch });
    }
    // An upstream that is already level has nothing to publish; a branch with no
    // upstream always does, because setting one IS the act being asked for.
    let set_upstream = match &st.tracking {
        Some(t) if t.ahead == 0 => return Ok(PushOutcome::UpToDate),
        Some(_) => false,
        None => true,
    };
    let mut argv = vec!["push", "--quiet"];
    if set_upstream {
        argv.push("--set-upstream");
    }
    // `--end-of-options`: both the remote and the branch name come from the
    // repo's own config/refs, and either starting with `-` would otherwise be
    // parsed as a git option.
    argv.extend_from_slice(&["--end-of-options", &remote, &branch]);
    let out = raw_env(repo, &argv, &[("GIT_TERMINAL_PROMPT", "0")])
        .with_context(|| format!("pushing {branch} to {remote}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // As in `pull`: the refusal prose is this module's own, but git's stderr
        // is the only account of a cause this classification does not model, so
        // it is logged rather than dropped unread.
        tracing::warn!(%remote, %branch, stderr = %stderr.trim(), "`git push` refused");
        return Ok(classify_push_failure(&stderr, remote));
    }
    Ok(PushOutcome::Pushed {
        remote,
        branch,
        set_upstream,
    })
}

/// Which of git's two refusals this was. Anything unrecognized reads as
/// `Rejected`: it is the outcome whose advice ("pull first") is harmless when
/// wrong, whereas claiming an auth failure would send the operator to fix a
/// credential that works.
fn classify_push_failure(stderr: &str, remote: String) -> PushOutcome {
    let low = stderr.to_ascii_lowercase();
    // `terminal prompts disabled` is what GIT_TERMINAL_PROMPT=0 turns a
    // credential prompt into — it IS the auth failure, seen through the pin
    // that keeps this call from hanging.
    let auth = [
        "authentication failed",
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "permission denied",
        "access denied",
        "403 forbidden",
    ];
    if auth.iter().any(|m| low.contains(m)) {
        return PushOutcome::AuthFailed { remote };
    }
    PushOutcome::Rejected { remote }
}

/// The branches [`push`] will not publish over: the remote's own default branch
/// (`refs/remotes/<remote>/HEAD`) and the configured `base_branch`, each reduced
/// to a local branch name.
///
/// Both are ASKED rather than assumed — hardcoding `main`/`master` would be a
/// guess, and a repo whose trunk is `develop` deserves the same protection as
/// one whose trunk has the expected name. A repo that answers neither has no
/// protected branch, which is the honest answer: nothing here invents one.
fn protected_branches(repo: &Path, remote: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let head_ref = format!("refs/remotes/{remote}/HEAD");
    let sym = raw(repo, &["symbolic-ref", "--quiet", "--short", &head_ref])?;
    if sym.status.success() {
        let full = String::from_utf8_lossy(&sym.stdout).trim().to_string();
        if let Some(b) = strip_remote(&full, remote) {
            out.push(b);
        }
    }
    // `base_branch` is written as a remote-qualified ref (`origin/main`), and it
    // is the operator's own configuration — a repo pointed at `origin/dev`
    // protects `dev`.
    let ws = crate::Workspace::new(repo);
    let settings = crate::Settings::load(&ws).context("reading settings for the protected refs")?;
    if let Some(base) = settings.base_branch.as_deref() {
        let base = base.trim();
        let local = strip_remote(base, remote).unwrap_or_else(|| base.to_string());
        if !local.is_empty() && !out.contains(&local) {
            out.push(local);
        }
    }
    Ok(out)
}

/// `origin/main` → `main` for THIS remote; `None` when the ref names another
/// remote, so a `base_branch` of `upstream/main` does not silently protect
/// `main` on a different remote. A bare name (no slash) is returned as-is.
fn strip_remote(reference: &str, remote: &str) -> Option<String> {
    match reference.split_once('/') {
        Some((head, rest)) if head == remote => Some(rest.to_string()),
        Some(_) => None,
        None => Some(reference.to_string()),
    }
}

#[cfg(test)]
mod tests;
