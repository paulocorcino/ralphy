//! The jscpd exceptions (`.jscpd.json`): each start marker in the Rust and
//! TypeScript sources says why its two copies are two rules, and the number
//! of them only goes down.

use super::*;

/// The start markers under `crates/`. Lower it in the same change that
/// removes one: the count is exact, so it cannot grow back.
const JSCPD_MARKERS: usize = 23;

/// The marker word, split so this file does not hold a marker of its own.
const MARKER: &str = concat!("jscpd:", "ignore-start");

/// The Rust sources and the workbench modules, the places a marker may be.
fn sources() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut files = Vec::new();
    skipped_tests::rust_sources(&root.join("crates"), &mut files);
    files.extend(ts_modules(&root.join(UI_DIR)));
    files
}

/// A comment line with words, other than a marker: the reason of a marker.
fn is_reason(line: Option<&str>) -> bool {
    line.and_then(|l| l.trim().strip_prefix("//"))
        .is_some_and(|rest| !rest.trim().is_empty() && !rest.contains("jscpd:"))
}

/// `(file:line, has a reason)` of each marker. The reason is a comment on the
/// line above or the line below, or text after the marker on its own line.
fn markers() -> Vec<(String, bool)> {
    let marker = Regex::new(&format!(r"^\s*//\s*{MARKER}(.*)$")).expect("a fixed pattern");
    let mut found = Vec::new();
    for file in sources() {
        let text = read(&file);
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some(caps) = marker.captures(line) else {
                continue;
            };
            let inline = caps.get(1).is_some_and(|m| m.as_str().trim().len() > 1);
            let above = i.checked_sub(1).and_then(|k| lines.get(k)).copied();
            let reason = inline || is_reason(above) || is_reason(lines.get(i + 1).copied());
            found.push((format!("{}:{}", file.display(), i + 1), reason));
        }
    }
    found
}

#[test]
fn jscpd_markers_do_not_grow() {
    let found = markers();
    let bare: Vec<&String> = found
        .iter()
        .filter(|(_, why)| !why)
        .map(|(at, _)| at)
        .collect();
    assert!(
        bare.is_empty(),
        "a jscpd marker says why next to it, found: {bare:?}"
    );
    assert_eq!(
        found.len(),
        JSCPD_MARKERS,
        "the jscpd exceptions changed; a new one is the last resort, and a \
         removed one lowers JSCPD_MARKERS: {found:?}"
    );
}
