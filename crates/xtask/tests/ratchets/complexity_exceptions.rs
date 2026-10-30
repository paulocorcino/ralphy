//! The complexity exceptions (ADR-0081): a Rust function with
//! `// complexity-exception: <reason>` right above it, which
//! `xtask complexity --check` lets pass, and a workbench line that turns
//! oxlint's `complexity` rule off.

use super::*;

/// The exceptions under `crates/`. Exact, as the other baselines are: a
/// change that removes one lowers it, so the number can only go down.
const COMPLEXITY_EXCEPTIONS: usize = 0;

#[test]
fn complexity_exceptions_do_not_grow() {
    let rust = Regex::new(r"^\s*// complexity-exception:\s*\S").expect("a valid literal regex");
    let script = Regex::new(r"(?:oxlint|eslint)-disable[\w-]*\s.*\bcomplexity\b")
        .expect("a valid literal regex");
    let mut files = Vec::new();
    source_files(&workspace_root().join("crates"), &mut files);
    let mut found = Vec::new();
    for path in files {
        let marker = match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => &rust,
            Some("ts" | "js" | "mjs") => &script,
            _ => continue,
        };
        for (i, line) in read(&path).lines().enumerate() {
            if marker.is_match(line) {
                found.push(format!("{}:{}", path.display(), i + 1));
            }
        }
    }
    assert_eq!(
        found.len(),
        COMPLEXITY_EXCEPTIONS,
        "complexity exceptions under crates/: split the function instead of adding one; \
         when one was removed, lower COMPLEXITY_EXCEPTIONS in \
         crates/xtask/tests/ratchets/complexity_exceptions.rs:\n{}",
        found.join("\n")
    );
}

/// The lint config without its whole-line `//` comments, parsed.
fn oxlint_config() -> serde_json::Value {
    let text = read(&workspace_root().join(".oxlintrc.json"));
    let json: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("parsing .oxlintrc.json: {e}"))
}

/// The TypeScript limit lives only in `.oxlintrc.json`: the same 10 as the
/// Rust limit, with test code out of the measure (ADR-0081 D3).
#[test]
fn the_typescript_limit_is_ten() {
    let config = oxlint_config();
    let want = serde_json::json!(["error", { "max": 10, "variant": "modified" }]);
    assert_eq!(
        config["rules"]["complexity"], want,
        "the limit in .oxlintrc.json"
    );
    let off: Vec<&serde_json::Value> = config["overrides"]
        .as_array()
        .expect("overrides is a list")
        .iter()
        .filter(|o| o["rules"].get("complexity").is_some())
        .collect();
    assert_eq!(off.len(), 1, "one override sets the complexity rule");
    assert_eq!(off[0]["files"], serde_json::json!(["**/ui-tests/**"]));
    assert_eq!(off[0]["rules"]["complexity"], "off");
}

/// Every file under `dir`, but not the vendored libraries or build output.
fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let skipped = path
            .file_name()
            .is_some_and(|n| n == "vendor" || n == "node_modules" || n == "target");
        if path.is_dir() && !skipped {
            source_files(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}
