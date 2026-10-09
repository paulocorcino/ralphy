//! Ratchets over coupling that exists today and must not grow (ADR-0068).
//! Each baseline is exact: a count that goes down lowers its baseline in the
//! same change, so the ratchet never leaves room to grow back.
//!
//! The spawn ratchet sees only a literal program name:
//! `Command::new("git")`. A spawn through a variable (`find_program("ssh")`,
//! `Command::new(&program)`) is not seen and is reviewed in the PR.
//!
//! The forge ratchet reads `github::` paths as text. It does not see an alias
//! (`github as gh`), a root re-export (`ralphy_core::GhTracker`) or a name
//! after a nested group (`github::{a::{b}, c}`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use regex::Regex;

/// Distinct `github::<item>` names used by `crates/ralphy-cli/src`. 26 since
/// ADR-0017 A2: triage `--yes` reads each comment's trust (`IssueThread`,
/// `render_triage_threads`).
const FORGE_ITEMS: usize = 26;
/// Lines that tell the agent to run `gh issue view`, under `assets/prompts/`.
const PROMPT_GH_ISSUE_VIEW: usize = 20;

/// Lines of `crates/ralphy-daemon/assets/ui/app.ts`, the `shell()` script that
/// ADR-0073 cuts into components (D8). Each cut lowers it in the same change.
/// 5512 since the consoles group became the components of
/// `wb-consoles-tab.ts` (#621).
const APP_TS_LINES: usize = 5512;
/// Lines of `crates/ralphy-daemon/assets/ui/wb-console.ts`, the console
/// factory (ADR-0073 D8, which starts this ratchet with the first fold move).
/// 5842 since ADR-0075 phase 5 moved its pure folds, its GPU budget and its
/// terminal into their own modules (#597); 5396 since its window chrome and
/// the owner of its gestures moved to `wb-console-chrome.ts` (#609); 4780
/// since its popup registry moved to `wb-console-popups.ts` and its fences to
/// `wb-console-fences.ts` (#610); 4599 since its desk moved to
/// `wb-console-desk.ts` (#611); 4057 since its title, console names and
/// worktree switcher moved to `wb-console-title.ts` (#621).
const WB_CONSOLE_TS_LINES: usize = 4057;

const SPAWNED: [&str; 3] = ["git", "gh", "ssh"];

/// `(file, program, sites)` of every literal git/gh/ssh spawn in production
/// code, measured on 1b50e775 and lowered when `ralphy-git-read` took the
/// daemon's and the usage scan's git reads (#510).
const SPAWN_BASELINE: &[(&str, &str, usize)] = &[
    ("crates/ralphy-cli/build/git_version.rs", "git", 2),
    ("crates/ralphy-cli/src/init/gate.rs", "gh", 1),
    ("crates/ralphy-core/src/git.rs", "git", 1),
    ("crates/ralphy-core/src/github/client.rs", "gh", 1),
    ("crates/ralphy-git-read/src/lib.rs", "git", 1),
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

    let declared = "#[cfg(test)]\n\
                    mod tests;\n\
                    fn f() { Command::new(\"ssh\"); Command::new(\"ssh\"); }\n\
                    #[cfg(test)]\n\
                    pub(crate) mod more {\n\
                    fn t() { Command::new(\"git\"); }\n\
                    }\n";
    assert_eq!(
        spawn_sites(declared),
        BTreeMap::from([("ssh", 2)]),
        "a `mod x;` declaration is not the cut, and `pub(crate) mod x {{` is"
    );
}

#[test]
fn a_new_file_or_a_changed_count_fails_the_ratchet() {
    let baseline = [("a.rs", "git", 1)];
    let key = |file: &str, program: &str| (file.to_string(), program.to_string());

    let grown = BTreeMap::from([
        (key("a.rs", "git"), 2),
        (key("a.rs", "gh"), 1),
        (key("b.rs", "gh"), 1),
    ]);
    let errors = ratchet_errors(&grown, &baseline);
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("b.rs")), "{errors:#?}");
    assert!(
        errors
            .iter()
            .any(|e| e.starts_with("a.rs: new spawn site for gh")),
        "{errors:#?}"
    );
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

