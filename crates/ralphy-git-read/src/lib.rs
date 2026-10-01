//! The read-only git facts that core, the daemon and the usage scan share:
//! the current head, whether the working tree is dirty, the `origin` URL, the
//! `.git` pointer file of a linked worktree, and the repo's `user.email`. This
//! crate is their one definition (ADR-0069 D1), and it only reads (D2): every
//! git argv it can build is a [`Read`], and [`run`] refuses one that is not on
//! [`READ_ONLY`]. It depends on no other Ralphy crate.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};
use serde::Serialize;

/// What HEAD points at. Detached is a STATE, not a failure: a repo mid-bisect
/// or on a checked-out tag reports it and the UI renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Head {
    Branch { name: String },
    Detached { sha: String },
}

/// Every git command this crate runs. A new read is a new variant, so the D2
/// test sees its argv.
#[derive(Debug, Clone, Copy)]
enum Read {
    SymbolicHead,
    ShortSha,
    Status,
    OriginUrl,
    UserEmail,
}

impl Read {
    #[cfg(test)]
    const ALL: [Read; 5] = [
        Read::SymbolicHead,
        Read::ShortSha,
        Read::Status,
        Read::OriginUrl,
        Read::UserEmail,
    ];

    fn argv(self) -> &'static [&'static str] {
        match self {
            Read::SymbolicHead => &["symbolic-ref", "--quiet", "--short", "HEAD"],
            Read::ShortSha => &["rev-parse", "--short", "HEAD"],
            Read::Status => &["status", "--porcelain=v2", "-z"],
            Read::OriginUrl => &["remote", "get-url", "origin"],
            Read::UserEmail => &["config", "--get", "user.email"],
        }
    }
}

/// The argv prefixes that only read. Prefixes, not bare subcommands:
/// `remote`, `config` and `symbolic-ref` can also write.
const READ_ONLY: &[&[&str]] = &[
    &["symbolic-ref", "--quiet"],
    &["rev-parse"],
    &["status"],
    &["remote", "get-url"],
    &["config", "--get"],
];

fn is_read_only(argv: &[&str]) -> bool {
    READ_ONLY.iter().any(|prefix| argv.starts_with(prefix))
}

