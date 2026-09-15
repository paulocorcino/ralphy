//! The `checkout` argument of a repo-scoped verb (ADR-0036 `checkout`
//! amendment, ADR-0063 §2): a worktree NAME the browser sends beside `repo`,
//! resolved here to a `rel` prefix under the SAME registered root —
//! `.ralphy/worktrees/<name>/…` — so [`crate::confine`] and the watcher never
//! learn a second root and a client can never name a path.
//!
//! The resolver never spawns. Every Observe verb is documented "reads state,
//! never spawns" ([`crate::dispatch`]), so the name is checked against the
//! pointer FILE git writes for a linked worktree (`<wt>/.git` = `gitdir: …`)
//! rather than against a `worktree list` child — the same read ADR-0063 §5
//! chose for the usage scan. The pointer is what git's own listing reads, so
//! this still resolves against git's record, not against a directory that
//! merely exists. A lexical shape gate ([`lexical`]) runs BEFORE the name is
//! ever joined to a path.

use std::path::{Component, Path};

/// Where linked worktrees live, relative to the primary root (ADR-0063 §1;
/// the same constant the CLI's `worktree add` writes under).
pub const WORKTREES_REL: &str = ".ralphy/worktrees";

/// The one refusal message a verb answers for a name that does not resolve —
/// the shell drops its selection on exactly this text (ADR-0063 §4).
pub const UNKNOWN: &str = "unknown checkout";

/// A resolved worktree name: shape-gated and (for [`resolve`]) known to the
/// caller's predicate. Its only power is [`Checkout::prefix`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    name: String,
}

impl Checkout {
    /// The worktree name as the browser sent it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The operator's `rel` prefixed under this worktree's directory. An empty
    /// (or all-slash) `rel` is the worktree root itself.
    pub fn prefix(&self, rel: &str) -> String {
        let rel = rel.trim_matches('/');
        if rel.is_empty() {
            format!("{WORKTREES_REL}/{}", self.name)
        } else {
            format!("{WORKTREES_REL}/{}/{rel}", self.name)
        }
    }
}

/// Why a `checkout` value did not resolve. One variant on purpose: a malformed
/// name, a missing worktree and a non-string value all answer [`UNKNOWN`] — the
/// reply must not distinguish "no such name" from "not a name" to a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutError {
    /// The name is malformed, unknown, or not a string.
    Unknown,
}

impl std::fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckoutError::Unknown => f.write_str(UNKNOWN),
        }
    }
}

impl std::error::Error for CheckoutError {}

/// The shape gate: a name is exactly ONE normal path component — so not empty,
/// no separator, not `.`/`..`, and not a Windows drive or root prefix either
/// (`C:` is a `Prefix` component, and `Path::join("C:")` would REPLACE the
/// base). Runs before any join; `None` is a refusal.
pub fn lexical(name: &str) -> Option<Checkout> {
    let mut parts = Path::new(name).components();
    let one_normal = matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none();
    // `components()` normalises a lone `.` away and never yields a separator,
    // so the byte checks stay as the belt to its braces.
    if !one_normal || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
        return None;
    }
    Some(Checkout {
        name: name.to_string(),
    })
}

/// Shape-gate `name`, then ask `known` whether it is a worktree of this root.
/// `known` is only consulted for a name that passed the gate.
pub fn resolve(name: &str, known: impl FnOnce(&str) -> bool) -> Result<Checkout, CheckoutError> {
    let c = lexical(name).ok_or(CheckoutError::Unknown)?;
    if known(c.name()) {
        Ok(c)
    } else {
        Err(CheckoutError::Unknown)
    }
}

/// `true` when `<root>/.ralphy/worktrees/<name>/.git` is a FILE whose first
/// line is a `gitdir:` pointer — what git writes for a linked worktree. A
/// `.git` DIRECTORY (a nested repository) fails the read and answers `false`.
pub fn is_linked(root: &Path, name: &str) -> bool {
    let pointer = root.join(WORKTREES_REL).join(name).join(".git");
    match std::fs::read_to_string(pointer) {
        Ok(text) => text
            .lines()
            .next()
            .is_some_and(|line| line.trim().starts_with("gitdir:")),
        Err(_) => false,
    }
}

