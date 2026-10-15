//! `ui-groups` — the members of `shell()` in the workbench script, by group,
//! and what each group shares with the others.
//!
//! ## Why this is a command
//!
//! ADR-0073 cuts `shell()` (`crates/ralphy-daemon/assets/ui/app.ts`) into
//! Alpine components, one group at a time. A group can leave only when the
//! state it shares has one owner, so every cut needs the same numbers: the
//! members of each group, the names other groups read, the fields other groups
//! write, and the `index.html` bindings that name a member. A throwaway script
//! first counted them and needed 54 hand corrections (#606). This command
//! takes the group of a member from the section heading it sits under and from
//! nothing else, so the headings in `app.ts` are the one place that sets it.
//! A member under the wrong heading shows as a read or a write from outside its
//! group; the fix is to move the member, not to correct the measure.
//!
//! ## What it counts, and its limits
//!
//! It is not a JavaScript parser: it reads the tokens of the `ui-copy` lexer.
//! - A section starts at a line `    // --- name ---` (three dashes or more,
//!   then a space). Each heading maps to a group in [`SECTIONS`]; a heading
//!   with no entry there is an error, so a new section must name its group.
//! - A member is a property of the object literal passed to `shellData`.
//!   Its lines run from the line after the previous member to its last token,
//!   so they include the comments and blank lines above it.
//! - An access is `this.<member>` (and `self.<member>`, the alias inside
//!   `scrim()`), also inside a template literal hole. It belongs to the group
//!   of the member it sits in; an access of a member to itself is not counted.
//!   It is a write when it is an assignment target (`=`, `+=`, `||=`, …),
//!   `++`/`--`, a mutating call (`.push(`, `.set(`, …), or a write inside it
//!   (`this.x.y =`, `this.x[k] =`, `delete this.x[k]`). Any other access,
//!   a call included, is a read.
//! - A binding is an Alpine attribute (`x-*`, `@…`, `:…`) of `index.html`
//!   whose nearest `x-data` is `shell`. `x-data`, `x-ref`, `x-cloak`,
//!   `x-ignore`, `x-teleport`, `x-id` and `x-transition*` are not bindings.
//!   A binding names a member by a bare identifier: not after a `.`, not an
//!   object key, not an arrow parameter or a `let` name, not an `x-for`
//!   variable, not `$`-prefixed. The first string argument of `scrim('flag', …)`
//!   names `flag`.
//! - Code outside `shell()` that reaches it through `window.getShell()` is not
//!   counted.

// The `ui-copy` lexer, the same file compiled a second time so that `ui_copy`
// keeps its modules private: one lexer, one rule, no second copy of the text.
#[allow(clippy::duplicate_mod)]
#[path = "ui_copy/lex.rs"]
mod lex;

mod html;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use html::scan_bindings;
use lex::{lex, Piece, Tok, Token};

const APP: &str = "crates/ralphy-daemon/assets/ui/app.ts";
const HTML: &str = "crates/ralphy-daemon/assets/ui/index.html";

/// The groups `shell()` is cut by (#605). Layout core is what `shell()` keeps.
/// The consoles group left `shell()` for `wb-consoles-tab.ts`, and the files
/// group for `wb-files.ts` and `wb-move-dialog.ts` (#621). The Runs panel
/// left the board group for `wb-runs-panel.ts`, and the board for
/// `wb-board.ts` (#621). The canvas tabs, the slot, the per-client view and
/// the tab menu left layout core for the Alpine store `tabs`
/// (`wb-tabs-store.ts`, #621).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Group {
    Core,
    Git,
    Board,
}

impl Group {
    const ALL: [Group; 3] = [Group::Core, Group::Git, Group::Board];

    fn label(self) -> &'static str {
        match self {
            Group::Core => "layout core",
            Group::Git => "git of the open project",
            Group::Board => "board and runs",
        }
    }

    fn short(self) -> &'static str {
        match self {
            Group::Core => "core",
            Group::Git => "git",
            Group::Board => "board",
        }
    }
}

