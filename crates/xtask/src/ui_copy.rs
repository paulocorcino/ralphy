//! `ui-copy` — an inventory of every piece of text the workbench shows
//! (PRD #422, issue #423).
//!
//! It is read-only and heuristic, and the report says so: it prints the rules
//! it extracts by and the text it knowingly misses. It is the working sheet for
//! the UI style guide, and its `--json` form is what the copy lint reads.
//!
//! Each row says whether a daemon test pins the text (the claims come from
//! `asset_pins::gather`, not from a second parser of the Rust tests), so an
//! editorial pass knows which assertions a rewrite touches.

mod check;
mod html;
mod js;
mod lex;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::asset_pins::{self, Pin};

use check::CopyCall;

/// Where the workbench's assets live, from the repo root.
const UI_DIR: &str = "crates/ralphy-daemon/assets/ui";

/// The checkable part of ADR-0065, from the repo root.
const RULES_FILE: &str = "docs/ui-copy-rules.json";

/// The functions whose text is copy whatever their name says: `copy_helpers`
/// return it, and `copy_calls` take it as an argument (`docs/ui-copy-rules.json`).
#[derive(Clone, Copy, Default)]
pub(crate) struct CopyFns<'a> {
    pub(crate) helpers: &'a [String],
    pub(crate) calls: &'a [CopyCall],
}

const RULES: &[&str] = &[
    "HTML: static `title`, `aria-label` and `placeholder` values; text nodes outside `<script>` and `<style>` (this includes `<title>`), whitespace squeezed, entities decoded, skipped when they hold no letter.",
    "Alpine: `:title`, `:aria-label`, `:placeholder` (or `x-bind:`) and `x-text`, read as a JavaScript expression.",
    "An expression becomes one row per thing it can show: a ternary gives its two branches (never its condition), `||`/`??` give each side, `&&` gives its right side. Inside a row, a non-literal part is written `{expr}` and the row is flagged concatenated.",
    "JavaScript sinks: the `title`/`message`/`text`/`label`/`confirmLabel`/`cancelLabel`/`placeholder`/`hint`/`caption`/`tooltip`/`ariaLabel`/`help`/`blurb` keys of an object literal (`js:toast` inside `toast(…)`, `js:confirm` inside `askConfirm`/`askNotice`/`askPrompt`, else `js:property`); `window.confirm`/`prompt`/`alert`; `.textContent`/`.innerText`/`.innerHTML =` (an `innerHTML` value that holds elements is read as HTML, one row per element and attribute; otherwise tags are stripped); `.title`/`.placeholder`/`.ariaLabel =` and `setAttribute(\"title\"|\"aria-label\"|\"placeholder\", …)`; `term.write(…)`; the key bar's `key(name, html, title)`; `this.x = …` when the text has a space (`js:state`); `const NAME = …` with a SCREAMING_CASE name when the text has a space and does not start with `(` or `[` (`js:const`); `return` inside a function whose name ends in Title/Label/Text/Hint/Tooltip/Message/Caption, or is listed in `copy_helpers` of `docs/ui-copy-rules.json` (`js:helper`); the argument of a call that `copy_calls` names, such as the text of `paintState(el, text)` (`js:call`).",
    "A literal with no space that reads as code (kebab-case, a dotted name, a path, a selector) is not copy.",
    "Pinned: a daemon test claim on the same asset (or on no named asset) with a literal that holds the text or one of its literal parts. Text of two words or more counts when it is equal, quoted (`\"…\"`, `'…'`, backticks, `>…<`), or (four words or more) anywhere in the literal. One word counts only next to its sink: `title: \"…\"`, `title=\"…\"`, `textContent = \"…\"`, `>…<`, or with its Alpine attribute in the same literal.",
];

