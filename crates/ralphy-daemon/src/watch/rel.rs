//! The watch-set form of a repo-relative dir.

/// Normalize an incoming rel dir to the watch-set form: `/`-separated, no leading
/// or trailing slash; `""` stays the root. Public so a tree connection stores
/// a connection's watched dirs in the SAME form the nudges carry, so its
/// per-connection filter matches.
pub fn norm_rel(rel: &str) -> String {
    rel.replace('\\', "/").trim_matches('/').to_owned()
}

/// Whether `rel` is a dir the HEAD watch holds: a gitdir in the standard
/// layout (`.git`, `.git/worktrees/<name>`) or its `logs/`. Lexical only.
pub fn is_head_rel(rel: &str) -> bool {
    let gitdir = rel.strip_suffix("/logs").unwrap_or(rel);
    if gitdir == ".git" {
        return true;
    }
    gitdir
        .strip_prefix(".git/worktrees/")
        .is_some_and(|name| !name.is_empty() && !name.contains('/'))
}