/// The group of each section heading of `shell()`, by the start of its name.
/// The part before the first heading is layout core.
const SECTIONS: &[(&str, Group)] = &[
    ("the open project", Group::Core),
    ("chrome panels", Group::Core),
    ("branch switcher", Group::Git),
    ("Runs panel", Group::Board),
];

/// Calls that change the value they are called on.
const MUTATORS: &[&str] = &[
    "push", "splice", "set", "delete", "add", "clear", "unshift", "pop", "shift", "sort",
];

pub fn ui_groups_cmd(args: &[String]) -> Result<()> {
    let mut root: Option<PathBuf> = None;
    let mut verbose = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--root" => root = Some(PathBuf::from(crate::next_value(&mut it, "--root")?)),
            "--verbose" => verbose = true,
            other => bail!("unknown argument: {other}"),
        }
    }
    let root = root.unwrap_or_else(crate::asset_pins::repo_root);
    let read = |rel: &str| {
        let path = root.join(rel);
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    };
    let shell = parse_shell(&read(APP)?)?;
    let names: BTreeSet<String> = shell.members.iter().map(|m| m.name.clone()).collect();
    let bindings = scan_bindings(&read(HTML)?, &names);
    print!("{}", render(&shell, &bindings, verbose));
    Ok(())
}

/// The name of a section heading, if `line` is one.
fn heading(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("    // ")?;
    let after = rest.trim_start_matches('-');
    if rest.len() - after.len() < 3 {
        return None;
    }
    let name = after
        .strip_prefix(' ')?
        .trim_end()
        .trim_end_matches('-')
        .trim_end();
    (!name.is_empty()).then_some(name)
}

fn group_of(heading: &str) -> Option<Group> {
    SECTIONS
        .iter()
        .find(|(prefix, _)| heading.starts_with(prefix))
        .map(|&(_, group)| group)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Field,
    Method,
    Getter,
    Setter,
}

struct Section {
    name: String,
    group: Group,
    line: usize,
}

struct Member {
    name: String,
    kind: Kind,
    /// The line after the end of the previous member: a member's lines
    /// include the comments and blank lines above it, as a cut moves them.
    from: usize,
    /// The line of its first token.
    first: usize,
    last: usize,
    section: usize,
}

struct Access {
    name: String,
    /// Index of the member the access sits in.
    within: usize,
    write: bool,
}

struct Shell {
    sections: Vec<Section>,
    members: Vec<Member>,
    accesses: Vec<Access>,
}

impl Shell {
    fn group(&self, member: usize) -> Group {
        self.sections[self.members[member].section].group
    }
}

/// Read the sections, members and accesses of `shell()` from `app.ts`.
fn parse_shell(app: &str) -> Result<Shell> {
    let cs: Vec<char> = app.chars().collect();
    let toks = lex(app, 1);
    let bounds = bounds(&toks)?;
    let sections = sections(app, &toks, &bounds)?;
    let (members, ranges) = members(&toks, &cs, &sections, &bounds)?;
    let accesses = accesses(&toks, &members, &ranges);
    Ok(Shell {
        sections,
        members,
        accesses,
    })
}

/// Where `shell()` sits in the tokens of `app.ts`.
struct Bounds {
    /// The `function` of `function shell`.
    start: usize,
    /// The `{` of `shellData({`.
    open: usize,
    /// The `}` that closes `open`.
    close: usize,
}

fn tok_is(toks: &[Token], i: usize, p: &str) -> bool {
    toks.get(i).is_some_and(|t| t.is(p))
}

fn ident_at(toks: &[Token], i: usize) -> Option<&str> {
    toks.get(i).and_then(Token::ident)
}

/// The line of the last character of the token at `i`.
fn end_line(toks: &[Token], cs: &[char], i: usize) -> usize {
    let t = &toks[i];
    t.line + cs[t.start..t.end].iter().filter(|&&c| c == '\n').count()
}

fn bounds(toks: &[Token]) -> Result<Bounds> {
    let start = (0..toks.len())
        .find(|&i| ident_at(toks, i) == Some("function") && ident_at(toks, i + 1) == Some("shell"))
        .context("app.ts has no `function shell`")?;
    let open = (start..toks.len())
        .find(|&i| {
            ident_at(toks, i) == Some("shellData")
                && tok_is(toks, i + 1, "(")
                && tok_is(toks, i + 2, "{")
        })
        .map(|i| i + 2)
        .context("shell() does not return shellData({ … })")?;
    let close = close_of(toks, open);
    if close >= toks.len() {
        bail!("the object literal of shell() is not closed");
    }
    Ok(Bounds { start, open, close })
}

