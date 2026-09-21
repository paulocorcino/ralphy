//! The two commands that cut a release: `changelog` folds the fragments,
//! `bump` moves every crate's version in step (ADR-0056 §2).
//!
//! Both write only files they own wholesale. `changelog.d/` is the human's;
//! `CHANGELOG.md`, `changelog.json` and the crate manifests' version lines are
//! this tool's.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::changelog::{fold, load_history, read_fragments, render_changelog, render_notes, Entry};

/// `<workspace root>`, located from this crate's compile-time manifest dir
/// (`<root>/crates/xtask`).
pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask crate lives at <root>/crates/xtask")
}

pub fn changelog_cmd(args: &[String]) -> Result<()> {
    let mut check = false;
    let mut force = false;
    let mut pending = false;
    let mut notes_only: Option<String> = None;
    let mut version: Option<String> = None;
    let mut date: Option<String> = None;
    let mut root = workspace_root().to_path_buf();
    let mut out: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--check" => check = true,
            "--force" => force = true,
            "--pending" => pending = true,
            "--notes" => notes_only = Some(crate::next_value(&mut it, "--notes")?),
            "--release" => version = Some(crate::next_value(&mut it, "--release")?),
            "--date" => date = Some(crate::next_value(&mut it, "--date")?),
            "--root" => root = PathBuf::from(crate::next_value(&mut it, "--root")?),
            "--out" => out = Some(PathBuf::from(crate::next_value(&mut it, "--out")?)),
            other => bail!("unknown flag {other}"),
        }
    }

    // `--notes` reads the record a past fold wrote; it must not touch, parse or
    // consume the fragments a later release will carry.
    if let Some(wanted) = notes_only {
        let out = out.unwrap_or_else(|| root.join("target/changelog"));
        return write_notes(&root, &wanted, &out);
    }

    let fragments_dir = root.join("changelog.d");
    let entries = read_fragments(&fragments_dir)?;

    if check {
        report_fragments(&entries);
        return Ok(());
    }
    if pending {
        print!("{}", pending_report(&entries));
        return Ok(());
    }

    let Some(version) = version else {
        bail!("nothing to do: pass --check, --pending or --release <version>");
    };
    let date = match date {
        Some(d) => d,
        None => today(),
    };
    let out = out.unwrap_or_else(|| root.join("target/changelog"));

    let history_path = root.join("changelog.json");
    let mut history = load_history(&history_path)?;
    // Folding a version twice is not idempotent, it is destructive: the first
    // fold consumed the fragments, so the second one folds an empty set over a
    // good record and the entries are gone with their sources. A retried CI
    // step or a corrected --date is enough to reach it.
    if !force && history.releases.iter().any(|r| r.version == version) {
        bail!(
            "{version} is already in the record and its fragments were consumed by that fold; \
             re-run with --force only to replace what it recorded"
        );
    }
    fold(&mut history, &version, &date, entries);
    let record = history
        .releases
        .first()
        .cloned()
        .expect("fold just inserted this release");

    let mut json = serde_json::to_string_pretty(&history).context("serializing the history")?;
    json.push('\n');
    atomic_write(&history_path, json.as_bytes())?;

    let changelog_path = root.join("CHANGELOG.md");
    atomic_write(&changelog_path, render_changelog(&history).as_bytes())?;

    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    std::fs::write(out.join("notes.md"), render_notes(&record))
        .with_context(|| format!("writing the notes into {}", out.display()))?;
    // A plain file rather than stdout: the release workflow reads it with `cat`,
    // and the runner has no JSON tooling guaranteed on every platform.
    std::fs::write(
        out.join("announce"),
        if record.announce { "yes\n" } else { "no\n" },
    )
    .with_context(|| format!("writing the announce flag into {}", out.display()))?;

    // The fragments are consumed: their content now lives in the history, and
    // leaving them would fold them into the next release as well.
    remove_fragments(&fragments_dir)?;

    println!(
        "{version} folded: {} user-visible {}, announce={}",
        record.entries.len(),
        if record.entries.len() == 1 {
            "entry"
        } else {
            "entries"
        },
        record.announce
    );
    println!("  {}", changelog_path.display());
    println!("  {}", history_path.display());
    println!("  {}", out.join("notes.md").display());
    Ok(())
}

/// Render the release body for a version already in the record — what the
/// release workflow publishes. The rendering lives here and only here, so the
/// workflow never re-implements it against the markdown.
fn write_notes(root: &Path, wanted: &str, out: &Path) -> Result<()> {
    let history = load_history(&root.join("changelog.json"))?;
    // The maintainer folds under the tag; accept the bare version too, so a
    // `v`-prefixed tag and an unprefixed fold still meet.
    let bare = wanted.strip_prefix('v').unwrap_or(wanted);
    let record = history
        .releases
        .iter()
        .find(|r| r.version == wanted || r.version.strip_prefix('v').unwrap_or(&r.version) == bare);

    let (body, announce) = match record {
        Some(record) => (render_notes(record), record.announce),
        None => {
            // Not a reason to fail a release that is already built: publish an
            // honest body and say so in the log.
            eprintln!("warning: no changelog record for {wanted} — publishing a pointer instead");
            // Absolute, not relative: this renders on a GitHub release page,
            // where `../CHANGELOG.md` resolves to nothing.
            (
                concat!(
                    "No changelog entry was recorded for this release. See the ",
                    "[changelog](https://github.com/paulocorcino/ralphy/blob/main/CHANGELOG.md).\n"
                )
                .to_string(),
                false,
            )
        }
    };

    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    std::fs::write(out.join("notes.md"), body)
        .with_context(|| format!("writing the notes into {}", out.display()))?;
    std::fs::write(
        out.join("announce"),
        if announce { "yes\n" } else { "no\n" },
    )
    .with_context(|| format!("writing the announce flag into {}", out.display()))?;
    println!(
        "{wanted}: notes written to {}, announce={announce}",
        out.display()
    );
    Ok(())
}

