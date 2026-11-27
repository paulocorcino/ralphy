//! Every name an Alpine attribute reads inside the `x-data` element of a
//! component is one of the component's own members or a name of its `uses`
//! list (ADR-0073 D4, #1056), and no name read outside every component
//! element is a member that only one component has. The members are read
//! from the `.ts` source through the `ui-copy` lexer: the keys of the object passed to
//! `component(uses, { … })`, its methods and getters, and the `uses` strings.

use super::*;
use html_tags::{read_tag, skip_declaration, skip_past, starts};
use lex::{lex, Tok, Token};

/// The words an expression may hold that read no name: literals and
/// operators written as words.
const NOT_MEMBERS: &[&str] = &[
    "true",
    "false",
    "null",
    "undefined",
    "this",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "void",
    "delete",
    "NaN",
    "Infinity",
];

/// The page globals the markup reads; Alpine finds a name outside the scope
/// there.
const GLOBALS: &[&str] = &["window", "Boolean"];

/// Elements with no end tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// A component's own members and its `uses` names.
#[derive(Default)]
struct Members {
    own: BTreeSet<String>,
    uses: BTreeSet<String>,
}

/// What the walk found: one line per name read that is not allowed, and the
/// count of names it checked.
#[derive(Default)]
struct Report {
    findings: Vec<String>,
    outside: Vec<String>,
    checked: usize,
}

/// One open element: its tag, the component whose scope it is in (`None`
/// outside any checked component), and the `x-for` names in scope.
struct Frame {
    tag: String,
    component: Option<String>,
    locals: BTreeSet<String>,
}

#[test]
fn every_markup_binding_names_a_member_or_a_use() {
    let report = check_the_workbench();
    assert!(
        report.findings.is_empty(),
        "a markup binding reads a name that is neither the component's own nor in \
         its uses list: add the member, or list the shell() name in uses:\n{}",
        report.findings.join("\n")
    );
}

#[test]
fn no_binding_outside_a_component_reads_a_member_only_it_has() {
    let report = check_the_workbench();
    assert!(
        report.outside.is_empty(),
        "a markup binding outside a component reads a member only that component has: \
         move the binding inside the component, or give the outer scope the member:\n{}",
        report.outside.join("\n")
    );
}

#[test]
fn the_binding_check_reads_the_markup() {
    let report = check_the_workbench();
    assert!(
        report.checked > 100,
        "expected the check to read more than 100 names, it read {}",
        report.checked
    );
}

#[test]
fn the_binding_check_catches_the_known_bad_fixture() {
    let dir = workspace_root().join("crates/xtask/tests/fixtures/markup_bindings");
    let mut table = BTreeMap::new();
    let members = members_of(&read(&dir.join("wb-bad.ts")))
        .expect("the fixture component is in a shape the check reads");
    table.insert("wbBad".to_owned(), members);
    let mut report = Report::default();
    check_file(
        "wb-bad.part.html",
        &read(&dir.join("wb-bad.part.html")),
        &table,
        &mut report,
    );
    assert_eq!(
        report.findings,
        vec![
            "wb-bad.part.html:3: wbBad reads missing".to_owned(),
            "wb-bad.part.html:8: wbBad reads sideOpen".to_owned(),
        ]
    );
    assert_eq!(
        report.outside,
        vec![
            "wb-bad.part.html:11: a binding outside wbBad reads its count".to_owned(),
            "wb-bad.part.html:12: a binding outside wbBad reads its open".to_owned(),
        ]
    );
}

fn check_the_workbench() -> Report {
    let ui = workspace_root().join(UI_DIR);
    let table = component_table(&ui);
    assert!(
        table.len() > 20,
        "expected the components that main.ts registers, found {}",
        table.len()
    );
    let mut report = Report::default();
    let entries = std::fs::read_dir(&ui)
        .map_err(|e| format!("reading {}: {e}", ui.display()))
        .expect("this test needs the call to succeed");
    for entry in entries {
        let path = entry
            .map_err(|e| format!("reading an entry of {}: {e}", ui.display()))
            .expect("this test needs the call to succeed")
            .path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "html") {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            check_file(&name.unwrap_or_default(), &read(&path), &table, &mut report);
        }
    }
    report
}