/// The part before the first heading, then one section per heading line
/// inside the object literal.
fn sections(app: &str, toks: &[Token], bounds: &Bounds) -> Result<Vec<Section>> {
    let mut sections = vec![Section {
        name: "(top)".to_string(),
        group: Group::Core,
        line: toks[bounds.start].line,
    }];
    for (n, line) in app.lines().enumerate() {
        let number = n + 1;
        if number <= toks[bounds.open].line || number >= toks[bounds.close].line {
            continue;
        }
        if let Some(name) = heading(line) {
            let group = group_of(name).with_context(|| {
                format!("the heading {name:?} at app.ts:{number} has no group in SECTIONS")
            })?;
            sections.push(Section {
                name: name.to_string(),
                group,
                line: number,
            });
        }
    }
    Ok(sections)
}

/// The first and the last token of a member.
type TokenRange = (usize, usize);

/// The members of the object literal, and the range of tokens of each, in
/// the same order.
fn members(
    toks: &[Token],
    cs: &[char],
    sections: &[Section],
    bounds: &Bounds,
) -> Result<(Vec<Member>, Vec<TokenRange>)> {
    let mut members = Vec::new();
    let mut ranges = Vec::new();
    let mut i = bounds.open + 1;
    while i < bounds.close {
        let first = i;
        let (name_at, name, kind) = member_head(toks, i)?;
        let j = member_end(toks, name_at, bounds.close);
        let last = j.min(bounds.close) - 1;
        let line = toks[first].line;
        let section = sections.iter().rposition(|s| s.line <= line).unwrap_or(0);
        let from = members
            .last()
            .map_or(toks[bounds.open].line, |m: &Member| m.last)
            + 1;
        members.push(Member {
            name,
            kind,
            from,
            first: line,
            last: end_line(toks, cs, last),
            section,
        });
        ranges.push((first, last));
        i = j + 1;
    }
    Ok((members, ranges))
}

/// The head of the member that starts at `i`: the index of its name, its
/// name and its kind.
fn member_head(toks: &[Token], i: usize) -> Result<(usize, String, Kind)> {
    let mut name_at = i;
    let mut accessor = None;
    if matches!(ident_at(toks, i), Some("async" | "get" | "set"))
        && toks.get(i + 1).is_some_and(|t| t.ident().is_some())
    {
        accessor = ident_at(toks, i);
        name_at = i + 1;
    }
    if tok_is(toks, name_at, "*") {
        name_at += 1;
    }
    let name = match toks.get(name_at).map(|t| &t.tok) {
        Some(Tok::Ident(n) | Tok::Str(n)) => n.clone(),
        _ => bail!(
            "app.ts:{}: a member of shell() with no plain name",
            toks[name_at].line
        ),
    };
    Ok((name_at, name, member_kind(toks, name_at, accessor)))
}

/// The kind of the member whose name is at `name_at`; `accessor` is the
/// `async`, `get` or `set` before the name.
fn member_kind(toks: &[Token], name_at: usize, accessor: Option<&str>) -> Kind {
    if tok_is(toks, name_at + 1, "(") {
        match accessor {
            Some("get") => Kind::Getter,
            Some("set") => Kind::Setter,
            _ => Kind::Method,
        }
    } else if tok_is(toks, name_at + 1, ":") && is_function(toks, name_at + 2) {
        Kind::Method
    } else {
        Kind::Field
    }
}

/// Index of the `,` that ends the member whose name is at `name_at`, or
/// `close`. The member ends before the next `,` at its own depth.
fn member_end(toks: &[Token], name_at: usize, close: usize) -> usize {
    let mut j = name_at + 1;
    let (mut in_type, mut angle) = (false, 0usize);
    while j < close && !(tok_is(toks, j, ",") && angle == 0) {
        (in_type, angle) = type_args(toks, j, in_type, angle);
        j = match &toks[j].tok {
            Tok::Punct("(" | "[" | "{") => close_of(toks, j) + 1,
            _ => j + 1,
        };
    }
    j
}

