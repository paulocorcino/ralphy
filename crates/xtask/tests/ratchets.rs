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
//!
//! The `any` ratchet reads the workbench modules through the `ui-copy` lexer,
//! so a comment or a string that says `any` is not counted. It counts what
//! oxlint's `typescript/no-explicit-any` reports, file by file (#613).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use regex::Regex;

// The `ui-copy` lexer, compiled here a third time: xtask has no `[lib]`, and
// one lexer means the ratchet and `ui-copy` drop comments and strings by the
// same rule. The ratchets read only part of its API.
#[allow(dead_code)]
#[path = "../src/ui_copy/lex.rs"]
mod lex;

/// Distinct `github::<item>` names used by `crates/ralphy-cli/src`. 26 since
/// ADR-0017 A2: triage `--yes` reads each comment's trust (`IssueThread`,
/// `render_triage_threads`).
const FORGE_ITEMS: usize = 26;
/// Lines that tell the agent to run `gh issue view`, under `assets/prompts/`.
const PROMPT_GH_ISSUE_VIEW: usize = 20;

/// Lines of `crates/ralphy-daemon/assets/ui/app.ts`, the `shell()` script that
/// ADR-0073 cuts into components (D8). Each cut lowers it in the same change.
/// 4246 since the files group became the components of `wb-files.ts` and
/// `wb-move-dialog.ts` (#621).
const APP_TS_LINES: usize = 4243;
/// Lines of `crates/ralphy-daemon/assets/ui/wb-console.ts`, the console
/// factory (ADR-0073 D8, which starts this ratchet with the first fold move).
/// 5842 since ADR-0075 phase 5 moved its pure folds, its GPU budget and its
/// terminal into their own modules (#597); 5396 since its window chrome and
/// the owner of its gestures moved to `wb-console-chrome.ts` (#609); 4780
/// since its popup registry moved to `wb-console-popups.ts` and its fences to
/// `wb-console-fences.ts` (#610); 4599 since its desk moved to
/// `wb-console-desk.ts` (#611); 4057 since its title, console names and
/// worktree switcher moved to `wb-console-title.ts` (#621); 3708 since its
/// view moved to `wb-console-view.ts` (#621); 3423 since its fence list moved
/// to `wb-console-fence-list.ts` (#621); 3084 since the opener's side of its
/// detach moved to `wb-console-detach.ts` (#621).
const WB_CONSOLE_TS_LINES: usize = 3081;

/// The served workbench modules, from the repo root. `vendor/` and
/// `ui-tests/` are not read.
const UI_DIR: &str = "crates/ralphy-daemon/assets/ui";

/// `(module, count)` of every explicit `any` under `UI_DIR`, the count oxlint's
/// `typescript/no-explicit-any` reports: 2122 in 50 modules on e75f2394 (#613).
/// The table is exact and only gets shorter: a module at zero leaves it (and
/// the `.oxlintrc.json` override) in the same change, and a new module is
/// not in it.
const ANY_BASELINE: &[(&str, usize)] = &[
    ("app.ts", 333),
    ("wb-console.ts", 191),
    ("wb-files.ts", 108),
    ("wb-notes.ts", 224),
    ("wb-projects-store.ts", 1),
    ("wb-viewer.ts", 144),
];

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
    let errors = spawn_errors(&actual, SPAWN_BASELINE);
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
    let errors = spawn_errors(&grown, &baseline);
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
    assert!(spawn_errors(&same, &baseline).is_empty());

    let gone = BTreeMap::new();
    let errors = spawn_errors(&gone, &baseline);
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

/// #613: no module gets a new explicit `any`, and a count that went down
/// stays down. oxlint (`typescript/no-explicit-any`) reports the same `any`s
/// per file; it guards only the modules outside its override.
#[test]
fn explicit_any_matches_the_baseline() {
    let actual = any_counts(&workspace_root().join(UI_DIR));
    let errors = any_errors(&actual, ANY_BASELINE);
    assert!(
        errors.is_empty(),
        "explicit `any` in {UI_DIR}: give the value its real type, or, when a change \
         removed some, lower ANY_BASELINE in crates/xtask/tests/ratchets.rs:\n{}",
        errors.join("\n")
    );
}

