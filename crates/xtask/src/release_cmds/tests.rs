use super::*;
use crate::changelog::{parse_fragment, Kind};

fn fragment(kind: &str, prose: &str) -> String {
    format!("---\nkind: {kind}\n---\n{prose}\n")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xtask-changelog-{}-{tag}", std::process::id()));
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
    assert!(notes.contains("### ✨ New"), "{notes}");
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

    let core = std::fs::read_to_string(root.join("crates/ralphy-core/Cargo.toml")).expect("core");
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
