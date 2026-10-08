//! The contract between the browser and the daemon (ADR-0070 Compliance).
//!
//! - Shared replies: every message type the UI reads, and how many of them
//!   have no reply file under `crates/ralphy-daemon/ui-tests/fixtures/`.
//!   Each ok reply file carries its JSON in the field the daemon uses.
//! - Error literals: every string the UI compares with a reply's `reason`,
//!   `message` or `state` is a literal in the daemon's or the CLI's Rust code.
//! - Mirrored limits: each value the UI repeats equals its Rust constant.
//!
//! Each check reads both sides as text, like `ratchets.rs`: xtask may not
//! depend on the daemon.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Message types with no shared reply file. Measured on 4e092da4 + the
/// registry verbs: 51 registry verbs + 30 UI routes + 10 pushes = 91 types, 9
/// of them covered. Counting rules are in [`message_types`]. A new type starts with a
/// shared reply; a new reply file lowers this number.
const UNSHARED_BASELINE: usize = 80;

const FIXTURES: &str = "crates/ralphy-daemon/ui-tests/fixtures";

/// Reasons the UI makes itself, with where. Each one must NOT be a Rust
/// literal (an exemption cannot hide a producer) and must be produced in JS.
const UI_OWNED: &[(&str, &str)] = &[(
    "transport",
    "app.ts refuse(\"transport\"): the browser could not reach the daemon",
)];

/// Measured 28 distinct literals on this change (23 `CAUSE` keys and 5
/// compares); the floor keeps a margin. Fewer means the scan stopped reading
/// the UI.
const LITERAL_FLOOR: usize = 25;

#[test]
fn message_types_without_a_shared_reply_match_the_baseline() {
    let root = workspace_root();
    let types = message_types(
        &read(&root.join("crates/ralphy-daemon/src/dispatch.rs")),
        &router_text(&root),
        &routes_production_text(&root),
    );
    let covered = covered(&root);

    let stray: Vec<&String> = covered.difference(&types).collect();
    assert!(
        stray.is_empty(),
        "these fixtures name no message type the daemon serves: {stray:?}"
    );

    let tests = ui_test_text(&root);
    let orphans: Vec<String> = fixture_names(&root)
        .into_iter()
        .filter(|name| !tests.contains(&format!("fixture(\"{name}\")")))
        .collect();
    assert!(
        orphans.is_empty(),
        "no ui-tests/*.test.mjs reads these fixtures: {orphans:?}"
    );

    let unshared: Vec<&String> = types.difference(&covered).collect();
    assert_eq!(
        unshared.len(),
        UNSHARED_BASELINE,
        "message types without a shared reply changed. A new verb, route or push \
         starts with a fixture; when a fixture is added, lower UNSHARED_BASELINE. \
         Without a reply: {unshared:?}"
    );
}

#[test]
fn message_type_scan_finds_verbs_routes_and_pushes() {
    let dispatch = r#"
    pub fn from_query(value: &str) -> Option<BranchMode> {
        match value {
            "new" => Some(BranchMode::New),
            _ => None,
        }
    }
    pub fn from_query(value: &str) -> Option<Verb> {
        match value {
            "a.b" => Some(Verb::A),
            "c.d" => Some(Verb::C),
            _ => None,
        }
    }
"#;
    let routes = r#"Router::new()
        .route("/api/x", get(x))
        .route(
            "/api/x",
            put(y),
        )
        .route("/api/peer/y", post(z))"#;
    let pushes = r#"send("tree.dirty"); send("tree.failed"); let n = "not.a.push";"#;
    let found: Vec<String> = message_types(dispatch, routes, pushes)
        .into_iter()
        .collect();
    assert_eq!(found, ["a.b", "api-x", "c.d", "tree.dirty", "tree.failed"]);

    let root = workspace_root();
    let dispatch = read(&root.join("crates/ralphy-daemon/src/dispatch.rs"));
    let routes = router_text(&root);
    let pushes = routes_production_text(&root);
    assert!(verbs(&dispatch).len() >= 40, "the verb scan is blind");
    assert!(ui_routes(&routes).len() >= 20, "the route scan is blind");
    assert!(push_types(&pushes).len() >= 5, "the push scan is blind");
}

