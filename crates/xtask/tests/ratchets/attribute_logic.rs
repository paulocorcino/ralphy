//! An Alpine attribute of the workbench markup holds one member read or one
//! call with plain arguments (#942). A compound site has one of five shapes:
//! more than one `&&` or `||`, a `? :`, a template string, the comma
//! operator, or more than one statement. The count per file only goes down.

use super::*;
use lex::{lex, Tok, Token};

/// The compound sites of each markup file, as measured. A file with none is
/// not listed.
const COUNTED_SITES: &[(&str, usize)] = &[];

#[test]
fn attribute_logic_matches_the_baseline() {
    let ui = workspace_root().join(UI_DIR);
    let entries = std::fs::read_dir(&ui)
        .map_err(|e| format!("reading {}: {e}", ui.display()))
        .expect("this test needs the call to succeed");
    let mut sites: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("reading an entry of {}: {e}", ui.display()))
            .expect("this test needs the call to succeed")
            .path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "html") {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let lines = compound_sites(&read(&path));
        if !lines.is_empty() {
            sites.insert(name.unwrap_or_default(), lines);
        }
    }
    let actual: BTreeMap<String, usize> = sites.iter().map(|(f, l)| (f.clone(), l.len())).collect();
    let baseline: BTreeMap<String, usize> = COUNTED_SITES
        .iter()
        .map(|(f, n)| ((*f).to_owned(), *n))
        .collect();
    let list = |file: &String| {
        let lines = sites.get(file).map(Vec::as_slice).unwrap_or_default();
        let at: Vec<String> = lines
            .iter()
            .map(|l| format!("  {UI_DIR}/{file}:{l}"))
            .collect();
        at.join("\n")
    };
    let errors = ratchet_errors(
        &actual,
        &baseline,
        |file, n| {
            format!(
                "{file}: {n} compound sites, not in COUNTED_SITES:\n{}",
                list(file)
            )
        },
        |file, was, now| {
            format!(
                "{file}: COUNTED_SITES says {was}, found {now}:\n{}",
                list(file)
            )
        },
    );
    assert!(
        errors.is_empty(),
        "move each compound expression into a member of the component and call it; \
         when a count goes down, lower COUNTED_SITES:\n{}",
        errors.join("\n")
    );
}

/// The start line of each Alpine attribute of `html` that holds compound logic.
fn compound_sites(html: &str) -> Vec<usize> {
    markup::attributes(html)
        .into_iter()
        .filter(|(expr, line)| is_compound(&lex(expr, *line)))
        .map(|(_, line)| line)
        .collect()
}

fn is_compound(toks: &[Token]) -> bool {
    let logical = toks.iter().filter(|t| t.is("&&") || t.is("||")).count();
    logical > 1
        || toks
            .iter()
            .any(|t| t.is("?") || matches!(t.tok, Tok::Tpl(_)))
        || has_comma_operator(toks)
        || has_statement_sequence(toks)
}

/// Each token with the bracket depth it sits at: a `(`, `[` or `{` is at the
/// depth outside it.
fn depths(toks: &[Token]) -> Vec<usize> {
    let mut depth = 0usize;
    toks.iter()
        .map(|t| {
            if t.is(")") || t.is("]") || t.is("}") {
                depth = depth.saturating_sub(1);
            }
            let at = depth;
            if t.is("(") || t.is("[") || t.is("{") {
                depth += 1;
            }
            at
        })
        .collect()
}

fn has_comma_operator(toks: &[Token]) -> bool {
    toks.iter()
        .zip(depths(toks))
        .any(|(t, d)| d == 0 && t.is(","))
}

/// A `;` outside brackets with another statement after it.
fn has_statement_sequence(toks: &[Token]) -> bool {
    let mut ended = false;
    for (t, d) in toks.iter().zip(depths(toks)) {
        if d == 0 && t.is(";") {
            ended = true;
        } else if ended {
            return true;
        }
    }
    false
}

#[test]
fn counts_each_shape_of_compound_logic() {
    let counted = |expr: &str| is_compound(&lex(expr, 1));
    for shape in [
        "open && ready || busy",
        "open ? 'a' : 'b'",
        "`row-${id}`",
        "pick(), close()",
        "$event.preventDefault(); save()",
        "menu = false; open(row)",
    ] {
        assert!(counted(shape), "expected compound: {shape}");
    }
    for plain in [
        "open",
        "open && ready",
        "pick(item, 'x', 2)",
        "{ on: active, off: idle }",
        "$event.key === 'Enter'",
        "say('a ? b, c')",
        "items.map((a, b) => a)",
        "$event.preventDefault()",
        "open = true",
        "save();",
        "run(() => { a(); b() })",
    ] {
        assert!(!counted(plain), "expected plain: {plain}");
    }
    let html = concat!(
        "<div x-data=\"{ a: b ? c : d }\" x-ref=\"x\" x-transition:enter=\"a ? b : c\"\n",
        "  x-for=\"(row, i) in rows\" @click.prevent=\"go(row)\"></div>\n",
        "<p :class=\"on ? 'a' : 'b'\" x-text=\"name\"></p>\n",
        "<template x-for=\"row in a ? b : c\"></template>\n",
    );
    assert_eq!(compound_sites(html), vec![3, 4]);
}