#[test]
fn forge_use_matches_the_baseline() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rs(&root.join("crates/ralphy-cli/src"), &mut files);
    let mut items = BTreeSet::new();
    for path in &files {
        items.extend(forge_items(&read(path)));
    }

    let mut prompts = Vec::new();
    collect_all(&root.join("assets/prompts"), &mut prompts);
    let issue_views: usize = prompts
        .iter()
        .map(|path| {
            read(path)
                .lines()
                .filter(|line| line.contains("gh issue view"))
                .count()
        })
        .sum();

    let mut errors = Vec::new();
    if items.len() != FORGE_ITEMS {
        errors.push(format!(
            "github:: items used by ralphy-cli: {FORGE_ITEMS} -> {}: {items:?}",
            items.len()
        ));
    }
    if issue_views != PROMPT_GH_ISSUE_VIEW {
        errors.push(format!(
            "`gh issue view` lines under assets/prompts: {PROMPT_GH_ISSUE_VIEW} -> {issue_views}"
        ));
    }
    assert!(
        errors.is_empty(),
        "the forge does not spread (docs/ARCHITECTURE.md §6); \
         a lower count lowers the constant in the same change:\n{}",
        errors.join("\n")
    );
}

/// ADR-0073 D8: the workbench script never grows back. A change that adds a
/// line to `app.ts` fails here, and a change that removes lines lowers the
/// constant.
#[test]
fn the_workbench_script_matches_the_line_baseline() {
    let path = workspace_root().join("crates/ralphy-daemon/assets/ui/app.ts");
    let lines = read(&path).lines().count();
    assert!(
        lines == APP_TS_LINES,
        "lines of crates/ralphy-daemon/assets/ui/app.ts: {APP_TS_LINES} -> {lines}; \
         a lower count lowers APP_TS_LINES in the same change, and new code goes \
         into a component file instead"
    );
}

/// ADR-0073 D8: the console factory never grows back. A change that adds a
/// line to `wb-console.ts` fails here, and a change that removes lines lowers
/// the constant.
#[test]
fn the_console_script_matches_the_line_baseline() {
    let path = workspace_root().join("crates/ralphy-daemon/assets/ui/wb-console.ts");
    let lines = read(&path).lines().count();
    assert!(
        lines == WB_CONSOLE_TS_LINES,
        "lines of crates/ralphy-daemon/assets/ui/wb-console.ts: {WB_CONSOLE_TS_LINES} -> {lines}; \
         a lower count lowers WB_CONSOLE_TS_LINES in the same change, and new code goes \
         into a module of its own instead"
    );
}

#[test]
fn forge_items_count_brace_groups_and_skip_tests() {
    let src = "use ralphy_core::{github, git};\n\
               // github::commented()\n\
               fn f() { github::a(); github::a(); }\n\
               use ralphy_core::github::{b, c as d};\n\
               #[cfg(test)]\n\
               mod tests {\n\
               fn t() { github::e(); }\n\
               }\n";
    let items = forge_items(src);
    assert_eq!(
        items,
        BTreeSet::from(["a".to_string(), "b".to_string(), "c".to_string()])
    );
    assert_eq!(
        forge_items("use ralphy_core::github::*;\n"),
        BTreeSet::from(["*".to_string()])
    );
}

/// The `github::` item names used in the production part of `text`, line
/// comments removed: `github::x`, each name of `github::{x, y as z}`, and
/// `*` for a glob import.
fn forge_items(text: &str) -> BTreeSet<String> {
    let plain = Regex::new(r"\bgithub::([A-Za-z_][A-Za-z0-9_]*)")
        .expect("the pattern is a valid regex literal");
    let group = Regex::new(r"\bgithub::\{([^}]*)\}").expect("the pattern is a valid regex literal");
    let first_ident =
        Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)").expect("the pattern is a valid regex literal");
    let code: String = production(text)
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let mut items: BTreeSet<String> = plain
        .captures_iter(&code)
        .map(|c| c[1].to_string())
        .collect();
    for c in group.captures_iter(&code) {
        for part in c[1].split(',') {
            if let Some(name) = first_ident.captures(part) {
                items.insert(name[1].to_string());
            }
        }
    }
    if code.contains("github::*") {
        items.insert("*".to_string());
    }
    items
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
        if line.trim() == "#[cfg(test)]" && lines.peek().is_some_and(|next| opens_inline_mod(next))
        {
            return &text[..offset];
        }
        offset += line.len();
    }
    text
}

/// `mod x {` or `pub(crate) mod x {`. A `mod x;` declaration is not a cut:
/// the production code after it is still read.
fn opens_inline_mod(line: &str) -> bool {
    let head = line.trim();
    let head = head.strip_prefix("pub(crate) ").unwrap_or(head);
    head.starts_with("mod ") && head.ends_with('{')
}

/// Every file under `dir`, recursively.
fn collect_all(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            collect_all(&path, out);
        } else {
            out.push(path);
        }
    }
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
