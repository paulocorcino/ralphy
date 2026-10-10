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
//!
//! The workbench coupling ratchets (#623) read the modules through the same
//! lexer: the uses of `WBConsole` in `wb-notes.ts`, the uses of the shell's
//! `_flashAction` in the `wb-*.ts` modules, and the one owner of each column
//! paint function.
//!
//! The peer reply pin is not a count. It ties the page types that read a
//! reply a peer daemon wrote to the peer protocol version (#653), through the
//! same lexer, so a comment or a layout change does not move it.

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
/// `wb-move-dialog.ts` (#621); 4243 since the workbench events became one
/// typed list sent through `wb-events.ts` (#633); 4227 since the shell's types
/// moved to a `.d.ts` file and `FleetPeer` to `wb-fleet.ts` (#613); 4221 since
/// the shell's `WB.emit` moved to `wb-events.ts` (#651); 4211 since its daemon
/// replies are read through `wb-api.ts` (#653); 4212 since it imports the
/// check of a detached file window's message from `wb-detached.ts` (#653);
/// 4209 since a file tab passes its bytes or its refusal to the viewer as one
/// value (#653). Its shell-only types are in `wb-shell-types.d.ts` and it
/// imports them on one line (#653).
const APP_TS_LINES: usize = 4209;
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
/// detach moved to `wb-console-detach.ts` (#621); 3077 since the paste key's
/// work after its clipboard read moved to `wb-console-input.ts` (#652); 3081
/// since each type it reads from another module is imported on an
/// `import type` line that names the owner module, not written inline (#653);
/// 3083 since it reads its daemon replies through `wb-api.ts` (#653); 3086
/// since its two peer buttons read their label once, typed by the peer's
/// action (#653).
const WB_CONSOLE_TS_LINES: usize = 3086;
/// Lines of `crates/ralphy-daemon/assets/ui/wb-notes.ts`, the note cards
/// (ADR-0073 D8, started by its 2026-10-10 amendment). 2828 at 823cc84c, before
/// its first cut (#623).
const WB_NOTES_TS_LINES: usize = 2828;

/// Uses of the name `WBConsole` in the code of `wb-notes.ts`: the cards reach
/// the console through `window.WBConsole`, with no import and no type that
/// lists what they read. 49 at 823cc84c (#623). The target is 0: the cards
/// take the console as a typed dep (ADR-0073, the 2026-10-10 amendment).
const NOTES_CONSOLE_REACH: usize = 49;

/// `(module, count)` of every use of `_flashAction`, a private member of
/// `shell()`, in a `wb-*.ts` module: a call, or a name in a component's
/// `uses` list. `app.ts` owns it and is not read. Exact, as `ANY_BASELINE`
/// is. 19 in 6 modules at 823cc84c (#623). The target is an empty table:
/// every operator message goes through one door (ADR-0073, the 2026-10-10
/// amendment).
const SHELL_FLASH_REACH: &[(&str, usize)] = &[
    ("wb-consoles-tab.ts", 4),
    ("wb-daemon.ts", 1),
    ("wb-files.ts", 6),
    ("wb-hosts-dialog.ts", 2),
    ("wb-settings-dialog.ts", 3),
    ("wb-viewer.ts", 3),
];

/// The functions that paint the columns on the stage. Each is declared in one
/// module only: `wb-console.ts` at 823cc84c (#623).
const COLUMN_PAINT: [&str; 6] = [
    "applyColumns",
    "columnMeasure",
    "clearColumn",
    "focusColumn",
    "focusedId",
    "columnRoster",
];

/// The served workbench modules, from the repo root. `vendor/` and
/// `ui-tests/` are not read.
const UI_DIR: &str = "crates/ralphy-daemon/assets/ui";

/// `(module, count)` of every explicit `any` under `UI_DIR`, the count oxlint's
/// `typescript/no-explicit-any` reports: 2122 in 50 modules on e75f2394, and
/// none since #613. The table is empty and stays empty: every module has zero.
const ANY_BASELINE: &[(&str, usize)] = &[];