/// Read the optional `checkout` key of a verb payload. Absent or `null` is
/// `Ok(None)` (the primary tree); a string resolves against the pointer file
/// under `root`; anything else is [`CheckoutError::Unknown`].
pub fn from_payload(
    payload: &serde_json::Value,
    root: &Path,
) -> Result<Option<Checkout>, CheckoutError> {
    match payload.get("checkout") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => resolve(s, |n| is_linked(root, n)).map(Some),
        Some(_) => Err(CheckoutError::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_known_name_prefixes_under_the_fixed_location() {
        let c = resolve("wt-a", |n| n == "wt-a").expect("known name resolves");
        assert_eq!(c.name(), "wt-a");
        assert_eq!(c.prefix(""), ".ralphy/worktrees/wt-a");
        assert_eq!(c.prefix("src"), ".ralphy/worktrees/wt-a/src");
        assert_eq!(c.prefix("src/"), ".ralphy/worktrees/wt-a/src");
    }

    #[test]
    fn unknown_name_is_unknown_checkout() {
        let e = resolve("nope", |_| false).expect_err("unknown name is refused");
        assert_eq!(e, CheckoutError::Unknown);
        assert_eq!(e.to_string(), "unknown checkout");
    }

    #[test]
    fn shape_gate_refuses_before_asking() {
        // Negative control: a `known` that says yes to everything must still
        // lose to the shape gate.
        // `C:`/`C:x` are the Windows drive-relative shapes `Path::join` would
        // let REPLACE the base; `/x` and `\x` are rooted.
        for name in [
            "", "a/b", "a\\b", ".", "..", "../x", "C:", "C:x", "/x", "\\x",
        ] {
            assert!(
                resolve(name, |_| true).is_err(),
                "{name:?} must fail the shape gate"
            );
            assert!(lexical(name).is_none(), "{name:?} must fail lexical");
        }
        assert!(lexical("wt-a").is_some());
    }

    #[test]
    fn from_payload_absent_or_null_is_none() {
        let root = Path::new("C:/nowhere");
        assert_eq!(from_payload(&json!({}), root), Ok(None));
        assert_eq!(from_payload(&json!({ "checkout": null }), root), Ok(None));
        assert_eq!(
            from_payload(&json!({ "checkout": 5 }), root),
            Err(CheckoutError::Unknown)
        );
    }

    #[test]
    fn is_linked_wants_a_gitdir_pointer_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let wts = root.join(WORKTREES_REL);
        std::fs::create_dir_all(wts.join("wt-a")).expect("mkdir wt-a");
        std::fs::write(
            wts.join("wt-a").join(".git"),
            "gitdir: C:/r/.git/worktrees/wt-a\n",
        )
        .expect("write pointer");
        assert!(is_linked(root, "wt-a"));

        assert!(!is_linked(root, "missing"));

        std::fs::create_dir_all(wts.join("wt-b").join(".git")).expect("mkdir wt-b/.git");
        assert!(
            !is_linked(root, "wt-b"),
            "a .git DIRECTORY is not a pointer"
        );

        std::fs::create_dir_all(wts.join("wt-c")).expect("mkdir wt-c");
        std::fs::write(wts.join("wt-c").join(".git"), "not a pointer\n").expect("write junk");
        assert!(!is_linked(root, "wt-c"));
    }

    #[test]
    fn from_payload_resolves_a_linked_name_against_the_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let wt = root.join(WORKTREES_REL).join("wt-a");
        std::fs::create_dir_all(&wt).expect("mkdir");
        std::fs::write(wt.join(".git"), "gitdir: /r/.git/worktrees/wt-a\n").expect("pointer");
        let c = from_payload(&json!({ "checkout": "wt-a" }), root)
            .expect("resolves")
            .expect("some");
        assert_eq!(c.prefix("x"), ".ralphy/worktrees/wt-a/x");
        assert_eq!(
            from_payload(&json!({ "checkout": "wt-z" }), root),
            Err(CheckoutError::Unknown)
        );
    }
}
