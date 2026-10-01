//! The forge writes a headless session must not run, for an adapter whose
//! vendor has a deny policy of its own and no pre-tool hook (ADR-0072 D6).
//! The Claude adapter's guard hook keeps a wider list; this one is the
//! minimal set that issue #516 decided: `git push` and every `gh pr` verb
//! that writes. The `gh pr` reads and `gh api` stay allowed.

/// Each denied command as its leading words. A vendor rule matches a command
/// that starts with these words, whatever arguments follow.
pub const DENIED_FORGE_WRITES: [&[&str]; 9] = [
    &["git", "push"],
    &["gh", "pr", "create"],
    &["gh", "pr", "edit"],
    &["gh", "pr", "ready"],
    &["gh", "pr", "reopen"],
    &["gh", "pr", "review"],
    &["gh", "pr", "comment"],
    &["gh", "pr", "merge"],
    &["gh", "pr", "close"],
];