/// Each component `main.ts` registers with `Alpine.data`, but `shell`, with
/// the members of its factory in the module it imports the factory from.
fn component_table(ui: &Path) -> BTreeMap<String, Members> {
    let toks = lex(&read(&ui.join("main.ts")), 1);
    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    for (k, t) in toks.iter().enumerate() {
        if t.ident() != Some("import") || !toks.get(k + 1).is_some_and(|t| t.is("{")) {
            continue;
        }
        let close = (k..toks.len())
            .find(|&j| is_at(&toks, j, "}"))
            .unwrap_or(toks.len());
        let from = toks
            .get(close + 2)
            .and_then(string)
            .map(|s| s.trim_start_matches("./").to_owned());
        let names = toks.get(k + 1..close).unwrap_or_default();
        for name in names.iter().filter_map(Token::ident) {
            modules.insert(name.to_owned(), from.clone().unwrap_or_default());
        }
    }
    let mut table = BTreeMap::new();
    for w in toks.windows(7) {
        let [alpine_obj, dot, data, paren, name, comma, factory] = w else {
            continue;
        };
        let registers = alpine_obj.ident() == Some("Alpine") && dot.is(".");
        if !(registers && data.ident() == Some("data") && paren.is("(") && comma.is(",")) {
            continue;
        }
        let (Some(alpine), Some(factory)) = (string(name), factory.ident()) else {
            continue;
        };
        if alpine == "shell" {
            continue;
        }
        let module = modules.get(factory).cloned().unwrap_or_default();
        let members = members_of(&read(&ui.join(&module)))
            .map_err(|e| format!("{alpine} in {module}: {e}"))
            .expect("every component is in a shape the check reads");
        table.insert(alpine, members);
    }
    table
}

/// The members of the one `component(uses, { … })` call in `src`. A spread
/// or a computed key cannot be read exactly, so it is an error.
fn members_of(src: &str) -> Result<Members, String> {
    let toks = lex(src, 1);
    let at = (0..toks.len().saturating_sub(2))
        .find(|&k| {
            toks.get(k).and_then(Token::ident) == Some("component")
                && is_at(&toks, k + 1, "(")
                && is_at(&toks, k + 2, "[")
                && !k.checked_sub(1).is_some_and(|p| is_at(&toks, p, "."))
        })
        .ok_or("no component([…], { … }) call")?;
    let mut members = Members::default();
    let mut k = at + 3;
    while let Some(t) = toks.get(k) {
        k += 1;
        if let Some(name) = string(t) {
            members.uses.insert(name);
        } else if t.is("]") {
            break;
        } else if !t.is(",") {
            let line = t.line;
            return Err(format!(
                "line {line}: the uses list holds more than strings"
            ));
        }
    }
    if !(toks.get(k).is_some_and(|t| t.is(",")) && toks.get(k + 1).is_some_and(|t| t.is("{"))) {
        return Err("the data is not an object literal".to_owned());
    }
    read_keys(&toks, k + 2, &mut members.own)?;
    Ok(members)
}

/// The keys of the object whose first token after `{` is at `from`. A `,`
/// outside brackets that no key follows is inside type arguments
/// (`Map<string, number>`), so the property goes on.
fn read_keys(toks: &[Token], from: usize, own: &mut BTreeSet<String>) -> Result<(), String> {
    let mut depth = 0usize;
    let mut key_next = true;
    for (k, t) in toks.iter().enumerate().skip(from) {
        if depth == 0 && key_next {
            key_next = false;
            if t.is("}") {
                return Ok(());
            }
            if let Some(key) = key_at(toks, k)? {
                own.insert(key);
            } else if k == from {
                return Err(format!(
                    "line {}: a member key the check cannot read",
                    t.line
                ));
            }
        }
        if t.is("(") || t.is("[") || t.is("{") {
            depth += 1;
        } else if t.is(")") || t.is("]") || t.is("}") {
            if depth == 0 {
                return Ok(());
            }
            depth -= 1;
        } else if depth == 0 && t.is(",") {
            key_next = true;
        }
    }
    Err("the data object does not end".to_owned())
}