/// #613: the ways around a type check stay at zero in the workbench.
#[test]
fn no_escape_hatch_in_the_workbench() {
    let ui = workspace_root().join(UI_DIR);
    let mut errors = Vec::new();
    for path in ts_modules(&ui) {
        let text = read(&path);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let casts = double_casts(&text);
        if casts > 0 {
            errors.push(format!("{name}: {casts} `as unknown as`"));
        }
        let directives = ts_directives(&text);
        if directives > 0 {
            errors.push(format!("{name}: {directives} `@ts-` directive(s)"));
        }
    }
    assert!(
        errors.is_empty(),
        "the workbench has no `as unknown as` and no `@ts-ignore`, `@ts-expect-error` \
         or `@ts-nocheck`; type the value instead:\n{}",
        errors.join("\n")
    );
}

#[test]
fn explicit_any_counts_types_and_skips_names() {
    let src = "// any in a comment\n\
               const s = \"any\";\n\
               function f(a: any, b: Array<any>): any { return a as any; }\n\
               const t = `${xs.map((x: any) => x.any)}`;\n\
               const o = { any: 1, other: 2 };\n\
               type K = { any?: string };\n\
               type C<T> = T extends string ? any : never;\n";
    assert_eq!(explicit_any(src), 6);
    assert_eq!(
        double_casts("const x = y as unknown as Z; // as unknown as\n"),
        1
    );
    assert_eq!(
        ts_directives("// @ts-ignore\n/* @ts-expect-error */ // @ts-nocheck\n"),
        3
    );
}

#[test]
fn a_new_module_or_a_changed_any_count_fails_the_ratchet() {
    let baseline = [("a.ts", 2), ("b.ts", 1)];
    let actual = |pairs: &[(&str, usize)]| -> BTreeMap<String, usize> {
        pairs.iter().map(|(f, n)| (f.to_string(), *n)).collect()
    };

    assert!(any_errors(&actual(&[("a.ts", 2), ("b.ts", 1)]), &baseline).is_empty());

    let errors = any_errors(&actual(&[("a.ts", 3), ("b.ts", 1), ("c.ts", 1)]), &baseline);
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(
        errors
            .iter()
            .any(|e| e.starts_with("c.ts: 1 explicit `any`")),
        "{errors:#?}"
    );
    assert!(
        errors.iter().any(|e| e.contains("went up 2 -> 3")),
        "{errors:#?}"
    );

    let errors = any_errors(&actual(&[("a.ts", 1)]), &baseline);
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(
        errors
            .iter()
            .any(|e| e.contains("a.ts") && e.contains("went down 2 -> 1")),
        "{errors:#?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("b.ts") && e.contains("went down 1 -> 0")),
        "{errors:#?}"
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

/// The `.ts` modules directly under `ui`: `vendor/` is not read.
fn ts_modules(ui: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(ui).unwrap_or_else(|e| panic!("reading {}: {e}", ui.display()));
    let mut modules = Vec::new();
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", ui.display()))
            .path();
        let is_ts = path.extension().is_some_and(|ext| ext == "ts");
        if path.is_file() && is_ts {
            modules.push(path);
        }
    }
    assert!(
        modules.len() > 10,
        "expected the workbench modules under {}, found {}",
        ui.display(),
        modules.len()
    );
    modules
}

/// The explicit `any` count of each workbench module that has one.
fn any_counts(ui: &Path) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for path in ts_modules(ui) {
        let n = explicit_any(&read(&path));
        if n > 0 {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            counts.insert(name.to_string(), n);
        }
    }
    counts
}

/// The `as unknown as` casts in the code of `src`, template holes included.
fn double_casts(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    let mut count = 0;
    for (i, t) in toks.iter().enumerate() {
        let words: Vec<_> = toks[i..].iter().take(3).map(|t| t.ident()).collect();
        if words == [Some("as"), Some("unknown"), Some("as")] {
            count += 1;
        }
        if let lex::Tok::Tpl(pieces) = &t.tok {
            for piece in pieces {
                if let lex::Piece::Expr(code) = piece {
                    count += double_casts(code);
                }
            }
        }
    }
    count
}