/// `(module, count)` of every cast written over another cast (`x as A as B`)
/// under `UI_DIR`: each is a new element typed as a console window or a note
/// card (#653). Exact, as `ANY_BASELINE` is.
const CHAINED_CAST_BASELINE: &[(&str, usize)] = &[("wb-console-chrome.ts", 1), ("wb-notes.ts", 1)];

/// `(module under UI_DIR, type)` of each page type that reads a reply a peer
/// daemon wrote. The local daemon passes these replies on as the peer wrote
/// them: the one-reply verbs of a peer repo and the registry verbs that name
/// a peer (`relay_to_peer` in `routes/ws_command.rs`), the frames of a spawn
/// verb, the `/ws/session` frames, and `GET /api/agents?repo=`. The page
/// reads only the status of `POST /api/sessions/close`, so no type reads its
/// body. `/api/sessions`, `/api/fleet`, `/api/usage` and the tree pushes are
/// written again by the local daemon, so they are not here. A type that a
/// listed type names is listed too, or is in `NOT_PEER_REPLIES`.
const PEER_REPLY_TYPES: &[(&str, &str)] = &[
    ("globals.d.ts", "DaemonReplies"),
    ("globals.d.ts", "DaemonReply"),
    ("globals.d.ts", "FileReadOk"),
    ("globals.d.ts", "FileReadReply"),
    ("globals.d.ts", "JsonValue"),
    ("globals.d.ts", "ReplyOf"),
    ("globals.d.ts", "SpawnStatus"),
    ("globals.d.ts", "TreeEntry"),
    ("globals.d.ts", "WriteReply"),
    ("wb-api.ts", "AgentRow"),
    ("wb-changes.ts", "ChangeRow"),
    ("wb-changes.ts", "SyncBody"),
    ("wb-console-terminal.ts", "FramePayload"),
    ("wb-console-terminal.ts", "SessionFrame"),
    ("wb-kanban.ts", "BoardLabel"),
    ("wb-kanban.ts", "BoardRow"),
    ("wb-kanban.ts", "IssueComment"),
    ("wb-kanban.ts", "IssueDetail"),
    ("wb-project.ts", "Listing"),
    ("wb-project.ts", "Worktree"),
    ("wb-runs.ts", "RunSleep"),
    ("wb-runs.ts", "RunSnapshot"),
    // `FramePayload` reads it through the type of `announcement`'s parameter.
    ("wb-session-route.ts", "SessionOpen"),
];

/// `(module, type, why)` of each type a listed type names that no peer writes.
const NOT_PEER_REPLIES: &[(&str, &str, &str)] = &[
    (
        "wb-hosts.ts",
        "Alias",
        "a host verb is never relayed to a peer",
    ),
    (
        "wb-hosts.ts",
        "KeyReply",
        "a host verb is never relayed to a peer",
    ),
];

/// `(PEER_PROTOCOL_VERSION, hash of PEER_REPLY_TYPES)`. Version 4 pays for
/// the fields that version 3 got with no raise: `file.read`'s `encoding` and
/// `bom`, and the `char_index` of a refused write (#653).
const PEER_REPLY_PIN: (u32, u64) = (4, 0xfdaa_c9b2_6860_c15a);

/// Where `PEER_PROTOCOL_VERSION` is, from the repo root.
const PEER_RS: &str = "crates/ralphy-daemon/src/peer.rs";

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

/// ADR-0073 D8: the note cards never grow back. A change that adds a line to
/// `wb-notes.ts` fails here, and a change that removes lines lowers the
/// constant.
#[test]
fn the_notes_script_matches_the_line_baseline() {
    let path = workspace_root().join("crates/ralphy-daemon/assets/ui/wb-notes.ts");
    let lines = read(&path).lines().count();
    assert!(
        lines == WB_NOTES_TS_LINES,
        "lines of crates/ralphy-daemon/assets/ui/wb-notes.ts: {WB_NOTES_TS_LINES} -> {lines}; \
         a lower count lowers WB_NOTES_TS_LINES in the same change, and new code goes \
         into a module of its own instead"
    );
}

