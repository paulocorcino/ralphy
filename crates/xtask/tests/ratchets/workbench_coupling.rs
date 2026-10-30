//! The ratchets over how the workbench modules reach each other (#623).

use super::*;

/// Uses of the name `WBConsole` in the code of `wb-notes.ts`: 49 at 823cc84c,
/// when the cards reached the console through `window.WBConsole`. 0 since the
/// cards take the console as a typed dep, `deps.console` (#623, ADR-0073, the
/// 2026-10-10 amendment). The name never comes back.
const NOTES_CONSOLE_REACH: usize = 0;

/// The console members a note card may read: the names in the `CardHost`
/// union of `wb-types.d.ts`. Exact, like the other ratchets: a new name is a
/// new thing a card reads of the console, and it is a design decision (#623).
/// 18 since `toast` and `askConfirm` are the door's, `deps.messages` (#623).
/// 11 since `focusWin`, `stackWin` and `inGesture` are the document's z stack
/// and gestures, `deps.stack` and `deps.gestures`, and the drag folds and the
/// dormancy constants are imported from their own modules (#623).
/// 9 since `atNoteCap` and `NOTE_MAX` are the cap fold and the constant of
/// `wb-desk-folds.ts` (#623).
const CARD_HOST_NAMES: usize = 9;

/// `(module, count)` of every use of `_flashAction`, a private member of
/// `shell()`, in a `wb-*.ts` module: a call, or a name in a component's
/// `uses` list. `app.ts` owns it and is not read. Exact, as `ANY_BASELINE`
/// is. 19 in 6 modules at 823cc84c (#623). Empty since every operator
/// message goes through one door, `wb-messages.ts` (ADR-0073, the 2026-10-10
/// amendment); the table stays empty.
const SHELL_FLASH_REACH: &[(&str, usize)] = &[];

/// The functions that paint the columns on the stage. Each is declared in one
/// module only: `wb-stage-columns.ts` (#623; `wb-console.ts` until it moved).
const COLUMN_PAINT: [&str; 6] = [
    "applyColumns",
    "columnMeasure",
    "clearColumn",
    "focusColumn",
    "focusedId",
    "columnRoster",
];

/// #623: the note cards do not name `WBConsole`; they take the console as a
/// typed dep.
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

/// #623: the console members a card reads are the names of `CardHost`, and
/// that list does not grow without a decision.
#[test]
fn the_card_host_lists_the_decided_names() {
    let path = workspace_root().join(UI_DIR).join("wb-types.d.ts");
    let names = card_host_names(&read(&path));
    assert!(
        names == Some(CARD_HOST_NAMES),
        "names in CardHost of crates/ralphy-daemon/assets/ui/wb-types.d.ts: \
         {CARD_HOST_NAMES} -> {names:?}; a change to the list lowers or raises \
         CARD_HOST_NAMES in the same change, and a raise is a design decision"
    );
}

#[test]
fn card_host_names_count_the_members_and_not_the_module_path() {
    let src =
        "export type CardHost = Pick<ReturnType<typeof import(\"./a.ts\").make>, \"x\" | \"y\">;
               export type Other = Pick<T, \"z\">;
";
    assert_eq!(card_host_names(src), Some(2));
    assert_eq!(card_host_names("export type Other = 1;"), None);
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
         SHELL_FLASH_REACH in crates/xtask/tests/ratchets/workbench_coupling.rs:\n{}",
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
    let mut errors = column_owner_errors(&owners, &COLUMN_PAINT);
    for (function, modules) in &owners {
        if modules != &["wb-stage-columns.ts".to_string()] {
            errors.push(format!(
                "{function}: owner is {modules:?}, not wb-stage-columns.ts"
            ));
        }
    }
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
            lex::Tok::Ident(_) | lex::Tok::Punct(_) | lex::Tok::Str(_) | lex::Tok::Other => {}
        }
    }
    count
}

/// The member names in `type CardHost = Pick<…, "a" | "b">`: the string
/// tokens of the declaration that are not a module path.
fn card_host_names(src: &str) -> Option<usize> {
    let toks = lex::lex(src, 1);
    let decl = type_declaration(&toks, "CardHost")?;
    Some(
        toks[decl]
            .iter()
            .filter(|t| matches!(&t.tok, lex::Tok::Str(text) if !text.starts_with('.')))
            .count(),
    )
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