/// The key of the property that starts at `k`: `name:`, `name(`, `name,`,
/// `get name(`, `async name(`, or `'name':`. `None` when no key starts there.
/// A spread or a computed key cannot be read exactly: an error.
fn key_at(toks: &[Token], k: usize) -> Result<Option<String>, String> {
    let Some(t) = toks.get(k) else {
        return Ok(None);
    };
    let next_is = |ps: &[&str]| toks.get(k + 1).is_some_and(|n| ps.iter().any(|p| n.is(p)));
    if t.is("...") || t.is("[") || t.is("*") {
        return Err(format!(
            "line {}: a spread or a computed member key",
            t.line
        ));
    }
    let after = toks.get(k + 1).and_then(Token::ident);
    let key = match t.ident() {
        Some("get" | "set" | "async") if after.is_some() => after.map(str::to_owned),
        Some(name) if next_is(&[":", "(", ",", "}"]) => Some(name.to_owned()),
        Some(_) => None,
        None => string(t).filter(|_| next_is(&[":"])),
    };
    Ok(key)
}

/// Check every Alpine attribute of `html` against the component its element
/// is in.
fn check_file(file: &str, html: &str, table: &BTreeMap<String, Members>, report: &mut Report) {
    let cs: Vec<char> = html.chars().collect();
    let mut stack: Vec<Frame> = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while let Some(&c) = cs.get(i) {
        if starts(&cs, i, "<!") {
            i = skip_declaration(&cs, i, &mut line);
        } else if starts(&cs, i, "</") {
            let end = skip_past(&cs, i, ">", &mut line);
            let tag: String = cs
                .get(i + 2..end.saturating_sub(1))
                .unwrap_or_default()
                .iter()
                .collect();
            close(&mut stack, &tag.trim().to_ascii_lowercase());
            i = end;
        } else if c == '<' && cs.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
            let (tag, attrs, self_closing, end) = read_tag(&cs, i + 1, &mut line);
            let frame = open(&stack, tag.clone(), &attrs, table);
            for (expr, at) in attrs
                .into_iter()
                .filter_map(|a| markup::expression(&a.name, a.value?).map(|e| (e, a.line)))
            {
                for read in reads(&decode(&expr)) {
                    report.checked += 1;
                    if let Some(finding) = finding(&frame, &read, table) {
                        let list = if frame.component.is_some() {
                            &mut report.findings
                        } else {
                            &mut report.outside
                        };
                        list.push(format!("{file}:{at}: {finding}"));
                    }
                }
            }
            i = if tag == "script" || tag == "style" {
                let body_end = skip_past(&cs, end, &format!("</{tag}"), &mut line);
                skip_past(&cs, body_end, ">", &mut line)
            } else {
                if !self_closing && !VOID.contains(&tag.as_str()) {
                    stack.push(frame);
                }
                end
            };
        } else {
            if c == '\n' {
                line += 1;
            }
            i += 1;
        }
    }
}

/// Why `read` may not be read where `frame` is: inside a component, a name
/// that is not its own, a use, or a loop name; outside every checked
/// component (`shell` too), a member that exactly one component owns.
fn finding(frame: &Frame, read: &str, table: &BTreeMap<String, Members>) -> Option<String> {
    if frame.locals.contains(read) {
        return None;
    }
    if let Some(name) = &frame.component {
        let members = table.get(name)?;
        let allowed = members.own.contains(read) || members.uses.contains(read);
        return (!allowed).then(|| format!("{name} reads {read}"));
    }
    let mut owners = table.iter().filter(|(_, m)| m.own.contains(read));
    match (owners.next(), owners.next()) {
        (Some((owner, _)), None) => Some(format!("a binding outside {owner} reads its {read}")),
        _ => None,
    }
}