/// Whether a type has started and the depth of `<` after the token at `j`.
/// After `as` and after the `:` of a return type, `<…>` is a type argument
/// list, whose commas are not the end of a member.
fn type_args(toks: &[Token], j: usize, in_type: bool, angle: usize) -> (bool, usize) {
    if ident_at(toks, j) == Some("as") || (tok_is(toks, j, ":") && tok_is(toks, j - 1, ")")) {
        (true, angle)
    } else if in_type && tok_is(toks, j, "<") {
        (true, angle + 1)
    } else if in_type && tok_is(toks, j, ">") {
        (true, angle.saturating_sub(1))
    } else {
        (in_type, angle)
    }
}

/// The accesses of each member to the other members, from the tokens in its
/// range.
fn accesses(toks: &[Token], members: &[Member], ranges: &[TokenRange]) -> Vec<Access> {
    let known: BTreeMap<&str, usize> = members
        .iter()
        .enumerate()
        .map(|(n, m)| (m.name.as_str(), n))
        .collect();
    let mut accesses = Vec::new();
    for (within, &(first, last)) in ranges.iter().enumerate() {
        let mut found = Vec::new();
        scan_accesses(&toks[first..=last], &known, &mut found);
        for (name, write) in found {
            if name != members[within].name {
                accesses.push(Access {
                    name,
                    within,
                    write,
                });
            }
        }
    }
    accesses
}

/// Index of the bracket that closes the one at `open`, or `toks.len()`.
fn close_of(toks: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t.tok {
            Tok::Punct("(" | "[" | "{") => depth += 1,
            Tok::Punct(")" | "]" | "}") => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    toks.len()
}

/// Does a function value start at `i`: `function`, `async`, `x =>` or `(…) =>`?
fn is_function(toks: &[Token], i: usize) -> bool {
    match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Ident(w)) if w == "function" || w == "async" => true,
        Some(Tok::Ident(_)) => toks.get(i + 1).is_some_and(|t| t.is("=>")),
        Some(Tok::Punct("(")) => toks.get(close_of(toks, i) + 1).is_some_and(|t| t.is("=>")),
        _ => false,
    }
}

/// Every `this.<member>` / `self.<member>` in `toks`, with whether it writes.
fn scan_accesses(toks: &[Token], known: &BTreeMap<&str, usize>, out: &mut Vec<(String, bool)>) {
    for (j, t) in toks.iter().enumerate() {
        if let Tok::Tpl(pieces) = &t.tok {
            for piece in pieces {
                if let Piece::Expr(code) = piece {
                    scan_accesses(&lex(code, t.line), known, out);
                }
            }
            continue;
        }
        if !matches!(t.ident(), Some("this" | "self"))
            || !toks.get(j + 1).is_some_and(|d| d.is("."))
        {
            continue;
        }
        if let Some(name) = toks.get(j + 2).and_then(Token::ident) {
            if known.contains_key(name) {
                out.push((name.to_string(), writes(toks, j, j + 2)));
            }
        }
    }
}

/// Is the access whose receiver is at `recv` and whose name is at `name` a write?
fn writes(toks: &[Token], recv: usize, name: usize) -> bool {
    let at = |i: usize, p: &str| toks.get(i).is_some_and(|t| t.is(p));
    let counts = |i: usize| at(i, "++") || at(i, "--");
    let next = name + 1;
    if assigns(toks, next) || counts(next) || (recv > 0 && counts(recv - 1)) {
        return true;
    }
    if at(next, ".")
        && toks
            .get(next + 1)
            .and_then(Token::ident)
            .is_some_and(|m| MUTATORS.contains(&m))
        && at(next + 2, "(")
    {
        return true;
    }
    // A write inside the member: `this.x.y =`, `this.x[k] =`, `this.x.y++`.
    let mut k = next;
    let mut inside = false;
    loop {
        if (at(k, ".") || at(k, "?.")) && toks.get(k + 1).and_then(Token::ident).is_some() {
            k += 2;
        } else if at(k, "[") {
            k = close_of(toks, k) + 1;
        } else {
            break;
        }
        inside = true;
        if assigns(toks, k) || counts(k) {
            return true;
        }
    }
    inside && recv > 0 && toks[recv - 1].ident() == Some("delete")
}

