//! `complexity` checks the complexity of every Rust production function
//! (ADR-0081). A function is scored by its cognitive complexity, read from
//! the source text ([`cognitive`]), so every `#[cfg]` branch counts on every
//! platform. `--check` fails a function over [`LIMIT`], unless the comment
//! lines right above it hold `// complexity-exception: <reason>`, with a
//! reason. No list of functions is stored. The TypeScript functions have the
//! same limit through the `complexity` rule of the workbench lint config. With
//! `--base <rev>`, the Rust functions at [`NEAR`] to [`LIMIT`] in the files
//! changed since `<rev>` are listed, and do not fail.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use regex::Regex;

use crate::asset_pins::repo_root;
use crate::capabilities::git;

/// The highest score a function may have.
const LIMIT: u32 = 10;
/// The lowest score of the near-limit list.
const NEAR: u32 = 7;
/// The attribute a marker comment becomes before the file is parsed: syn
/// drops `//` comments, and an attribute stays on the function it is above.
const EXCEPTION_ATTR: &str = "complexity_exception";

/// The score of one function. `path` is from the repo root, with `/`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    path: String,
    name: String,
    value: u32,
    /// The function has a `// complexity-exception: <reason>` marker.
    marked: bool,
}

pub fn complexity_cmd(args: &[String]) -> Result<()> {
    let mut check = false;
    let mut base = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--check" => check = true,
            "--base" => base = Some(crate::next_value(&mut it, "--base")?),
            other => bail!("unknown argument: {other}"),
        }
    }
    if !check {
        bail!("usage: complexity --check [--base <rev>]");
    }
    let root = repo_root();
    let mut scores = rust_scores(&root)?;
    scores.sort();
    if let Some(base) = base {
        print_near(&near_limit(&scores, &changed_files(&root, &base)?));
    }
    let errors = over_limit(&scores);
    if errors.is_empty() {
        let marked = scores
            .iter()
            .filter(|s| s.marked && s.value > LIMIT)
            .count();
        println!("no unmarked Rust production function over {LIMIT}; {marked} marked exception(s)");
        return Ok(());
    }
    for error in &errors {
        eprintln!("{error}");
    }
    bail!(
        "{} function(s) over {LIMIT}: split each one along a responsibility it already has",
        errors.len()
    )
}

/// Scores every function of the production Rust files under `crates/`,
/// from the source text (ADR-0081, amendment 2026-10-12).
fn rust_scores(root: &Path) -> Result<Vec<Score>> {
    let crates = root.join("crates");
    let mut scores = Vec::new();
    for entry in
        std::fs::read_dir(&crates).with_context(|| format!("reading {}", crates.display()))?
    {
        let dir = entry.context("reading a crate entry")?.path();
        if !dir.join("Cargo.toml").is_file() {
            continue;
        }
        for file in cognitive::production_files(&dir)? {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let path = rel_path(root, &file)?;
            scores.extend(file_check(&path, &text).with_context(|| format!("parsing {path}"))?);
        }
    }
    Ok(scores)
}

/// The scores of the functions of one file, with the marked ones known.
fn file_check(path: &str, text: &str) -> Result<Vec<Score>> {
    let found = cognitive::file_scores(&mark_exceptions(text))?;
    Ok(found
        .into_iter()
        .map(|f| Score {
            path: path.to_string(),
            name: f.name,
            value: f.value,
            marked: f.marked,
        })
        .collect())
}

/// `text` with each line `// complexity-exception: <reason>` turned into
/// the attribute [`EXCEPTION_ATTR`]. A marker with no reason stays a
/// comment, so it does not count. The line count does not change.
fn mark_exceptions(text: &str) -> String {
    static MARKER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^([ \t]*)// complexity-exception:[ \t]*\S.*$")
            .expect("a valid literal regex")
    });
    MARKER
        .replace_all(text, format!("${{1}}#[{EXCEPTION_ATTR}]"))
        .into_owned()
}

/// One error for each function over [`LIMIT`] that is not marked.
fn over_limit(scores: &[Score]) -> Vec<String> {
    scores
        .iter()
        .filter(|s| s.value > LIMIT && !s.marked)
        .map(|s| {
            format!(
                "{}: {}: {}, over the limit of {LIMIT}",
                s.path, s.name, s.value
            )
        })
        .collect()
}

/// The scores from [`NEAR`] to [`LIMIT`] in `files`.
fn near_limit<'a>(scores: &'a [Score], files: &BTreeSet<String>) -> Vec<&'a Score> {
    scores
        .iter()
        .filter(|s| (NEAR..=LIMIT).contains(&s.value) && files.contains(&s.path))
        .collect()
}

fn print_near(near: &[&Score]) {
    if near.is_empty() {
        return;
    }
    println!("near the limit of {LIMIT} in the changed files (not an error):");
    for s in near {
        println!("  {}: {} {}", s.path, s.name, s.value);
    }
}

/// The files changed since the merge base with `base`, committed or not,
/// and the new files git does not ignore.
fn changed_files(root: &Path, base: &str) -> Result<BTreeSet<String>> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])
        .with_context(|| format!("finding the merge base with {base}"))?;
    let changed = git(root, &["diff", "--name-only", merge_base.trim()])?;
    let new = git(root, &["ls-files", "--others", "--exclude-standard"])?;
    Ok(changed.lines().chain(new.lines()).map(slashes).collect())
}

/// `path` relative to `root`, with `/` separators on every platform.
fn rel_path(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
    Ok(slashes(&rel.to_string_lossy()))
}

fn slashes(path: &str) -> String {
    path.replace('\\', "/")
}

mod cognitive;

#[cfg(test)]
mod tests;