/// The `@ts-ignore`, `@ts-expect-error` and `@ts-nocheck` directives in
/// `src`. A directive lives in a comment, which the lexer drops, so this
/// reads the text.
fn ts_directives(src: &str) -> usize {
    ["@ts-ignore", "@ts-expect-error", "@ts-nocheck"]
        .iter()
        .map(|directive| src.matches(directive).count())
        .sum()
}

/// The `any` type names in `src`, the ones oxlint's
/// `typescript/no-explicit-any` reports. The lexer drops comments and
/// strings; the code inside a template hole (`${xs.map((x: any) => x)}`) is
/// lexed again. A member (`x.any`) and a key (`{ any: 1 }`, `any?: 1`) are
/// names, not types; `T extends U ? any : never` is a type.
fn explicit_any(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    let mut count = 0;
    for (i, t) in toks.iter().enumerate() {
        match &t.tok {
            lex::Tok::Ident(name) if name == "any" => {
                let prev = i.checked_sub(1).map(|k| &toks[k]);
                let next = toks.get(i + 1);
                let member = prev.is_some_and(|p| p.is(".") || p.is("?."));
                let optional =
                    next.is_some_and(|n| n.is("?")) && toks.get(i + 2).is_some_and(|n| n.is(":"));
                let key = (next.is_some_and(|n| n.is(":")) || optional)
                    && !prev.is_some_and(|p| p.is("?"));
                if !member && !key {
                    count += 1;
                }
            }
            lex::Tok::Tpl(pieces) => {
                for piece in pieces {
                    if let lex::Piece::Expr(code) = piece {
                        count += explicit_any(code);
                    }
                }
            }
            _ => {}
        }
    }
    count
}

/// One line per `(file, program)` whose spawn count is not the baseline's.
fn spawn_errors(
    actual: &BTreeMap<(String, String), usize>,
    baseline: &[(&str, &str, usize)],
) -> Vec<String> {
    let baseline = baseline
        .iter()
        .map(|(file, program, sites)| ((file.to_string(), program.to_string()), *sites))
        .collect();
    ratchet_errors(
        actual,
        &baseline,
        |(file, program), sites| {
            format!(
                "{file}: new spawn site for {program} ({sites}); spawn through its owner instead"
            )
        },
        |(file, program), expected, found| {
            format!(
                "{file}: {program} count changed {expected} -> {found}; \
                 if it went down, lower the table in this change"
            )
        },
    )
}

/// One line per workbench module whose explicit `any` count is not the
/// baseline's.
fn any_errors(actual: &BTreeMap<String, usize>, baseline: &[(&str, usize)]) -> Vec<String> {
    let baseline = baseline
        .iter()
        .map(|(file, n)| (file.to_string(), *n))
        .collect();
    ratchet_errors(
        actual,
        &baseline,
        |file, n| {
            format!(
                "{file}: {n} explicit `any` in a module that has none in ANY_BASELINE; \
                 type the values (a new module starts at zero)"
            )
        },
        |file, expected, found| {
            if found > expected {
                format!(
                    "{file}: explicit `any` count went up {expected} -> {found}; type the value"
                )
            } else {
                format!(
                    "{file}: explicit `any` count went down {expected} -> {found}; \
                     lower ANY_BASELINE in this change (remove the entry at zero)"
                )
            }
        },
    )
}

/// One line per key whose count is not the baseline's: a key the baseline
/// does not have, or a count that moved in either direction. A baseline is
/// exact, so a count that went down cannot grow back later.
fn ratchet_errors<K: Ord>(
    actual: &BTreeMap<K, usize>,
    baseline: &BTreeMap<K, usize>,
    new: impl Fn(&K, usize) -> String,
    changed: impl Fn(&K, usize, usize) -> String,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (key, count) in actual {
        if !baseline.contains_key(key) {
            errors.push(new(key, *count));
        }
    }
    for (key, expected) in baseline {
        let found = actual.get(key).copied().unwrap_or(0);
        if found != *expected {
            errors.push(changed(key, *expected, found));
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
