//! `ralphy-core` depends on no vendor (ADR-0002): its manifest names no
//! `ralphy-agent-*` crate and not `ralphy-adapter-support`.

use super::*;

/// The `ralphy-agent-*` and `ralphy-adapter-support` crates the manifest in
/// `text` names, with the line, in any section: dependencies, dev-dependencies,
/// build-dependencies and their `target` forms. A comment is not read.
fn vendor_crates(text: &str) -> Vec<String> {
    let vendor =
        Regex::new(r"^\s*(ralphy-agent-[\w-]+|ralphy-adapter-support)\b").expect("a fixed pattern");
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let name = vendor.captures(line)?.get(1)?.as_str();
            Some(format!("line {}: {name}", i + 1))
        })
        .collect()
}

#[test]
fn core_depends_on_no_vendor() {
    let manifest = workspace_root().join("crates/ralphy-core/Cargo.toml");
    let text = read(&manifest);
    assert!(
        text.contains("[dependencies]"),
        "expected the manifest of ralphy-core at {}",
        manifest.display()
    );
    let found = vendor_crates(&text);
    assert!(
        found.is_empty(),
        "ralphy-core must depend on no vendor crate: put what it needs behind the agent \
         contract instead:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_vendor_scan_sees_every_dependency_section() {
    let text = "[dependencies]\nralphy-agent-claude.workspace = true\n\
                [dev-dependencies]\nralphy-adapter-support = { path = \"x\" }\n\
                [build-dependencies]\nralphy-agent-codex = \"1\"\n\
                # ralphy-agent-kimi is a comment\nralphy-git-read.workspace = true\n";
    assert_eq!(vendor_crates(text).len(), 3);
}