/// Does an assignment operator start at `i`? The lexer splits `||=` into
/// `||` and `=`, so two touching tokens count as one.
fn assigns(toks: &[Token], i: usize) -> bool {
    let Some(t) = toks.get(i) else { return false };
    match t.tok {
        Tok::Punct("=" | "+=" | "-=" | "*=" | "/=" | "**=") => true,
        Tok::Punct("||" | "&&" | "??" | "%" | "&" | "|" | "^") => toks
            .get(i + 1)
            .is_some_and(|n| n.is("=") && n.start == t.end),
        _ => false,
    }
}

/// One Alpine attribute in the `shell()` scope of `index.html`.
struct Binding {
    line: usize,
    names: BTreeSet<String>,
}

/// The free identifiers an Alpine expression reads, and the flag named by the
/// first argument of `scrim('flag', …)`.
fn expression_names(expr: &str) -> Vec<String> {
    let toks = lex(expr, 1);
    let at = |i: usize, p: &str| toks.get(i).is_some_and(|t| t.is(p));
    let mut declared = BTreeSet::new();
    for (i, t) in toks.iter().enumerate() {
        if t.is("=>") {
            if let Some(p) = i.checked_sub(1).and_then(|p| toks[p].ident()) {
                declared.insert(p.to_string());
            } else if i > 0 && at(i - 1, ")") {
                let open = (0..i - 1).rev().find(|&k| close_of(&toks, k) == i - 1);
                for p in open.map_or(&toks[0..0], |o| &toks[o..i]) {
                    declared.extend(p.ident().map(str::to_string));
                }
            }
        }
        if matches!(t.ident(), Some("let" | "const" | "var")) {
            declared.extend(toks.get(i + 1).and_then(Token::ident).map(str::to_string));
        }
    }
    let mut names = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.ident() == Some("scrim") && at(i + 1, "(") {
            if let Some(Tok::Str(flag)) = toks.get(i + 2).map(|t| &t.tok) {
                names.push(flag.split('.').next().unwrap_or("").to_string());
            }
        }
        let Some(id) = t.ident() else { continue };
        let after_dot = i > 0 && (at(i - 1, ".") || at(i - 1, "?."));
        let key = at(i + 1, ":") && (i == 0 || at(i - 1, "{") || at(i - 1, ","));
        if !after_dot && !key && !declared.contains(id) {
            names.push(id.to_string());
        }
    }
    names
}

/// What one group holds and shares.
#[derive(Default)]
struct Stats {
    members: Vec<usize>,
    lines: usize,
    /// Members of this group read by code of another group, with those groups.
    read_outside: BTreeMap<String, BTreeSet<Group>>,
    /// Members of this group written by code of another group.
    written_outside: BTreeMap<String, BTreeSet<Group>>,
    bindings: usize,
    only_this: usize,
}

fn stats(shell: &Shell, bindings: &[Binding]) -> BTreeMap<Group, Stats> {
    let mut by: BTreeMap<Group, Stats> =
        Group::ALL.iter().map(|&g| (g, Stats::default())).collect();
    let index: BTreeMap<&str, usize> = shell
        .members
        .iter()
        .enumerate()
        .map(|(n, m)| (m.name.as_str(), n))
        .collect();
    for (n, m) in shell.members.iter().enumerate() {
        let s = by.entry(shell.group(n)).or_default();
        s.members.push(n);
        s.lines += m.last + 1 - m.from;
    }
    for a in &shell.accesses {
        let Some(&target) = index.get(a.name.as_str()) else {
            continue;
        };
        let (owner, from) = (shell.group(target), shell.group(a.within));
        if owner == from {
            continue;
        }
        let s = by.entry(owner).or_default();
        let list = if a.write {
            &mut s.written_outside
        } else {
            &mut s.read_outside
        };
        list.entry(a.name.clone()).or_default().insert(from);
    }
    for b in bindings {
        let groups = binding_groups(shell, &index, b);
        for &g in &groups {
            let s = by.entry(g).or_default();
            s.bindings += 1;
            if groups.len() == 1 {
                s.only_this += 1;
            }
        }
    }
    by
}

