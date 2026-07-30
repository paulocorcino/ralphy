//! `ralphy daemon add` on a directory that is not a git repository yet (#363).
//!
//! Registration needs a toplevel, and a plain directory has none — until now
//! that was a dead end with an error. Here it becomes an offer: initialize a
//! repository at the path (the same `git init` + initial-commit pair
//! `ralphy init`'s bootstrap uses) and register what it produced.
//!
//! It stops short of `ralphy init`'s bootstrap in one deliberate way: NO GitHub
//! repo is created. `daemon add` must not require an authenticated `gh` just to
//! put a local directory in the sidebar.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use ralphy_core::git;

/// Resolve `path`'s repository toplevel, offering to initialize one when the
/// directory is not a repository. `force` (the `--init` flag) skips the question.
///
/// A non-terminal stdin DECLINES without ever asking, unless `--init` was
/// passed: [`crate::init::ask_yes_no`] reads a line and EOF yields `""`, which
/// `create_repo_decision` maps to YES — so a piped or CI caller would otherwise
/// silently get a repository it never assented to. The flag is precisely the
/// non-interactive path, which is why the decline is spelled here, as the
/// answer channel, rather than as a second branch inside the core.
pub(crate) fn resolve_or_init_repo(path: &Path, force: bool) -> Result<PathBuf> {
    resolve_or_init_repo_with(path, force, || {
        if std::io::stdin().is_terminal() {
            crate::init::ask_yes_no("Initialize a git repository here?", true)
        } else {
            Ok("n".to_string())
        }
    })
}

/// The testable core: `ask` supplies the raw answer line, so the decision is
/// exercised without a terminal.
pub(crate) fn resolve_or_init_repo_with<F>(path: &Path, force: bool, ask: F) -> Result<PathBuf>
where
    F: FnOnce() -> Result<String>,
{
    if git::is_repo(path) {
        return git::resolve_toplevel(path);
    }
    if !force && !crate::init::create_repo_decision(&ask()?) {
        bail!(
            "not a git repository: {} (pass --init to initialize one here)",
            path.display()
        );
    }
    git::init(path)
        .with_context(|| format!("initializing a git repository at {}", path.display()))?;
    // A failure HERE leaves a `.git` this call created, with no commit in it —
    // say so, because the next `daemon add` will see a repository and take the
    // unchanged path over a half-built one. `commit.gpgsign` and a missing
    // `user.email` are the two that actually bite.
    git::initial_commit(path).with_context(|| {
        format!(
            "recording the initial commit in the repository just created at {} (it exists but is empty)",
            path.display()
        )
    })?;
    println!("Initialized git repository.");
    git::resolve_toplevel(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Give `git commit` an identity and disable signing for the whole test
    /// binary. Without this the two tests that actually commit fail on any host
    /// with no global `user.email` (a fresh CI image) or with `commit.gpgsign`
    /// on — a failure of the HOST, not of the code under test. Process-wide and
    /// idempotent, so parallel test threads cannot race it.
    fn git_identity() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            for (k, v) in [
                ("GIT_AUTHOR_NAME", "ralphy-test"),
                ("GIT_AUTHOR_EMAIL", "ralphy-test@example.com"),
                ("GIT_COMMITTER_NAME", "ralphy-test"),
                ("GIT_COMMITTER_EMAIL", "ralphy-test@example.com"),
                ("GIT_CONFIG_COUNT", "1"),
                ("GIT_CONFIG_KEY_0", "commit.gpgsign"),
                ("GIT_CONFIG_VALUE_0", "false"),
            ] {
                std::env::set_var(k, v);
            }
        });
    }

    /// `--init` initializes, commits, and the result registers — with the prompt
    /// never reached (the closure panics if it is).
    #[test]
    fn force_initializes_and_registers() {
        git_identity();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("fresh");
        std::fs::create_dir_all(&dir).expect("mkdir");

        let top = resolve_or_init_repo_with(&dir, true, || panic!("must not ask")).expect("init");

        assert!(dir.join(".git").exists(), "a repository was created");
        let head = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .expect("git rev-parse");
        assert!(head.status.success(), "the initial commit exists");

        let registry_path = tmp.path().join("repos.toml");
        super::super::register_repo_at(&registry_path, &top).expect("register");
        let store = ralphy_daemon::registry::load_from(&registry_path).expect("load");
        assert_eq!(store.repos.len(), 1, "the registry holds the new repo");
    }

    /// An existing repository takes the unchanged path: git's own toplevel, and
    /// no question asked.
    #[test]
    fn existing_repo_is_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir)
            .status()
            .expect("git init");
        assert!(status.success());

        let got =
            resolve_or_init_repo_with(dir, false, || panic!("must not ask")).expect("resolve");
        let expected = git::resolve_toplevel(dir).expect("toplevel");
        assert_eq!(got, expected);
    }

    /// The negative control: invert the decision and this goes red. Declining
    /// reproduces today's error verbatim and creates NOTHING.
    #[test]
    fn decline_reproduces_the_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("fresh");
        std::fs::create_dir_all(&dir).expect("mkdir");

        let err =
            resolve_or_init_repo_with(&dir, false, || Ok("n\n".to_string())).expect_err("declined");
        assert!(
            err.to_string().starts_with("not a git repository:"),
            "got: {err}"
        );
        assert!(!dir.join(".git").exists(), "nothing was created");
    }

    /// Enter on the `[Y/n]` prompt initializes — the default is yes, which is
    /// what makes the `[Y/n]` hint honest.
    #[test]
    fn prompt_defaults_to_yes() {
        git_identity();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("fresh");
        std::fs::create_dir_all(&dir).expect("mkdir");

        resolve_or_init_repo_with(&dir, false, || Ok("\n".to_string()))
            .expect("empty answer means yes");
        assert!(dir.join(".git").exists());
    }
}
