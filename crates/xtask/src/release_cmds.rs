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
mod tests {
    use super::*;
    use crate::changelog::{parse_fragment, Kind};

    fn fragment(kind: &str, prose: &str) -> String {
        format!("---\nkind: {kind}\n---\n{prose}\n")
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xtask-changelog-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("changelog.d")).expect("scratch dir");
        dir
    }

    fn write_fragment(root: &Path, id: &str, kind: &str, prose: &str) {
        std::fs::write(
            root.join("changelog.d").join(format!("{id}.md")),
            fragment(kind, prose),
        )
        .expect("write fragment");
    }

    #[test]
    fn a_release_writes_the_record_the_notes_and_the_flag() {
        let root = scratch("release");
        write_fragment(
            &root,
            "389",
            "feature",
            "Paste a screenshot into a console.",
        );
        write_fragment(
            &root,
            "390",
            "fix",
            "The keyboard stops covering the prompt.",
        );

        changelog_cmd(&[
            "--release".into(),
            "v0.1.0-rc.20".into(),
            "--date".into(),
            "2026-09-08".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("fold");

        let changelog = std::fs::read_to_string(root.join("CHANGELOG.md")).expect("changelog");
        assert!(changelog.contains("## v0.1.0-rc.20 — 2026-09-08"));
        assert!(changelog.contains("- Paste a screenshot into a console. (#389)"));

        let notes = std::fs::read_to_string(root.join("target/changelog/notes.md")).expect("notes");
        assert!(notes.contains("### New"));
        assert!(
            !notes.contains("## v0.1.0-rc.20"),
            "the tag is the release title; the body must not repeat it: {notes}"
        );

        let announce =
            std::fs::read_to_string(root.join("target/changelog/announce")).expect("announce");
        assert_eq!(announce.trim(), "yes");

        assert!(
            !root.join("changelog.d/389.md").exists(),
            "a folded fragment is consumed, or it lands in the next release too"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_release_keeps_the_first_one() {
        let root = scratch("history");
        write_fragment(&root, "1", "fix", "Older.");
        changelog_cmd(&[
            "--release".into(),
            "v0.1.0-rc.20".into(),
            "--date".into(),
            "2026-09-08".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("first fold");

        write_fragment(&root, "2", "fix", "Newer.");
        changelog_cmd(&[
            "--release".into(),
            "v0.1.0-rc.21".into(),
            "--date".into(),
            "2026-09-09".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("second fold");

        let changelog = std::fs::read_to_string(root.join("CHANGELOG.md")).expect("changelog");
        assert!(changelog.contains("Older."), "history must accumulate");
        assert!(changelog.contains("Newer."));
        assert!(
            changelog.find("rc.21").expect("rc.21") < changelog.find("rc.20").expect("rc.20"),
            "newest first"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn notes_reads_a_past_release_without_touching_the_pending_fragments() {
        let root = scratch("notes");
        write_fragment(&root, "1", "feature", "A feature.");
        changelog_cmd(&[
            "--release".into(),
            "v0.1.0-rc.20".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("fold");

        // A fragment for the release after this one is already on the branch.
        write_fragment(&root, "2", "fix", "A later fix.");

        let out = root.join("ci-out");
        changelog_cmd(&[
            "--notes".into(),
            "v0.1.0-rc.20".into(),
            "--root".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ])
        .expect("notes");

        let notes = std::fs::read_to_string(out.join("notes.md")).expect("notes");
        assert!(notes.contains("- A feature. (#1)"));
        assert!(
            !notes.contains("A later fix."),
            "notes are the record, not the working tree: {notes}"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("announce"))
                .expect("announce")
                .trim(),
            "yes"
        );
        assert!(
            root.join("changelog.d/2.md").exists(),
            "--notes must never consume a fragment"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn notes_meet_a_v_prefixed_tag_and_an_unprefixed_fold() {
        let root = scratch("prefix");
        write_fragment(&root, "1", "fix", "A fix.");
        changelog_cmd(&[
            "--release".into(),
            "0.1.0-rc.20".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("fold without the v");

        let out = root.join("ci-out");
        changelog_cmd(&[
            "--notes".into(),
            "v0.1.0-rc.20".into(),
            "--root".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ])
        .expect("notes with the v");
        assert!(std::fs::read_to_string(out.join("notes.md"))
            .expect("notes")
            .contains("A fix."));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unrecorded_version_publishes_a_pointer_rather_than_failing() {
        let root = scratch("unrecorded");
        let out = root.join("ci-out");
        changelog_cmd(&[
            "--notes".into(),
            "v9.9.9".into(),
            "--root".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ])
        .expect("a release already built must still publish");

        let notes = std::fs::read_to_string(out.join("notes.md")).expect("notes");
        assert!(notes.contains("No changelog entry was recorded"), "{notes}");
        assert_eq!(
            std::fs::read_to_string(out.join("announce"))
                .expect("announce")
                .trim(),
            "no",
            "an unrecorded release is never announced"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refolding_a_version_is_refused_because_its_fragments_are_gone() {
        // The destructive path: the first fold consumed the fragments, so a
        // second run for the same version folds an empty set over the record and
        // takes the entries with it. A retried CI step reaches this.
        let root = scratch("refold");
        write_fragment(&root, "1", "feature", "A real feature.");
        let release = |root: &Path| {
            vec![
                "--release".to_string(),
                "v9.9.9".to_string(),
                "--root".to_string(),
                root.display().to_string(),
            ]
        };
        changelog_cmd(&release(&root)).expect("first fold");

        let err = changelog_cmd(&release(&root)).expect_err("a second fold must refuse");
        assert!(err.to_string().contains("already in the record"), "{err}");

        let history = load_history(&root.join("changelog.json")).expect("history");
        assert_eq!(
            history.releases[0].entries.len(),
            1,
            "the record survives the refusal"
        );
        assert!(history.releases[0].announce);

        // --force is the deliberate override, and it does replace.
        let mut forced = release(&root);
        forced.push("--force".to_string());
        changelog_cmd(&forced).expect("--force is allowed to replace");
        let history = load_history(&root.join("changelog.json")).expect("history");
        assert!(history.releases[0].entries.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bump_refuses_a_version_cargo_would_reject() {
        let root = scratch("bump-refuse");
        std::fs::create_dir_all(root.join("crates/demo")).expect("crate dir");
        let manifest = root.join("crates/demo/Cargo.toml");
        let before = "[package]
name = \"demo\"
version = \"0.1.0-rc19\"
";
        std::fs::write(&manifest, before).expect("manifest");

        let err = bump_cmd(&[
            "not-a-version".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect_err("garbage must be refused");
        assert!(
            err.to_string().contains("not a version Cargo will accept"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&manifest).expect("read"),
            before,
            "nothing is written until the version is known good"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bump_accepts_the_tag_spelling_and_writes_the_manifest_one() {
        // The tag is `v0.1.0-rc.20`; a manifest version is not. Writing the tag
        // verbatim produced a workspace Cargo cannot parse.
        let root = scratch("bump-tag");
        std::fs::create_dir_all(root.join("crates/demo")).expect("crate dir");
        let manifest = root.join("crates/demo/Cargo.toml");
        std::fs::write(
            &manifest,
            "[package]
name = \"demo\"
version = \"0.1.0-rc19\"
",
        )
        .expect("manifest");

        bump_cmd(&[
            "v0.1.0-rc.20".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("the v spelling is accepted");
        let text = std::fs::read_to_string(&manifest).expect("read");
        assert!(text.contains("version = \"0.1.0-rc.20\""), "{text}");
        assert!(
            !text.contains("\"v0.1.0"),
            "the v must not reach the manifest: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pointer_body_is_exactly_what_gets_published() {
        // Asserting a prefix let a malformed body — an 18-space run from a
        // broken line continuation — ship as the release note.
        let root = scratch("pointer");
        let out = root.join("out");
        changelog_cmd(&[
            "--notes".into(),
            "v9.9.9-absent".into(),
            "--root".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ])
        .expect("an unrecorded version still publishes");
        assert_eq!(
            std::fs::read_to_string(out.join("notes.md")).expect("notes"),
            concat!(
                "No changelog entry was recorded for this release. See the ",
                "[changelog](https://github.com/paulocorcino/ralphy/blob/main/CHANGELOG.md).\n"
            )
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fix_only_release_is_not_announceable() {
        let root = scratch("quiet");
        write_fragment(&root, "1", "fix", "A fix.");
        changelog_cmd(&[
            "--release".into(),
            "v1".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("fold");
        let announce =
            std::fs::read_to_string(root.join("target/changelog/announce")).expect("announce");
        assert_eq!(announce.trim(), "no");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn check_refuses_a_malformed_fragment_and_writes_nothing() {
        let root = scratch("check");
        std::fs::write(root.join("changelog.d/1.md"), "no front matter\n").expect("write");
        let err = changelog_cmd(&[
            "--check".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect_err("a malformed fragment must red the gate");
        assert!(
            err.chain().any(|c| c.to_string().contains("front-matter")),
            "{err:?}"
        );
        assert!(!root.join("CHANGELOG.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pending_reads_the_fragments_without_consuming_them() {
        let root = scratch("pending");
        write_fragment(&root, "7", "feature", "A feature.");
        let entries = read_fragments(&root.join("changelog.d")).expect("read");
        let report = pending_report(&entries);
        assert!(report.contains("- A feature. (#7)"), "{report}");
        assert!(report.contains("announce: true"), "{report}");
        assert!(root.join("changelog.d/7.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_release_with_no_fragments_still_cuts() {
        let root = scratch("empty");
        changelog_cmd(&[
            "--release".into(),
            "v1".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("an internal-only release is still a release");
        let notes = std::fs::read_to_string(root.join("target/changelog/notes.md")).expect("notes");
        assert!(notes.contains("No user-visible changes"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_package_version_moves_and_a_dependency_pin_does_not() {
        let manifest = "[package]\nname = \"ralphy-cli\"\nversion = \"0.1.0-rc19\"\n\n\
                        [dependencies]\nureq = { version = \"2\" }\n";
        let moved = replace_package_version(manifest, "0.1.0-rc.20").expect("a package version");
        assert!(moved.contains("version = \"0.1.0-rc.20\""));
        assert!(
            moved.contains("ureq = { version = \"2\" }"),
            "a dependency pin is not a package version: {moved}"
        );
    }

    #[test]
    fn a_manifest_with_no_version_line_is_left_alone() {
        assert_eq!(
            replace_package_version("[package]\nname = \"x\"\n", "9"),
            None
        );
    }

    #[test]
    fn bump_moves_the_crates_and_skips_the_unpublished_tool() {
        let root = scratch("bump");
        for (name, extra) in [("ralphy-core", ""), ("xtask", "publish = false\n")] {
            let dir = root.join("crates").join(name);
            std::fs::create_dir_all(&dir).expect("crate dir");
            std::fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0-rc19\"\n{extra}"),
            )
            .expect("manifest");
        }

        bump_cmd(&[
            "0.1.0-rc.20".into(),
            "--root".into(),
            root.display().to_string(),
        ])
        .expect("bump");

        let core =
            std::fs::read_to_string(root.join("crates/ralphy-core/Cargo.toml")).expect("core");
        assert!(core.contains("version = \"0.1.0-rc.20\""));
        let tool = std::fs::read_to_string(root.join("crates/xtask/Cargo.toml")).expect("xtask");
        assert!(
            tool.contains("version = \"0.1.0-rc19\""),
            "the out-of-band tool does not ride the release train: {tool}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_kind_is_never_silently_widened() {
        // The closed set is the contract the gate and the workbench both read.
        for kind in ["breaking", "security", "feature", "fix", "internal"] {
            assert!(
                parse_fragment("1", &fragment(kind, "Text.")).is_ok(),
                "{kind}"
            );
        }
        assert!(parse_fragment("1", &fragment("docs", "Text.")).is_err());
        assert_ne!(Kind::Feature, Kind::Fix);
    }
}