fn binding_groups(shell: &Shell, index: &BTreeMap<&str, usize>, b: &Binding) -> BTreeSet<Group> {
    b.names
        .iter()
        .filter_map(|n| index.get(n.as_str()))
        .map(|&m| shell.group(m))
        .collect()
}

fn render(shell: &Shell, bindings: &[Binding], verbose: bool) -> String {
    let by = stats(shell, bindings);
    let index: BTreeMap<&str, usize> = shell
        .members
        .iter()
        .enumerate()
        .map(|(n, m)| (m.name.as_str(), n))
        .collect();
    let named = bindings.iter().filter(|b| !b.names.is_empty()).count();
    let mut mixed = 0;
    let mut mixed_features = Vec::new();
    for b in bindings {
        let groups = binding_groups(shell, &index, b);
        if groups.len() >= 2 {
            mixed += 1;
        }
        if groups.iter().filter(|&&g| g != Group::Core).count() >= 2 {
            mixed_features.push(b);
        }
    }

    let mut o = String::new();
    let mut line = |text: String| {
        o.push_str(&text);
        o.push('\n');
    };
    line(format!(
        "ui-groups — the members of shell() in {APP}, by the section heading they sit under\n"
    ));
    line(format!(
        "{} members in {} sections; {HTML}: {} bindings in shell() scope, {named} name a member\n",
        shell.members.len(),
        shell.sections.len(),
        bindings.len()
    ));
    line(format!(
        "{:<26}{:>8}{:>7}{:>16}{:>19}{:>10}{:>11}",
        "group", "members", "lines", "read outside", "written outside", "bindings", "only this"
    ));
    for (g, s) in &by {
        line(format!(
            "{:<26}{:>8}{:>7}{:>16}{:>19}{:>10}{:>11}",
            g.label(),
            s.members.len(),
            s.lines,
            s.read_outside.len(),
            s.written_outside.len(),
            s.bindings,
            s.only_this
        ));
    }
    line(format!(
        "\nbindings that mix groups: {mixed}; that mix two feature groups: {}",
        mixed_features.len()
    ));
    for b in &mixed_features {
        let names: Vec<&str> = b.names.iter().map(String::as_str).collect();
        line(format!("  index.html:{} [{}]", b.line, names.join(", ")));
    }

    line("\nsections:".to_string());
    for (k, s) in shell.sections.iter().enumerate() {
        let count = shell.members.iter().filter(|m| m.section == k).count();
        line(format!(
            "  {:>5}  {:<9} {:>4} members  {}",
            s.line,
            s.group.short(),
            count,
            s.name
        ));
    }

    let named_list = |list: &BTreeMap<String, BTreeSet<Group>>| -> String {
        list.iter()
            .map(|(n, gs)| {
                let gs: Vec<&str> = gs.iter().map(|g| g.short()).collect();
                format!("{n} [{}]", gs.join(", "))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (g, s) in &by {
        line(format!(
            "\n## {} ({} members, {} lines)",
            g.label(),
            s.members.len(),
            s.lines
        ));
        let names: Vec<&str> = s
            .members
            .iter()
            .map(|&n| shell.members[n].name.as_str())
            .collect();
        line(format!("members: {}", names.join(", ")));
        line(format!(
            "read from outside ({}): {}",
            s.read_outside.len(),
            named_list(&s.read_outside)
        ));
        line(format!(
            "written from outside ({}): {}",
            s.written_outside.len(),
            named_list(&s.written_outside)
        ));
        if verbose {
            for &n in &s.members {
                let m = &shell.members[n];
                line(format!(
                    "  {:>5}-{:<5} {:<7} {:<32} {}",
                    m.first,
                    m.last,
                    format!("{:?}", m.kind).to_lowercase(),
                    m.name,
                    shell.sections[m.section].name
                ));
            }
        }
    }
    o
}
