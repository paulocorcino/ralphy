//! Which entries of a listed directory the repo ignores: the `ignored` mark on
//! a [`super::Entry`] (ADR-0036, amendment 2026-09-30). The tree still lists
//! every entry; the mark only lets the workbench draw an ignored one muted.
//!
//! The cost is bounded on purpose. Only rule FILES are read — the `.gitignore`
//! of each directory from the root down to the listed one, `info/exclude`, and
//! the global excludes — never a directory. Nothing is cached, so the listing
//! that follows an edited `.gitignore` already honours it. Once an ancestor of
//! the listed directory is itself ignored, reading stops: git never looks
//! inside an ignored directory, so every entry below it is ignored.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;

/// One rule source and the directory it applies from: `base` is `/`-joined
/// and relative to the root, `""` for the root itself.
pub(crate) struct Layer {
    base: String,
    rules: Gitignore,
}

/// The rules that decide the entries of one listed directory.
pub(crate) enum Rules {
    /// An ancestor of the listed directory is ignored, so every entry is.
    AllIgnored,
    /// The rule sources, lowest precedence first: global excludes,
    /// `info/exclude`, then each `.gitignore` from the root downwards. A layer
    /// is shared, so the rules of a child folder reuse those of its parent.
    Layers(Vec<Arc<Layer>>),
}

impl Rules {
    /// The rules for the entries of the directory `dir` — its components
    /// relative to `root`, which is the checkout root the `.gitignore` files
    /// are anchored to.
    pub(crate) fn for_dir(root: &Path, dir: &[String]) -> Rules {
        let mut layers = Vec::new();
        push_global(&mut layers, root);
        if let Some(exclude) = exclude_file(root) {
            push_file(&mut layers, root, "", &exclude);
        }
        push_file(&mut layers, root, "", &root.join(".gitignore"));
        let mut rules = Rules::Layers(layers);
        let mut base = String::new();
        for part in dir {
            base = join(&base, part);
            rules = rules.descend(&base, &root.join(&base));
        }
        rules
    }

    /// The rules for the entries of the child folder `rel` (`/`-joined,
    /// relative to the root) at `dir` on the disk. An ignored folder gives
    /// [`Rules::AllIgnored`], and its own `.gitignore` is not read: git never
    /// looks inside it. Otherwise the folder's `.gitignore` is the deepest
    /// layer. The parent's layers are shared, never read again.
    pub(crate) fn descend(&self, rel: &str, dir: &Path) -> Rules {
        match self {
            Rules::AllIgnored => Rules::AllIgnored,
            Rules::Layers(layers) => {
                if decide(layers, rel, true) {
                    return Rules::AllIgnored;
                }
                let mut next = layers.clone();
                push_file(&mut next, dir, rel, &dir.join(".gitignore"));
                Rules::Layers(next)
            }
        }
    }

    /// Is the entry at `rel` (`/`-joined, relative to the root) ignored?
    pub(crate) fn ignored(&self, rel: &str, is_dir: bool) -> bool {
        match self {
            Rules::AllIgnored => true,
            Rules::Layers(layers) => decide(layers, rel, is_dir),
        }
    }
}

/// The components of the confined `dir`, relative to the confined `base`, as
/// the rules take them: the one place that cuts a path into rule coordinates.
pub(crate) fn components_below(base: &Path, dir: &Path) -> Vec<String> {
    dir.strip_prefix(base)
        .map(|r| {
            r.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `base` and `name` joined with `/`, `base` empty for the root.
pub(crate) fn join(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

/// Git's precedence: the deepest source that has an opinion decides, and a
/// `!pattern` whitelist is an opinion. Every layer is an ancestor of `rel` by
/// construction, so the strip always succeeds. Paths are handed to the matcher
/// RELATIVE to the layer's directory: `Gitignore::matched` strips an absolute
/// prefix with `/` only, which a Windows path does not use.
fn decide(layers: &[Arc<Layer>], rel: &str, is_dir: bool) -> bool {
    for layer in layers.iter().rev() {
        let local = if layer.base.is_empty() {
            rel
        } else {
            match rel
                .strip_prefix(layer.base.as_str())
                .and_then(|r| r.strip_prefix('/'))
            {
                Some(local) => local,
                None => continue,
            }
        };
        match layer.rules.matched(local, is_dir) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
    }
    false
}

/// Add the rules of the ignore file `file`, anchored at `dir`. A missing file
/// is the common case and adds nothing. A file that cannot be read or holds a
/// bad glob adds what it can: the listing must open whatever the rules say.
fn push_file(layers: &mut Vec<Arc<Layer>>, dir: &Path, base: &str, file: &Path) {
    if !file.is_file() {
        return;
    }
    let mut builder = GitignoreBuilder::new(dir);
    if let Some(e) = builder.add(file) {
        tracing::debug!(path = %file.display(), error = %e, "partly unreadable ignore file");
    }
    match builder.build() {
        Ok(rules) => layers.push(Arc::new(Layer {
            base: base.to_string(),
            rules,
        })),
        Err(e) => {
            tracing::debug!(path = %file.display(), error = %e, "ignore file gives no rules");
        }
    }
}

/// Add the global excludes (`core.excludesFile`, else the XDG default).
fn push_global(layers: &mut Vec<Arc<Layer>>, root: &Path) {
    let (rules, err) = GitignoreBuilder::new(root).build_global();
    if let Some(e) = err {
        tracing::debug!(error = %e, "global excludes give partial rules");
    }
    if !rules.is_empty() {
        layers.push(Arc::new(Layer {
            base: String::new(),
            rules,
        }));
    }
}

/// The `info/exclude` that applies to the checkout at `root`. A linked
/// worktree's `.git` is a FILE naming its git dir, and that dir's `commondir`
/// names the shared one: worktrees share the primary's `info/exclude`, which
/// is where ralphy's own worktree setup writes.
fn exclude_file(root: &Path) -> Option<PathBuf> {
    let git_dir = ralphy_git_read::git_dir(root)?;
    Some(
        ralphy_git_read::common_dir(&git_dir)?
            .join("info")
            .join("exclude"),
    )
}

#[cfg(test)]
mod tests;