/// The one git spawn of this crate.
fn run(repo: &Path, read: Read) -> Result<Output> {
    let argv = read.argv();
    if !is_read_only(argv) {
        bail!("`git {}` is not a read-only git command", argv.join(" "));
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(argv);
    // `git status` otherwise refreshes `index`, and that write retriggers the
    // daemon's HEAD watch (ADR-0036 HEAD-watch notes).
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`: a console-less daemon child never flashes a window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
        .with_context(|| format!("failed to spawn `git {}`", argv.join(" ")))
}

/// Trimmed stdout of a successful read, `None` when git failed or printed
/// nothing.
fn stdout_of(repo: &Path, read: Read) -> Option<String> {
    let out = run(repo, read).ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// HEAD as a branch name, or the short sha when detached. `symbolic-ref` exits
/// 1 on a detached HEAD, which is the whole discrimination: an unborn branch
/// still resolves here, so a fresh `git init` reports `Branch`.
pub fn head(repo: &Path) -> Result<Head> {
    let out = run(repo, Read::SymbolicHead)?;
    if out.status.success() {
        return Ok(Head::Branch {
            name: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        });
    }
    let out = run(repo, Read::ShortSha).context("resolving the sha of a detached HEAD")?;
    if !out.status.success() {
        bail!(
            "resolving the sha of a detached HEAD: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(Head::Detached {
        sha: String::from_utf8_lossy(&out.stdout).trim().to_string(),
    })
}

/// Whether the working tree differs from HEAD, by the change-set rule: a path
/// under the run directory ([`is_run_artifact`]) never counts. `repo` must be
/// the git toplevel, because the rule is anchored at the root.
pub fn dirty(repo: &Path) -> Result<bool> {
    let out = run(repo, Read::Status)?;
    if !out.status.success() {
        bail!(
            "reading the status of {}: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let records: Vec<&str> = text.split('\0').filter(|t| !t.is_empty()).collect();
    let mut i = 0;
    while i < records.len() {
        let record = records[i];
        i += 1;
        // The same field counts as the change set in `ralphy-core`: a `2`
        // (rename/copy) record takes the NEXT token as its original path.
        let (path, original) = match record.as_bytes().first() {
            Some(b'?') => (path_field(record, 2), None),
            Some(b'1') => (path_field(record, 9), None),
            Some(b'2') => {
                let original = records.get(i).copied();
                i += 1;
                (path_field(record, 10), original)
            }
            Some(b'u') => (path_field(record, 11), None),
            // `!` (ignored) and `#` (headers) carry no change.
            _ => continue,
        };
        if path.is_some_and(|p| !is_run_artifact(p))
            || original.is_some_and(|p| !is_run_artifact(p))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The path field of a porcelain-v2 record: everything after the first
/// `count - 1` space-separated fields. Only the path may contain a space.
fn path_field(record: &str, count: usize) -> Option<&str> {
    record
        .splitn(count, ' ')
        .nth(count - 1)
        .filter(|p| !p.is_empty())
}

/// Anchored at the repo root: a nested `docs/.ralphy/x` is a real change, only
/// the run directory itself is scratch.
pub fn is_run_artifact(path: &str) -> bool {
    path.starts_with(".ralphy/") || path.starts_with(".ralphy\\")
}

/// `git remote get-url origin`. `None` when there is no `origin` remote (a
/// local-only repo) or git cannot answer.
pub fn origin_url(repo: &Path) -> Option<String> {
    stdout_of(repo, Read::OriginUrl)
}

/// `git config --get user.email`. `None` when unset or empty.
pub fn user_email(repo: &Path) -> Option<String> {
    stdout_of(repo, Read::UserEmail)
}

/// The raw text after `gitdir:` on the first line of `<dir>/.git`, trimmed.
/// git writes this pointer FILE for a linked worktree (and a relative target
/// under `worktree.useRelativePaths`). `None` for a missing file, a `.git`
/// directory, or a first line that is not a `gitdir:` pointer.
pub fn pointer_target(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(".git")).ok()?;
    let target = text.lines().next()?.trim().strip_prefix("gitdir:")?.trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// The gitdir a `<dir>/.git` pointer file names, joined to `dir` (an absolute
/// target replaces it).
pub fn pointer_gitdir(dir: &Path) -> Option<PathBuf> {
    Some(dir.join(pointer_target(dir)?))
}

/// The git directory of the checkout at `dir`: `<dir>/.git` when that is a
/// directory, else what its pointer file names.
pub fn git_dir(dir: &Path) -> Option<PathBuf> {
    let dot_git = dir.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    pointer_gitdir(dir)
}

/// The shared git directory of `git_dir`: what its `commondir` file names (a
/// linked worktree's gitdir has one), else `git_dir` itself. `None` when
/// `commondir` exists but cannot be read or is empty.
pub fn common_dir(git_dir: &Path) -> Option<PathBuf> {
    let file = git_dir.join("commondir");
    if !file.exists() {
        return Some(git_dir.to_path_buf());
    }
    let text = std::fs::read_to_string(&file).ok()?;
    let target = text.lines().next()?.trim();
    (!target.is_empty()).then(|| git_dir.join(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git is on PATH in the test environment");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo on `main` with one empty commit.
    fn committed_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        git(
            dir,
            &[
                "-c",
                "user.email=t@e",
                "-c",
                "user.name=T",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
    }

    #[test]
    fn every_read_is_on_the_read_only_list() {
        for r in Read::ALL {
            assert!(
                is_read_only(r.argv()),
                "{r:?} builds a write: {:?}",
                r.argv()
            );
        }
        assert!(!is_read_only(&["config", "user.email", "x"]));
        assert!(!is_read_only(&["remote", "add", "origin", "u"]));
        assert!(!is_read_only(&["checkout", "main"]));
        assert!(!is_read_only(&["symbolic-ref", "HEAD", "refs/heads/x"]));
    }

    #[test]
    fn head_reports_branch_detached_and_linked_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        committed_repo(&repo);
        assert_eq!(
            head(&repo).unwrap(),
            Head::Branch {
                name: "main".into()
            }
        );

        let wt = tmp.path().join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt-branch",
                wt.to_str().unwrap(),
            ],
        );
        assert_eq!(
            head(&wt).unwrap(),
            Head::Branch {
                name: "wt-branch".into()
            }
        );

        git(&repo, &["checkout", "-q", "--detach"]);
        match head(&repo).unwrap() {
            Head::Detached { sha } => {
                assert!(sha.len() >= 7, "short sha too short: {sha}");
                assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "not hex: {sha}");
            }
            other => panic!("expected a detached head, got {other:?}"),
        }
    }

    #[test]
    fn origin_url_and_user_email_read_config() {
        let tmp = tempfile::tempdir().unwrap();
        committed_repo(tmp.path());
        assert_eq!(origin_url(tmp.path()), None);
        git(
            tmp.path(),
            &["remote", "add", "origin", "https://github.com/o/r.git"],
        );
        assert_eq!(
            origin_url(tmp.path()).as_deref(),
            Some("https://github.com/o/r.git")
        );
        git(tmp.path(), &["config", "user.email", "t@example.com"]);
        assert_eq!(user_email(tmp.path()).as_deref(), Some("t@example.com"));
    }

    #[test]
    fn dirty_ignores_the_run_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        committed_repo(dir);
        std::fs::create_dir(dir.join(".ralphy")).unwrap();
        std::fs::write(dir.join(".ralphy").join("x.txt"), "scratch").unwrap();
        assert!(!dirty(dir).unwrap(), "only the run directory changed");

        std::fs::write(dir.join("a.txt"), "a").unwrap();
        assert!(dirty(dir).unwrap(), "an untracked file is a change");

        git(dir, &["add", "a.txt"]);
        git(
            dir,
            &[
                "-c",
                "user.email=t@e",
                "-c",
                "user.name=T",
                "commit",
                "-q",
                "-m",
                "a",
            ],
        );
        assert!(!dirty(dir).unwrap(), "committed");
        git(dir, &["mv", "a.txt", "b.txt"]);
        assert!(dirty(dir).unwrap(), "a staged rename is a change");
    }

    #[test]
    fn pointer_file_resolves_with_or_without_space_and_commondir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        std::fs::write(dir.join(".git"), "gitdir: ../g\n").unwrap();
        assert_eq!(pointer_target(dir).as_deref(), Some("../g"));
        std::fs::write(dir.join(".git"), "gitdir:../g\r\n").unwrap();
        assert_eq!(pointer_target(dir).as_deref(), Some("../g"));
        assert_eq!(pointer_gitdir(dir), Some(dir.join("../g")));
        assert_eq!(git_dir(dir), Some(dir.join("../g")));

        std::fs::write(dir.join(".git"), "not a pointer\n").unwrap();
        assert_eq!(pointer_target(dir), None);
        assert_eq!(git_dir(dir), None);

        std::fs::remove_file(dir.join(".git")).unwrap();
        std::fs::create_dir(dir.join(".git")).unwrap();
        assert_eq!(pointer_target(dir), None);
        assert_eq!(git_dir(dir), Some(dir.join(".git")));

        let gd = dir.join("g");
        std::fs::create_dir(&gd).unwrap();
        assert_eq!(common_dir(&gd), Some(gd.clone()));
        std::fs::write(gd.join("commondir"), "../..\n").unwrap();
        assert_eq!(common_dir(&gd), Some(gd.join("../..")));
        std::fs::write(gd.join("commondir"), "\n").unwrap();
        assert_eq!(common_dir(&gd), None);
    }
}
