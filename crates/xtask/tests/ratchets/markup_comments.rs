//! The workbench markup holds bindings only (#941): no HTML comment, apart
//! from a part marker alone on its line. There is no baseline.

use super::*;

#[test]
fn workbench_markup_has_no_comments() {
    let ui = workspace_root().join(UI_DIR);
    let entries = std::fs::read_dir(&ui)
        .map_err(|e| format!("reading {}: {e}", ui.display()))
        .expect("this test needs the call to succeed");
    let mut checked = 0;
    let mut errors = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("reading an entry of {}: {e}", ui.display()))
            .expect("this test needs the call to succeed")
            .path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "html") {
            continue;
        }
        checked += 1;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_default();
        for line in markup::comments(&read(&path)) {
            errors.push(format!("{UI_DIR}/{name}:{line}"));
        }
    }
    assert!(
        checked > 10,
        "expected the markup files under {UI_DIR}, found {checked}"
    );
    assert!(
        errors.is_empty(),
        "the markup holds comments; delete each one, a part marker alone aside:\n{}",
        errors.join("\n")
    );
}