/// The message types the UI reads:
/// - verbs: the string keys of `Verb::from_query` in `dispatch.rs`;
/// - routes: the `.route("…")` paths of the router without `/api/peer/*`
///   (daemon to daemon), one type per path, named without the leading `/` and
///   with `/` as `-`;
/// - pushes: the `<word>.dirty`, `<word>.failed`, `session-open` and
///   `session-end` literals of the routes' production code.
fn message_types(dispatch: &str, routes: &str, pushes: &str) -> BTreeSet<String> {
    let mut all = verbs(dispatch);
    all.extend(ui_routes(routes));
    all.extend(push_types(pushes));
    all
}

fn verbs(dispatch: &str) -> BTreeSet<String> {
    let head = "fn from_query(value: &str) -> Option<Verb>";
    let start = dispatch
        .find(head)
        .expect("dispatch.rs has Verb::from_query");
    let body = &dispatch[start..];
    let end = body
        .find("_ => None")
        .expect("from_query ends with `_ => None`");
    let arm = Regex::new(r#""([^"]+)"\s*=>\s*Some\("#).expect("a valid regex");
    arm.captures_iter(&body[..end])
        .map(|c| c[1].to_string())
        .collect()
}

fn ui_routes(routes: &str) -> BTreeSet<String> {
    let route = Regex::new(r#"\.route\(\s*"([^"]+)""#).expect("a valid regex");
    route
        .captures_iter(routes)
        .map(|c| c[1].to_string())
        .filter(|path| !path.starts_with("/api/peer/"))
        .map(|path| path.trim_start_matches('/').replace('/', "-"))
        .collect()
}

fn push_types(text: &str) -> BTreeSet<String> {
    let push = Regex::new(r#""([a-z_]+\.dirty|[a-z_]+\.failed|session-open|session-end)""#)
        .expect("a valid regex");
    push.captures_iter(text).map(|c| c[1].to_string()).collect()
}

/// The text the router's paths are in: `routes.rs` builds the router, and each
/// area module under `routes/` registers its own routes.
fn router_text(root: &Path) -> String {
    let routes = read(&root.join("crates/ralphy-daemon/src/routes.rs"));
    format!(
        "{routes}
{}",
        routes_production_text(root)
    )
}

/// The production text of every file under `crates/ralphy-daemon/src/routes/`.
fn routes_production_text(root: &Path) -> String {
    let mut files = Vec::new();
    collect_rs(&root.join("crates/ralphy-daemon/src/routes"), &mut files);
    files.sort();
    files
        .iter()
        .map(|f| production(&read(f)).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The fixture file names, without `.json`.
fn fixture_names(root: &Path) -> BTreeSet<String> {
    let dir = root.join(FIXTURES);
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter_map(|name| name.strip_suffix(".json").map(str::to_string))
        .collect()
}

/// The message types with a fixture: a name without its `--<variant>`.
fn covered(root: &Path) -> BTreeSet<String> {
    fixture_names(root)
        .into_iter()
        .map(|name| match name.split_once("--") {
            Some((kind, _variant)) => kind.to_string(),
            None => name,
        })
        .collect()
}

fn ui_test_text(root: &Path) -> String {
    let dir = root.join("crates/ralphy-daemon/ui-tests");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
                .path()
        })
        .filter(|p| p.to_string_lossy().ends_with(".test.mjs"))
        .collect();
    files.sort();
    files.iter().map(|f| read(f)).collect::<Vec<_>>().join("\n")
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
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

/// `mod x {` or `pub(crate) mod x {`. A `mod x;` declaration is not a cut.
fn opens_inline_mod(line: &str) -> bool {
    let head = line.trim();
    let head = head.strip_prefix("pub(crate) ").unwrap_or(head);
    head.starts_with("mod ") && head.ends_with('{')
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
            if name != "tests" {
                collect_rs(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_test_child.rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_error_literal_the_ui_compares_is_produced_by_rust() {
    let root = workspace_root();
    let ui = ui_sources(&root);
    let mut found: Vec<(String, String)> = Vec::new();
    for (file, text) in &ui {
        for lit in ui_literals(text) {
            found.push((lit, file.clone()));
        }
    }
    let distinct: BTreeSet<&str> = found.iter().map(|(lit, _)| lit.as_str()).collect();
    assert!(
        distinct.len() >= LITERAL_FLOOR,
        "the scan found only {} literals: it is not reading the UI",
        distinct.len()
    );

    let corpus = rust_corpus(&root);
    let owned: BTreeSet<&str> = UI_OWNED.iter().map(|(lit, _)| *lit).collect();
    let checked: Vec<(String, String)> = found
        .iter()
        .filter(|(lit, _)| !owned.contains(lit.as_str()))
        .cloned()
        .collect();
    let missing = missing_literals(&checked, &corpus);
    assert!(
        missing.is_empty(),
        "the UI compares these strings, but no daemon or CLI Rust code produces them: {missing:?}"
    );

    let js: String = ui.iter().map(|(_, t)| t.as_str()).collect();
    for (lit, why) in UI_OWNED {
        assert!(
            !corpus.contains(&format!("\"{lit}\"")),
            "{lit:?} is produced by Rust now: remove it from UI_OWNED ({why})"
        );
        assert!(
            js.contains(&format!("refuse(\"{lit}\")"))
                || js.contains(&format!("reason: \"{lit}\"")),
            "{lit:?} is in UI_OWNED but no UI code produces it ({why})"
        );
    }
}

#[test]
fn a_ui_literal_no_rust_code_produces_is_reported() {
    let js = "if (reason === \"gone away\") x();\n\
              if (reply.message !== \"refused\") y();\n\
              if (typeof reply.message === \"string\") u();\n\
              if (iss.reason === \"not_planned\") t();\n\
              if (WBFail.message(reply, \"\") === \"not found\") z();\n\
              if (key?.state === \"unknown\") w();\n\
              if (/no such/i.test(reason)) v();\n\
              const CAUSE = {\n  \"lost\": \"y\",\n  bare: \"z\",\n};";
    let found: Vec<String> = ui_literals(js).into_iter().collect();
    assert_eq!(
        found,
        [
            "bare",
            "gone away",
            "lost",
            "no such",
            "not found",
            "refused",
            "unknown"
        ]
    );

    let pairs: Vec<(String, String)> = found
        .iter()
        .map(|lit| (lit.clone(), "t.js".to_string()))
        .collect();
    let corpus = r#"bail!("lost"); "bare" "no such" "not found" "refused" "unknown""#;
    assert_eq!(missing_literals(&pairs, corpus), ["gone away (t.js)"]);
}

/// The strings a UI file compares with a reply's `reason`, `message` or
/// `state`, plus the keys of the `CAUSE` (wb-fail.ts) and `REFUSAL_TEXT`
/// (wb-viewer.ts) tables.
fn ui_literals(js: &str) -> BTreeSet<String> {
    // A bare `reason`/`message` or one read off `reply`. Not `typeof x ===
    // "string"`, and not another object's field: `iss.reason` is the forge's
    // close reason of an issue row, not a reply's error.
    let field =
        Regex::new(r#"(typeof\s+)?\b(?:(\w+)\??\.)?(?:reason|message)\s*[!=]==\s*"([^"]+)""#)
            .expect("a valid regex");
    let mut out: BTreeSet<String> = field
        .captures_iter(js)
        .filter(|c| c.get(1).is_none() && c.get(2).is_none_or(|r| r.as_str() == "reply"))
        .map(|c| c[3].to_string())
        .collect();
    let compares = [
        r#"WBFail\.message\([^)]*\)\s*[!=]==\s*"([^"]+)""#,
        r#"\b(?:key|body|reply|group|h)\??\.state\s*[!=]==\s*"([^"]+)""#,
        r#"/([^/\\]+)/i?\.test\((?:reason|message)\)"#,
    ];
    for pattern in compares {
        let re = Regex::new(pattern).expect("a valid regex");
        out.extend(re.captures_iter(js).map(|c| c[1].to_string()));
    }
    let key = Regex::new(r#"^\s*(?:"([^"]+)"|([A-Za-z_]\w*))\s*:"#).expect("a valid regex");
    // A module types the table (`const CAUSE: Record<string, string> = {`).
    let table =
        Regex::new(r"const (?:CAUSE|REFUSAL_TEXT)(?::[^=\n]*)? = \{").expect("a valid regex");
    for found in table.find_iter(js) {
        let body = &js[found.end()..];
        let body = &body[..body.find("};").unwrap_or(body.len())];
        for line in body.lines() {
            if let Some(c) = key.captures(line) {
                let k = c.get(1).or_else(|| c.get(2)).expect("one group matched");
                out.insert(k.as_str().to_string());
            }
        }
    }
    out
}

/// `literal (file)` for each literal that is not a quoted string of `corpus`.
fn missing_literals(found: &[(String, String)], corpus: &str) -> Vec<String> {
    let mut missing: Vec<String> = found
        .iter()
        .filter(|(lit, _)| !corpus.contains(&format!("\"{lit}\"")))
        .map(|(lit, file)| format!("{lit} ({file})"))
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// `(file name, text)` of every UI script, without the vendored libraries:
/// the `.ts` modules (ADR-0075) and any first-party `.js` file, not the `.d.ts`
/// type files.
fn ui_sources(root: &Path) -> Vec<(String, String)> {
    let dir = root.join("crates/ralphy-daemon/assets/ui");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
                .path()
        })
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            p.is_file()
                && (name.ends_with(".js") || (name.ends_with(".ts") && !name.ends_with(".d.ts")))
        })
        .collect();
    files.sort();
    files
        .iter()
        .map(|f| {
            let name = f.file_name().expect("a file has a name").to_string_lossy();
            (name.into_owned(), read(f))
        })
        .collect()
}

/// The production text of the daemon's and the CLI's Rust code, without
/// comment lines: a word in a doc comment produces nothing.
fn rust_corpus(root: &Path) -> String {
    let mut files = Vec::new();
    collect_rs(&root.join("crates/ralphy-daemon/src"), &mut files);
    collect_rs(&root.join("crates/ralphy-cli/src"), &mut files);
    files.sort();
    files
        .iter()
        .flat_map(|f| {
            production(&read(f))
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A limit the UI repeats, and the Rust constant it repeats:
/// `(js file, js name, rust file, rust name)`, files under
/// `crates/ralphy-daemon/assets/ui/` and `crates/ralphy-daemon/src/`.
///
/// Not here, because one side has no named value: the note size cap
/// (`tree::MAX_READ_BYTES`, no JS copy), `FENCE_NAME_MAX` in wb-console.ts
/// (no Rust copy), and the file-search `MAX_HITS` (a field of the Rust
/// `SearchBudget::default` literal, not a constant).
const MIRRORS: &[(&str, &str, &str, &str)] = &[
    ("wb-console.ts", "DESK_MAX", "desk.rs", "DESK_MAX"),
    ("wb-console.ts", "FENCE_MAX", "desk.rs", "FENCE_MAX"),
    ("wb-console.ts", "NOTE_MAX", "desk.rs", "NOTE_MAX"),
    (
        "wb-console-name.ts",
        "NAME_MAX",
        "desk.rs",
        "CONSOLE_NAME_MAX",
    ),
    (
        "wb-console-input.ts",
        "IMAGE_PASTE_MAX",
        "tree.rs",
        "MAX_IMAGE_BYTES",
    ),
    (
        "wb-console-session.ts",
        "TAG_TERMINAL",
        "protocol.rs",
        "TAG_TERMINAL",
    ),
    (
        "wb-console-session.ts",
        "TAG_COMMAND",
        "protocol.rs",
        "TAG_COMMAND",
    ),
    (
        "wb-daemon.ts",
        "TAG_TERMINAL",
        "protocol.rs",
        "TAG_TERMINAL",
    ),
    ("wb-daemon.ts", "TAG_COMMAND", "protocol.rs", "TAG_COMMAND"),
    (
        "wb-daemon.ts",
        "TAG_PRESENCE",
        "protocol.rs",
        "TAG_PRESENCE",
    ),
    ("app.ts", "PROTECTED_DIRS", "fswrite.rs", "PROTECTED_DIRS"),
    (
        "wb-file-search.ts",
        "MIN_CHARS",
        "tree/search.rs",
        "MIN_QUERY_CHARS",
    ),
    ("wb-notes.ts", "DEFAULT_DIR", "note.rs", "DIR"),
];

/// The note exception to the write denylist, as a pair of functions
/// `(js file, js function, rust file, rust function)`: both bodies must hold
/// the same path words.
const NOTE_EXCEPTION: (&str, &str, &str, &str) = (
    "app.ts",
    "isNoteInNotesDir",
    "fswrite.rs",
    "is_note_in_notes_dir",
);

/// String literals of the JS body that only split the path, not words of it.
const JS_PATH_PLUMBING: &[&str] = &["/", "."];

#[test]
fn every_mirrored_limit_equals_its_rust_constant() {
    let root = workspace_root();
    let ui = root.join("crates/ralphy-daemon/assets/ui");
    let src = root.join("crates/ralphy-daemon/src");
    let mut errors = Vec::new();
    for (js_file, js_name, rs_file, rs_name) in MIRRORS {
        let js = js_value(&read(&ui.join(js_file)), js_name);
        let rs = rust_value(&read(&src.join(rs_file)), rs_name);
        let at = format!("{js_file} {js_name} / {rs_file} {rs_name}");
        match (js, rs) {
            (None, _) => errors.push(format!("{at}: {js_name} not found in {js_file}")),
            (_, None) => errors.push(format!("{at}: {rs_name} not found in {rs_file}")),
            (Some(j), Some(r)) => match (eval(&j), eval(&r)) {
                (Some(jv), Some(rv)) if jv == rv => {}
                (jv, rv) => errors.push(format!("{at}: {j} ({jv:?}) != {r} ({rv:?})")),
            },
        }
    }

    let (js_file, js_fn, rs_file, rs_fn) = NOTE_EXCEPTION;
    let js: BTreeSet<String> = fn_body_strings(&read(&ui.join(js_file)), js_fn)
        .into_iter()
        .filter(|s| !JS_PATH_PLUMBING.contains(&s.as_str()))
        .collect();
    let rs = fn_body_strings(&read(&src.join(rs_file)), rs_fn);
    if js.is_empty() || js != rs {
        errors.push(format!(
            "{js_file} {js_fn} {js:?} != {rs_file} {rs_fn} {rs:?}"
        ));
    }

    assert!(
        errors.is_empty(),
        "a value the UI repeats differs from the daemon's:\n{}",
        errors.join("\n")
    );
}

#[test]
fn a_mirrored_value_that_differs_or_is_missing_is_reported() {
    let js = js_value("  const DESK_MAX = 25;", "DESK_MAX").unwrap();
    let rs = rust_value("pub const DESK_MAX: usize = 24;", "DESK_MAX").unwrap();
    assert_ne!(eval(&js), eval(&rs));
    assert_eq!(eval(&rs), Some(Mirrored::Int(24)));
    assert_eq!(js_value("", "X"), None);
    assert_eq!(rust_value("const XY: u8 = 1;", "X"), None);
    assert_eq!(eval("4 * 1024 * 1024"), Some(Mirrored::Int(4_194_304)));
    assert_eq!(eval("0x03"), Some(Mirrored::Int(3)));
    assert_eq!(eval("2_usize"), Some(Mirrored::Int(2)));
    assert_eq!(
        eval(r#"[".git", ".ralphy"]"#),
        Some(Mirrored::Strs(vec![".git".into(), ".ralphy".into()]))
    );
    assert_eq!(
        eval(r#"".ralphy/notes""#),
        Some(Mirrored::Strs(vec![".ralphy/notes".into()]))
    );
    assert_eq!(eval("x + 1"), None);

    let js = "function f(a) {\n  return a === \"x\" && g(\"y\");\n}\nconst z = \"no\";\n";
    let rs = "fn f(a: &str) -> bool {\n    if a == \"x\" {\n        return g(\"y\");\n    }\n    false\n}\nconst Z: &str = \"no\";\n";
    let want: BTreeSet<String> = ["x", "y"].map(String::from).into();
    assert_eq!(fn_body_strings(js, "f"), want);
    assert_eq!(fn_body_strings(rs, "f"), want);
    assert!(fn_body_strings(js, "missing").is_empty());
}

#[derive(Debug, PartialEq, Eq)]
enum Mirrored {
    Int(u64),
    Strs(Vec<String>),
}

/// The expression of `const NAME = …;` in a JS file.
fn js_value(src: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r"\bconst\s+{name}\s*=\s*([^;]+);")).expect("a valid regex");
    re.captures(src).map(|c| c[1].trim().to_string())
}

/// The expression of `const NAME: T = …;` in a Rust file.
fn rust_value(src: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r"\bconst\s+{name}\s*:[^=]+=\s*([^;]+);")).expect("a valid regex");
    re.captures(src).map(|c| c[1].trim().to_string())
}

/// An integer that is a product of decimal or hex literals, a quoted string,
/// or an array of quoted strings. Anything else is `None`.
fn eval(expr: &str) -> Option<Mirrored> {
    let expr = expr.trim();
    if expr.starts_with('"') || expr.starts_with('[') {
        let body = expr
            .strip_prefix('[')
            .map_or(expr, |e| e.trim_end_matches(']'));
        let quoted = Regex::new(r#""([^"]*)""#).expect("a valid regex");
        let strs: Vec<String> = quoted
            .captures_iter(body)
            .map(|c| c[1].to_string())
            .collect();
        let rest = quoted.replace_all(body, "");
        if rest.chars().any(|c| !(c == ',' || c.is_whitespace())) {
            return None;
        }
        return Some(Mirrored::Strs(strs));
    }
    let suffix = Regex::new(r"_?(usize|u64|u32|u16|u8)$").expect("a valid regex");
    let mut product: u64 = 1;
    for factor in expr.split('*') {
        let factor = suffix.replace(factor.trim(), "").replace('_', "");
        let value = match factor.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).ok()?,
            None => factor.parse().ok()?,
        };
        product = product.checked_mul(value)?;
    }
    Some(Mirrored::Int(product))
}

/// The string literals in the body of function `name` (`function name(` or
/// `fn name(`): from its header line to the `}` line at the header's indent.
fn fn_body_strings(src: &str, name: &str) -> BTreeSet<String> {
    let mut lines = src.lines();
    let Some(header) = lines.by_ref().find(|l| {
        let t = l.trim_start();
        t.starts_with(&format!("function {name}(")) || t.contains(&format!("fn {name}("))
    }) else {
        return BTreeSet::new();
    };
    let indent = &header[..header.len() - header.trim_start().len()];
    let close = format!("{indent}}}");
    let quoted = Regex::new(r#""([^"]*)""#).expect("a valid regex");
    let mut out = BTreeSet::new();
    for line in std::iter::once(header).chain(lines.take_while(|l| l.trim_end() != close)) {
        out.extend(quoted.captures_iter(line).map(|c| c[1].to_string()));
    }
    out
}

/// Verbs whose ok reply is flat, as the ADR-0036 amendment "the registry
/// verbs" fixes it, and the daemon file that writes each key: a `"key"`
/// literal or a `pub key:` field of the serialized struct.
const FLAT_REPLIES: &[(&str, &str)] = &[
    ("dir.list", "crates/ralphy-daemon/src/dir_list.rs"),
    (
        "project.add",
        "crates/ralphy-daemon/src/routes/ws_command/registry_verbs.rs",
    ),
];

/// The daemon files that name the field a Query reply carries its JSON in.
const REPLY_FIELD_SOURCES: &[&str] = &[
    "crates/ralphy-daemon/src/routes/ws_command/oneshot.rs",
    "crates/ralphy-daemon/src/routes/ws_command/host.rs",
];

#[test]
fn every_ok_fixture_carries_its_json_in_the_field_the_daemon_uses() {
    let root = workspace_root();
    let dispatch = read(&root.join("crates/ralphy-daemon/src/dispatch.rs"));
    let daemon: String = REPLY_FIELD_SOURCES
        .iter()
        .map(|f| read(&root.join(f)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut checked = 0;
    for name in fixture_names(&root) {
        let path = root.join(FIXTURES).join(format!("{name}.json"));
        let reply: serde_json::Value = serde_json::from_str(&read(&path))
            .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
        if reply["status"] != "ok" {
            continue;
        }
        let verb = name.split_once("--").map_or(name.as_str(), |(v, _)| v);
        if let Some((_, source)) = FLAT_REPLIES.iter().find(|(v, _)| *v == verb) {
            let src = read(&root.join(source));
            for key in reply.as_object().expect("an ok reply is an object").keys() {
                assert!(
                    key == "status"
                        || src.contains(&format!("\"{key}\""))
                        || src.contains(&format!("pub {key}:")),
                    "{name}.json: `{key}` is not a field {source} writes"
                );
            }
            checked += 1;
            continue;
        }
        let variant = verb_variant(&dispatch, verb)
            .unwrap_or_else(|| panic!("{verb} is not a verb of Verb::from_query"));
        let field = reply_field(&daemon, &variant)
            .unwrap_or_else(|| panic!("the daemon names no reply field for Verb::{variant}"));
        let keys: Vec<&String> = reply
            .as_object()
            .expect("an ok reply is an object")
            .keys()
            .filter(|k| *k != "status")
            .collect();
        assert_eq!(
            keys,
            [&field],
            "{name}.json: the daemon sends {verb} in `{field}`"
        );
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} ok fixtures were checked");
}

#[test]
fn reply_field_scan_reads_both_arm_shapes() {
    let dispatch = r#"fn from_query(value: &str) -> Option<Verb> {
        match value {
            "board.list" => Some(Verb::BoardList),
            _ => None,"#;
    assert_eq!(
        verb_variant(dispatch, "board.list").as_deref(),
        Some("BoardList")
    );
    assert_eq!(verb_variant(dispatch, "nope"), None);
    let daemon = r#"
        dispatch::Verb::BoardList => (Ok(dispatch::board_argv()), "board"),
        Verb::HostKey => Some("key"),"#;
    assert_eq!(reply_field(daemon, "BoardList").as_deref(), Some("board"));
    assert_eq!(reply_field(daemon, "HostKey").as_deref(), Some("key"));
    assert_eq!(reply_field(daemon, "Board"), None);
}

/// The `Verb` variant `Verb::from_query` maps `verb` to.
fn verb_variant(dispatch: &str, verb: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#""{}"\s*=>\s*Some\(Verb::(\w+)\)"#,
        regex::escape(verb)
    ))
    .expect("a valid regex");
    re.captures(dispatch).map(|c| c[1].to_string())
}

/// The last string literal on the match arm of `Verb::<variant>`.
fn reply_field(daemon: &str, variant: &str) -> Option<String> {
    let arm = Regex::new(&format!(r"\bVerb::{variant}\s*=>")).expect("a valid regex");
    let quoted = Regex::new(r#""(\w+)""#).expect("a valid regex");
    let line = daemon.lines().find(|l| arm.is_match(l))?;
    quoted.captures_iter(line).last().map(|c| c[1].to_string())
}
