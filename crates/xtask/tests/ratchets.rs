//! Ratchets over coupling that exists today and must not grow (ADR-0068).
//! Each baseline is exact: a count that goes down lowers its baseline in the
//! same change, so the ratchet never leaves room to grow back.
//!
//! The spawn ratchet sees only a literal program name:
//! `Command::new("git")`. A spawn through a variable (`find_program("ssh")`,
//! `Command::new(&program)`) is not seen and is reviewed in the PR.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SPAWNED: [&str; 3] = ["git", "gh", "ssh"];

/// `(file, program, sites)` of every literal git/gh/ssh spawn in production
/// code, measured on 1b50e775.
const SPAWN_BASELINE: &[(&str, &str, usize)] = &[
    ("crates/ralphy-cli/build.rs", "git", 2),
    ("crates/ralphy-cli/src/init/gate.rs", "gh", 1),
    ("crates/ralphy-core/src/git.rs", "git", 1),
    ("crates/ralphy-core/src/github/client.rs", "gh", 1),
    ("crates/ralphy-daemon/build.rs", "git", 2),
    ("crates/ralphy-daemon/src/registry.rs", "git", 1),
    ("crates/ralphy-usage-scan/src/claude.rs", "git", 1),
    ("crates/ralphy-usage-scan/src/codex.rs", "git", 1),
    ("crates/ralphy-usage-scan/src/copilot.rs", "git", 1),
    ("crates/ralphy-usage-scan/src/cursor.rs", "git", 1),
    ("crates/ralphy-usage-scan/src/gemini.rs", "git", 1),
    ("crates/ralphy-usage-scan/src/kimi.rs", "git", 1),
    ("crates/ralphy-usage-scan/src/opencode.rs", "git", 1),
    ("crates/xtask/src/capabilities.rs", "git", 3),
];

#[test]
fn spawn_sites_match_the_baseline() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rs(&root.join("crates"), &mut files);
    assert!(
        files.len() > 100,
        "expected the workspace's sources under {}, found {} .rs files",
        root.display(),
        files.len()
    );

    let mut actual = BTreeMap::new();
    for path in &files {
        let text = read(path);
        for (program, sites) in spawn_sites(&text) {
            actual.insert((rel_path(&root, path), program.to_string()), sites);
        }
    }
    let errors = ratchet_errors(&actual, SPAWN_BASELINE);
    assert!(
        errors.is_empty(),
        "git, gh and ssh are spawned only by their owners — see docs/ARCHITECTURE.md §6:\n{}",
        errors.join("\n")
    );
}

#[test]
fn spawn_counting_cuts_at_the_test_module() {
    let src = "fn f() { std::process::Command::new(\"git\"); }\n\
               #[cfg(test)]\n\
               mod tests {\n\
               fn t() { Command::new(\"gh\"); }\n\
               }\n";
    let sites = spawn_sites(src);
    assert_eq!(sites, BTreeMap::from([("git", 1)]));
}

#[test]
fn a_new_file_or_a_changed_count_fails_the_ratchet() {
    let baseline = [("a.rs", "git", 1)];
    let key = |file: &str, program: &str| (file.to_string(), program.to_string());

    let grown = BTreeMap::from([(key("a.rs", "git"), 2), (key("b.rs", "gh"), 1)]);
    let errors = ratchet_errors(&grown, &baseline);
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("b.rs")), "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("1 -> 2")), "{errors:#?}");

    let same = BTreeMap::from([(key("a.rs", "git"), 1)]);
    assert!(ratchet_errors(&same, &baseline).is_empty());

    let gone = BTreeMap::new();
    let errors = ratchet_errors(&gone, &baseline);
    assert!(
        errors.len() == 1 && errors[0].contains("1 -> 0"),
        "{errors:#?}"
    );
}

/// Literal `Command::new("<program>")` sites per spawned program, in the
/// production part of `text`.
fn spawn_sites(text: &str) -> BTreeMap<&'static str, usize> {
    let code = production(text);
    SPAWNED
        .iter()
        .map(|program| {
            (
                *program,
                code.matches(&format!("Command::new(\"{program}\")"))
                    .count(),
            )
        })
        .filter(|(_, sites)| *sites > 0)
        .collect()
}

/// One line per `(file, program)` whose count is not the baseline's.
fn ratchet_errors(
    actual: &BTreeMap<(String, String), usize>,
    baseline: &[(&str, &str, usize)],
) -> Vec<String> {
    let mut errors = Vec::new();
    for ((file, program), sites) in actual {
        if !baseline.iter().any(|(f, p, _)| f == file && p == program) {
            errors.push(format!(
                "{file}: new spawn site for {program} ({sites}); spawn through its owner instead"
            ));
        }
    }
    for (file, program, expected) in baseline {
        let found = actual
            .get(&(file.to_string(), program.to_string()))
            .copied()
            .unwrap_or(0);
        if found != *expected {
            errors.push(format!(
                "{file}: {program} count changed {expected} -> {found}; \
                 if it went down, lower the table in this change"
            ));
        }
    }
    errors
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `path` relative to `root`, with `/` separators on every platform.
fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path
        .strip_prefix(root)
        .unwrap_or_else(|e| panic!("{} is not under {}: {e}", path.display(), root.display()));
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The part of a file above its first inline test module, the same cut the
/// file-split rule uses (ADR-0022).
fn production(text: &str) -> &str {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|next| next.trim_start().starts_with("mod "))
        {
            return &text[..offset];
        }
        offset += line.len();
    }
    text
}

/// Production `.rs` files: no `tests/` dir, no `tests.rs`, no test child.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            let skipped =
                matches!(name, "target" | "node_modules" | "tests") || name.starts_with('.');
            if !skipped {
                collect_rs(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_test_child.rs") {
            out.push(path);
        }
    }
}