/// The frame of an element: an `x-data` of a known component starts its
/// scope, any other `x-data` leaves the check, and an `x-for` adds its names.
fn open(
    stack: &[Frame],
    tag: String,
    attrs: &[html_tags::Attr],
    table: &BTreeMap<String, Members>,
) -> Frame {
    let parent = stack.last();
    let mut component = parent.and_then(|f| f.component.clone());
    let mut locals = parent.map_or_else(BTreeSet::new, |f| f.locals.clone());
    for a in attrs {
        let value = a.value.as_deref().unwrap_or_default();
        if a.name == "x-data" {
            let name = value.trim().to_owned();
            component = table.contains_key(&name).then_some(name);
            locals.clear();
        } else if a.name == "x-for" {
            let head = [" in ", " of "]
                .iter()
                .find_map(|sep| value.split_once(sep));
            let names = head.map_or("", |(names, _)| names);
            locals.extend(
                lex(names, 1)
                    .iter()
                    .filter_map(Token::ident)
                    .map(str::to_owned),
            );
        }
    }
    Frame {
        tag,
        component,
        locals,
    }
}

/// Pop up to and including the open element named `tag`; an end tag with no
/// open element of its name is ignored.
fn close(stack: &mut Vec<Frame>, tag: &str) {
    if let Some(at) = stack.iter().rposition(|f| f.tag == tag) {
        stack.truncate(at);
    }
}

/// The five character references the markup may write inside a value.
fn decode(expr: &str) -> String {
    expr.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The names an expression reads from its scope: not a member after `.`, not
/// a `$` magic, not an object key, not a literal word, and not an arrow
/// function's own parameter.
fn reads(expr: &str) -> Vec<String> {
    let toks = lex(expr, 1);
    let params = arrow_params(&toks);
    let mut names = Vec::new();
    for (k, t) in toks.iter().enumerate() {
        let Some(name) = t.ident() else { continue };
        let prev = k.checked_sub(1).and_then(|p| toks.get(p));
        let after_dot = prev.is_some_and(|p| p.is(".") || p.is("?."));
        let key = toks.get(k + 1).is_some_and(|n| n.is(":"))
            && prev.is_some_and(|p| p.is("{") || p.is(","));
        let skip = after_dot
            || key
            || name.starts_with('$')
            || NOT_MEMBERS.contains(&name)
            || GLOBALS.contains(&name)
            || params.contains(name);
        if !skip {
            names.push(name.to_owned());
        }
    }
    names
}

/// The parameters of every arrow function in `toks`: `x =>` or `(a, b) =>`.
fn arrow_params(toks: &[Token]) -> BTreeSet<String> {
    let mut params = BTreeSet::new();
    for (k, t) in toks.iter().enumerate() {
        let Some(before) = k.checked_sub(1).and_then(|p| toks.get(p)) else {
            continue;
        };
        if !t.is("=>") {
            continue;
        }
        if let Some(name) = before.ident() {
            params.insert(name.to_owned());
        } else if before.is(")") {
            let open = (0..k - 1)
                .rev()
                .find(|&j| is_at(toks, j, "("))
                .unwrap_or(k - 1);
            params.extend(
                toks.get(open..k)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Token::ident)
                    .map(str::to_owned),
            );
        }
    }
    params
}

/// Whether the token at `k` is the punctuation `p`.
fn is_at(toks: &[Token], k: usize, p: &str) -> bool {
    toks.get(k).is_some_and(|t| t.is(p))
}

/// The text of a string token.
fn string(t: &Token) -> Option<String> {
    if let Tok::Str(s) = &t.tok {
        Some(s.clone())
    } else {
        None
    }
}