/// #623: the note cards do not reach the console through `window` more than
/// they do today.
#[test]
fn the_notes_reach_the_console_as_measured() {
    let path = workspace_root().join("crates/ralphy-daemon/assets/ui/wb-notes.ts");
    let uses = name_uses(&read(&path), "WBConsole");
    assert!(
        uses == NOTES_CONSOLE_REACH,
        "uses of WBConsole in crates/ralphy-daemon/assets/ui/wb-notes.ts: \
         {NOTES_CONSOLE_REACH} -> {uses}; the cards take what they need from the console \
         as a typed dep, and a lower count lowers NOTES_CONSOLE_REACH in the same change"
    );
}

/// #623: no module calls the shell's private flash more than it does today.
#[test]
fn shell_flash_reach_matches_the_baseline() {
    let ui = workspace_root().join(UI_DIR);
    let mut actual = BTreeMap::new();
    for path in ts_modules(&ui) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("wb-") {
            continue;
        }
        let n = name_uses(&read(&path), "_flashAction");
        if n > 0 {
            actual.insert(name.to_string(), n);
        }
    }
    let errors = flash_errors(&actual, SHELL_FLASH_REACH);
    assert!(
        errors.is_empty(),
        "uses of the shell's `_flashAction` in {UI_DIR}/wb-*.ts: tell the operator through \
         the one door for operator messages, or, when a change removed some, lower \
         SHELL_FLASH_REACH in crates/xtask/tests/ratchets.rs:\n{}",
        errors.join("\n")
    );
}

/// #623: each function that paints the columns has one owner module.
#[test]
fn the_column_paint_has_one_owner() {
    let ui = workspace_root().join(UI_DIR);
    let mut owners: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for path in ts_modules(&ui) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let text = read(&path);
        for function in COLUMN_PAINT {
            if declared_functions(&text, function) > 0 {
                owners.entry(function).or_default().push(name.to_string());
            }
        }
    }
    let errors = column_owner_errors(&owners, &COLUMN_PAINT);
    assert!(
        errors.is_empty(),
        "each column paint function is declared (`function name(`) in exactly one module \
         under {UI_DIR}:\n{}",
        errors.join("\n")
    );
}

/// The coupling counts read code, not comments, and their baselines are
/// exact (#623).
#[test]
fn coupling_counts_read_code_and_are_exact() {
    let src = "// WBConsole in a comment\n\
               window.WBConsole.toast({ text: `${window.WBConsole.NOTE_MAX}` });\n\
               const uses = [\"_flashAction\", \"WBConsole\"]; const WBConsoleX = 1;\n";
    assert_eq!(name_uses(src, "WBConsole"), 3);
    assert_eq!(name_uses(src, "_flashAction"), 1);

    let baseline = [("a.ts", 2)];
    let one = |file: &str, n| BTreeMap::from([(file.to_string(), n)]);
    assert!(flash_errors(&one("a.ts", 2), &baseline).is_empty());
    assert_eq!(flash_errors(&one("a.ts", 1), &baseline).len(), 1);
    assert_eq!(flash_errors(&one("a.ts", 3), &baseline).len(), 1);
    assert_eq!(flash_errors(&one("b.ts", 1), &baseline).len(), 2);

    let decls = "function focusedId() {}\n\
                 // function focusedId() {}\n\
                 const x = focusedId(); const clearColumn = () => 1;\n\
                 async function applyColumns(a) {}\n";
    assert_eq!(declared_functions(decls, "focusedId"), 1);
    assert_eq!(declared_functions(decls, "clearColumn"), 0);
    assert_eq!(declared_functions(decls, "applyColumns"), 1);

    let names = ["a", "b", "c"];
    let owners = BTreeMap::from([
        ("a", vec!["x.ts".to_string()]),
        ("b", vec!["x.ts".to_string(), "y.ts".to_string()]),
    ]);
    let errors = column_owner_errors(&owners, &names);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors.iter().any(|e| e.starts_with("b: ")), "{errors:?}");
    assert!(errors.iter().any(|e| e.starts_with("c: ")), "{errors:?}");
}

/// #613: no module has an explicit `any`. oxlint (`typescript/no-explicit-any`)
/// reports the same `any`s per file, for every module.
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

