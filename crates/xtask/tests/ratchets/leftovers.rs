//! A `TODO`, `FIXME`, `XXX` or `HACK` comment is work left in the code. Rust
//! has none (clippy's `todo` sees only the macro); the workbench is held by
//! oxlint's `no-warning-comments`. The work goes in an issue instead.

use super::*;

#[test]
fn no_rust_comment_leaves_work_behind() {
    let marker = Regex::new(r"//[/!]?\s*(TODO|FIXME|XXX|HACK)\b").expect("a fixed pattern");
    let mut files = Vec::new();
    skipped_tests::rust_sources(&workspace_root().join("crates"), &mut files);
    let found: Vec<String> = files
        .iter()
        .flat_map(|file| {
            read(file)
                .lines()
                .enumerate()
                .filter(|(_, line)| marker.is_match(line))
                .map(|(i, _)| format!("{}:{}", file.display(), i + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        found.is_empty(),
        "open an issue for the work and remove the comment: {found:?}"
    );
}
