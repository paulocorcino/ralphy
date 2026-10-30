//! The ratchets over `any`, casts, directives and sockets in the workbench (#613, ADR-0082).

use super::*;

/// `(module, count)` of every explicit `any` under `UI_DIR`, the count oxlint's
/// `typescript/no-explicit-any` reports: 2122 in 50 modules on e75f2394, and
/// none since #613. The table is empty and stays empty: every module has zero.
const ANY_BASELINE: &[(&str, usize)] = &[];

/// The `new WebSocket(` sites in the workbench modules: `spawn`,
/// `persistentSocket` and the console terminal (ADR-0082 D1). A command with
/// one reply goes over `POST /api/command`, not a new socket. Exact: a site
/// that goes lowers it in the same change.
const WEBSOCKET_SITES: usize = 3;

/// `(module, count)` of every cast written over another cast (`x as A as B`)
/// under `UI_DIR`: each is a new element typed as a console window or a note
/// card (#653). Exact, as `ANY_BASELINE` is.
const CHAINED_CAST_BASELINE: &[(&str, usize)] = &[("wb-stage-chrome.ts", 1), ("wb-notes.ts", 1)];

/// #613: no module has an explicit `any`. oxlint (`typescript/no-explicit-any`)
/// reports the same `any`s per file, for every module.
#[test]
fn explicit_any_matches_the_baseline() {
    let actual = any_counts(&workspace_root().join(UI_DIR));
    let errors = any_errors(&actual, ANY_BASELINE);
    assert!(
        errors.is_empty(),
        "explicit `any` in {UI_DIR}: give the value its real type, or, when a change \
         removed some, lower ANY_BASELINE in crates/xtask/tests/ratchets/type_escapes.rs:\n{}",
        errors.join("\n")
    );
}

/// ADR-0082 D1: the page opens a WebSocket in three places only.
#[test]
fn websocket_sites_match_the_baseline() {
    let ui = workspace_root().join(UI_DIR);
    let actual: usize = ts_modules(&ui)
        .iter()
        .map(|path| websocket_sites(&read(path)))
        .sum();
    assert_eq!(
        actual, WEBSOCKET_SITES,
        "`new WebSocket(` sites in {UI_DIR}: a command with one reply goes over `POST /api/command`; when a site was removed, lower WEBSOCKET_SITES in crates/xtask/tests/ratchets/type_escapes.rs"
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
            lex::Tok::Ident(_) | lex::Tok::Punct(_) | lex::Tok::Str(_) | lex::Tok::Other => {}
        }
    }
    count
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
