//! `capabilities` — lists what a change adds that gives the code a new power:
//! network access, a new URL host, a subprocess, `unsafe`, a read of a secret,
//! encoded or minified text, a new crate, a build script, a changed workflow,
//! or changed agent instructions.
//!
//! No scanner can prove that code is not malicious. This check makes each new
//! power visible, so a reviewer reads those lines before the change merges.
//! The `capabilities.yml` workflow runs it from the base branch's code, so a
//! pull request cannot change the rules that check it.

mod rules;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Capability {
    Network,
    Process,
    Unsafe,
    Credential,
    DynamicCode,
    Encoded,
    Obfuscated,
    Dependency,
    BuildScript,
    Binary,
    Workflow,
    AgentText,
    SecurityPolicy,
    Vendored,
}

impl Capability {
    fn label(self) -> &'static str {
        match self {
            Capability::Network => "network",
            Capability::Process => "subprocess",
            Capability::Unsafe => "unsafe",
            Capability::Credential => "credential",
            Capability::DynamicCode => "dynamic code",
            Capability::Encoded => "encoded data",
            Capability::Obfuscated => "minified code",
            Capability::Dependency => "new crate",
            Capability::BuildScript => "build script",
            Capability::Binary => "binary file",
            Capability::Workflow => "workflow",
            Capability::AgentText => "agent instructions",
            Capability::SecurityPolicy => "security policy",
            Capability::Vendored => "vendored code",
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Finding {
    pub(crate) cap: Capability,
    pub(crate) path: String,
    pub(crate) line: Option<u32>,
    pub(crate) detail: String,
}

impl Finding {
    fn new(cap: Capability, path: &str, line: Option<u32>, detail: &str) -> Self {
        Finding {
            cap,
            path: path.to_string(),
            line,
            detail: detail.to_string(),
        }
    }
}

/// One file of a unified diff: its path after the change (before it, for a
/// deleted file), its added lines with their line numbers, and its removed
/// lines.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct FileDiff {
    pub(crate) path: String,
    pub(crate) binary: bool,
    pub(crate) added: Vec<(u32, String)>,
    pub(crate) removed: Vec<String>,
}

pub fn capabilities_cmd(args: &[String]) -> Result<()> {
    let mut repo: Option<PathBuf> = None;
    let mut base: Option<String> = None;
    let mut head = "HEAD".to_string();
    let mut github = false;
    let mut check = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--repo" => repo = Some(PathBuf::from(crate::next_value(&mut it, "--repo")?)),
            "--base" => base = Some(crate::next_value(&mut it, "--base")?),
            "--head" => head = crate::next_value(&mut it, "--head")?,
            "--github" => github = true,
            "--check" => check = true,
            other => bail!("unknown argument: {other}"),
        }
    }
    let base = base.context("--base <rev> is required")?;
    let repo = repo.unwrap_or_else(crate::asset_pins::repo_root);

    let findings = scan(&repo, &base, &head)?;
    if github {
        for f in &findings {
            println!("{}", annotation(f));
        }
    }
    print!("{}", to_markdown(&findings));
    if check && !findings.is_empty() {
        bail!("the change adds {} capabilities for review", findings.len());
    }
    Ok(())
}