/// #613: the ways around a type check stay at zero in the workbench, and the
/// chained casts stay at `CHAINED_CAST_BASELINE` (#653).
#[test]
fn no_escape_hatch_in_the_workbench() {
    let ui = workspace_root().join(UI_DIR);
    let mut errors = Vec::new();
    let mut chained = BTreeMap::new();
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
        let n = chained_casts(&text);
        if n > 0 {
            chained.insert(name.to_string(), n);
        }
    }
    errors.extend(chained_cast_errors(&chained, CHAINED_CAST_BASELINE));
    assert!(
        errors.is_empty(),
        "the workbench has no `as unknown as`, no `@ts-ignore`, `@ts-expect-error` \
         or `@ts-nocheck`, and no chained cast (`x as A as B`) past \
         CHAINED_CAST_BASELINE; type the value instead:\n{}",
        errors.join("\n")
    );
}

/// #653: a peer of the same protocol version sends the same reply shapes,
/// because a change to a type that reads one fails here until the change
/// shows one of two choices: a raise of `PEER_PROTOCOL_VERSION` (the default),
/// or new fields declared optional with the same version.
#[test]
fn peer_reply_types_are_pinned_to_the_protocol_version() {
    let root = workspace_root();
    let ui = root.join(UI_DIR);
    let mut sources = BTreeMap::new();
    for path in ts_modules(&ui) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        sources.insert(name, read(&path));
    }
    let (hash, mut errors) = peer_reply_hash(&sources, PEER_REPLY_TYPES, NOT_PEER_REPLIES);
    let peer_rs = read(&root.join(PEER_RS));
    let version = Regex::new(r"pub const PEER_PROTOCOL_VERSION: u32 = (\d+);")
        .expect("a valid regex")
        .captures(&peer_rs)
        .and_then(|c| c[1].parse::<u32>().ok());
    match version {
        None => errors.push(format!(
            "{PEER_RS} declares no `pub const PEER_PROTOCOL_VERSION: u32`"
        )),
        Some(version) if errors.is_empty() => {
            errors.extend(peer_pin_error(version, hash, PEER_REPLY_PIN));
        }
        Some(_) => {}
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn peer_reply_hash_skips_layout_and_sees_a_field() {
    let pin = |src: &str| {
        let sources = BTreeMap::from([("a.ts".to_string(), src.to_string())]);
        peer_reply_hash(&sources, &[("a.ts", "R")], &[])
    };
    let (base, errors) = pin("export type R = { a: string; b?: number; };\n");
    assert_eq!(errors, Vec::<String>::new());
    let (laid_out, _) =
        pin("/** R */\nexport type R = {\n  a: string; // the a\n  b?: number;\n};\n");
    assert_eq!(laid_out, base);
    let (changed, _) = pin("export type R = { a: string; b: number; };\n");
    assert_ne!(changed, base);

    let (_, errors) = pin("type S = { a: string };\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("a.ts declares no type R"),
        "{errors:?}"
    );
    let (_, errors) = pin("type R = { s: S };\ntype S = { a: string };\n");
    assert!(
        errors.len() == 1 && errors[0].starts_with("a.ts R names a.ts S"),
        "{errors:?}"
    );

    assert_eq!(peer_pin_error(4, 7, (4, 7)), None);
    for (version, hash) in [(4, 8), (5, 7), (5, 8)] {
        assert!(
            peer_pin_error(version, hash, (4, 7)).is_some(),
            "{version} {hash}"
        );
    }
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

/// The chained cast count reads code, not comments or strings, and its
/// baseline is exact (#653).
#[test]
fn chained_casts_count_a_cast_over_a_cast() {
    assert_eq!(
        chained_casts(
            "const a = b as HTMLElement as Win;\n\
             const c = d as M.T<string>[] as U; // e as A as B\n\
             const f = g as T; const h = i as T, j = k as V;\n\
             const s = \"l as A as B\";\n"
        ),
        2
    );
    let baseline = [("a.ts", 1)];
    let one = |file: &str, n| BTreeMap::from([(file.to_string(), n)]);
    assert!(chained_cast_errors(&one("a.ts", 1), &baseline).is_empty());
    assert_eq!(chained_cast_errors(&one("a.ts", 2), &baseline).len(), 1);
    assert_eq!(chained_cast_errors(&one("b.ts", 1), &baseline).len(), 2);
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

/// The casts in `src` written over another cast: `as` a type, then `as`
/// again. The type is a dotted name with optional type arguments and `[]`
/// suffixes, which is every cast type the workbench writes. Comments and
/// strings are not read; the code inside a template hole is.
fn chained_casts(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    let mut count = 0;
    for (i, t) in toks.iter().enumerate() {
        if t.ident() == Some("as") {
            let end = cast_type_end(&toks, i + 1);
            if end > i + 1 && toks.get(end).and_then(|t| t.ident()) == Some("as") {
                count += 1;
            }
        }
        if let lex::Tok::Tpl(pieces) = &t.tok {
            for piece in pieces {
                if let lex::Piece::Expr(code) = piece {
                    count += chained_casts(code);
                }
            }
        }
    }
    count
}

/// The index after the cast type that starts at `start`, or `start` when no
/// name starts there.
fn cast_type_end(toks: &[lex::Token], start: usize) -> usize {
    let name = |k: usize| toks.get(k).and_then(|t| t.ident()).is_some();
    let punct = |k: usize, p: &str| toks.get(k).is_some_and(|t| t.is(p));
    if !name(start) {
        return start;
    }
    let mut j = start + 1;
    loop {
        let member = punct(j, ".") && name(j + 1);
        let array = punct(j, "[") && punct(j + 1, "]");
        if member || array {
            j += 2;
        } else if punct(j, "<") {
            let mut depth = 0usize;
            while let Some(t) = toks.get(j) {
                if t.is("<") {
                    depth += 1;
                } else if t.is(">") {
                    depth = depth.saturating_sub(1);
                }
                j += 1;
                if depth == 0 {
                    break;
                }
            }
        } else {
            return j;
        }
    }
}

/// One line per workbench module whose chained cast count is not the
/// baseline's.
fn chained_cast_errors(
    actual: &BTreeMap<String, usize>,
    baseline: &[(&str, usize)],
) -> Vec<String> {
    let baseline = baseline
        .iter()
        .map(|(file, n)| (file.to_string(), *n))
        .collect();
    ratchet_errors(
        actual,
        &baseline,
        |file, n| {
            format!(
                "{file}: {n} chained cast(s) in a module that has none in CHAINED_CAST_BASELINE"
            )
        },
        |file, expected, found| {
            format!(
                "{file}: chained cast count changed {expected} -> {found}; \
                 if it went down, lower CHAINED_CAST_BASELINE in this change"
            )
        },
    )
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

/// Uses of `name` in the code of `src`: an identifier or a string whose text
/// is `name`, the code inside a template hole included. The lexer drops
/// comments, and a longer name (`WBConsoleX`) is another identifier.
fn name_uses(src: &str, name: &str) -> usize {
    let mut count = 0;
    for t in lex::lex(src, 1) {
        match &t.tok {
            lex::Tok::Ident(text) | lex::Tok::Str(text) if text == name => count += 1,
            lex::Tok::Tpl(pieces) => {
                for piece in pieces {
                    if let lex::Piece::Expr(code) = piece {
                        count += name_uses(code, name);
                    }
                }
            }
            _ => {}
        }
    }
    count
}

/// The `function name(` declarations in the code of `src`. An arrow function
/// bound to the name is not read: the column paint declares no such one.
fn declared_functions(src: &str, name: &str) -> usize {
    let toks = lex::lex(src, 1);
    toks.windows(3)
        .filter(|w| w[0].ident() == Some("function") && w[1].ident() == Some(name) && w[2].is("("))
        .count()
}

/// One line per module whose `_flashAction` count is not the baseline's.
fn flash_errors(actual: &BTreeMap<String, usize>, baseline: &[(&str, usize)]) -> Vec<String> {
    let baseline = baseline
        .iter()
        .map(|(file, n)| (file.to_string(), *n))
        .collect();
    ratchet_errors(
        actual,
        &baseline,
        |file, n| {
            format!("{file}: {n} use(s) of `_flashAction` in a module that has none in SHELL_FLASH_REACH")
        },
        |file, expected, found| {
            format!(
                "{file}: `_flashAction` count changed {expected} -> {found}; \
                 if it went down, lower SHELL_FLASH_REACH in this change"
            )
        },
    )
}

/// One line per name that is not declared in exactly one module.
fn column_owner_errors(owners: &BTreeMap<&str, Vec<String>>, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter_map(|name| {
            let modules = owners.get(name).map(Vec::as_slice).unwrap_or(&[]);
            (modules.len() != 1).then(|| {
                format!(
                    "{name}: declared in {} module(s) {modules:?}",
                    modules.len()
                )
            })
        })
        .collect()
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

/// The FNV-1a hash of the listed declarations, each as its tokens with no
/// comment and one space between tokens, and the errors: a listed type that
/// is not declared, and a type a listed type names that is not listed.
/// FNV-1a is written here because std's `DefaultHasher` may change between
/// Rust releases, and the pin must not.
fn peer_reply_hash(
    sources: &BTreeMap<String, String>,
    listed: &[(&str, &str)],
    excluded: &[(&str, &str, &str)],
) -> (u64, Vec<String>) {
    let lexed: BTreeMap<&str, (Vec<char>, Vec<lex::Token>)> = sources
        .iter()
        .map(|(name, src)| (name.as_str(), (src.chars().collect(), lex::lex(src, 1))))
        .collect();
    let known: BTreeSet<(&str, &str)> = listed
        .iter()
        .copied()
        .chain(excluded.iter().map(|(file, name, _)| (*file, *name)))
        .collect();
    let mut errors = Vec::new();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut sorted = listed.to_vec();
    sorted.sort_unstable();
    for (file, name) in sorted {
        let Some((cs, toks)) = lexed.get(file) else {
            errors.push(format!("{file} declares no type {name}: there is no {file} in {UI_DIR}; update PEER_REPLY_TYPES"));
            continue;
        };
        let Some(range) = type_declaration(toks, name) else {
            errors.push(format!(
                "{file} declares no type {name}: a type in PEER_REPLY_TYPES was renamed or moved; update the list"
            ));
            continue;
        };
        let decl = &toks[range];
        let text: Vec<String> = decl
            .iter()
            .map(|t| cs[t.start..t.end].iter().collect())
            .collect();
        for byte in format!("{file} {}\n", text.join(" ")).bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        for (ref_file, ref_name) in named_types(decl, file, toks, &lexed) {
            if ref_name != name && !known.contains(&(ref_file.as_str(), ref_name.as_str())) {
                errors.push(format!(
                    "{file} {name} names {ref_file} {ref_name}, which is not in PEER_REPLY_TYPES: \
                     add it, or add it to NOT_PEER_REPLIES with why no peer writes it"
                ));
            }
        }
    }
    (hash, errors)
}

/// The tokens of `type name … ;` or `interface name … { … }`, from the
/// keyword: `export` and `declare` are not part of the shape.
fn type_declaration(toks: &[lex::Token], name: &str) -> Option<std::ops::Range<usize>> {
    let starts_statement = |k: usize| {
        k == 0
            || toks[k - 1].is(";")
            || toks[k - 1].is("}")
            || matches!(toks[k - 1].ident(), Some("export" | "declare"))
    };
    let start = (0..toks.len().saturating_sub(1)).find(|&k| {
        matches!(toks[k].ident(), Some("type" | "interface"))
            && toks[k + 1].ident() == Some(name)
            && starts_statement(k)
    })?;
    let interface = toks[start].ident() == Some("interface");
    let mut depth = 0usize;
    for (k, t) in toks.iter().enumerate().skip(start) {
        if t.is("(") || t.is("[") || t.is("{") {
            depth += 1;
        } else if t.is(")") || t.is("]") || t.is("}") {
            depth = depth.saturating_sub(1);
            if interface && depth == 0 && t.is("}") {
                return Some(start..k + 1);
            }
        } else if !interface && depth == 0 && t.is(";") {
            return Some(start..k + 1);
        }
    }
    None
}

/// `(module, type)` of each type that `decl` names and that a module declares:
/// `import("./m.ts").T`, a type of the same module, a type the module imports
/// with `import type { … }`, or a type of `globals.d.ts`.
fn named_types(
    decl: &[lex::Token],
    file: &str,
    toks: &[lex::Token],
    lexed: &BTreeMap<&str, (Vec<char>, Vec<lex::Token>)>,
) -> BTreeSet<(String, String)> {
    let declares = |module: &str, name: &str| {
        lexed
            .get(module)
            .is_some_and(|(_, t)| type_declaration(t, name).is_some())
    };
    let imports = type_imports(toks);
    let mut found = BTreeSet::new();
    for (k, t) in decl.iter().enumerate() {
        let Some(name) = t.ident() else { continue };
        if k > 0 && decl[k - 1].is(".") {
            continue;
        }
        if name == "import" && decl.get(k + 1).is_some_and(|t| t.is("(")) {
            if let (Some(lex::Tok::Str(path)), Some(dot), Some(member)) = (
                decl.get(k + 2).map(|t| &t.tok),
                decl.get(k + 4),
                decl.get(k + 5).and_then(|t| t.ident()),
            ) {
                if dot.is(".") {
                    let module = path.trim_start_matches("./");
                    found.insert((module.to_string(), member.to_string()));
                }
            }
            continue;
        }
        if declares(file, name) {
            found.insert((file.to_string(), name.to_string()));
        } else if let Some((module, original)) = imports.get(name) {
            found.insert((module.clone(), original.clone()));
        } else if declares("globals.d.ts", name) {
            found.insert(("globals.d.ts".to_string(), name.to_string()));
        }
    }
    found
}

/// `local name -> (module, exported name)` of each `import type { … } from "./m.ts"`.
fn type_imports(toks: &[lex::Token]) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    for k in 0..toks.len() {
        if toks[k].ident() != Some("import")
            || toks.get(k + 1).and_then(|t| t.ident()) != Some("type")
            || !toks.get(k + 2).is_some_and(|t| t.is("{"))
        {
            continue;
        }
        let Some(close) = (k + 3..toks.len()).find(|&j| toks[j].is("}")) else {
            continue;
        };
        let Some(lex::Tok::Str(path)) = toks.get(close + 2).map(|t| &t.tok) else {
            continue;
        };
        let module = path.trim_start_matches("./");
        let names: Vec<&str> = toks[k + 3..close]
            .iter()
            .filter_map(|t| t.ident())
            .collect();
        let mut i = 0;
        while i < names.len() {
            let (local, original) = if names.get(i + 1) == Some(&"as") {
                (names.get(i + 2).copied().unwrap_or(names[i]), names[i])
            } else {
                (names[i], names[i])
            };
            out.insert(
                local.to_string(),
                (module.to_string(), original.to_string()),
            );
            i += if names.get(i + 1) == Some(&"as") {
                3
            } else {
                1
            };
        }
    }
    out
}

/// The failure, if any, of `version` and `hash` against `pin`.
fn peer_pin_error(version: u32, hash: u64, pin: (u32, u64)) -> Option<String> {
    let (pinned_version, pinned_hash) = pin;
    if (version, hash) == pin {
        return None;
    }
    if version == pinned_version {
        return Some(format!(
            "The shape of a reply that a peer daemon can answer changed: the types in \
             PEER_REPLY_TYPES hash to {hash:#018x}, and PEER_REPLY_PIN has {pinned_hash:#018x} \
             for peer protocol {version}. A peer of the same protocol version must send the \
             same shapes. Choose one:\n\
             - the default: raise PEER_PROTOCOL_VERSION in {PEER_RS} to {next}, and set \
             PEER_REPLY_PIN to ({next}, {hash:#018x});\n\
             - the exception, only when each change is a new field: declare each new field \
             optional in the page type, set PEER_REPLY_PIN to ({version}, {hash:#018x}), and \
             say in the commit why the page is correct when an older peer does not send it.",
            next = version + 1
        ));
    }
    if hash == pinned_hash {
        return Some(format!(
            "PEER_PROTOCOL_VERSION is {version}, and PEER_REPLY_PIN names version \
             {pinned_version}. The reply types did not change. Set PEER_REPLY_PIN to \
             ({version}, {hash:#018x}), so the pin names the version these types belong to."
        ));
    }
    Some(format!(
        "PEER_PROTOCOL_VERSION is {version} and the reply types changed: set PEER_REPLY_PIN \
         to ({version}, {hash:#018x})."
    ))
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