const MISSES: &[&str] = &[
    "Text built in a variable before it reaches a sink.",
    "Default values in a function signature (`confirmLabel = \"Confirm\"`).",
    "Helpers written as `const x = () => …`, and label maps indexed by a key.",
    "`x-html`, `alt`, CSS `content:`, and the icon names of `data-lucide`.",
    "Text the daemon (Rust) produces and the UI shows verbatim. Out of scope for PRD #422.",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Kind {
    Text,
    Title,
    AriaLabel,
    Placeholder,
    Alpine,
    Toast,
    Confirm,
    Property,
    TextContent,
    Attribute,
    TermNotice,
    KeyBar,
    State,
    Helper,
    Const,
    Call,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Title => "title",
            Kind::AriaLabel => "aria-label",
            Kind::Placeholder => "placeholder",
            Kind::Alpine => "alpine",
            Kind::Toast => "js:toast",
            Kind::Confirm => "js:confirm",
            Kind::Property => "js:property",
            Kind::TextContent => "js:text-content",
            Kind::Attribute => "js:attribute",
            Kind::TermNotice => "js:term-notice",
            Kind::KeyBar => "js:key-bar",
            Kind::State => "js:state",
            Kind::Helper => "js:helper",
            Kind::Const => "js:const",
            Kind::Call => "js:call",
        }
    }
}

impl Serialize for Kind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.label())
    }
}

/// One text found in one source, before it is tied to a file.
#[derive(Debug)]
pub(crate) struct Found {
    text: String,
    line: usize,
    kind: Kind,
    concatenated: bool,
    area: Option<String>,
    /// The literal parts as written, which are what a test pins.
    fragments: Vec<String>,
    /// The source text just before a literal at its sink (`title: `,
    /// `title=`, `>`). A one-word text is only pinned in that context:
    /// `"stage"` alone also matches `data-act="stage"`.
    anchor: Option<String>,
    /// A text node that goes on with a sentence an inline element broke
    /// (`Click <b>run</b> to start one.`): its first letter is not a start.
    continues: bool,
}

#[derive(Serialize, Debug)]
pub(crate) struct Row {
    text: String,
    file: String,
    line: usize,
    kind: Kind,
    area: String,
    concatenated: bool,
    pinned_by: Vec<String>,
    #[serde(skip)]
    continues: bool,
}

pub fn ui_copy_cmd(args: &[String]) -> Result<()> {
    let mut root: Option<PathBuf> = None;
    let mut json = false;
    let mut lint = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--root" => root = Some(PathBuf::from(crate::next_value(&mut it, "--root")?)),
            "--json" => json = true,
            "--check" => lint = true,
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    if json && lint {
        anyhow::bail!("--check and --json cannot be used together");
    }
    let root = root.unwrap_or_else(asset_pins::repo_root);
    let rules = check::load(&root)?;
    let pins = asset_pins::gather(&root)?;
    let fns = CopyFns {
        helpers: &rules.copy_helpers,
        calls: &rules.copy_calls,
    };
    let rows = inventory(&root.join(UI_DIR), &pins, fns)?;
    if lint {
        let report = check::check(&rows, &rules);
        print!("{}", check::to_text(&report));
        if report.fails() {
            anyhow::bail!(
                "ui-copy found {} violations and {} stale exemptions",
                report.violations.len(),
                report.stale_count()
            );
        }
    } else if json {
        println!("{}", to_json(&rows)?);
    } else {
        print!("{}", to_markdown(&rows));
    }
    Ok(())
}