fn report_fragments(entries: &[Entry]) {
    if entries.is_empty() {
        println!("no fragments pending");
        return;
    }
    println!(
        "{} fragment{} parse",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" }
    );
    for entry in entries {
        let numeric = !entry.id.is_empty() && entry.id.chars().all(|c| c.is_ascii_digit());
        let label = if numeric {
            format!("#{}", entry.id)
        } else {
            entry.id.clone()
        };
        println!("  {label:<16} {:?}", entry.kind);
    }
}

fn pending_report(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "no fragments pending\n".to_string();
    }
    let mut history = crate::changelog::History::default();
    fold(&mut history, "(pending)", "(unreleased)", entries.to_vec());
    let record = &history.releases[0];
    let mut out = render_notes(record);
    out.push_str(&format!("\nannounce: {}\n", record.announce));
    out
}

/// Write via temp file + atomic rename.
///
/// `changelog.json` is the durable record every past release lives in, and the
/// fragments that fed it were deleted by the folds that wrote it. A truncating
/// write interrupted by a crash or a full disk would lose all of it — so it gets
/// the same treatment `ralphy-release` already gives a cache it could always
/// refetch (ADR-0056 §6).
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(
        "{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e).with_context(|| format!("replacing {}", path.display()))
        }
    }
}

fn remove_fragments(dir: &Path) -> Result<()> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let is_fragment = path.extension().is_some_and(|e| e == "md")
            && path.file_stem().is_some_and(|s| s != "README");
        if is_fragment {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(())
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// `bump <version>` — move every workspace crate's version line in step.
///
/// Fifteen manifests carry a literal version today and no script moved them; at
/// a release every few days that is a recurring drift risk (ADR-0056 §2).
pub fn bump_cmd(args: &[String]) -> Result<()> {
    let mut version: Option<String> = None;
    let mut root = workspace_root().to_path_buf();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(crate::next_value(&mut it, "--root")?),
            other if other.starts_with("--") => bail!("unknown flag {other}"),
            other => version = Some(other.to_string()),
        }
    }
    let version = version.context("usage: bump <version>, e.g. bump 0.1.0-rc.20")?;
    // A tag is `v0.1.0-rc.20`; a manifest version is not. Writing the tag
    // spelling verbatim into sixteen manifests produces a workspace Cargo
    // cannot parse, and the `v` is the spelling ADR-0056 uses everywhere.
    let version = version.trim().trim_start_matches('v').to_string();
    semver::Version::parse(&version)
        .with_context(|| format!("`{version}` is not a version Cargo will accept"))?;

    let mut moved = Vec::new();
    let crates_dir = root.join("crates");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&crates_dir)
        .with_context(|| format!("reading {}", crates_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    // Staged, not written as we go: a failure halfway through used to leave the
    // workspace half-bumped with no way back.
    let mut staged: Vec<(PathBuf, String)> = Vec::new();
    for dir in dirs {
        let manifest = dir.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        // `publish = false` marks the out-of-band tooling, which is pinned at
        // 0.0.0 on purpose and does not ride the release train.
        if text.contains("publish = false") {
            continue;
        }
        let Some(replaced) = replace_package_version(&text, &version) else {
            continue;
        };
        if replaced != text {
            staged.push((manifest, replaced));
            moved.push(
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string(),
            );
        }
    }
    for (manifest, text) in staged {
        std::fs::write(&manifest, text)
            .with_context(|| format!("writing {}", manifest.display()))?;
    }

    if moved.is_empty() {
        println!("nothing to bump: every manifest already reads {version}");
    } else {
        println!(
            "bumped {} crates to {version}: {}",
            moved.len(),
            moved.join(", ")
        );
        println!("run `cargo check --workspace` to move Cargo.lock with them");
    }
    Ok(())
}

/// Replace the `[package]` version line — the first bare `version = "..."` in
/// the file. A dependency's inline version sits inside a table entry and is
/// never at the start of a line, so it cannot be hit by accident.
fn replace_package_version(text: &str, version: &str) -> Option<String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let idx = lines.iter().position(|l| l.starts_with("version = \""))?;
    lines[idx] = format!("version = \"{version}\"");
    let mut out = lines.join(newline);
    if text.ends_with('\n') {
        out.push_str(newline);
    }
    Some(out)
}

#[cfg(test)]
mod tests;