/// Every finding in the change from the merge base of `base` and `head` to
/// `head`.
fn scan(repo: &Path, base: &str, head: &str) -> Result<Vec<Finding>> {
    let merge_base = git(repo, &["merge-base", base, head])?.trim().to_string();
    let diff = git(
        repo,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--unified=0",
            &merge_base,
            head,
        ],
    )?;
    let files = parse_diff(&diff);
    // A line removed in one place and added in another is code that moved: the
    // capability it holds was already in the tree.
    let moved: HashSet<&str> = files
        .iter()
        .flat_map(|f| f.removed.iter().map(|l| l.trim()))
        .filter(|l| !l.is_empty())
        .collect();
    let mut findings = Vec::new();
    let mut hosts: BTreeMap<String, (String, u32)> = BTreeMap::new();
    for file in &files {
        findings.extend(rules::path_findings(file));
        for (line, text) in &file.added {
            if moved.contains(text.trim()) {
                continue;
            }
            let (found, named) = rules::line_findings(&file.path, *line, text);
            findings.extend(found);
            for host in named {
                hosts
                    .entry(host)
                    .or_insert_with(|| (file.path.clone(), *line));
            }
        }
    }
    for (host, (path, line)) in hosts {
        if !tree_mentions(repo, &merge_base, &host)? {
            let detail = format!("a URL host the repository did not name before: `{host}`");
            findings.push(Finding::new(
                Capability::Network,
                &path,
                Some(line),
                &detail,
            ));
        }
    }
    let before = rules::lock_crates(&git_show(repo, &merge_base, "Cargo.lock")?);
    let after = rules::lock_crates(&git_show(repo, head, "Cargo.lock")?);
    for name in after.difference(&before) {
        let detail = format!("adds the crate `{name}` to Cargo.lock");
        findings.push(Finding::new(
            Capability::Dependency,
            "Cargo.lock",
            None,
            &detail,
        ));
    }
    findings.sort();
    findings.dedup();
    Ok(findings)
}

/// Parse `git diff --unified=0` output.
pub(crate) fn parse_diff(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut next_line = 0u32;
    // The `+++`/`---` file headers come before the first hunk; inside a hunk,
    // a line that starts with `+++` is an added line.
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest.rsplit_once(" b/").map_or(rest, |(_, b)| b);
            files.push(FileDiff {
                path: path.to_string(),
                ..FileDiff::default()
            });
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(hunk) = line.strip_prefix("@@ ") {
            next_line = hunk_start(hunk).unwrap_or(0);
            in_hunk = true;
        } else if !in_hunk {
            if line.starts_with("Binary files ") {
                file.binary = true;
            }
        } else if let Some(text) = line.strip_prefix('+') {
            file.added.push((next_line, text.to_string()));
            next_line += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            file.removed.push(text.to_string());
        }
    }
    files
}

/// The first new-side line of a hunk header body such as `-3,2 +4,5 @@`.
fn hunk_start(hunk: &str) -> Option<u32> {
    let new = hunk.split_whitespace().find_map(|p| p.strip_prefix('+'))?;
    new.split(',').next()?.parse().ok()
}

/// A GitHub Actions annotation that shows the finding on the pull request diff.
fn annotation(f: &Finding) -> String {
    let line = f.line.map(|l| format!(",line={l}")).unwrap_or_default();
    format!(
        "::warning file={}{line},title=capability: {}::{}",
        f.path,
        f.cap.label(),
        f.detail
    )
}

fn to_markdown(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "No new capabilities.\n".to_string();
    }
    let mut out = format!(
        "## New capabilities\n\n{} finding(s). Read these lines before the change merges.\n\n\
         | Capability | Where | What |\n|---|---|---|\n",
        findings.len()
    );
    for f in findings {
        let place = match f.line {
            Some(l) => format!("`{}:{l}`", f.path),
            None => format!("`{}`", f.path),
        };
        out.push_str(&format!("| {} | {place} | {} |\n", f.cap.label(), f.detail));
    }
    out
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).context("git output is not UTF-8")
}

/// A file at a revision, or an empty string when the file does not exist there.
fn git_show(repo: &Path, rev: &str, path: &str) -> Result<String> {
    let spec = format!("{rev}:{path}");
    let exists = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e", &spec])
        .status()
        .with_context(|| format!("running git cat-file -e {spec}"))?;
    if !exists.success() {
        return Ok(String::new());
    }
    git(repo, &["show", &spec])
}

/// Whether any file of the tree at `rev` contains `needle`.
fn tree_mentions(repo: &Path, rev: &str, needle: &str) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["grep", "-q", "-F", "-I", "-e", needle, rev])
        .status()
        .with_context(|| format!("running git grep for {needle}"))?;
    // git grep exits 1 when nothing matches, and 2 or more on an error.
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git grep for {needle} failed"),
    }
}

#[cfg(test)]
mod tests;