/// The sources: every page, `app.js` and the `wb-*.js` modules. The walk does
/// not recurse, so `vendor/` and `styles/` are never read.
fn sources(ui: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(ui).with_context(|| format!("listing {}", ui.display()))? {
        let entry = entry.with_context(|| format!("listing {}", ui.display()))?;
        if !entry.file_type().context("reading a file type")?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let wanted = name.ends_with(".html")
            || name == "app.js"
            || (name.starts_with("wb-") && name.ends_with(".js"));
        if wanted {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn inventory(ui: &Path, pins: &[Pin], fns: CopyFns) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for name in sources(ui)? {
        let path = ui.join(&name);
        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        rows.extend(rows_of(&name, &src, pins, fns));
    }
    Ok(rows)
}

/// Every row of one source file, in line order.
fn rows_of(name: &str, src: &str, pins: &[Pin], fns: CopyFns) -> Vec<Row> {
    let mut found = Vec::new();
    if name.ends_with(".html") {
        html::scan(src, fns, &mut found);
    } else {
        js::scan(src, 1, fns, &mut found);
    }
    found.sort_by_key(|f| f.line);
    found
        .into_iter()
        .map(|f| Row {
            pinned_by: pinned_by(name, &f, pins),
            text: f.text,
            file: name.to_string(),
            line: f.line,
            kind: f.kind,
            area: f.area.unwrap_or_default(),
            concatenated: f.concatenated,
            continues: f.continues,
        })
        .collect()
}

/// The claims that pin this text, as `file:line`.
fn pinned_by(file: &str, found: &Found, pins: &[Pin]) -> Vec<String> {
    // A whole text is a form only when it was written as one literal; a
    // concatenation is matched by its literal parts, spaces included.
    let mut forms: Vec<&str> = Vec::new();
    if !found.concatenated {
        forms.push(&found.text);
    }
    forms.extend(
        found
            .fragments
            .iter()
            .map(String::as_str)
            .filter(|f| f.trim().chars().count() >= 4 || !found.concatenated),
    );
    let mut sites: Vec<String> = pins
        .iter()
        .filter(|p| p.asset.as_deref().is_none_or(|a| a == file))
        .filter(|p| {
            let attributed = p.asset.is_some();
            p.needles.iter().any(|needle| {
                forms
                    .iter()
                    .any(|form| pins_form(needle, form, found.anchor.as_deref(), attributed))
            })
        })
        .map(|p| format!("{}:{}", p.file, p.line))
        .collect();
    sites.dedup();
    sites
}

/// Does `needle` pin the literal `form`? `attributed` says the claim names
/// the row's own file rather than no file at all.
fn pins_form(needle: &str, form: &str, anchor: Option<&str>, attributed: bool) -> bool {
    let words = form.split_whitespace().count();
    if words == 0 {
        return false;
    }
    let quoted = ['"', '\'', '`'].map(|q| format!("{q}{form}{q}"));
    if words >= 2 {
        return needle == form
            || quoted.iter().any(|q| needle.contains(q.as_str()))
            || needle.contains(&format!(">{form}<"))
            || (words >= 4 && needle.contains(form));
    }
    match anchor {
        Some(">") => needle.contains(&format!(">{form}<")),
        // An Alpine literal sits inside its attribute's expression, so the
        // attribute and the quoted word are both in the needle, not adjacent.
        Some(attr) if attr.ends_with("=\"") => {
            needle.contains(attr) && quoted.iter().any(|q| needle.contains(q.as_str()))
        }
        Some(anchor) => quoted
            .iter()
            .any(|q| needle.contains(&format!("{anchor}{q}"))),
        None => {
            attributed && (needle == form || quoted.iter().any(|q| needle.contains(q.as_str())))
        }
    }
}

pub(crate) fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The HTML entities this UI writes, decoded. An unknown one is left as is.
pub(crate) fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest
            .find(';')
            .filter(|&end| end <= 10)
            .map(|end| &rest[1..end]);
        let decoded = entity.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "hellip" => Some('…'),
            "middot" => Some('·'),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "times" => Some('×'),
            "larr" => Some('←'),
            "rarr" => Some('→'),
            "uarr" => Some('↑'),
            "darr" => Some('↓'),
            "bull" => Some('•'),
            "copy" => Some('©'),
            _ => {
                let num = e.strip_prefix('#')?;
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => num.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (entity, decoded) {
            (Some(e), Some(c)) => {
                out.push(c);
                rest = &rest[e.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Tally {
    rows: usize,
    concatenated: usize,
    pinned: usize,
}

impl Tally {
    fn add(&mut self, row: &Row) {
        self.rows += 1;
        self.concatenated += usize::from(row.concatenated);
        self.pinned += usize::from(!row.pinned_by.is_empty());
    }
}

fn tallies<'a, K: Ord>(rows: &'a [Row], key: impl Fn(&'a Row) -> K) -> BTreeMap<K, Tally> {
    let mut out: BTreeMap<K, Tally> = BTreeMap::new();
    for row in rows {
        out.entry(key(row)).or_default().add(row);
    }
    out
}

fn to_markdown(rows: &[Row]) -> String {
    let cell = |s: &str| s.replace('|', "\\|");
    let mut md: Vec<String> = vec![
        "# UI copy inventory\n".into(),
        "Generated by `cargo run -p xtask -- ui-copy`. The extraction is heuristic: \
         these are its rules, and after them the text it knowingly misses.\n"
            .into(),
    ];
    md.extend(RULES.iter().map(|rule| format!("- {rule}")));
    md.push("\nNot found:\n".into());
    md.extend(MISSES.iter().map(|miss| format!("- {miss}")));

    let concatenated = rows.iter().filter(|r| r.concatenated).count();
    let pinned = rows.iter().filter(|r| !r.pinned_by.is_empty()).count();
    md.push(format!(
        "\n## Totals\n\n{} rows, {concatenated} concatenated, {pinned} pinned by a daemon test.\n",
        rows.len()
    ));
    for (title, table) in [
        ("kind", tallies(rows, |r| r.kind.label())),
        ("file", tallies(rows, |r| r.file.as_str())),
    ] {
        md.push(format!(
            "| {title} | rows | concatenated | pinned |\n|---|---:|---:|---:|"
        ));
        for (key, t) in &table {
            md.push(format!(
                "| {key} | {} | {} | {} |",
                t.rows, t.concatenated, t.pinned
            ));
        }
        md.push(String::new());
    }

    let mut current: Option<(&str, &str)> = None;
    for row in rows {
        if current.map(|c| c.0) != Some(row.file.as_str()) {
            md.push(format!("\n## {}", row.file));
            current = None;
        }
        if current != Some((row.file.as_str(), row.area.as_str())) {
            let area = if row.area.is_empty() {
                "(top level)"
            } else {
                &row.area
            };
            md.push(format!(
                "\n### {}\n\n| line | kind | text | concat | pinned by |\n|---:|---|---|---|---|",
                cell(area)
            ));
            current = Some((&row.file, &row.area));
        }
        md.push(format!(
            "| {} | {} | {} | {} | {} |",
            row.line,
            row.kind.label(),
            cell(&row.text),
            if row.concatenated { "yes" } else { "" },
            row.pinned_by.join(", ")
        ));
    }
    let mut out = md.join("\n");
    out.push('\n');
    out
}

fn to_json(rows: &[Row]) -> Result<String> {
    let counts = |table: BTreeMap<&str, Tally>| -> BTreeMap<String, usize> {
        table
            .into_iter()
            .map(|(k, t)| (k.to_string(), t.rows))
            .collect()
    };
    let doc = serde_json::json!({
        "rules": RULES,
        "misses": MISSES,
        "totals": {
            "rows": rows.len(),
            "concatenated": rows.iter().filter(|r| r.concatenated).count(),
            "pinned": rows.iter().filter(|r| !r.pinned_by.is_empty()).count(),
            "by_kind": counts(tallies(rows, |r| r.kind.label())),
            "by_file": counts(tallies(rows, |r| r.file.as_str())),
        },
        "rows": rows,
    });
    serde_json::to_string_pretty(&doc).context("serializing the inventory")
}

#[cfg(test)]
mod tests;
