//! The contract between the browser and the daemon (ADR-0070 Compliance).
//!
//! - Shared replies: every message type the UI reads, and how many of them
//!   have no reply file under `crates/ralphy-daemon/ui-tests/fixtures/`.
//! - Error literals: every string the UI compares with a reply's `reason`,
//!   `message` or `state` is a literal in the daemon's or the CLI's Rust code.
//!
//! Each check reads both sides as text, like `ratchets.rs`: xtask may not
//! depend on the daemon.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Message types with no shared reply file. Measured on 4e092da4 + this
/// change: 49 registry verbs + 30 UI routes + 10 pushes = 89 types, 7 of them
/// covered. Counting rules are in [`message_types`]. A new type starts with a
/// shared reply; a new reply file lowers this number.
const UNSHARED_BASELINE: usize = 82;

const FIXTURES: &str = "crates/ralphy-daemon/ui-tests/fixtures";

/// Reasons the UI makes itself, with where. Each one must NOT be a Rust
/// literal (an exemption cannot hide a producer) and must be produced in JS.
const UI_OWNED: &[(&str, &str)] = &[(
    "transport",
    "app.js refuse(\"transport\"): the browser could not reach the daemon",
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
        &read(&root.join("crates/ralphy-daemon/src/routes.rs")),
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
        .filter(|name| !tests.contains(&format!("\"{name}\"")))
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
    let pushes = r#"send("tree.dirty"); let n = "not.a.push";"#;
    let found: Vec<String> = message_types(dispatch, routes, pushes)
        .into_iter()
        .collect();
    assert_eq!(found, ["a.b", "api-x", "c.d", "tree.dirty"]);

    let root = workspace_root();
    let dispatch = read(&root.join("crates/ralphy-daemon/src/dispatch.rs"));
    let routes = read(&root.join("crates/ralphy-daemon/src/routes.rs"));
    let pushes = routes_production_text(&root);
    assert!(verbs(&dispatch).len() >= 40, "the verb scan is blind");
    assert!(ui_routes(&routes).len() >= 20, "the route scan is blind");
    assert!(push_types(&pushes).len() >= 5, "the push scan is blind");
}

/// The message types the UI reads:
/// - verbs: the string keys of `Verb::from_query` in `dispatch.rs`;
/// - routes: the `.route("…")` paths of `routes.rs` without `/api/peer/*`
///   (daemon to daemon), one type per path, named without the leading `/` and
///   with `/` as `-`;
/// - pushes: the `<word>.dirty`, `session-open` and `session-end` literals of
///   the routes' production code.
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
    let push = Regex::new(r#""([a-z_]+\.dirty|session-open|session-end)""#).expect("a valid regex");
    push.captures_iter(text).map(|c| c[1].to_string()).collect()
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
/// `state`, plus the keys of the `CAUSE` (wb-fail.js) and `REFUSAL_TEXT`
/// (wb-viewer.js) tables.
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
        r#"\b(?:key|body|reply)\??\.state\s*[!=]==\s*"([^"]+)""#,
        r#"/([^/\\]+)/i?\.test\((?:reason|message)\)"#,
    ];
    for pattern in compares {
        let re = Regex::new(pattern).expect("a valid regex");
        out.extend(re.captures_iter(js).map(|c| c[1].to_string()));
    }
    let key = Regex::new(r#"^\s*(?:"([^"]+)"|([A-Za-z_]\w*))\s*:"#).expect("a valid regex");
    for table in ["const CAUSE = {", "const REFUSAL_TEXT = {"] {
        let Some(start) = js.find(table) else {
            continue;
        };
        let body = &js[start + table.len()..];
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

/// `(file name, text)` of every UI script, without the vendored libraries.
fn ui_sources(root: &Path) -> Vec<(String, String)> {
    let dir = root.join("crates/ralphy-daemon/assets/ui");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
                .path()
        })
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "js"))
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

/// The production text of the daemon's and the CLI's Rust code.
fn rust_corpus(root: &Path) -> String {
    let mut files = Vec::new();
    collect_rs(&root.join("crates/ralphy-daemon/src"), &mut files);
    collect_rs(&root.join("crates/ralphy-cli/src"), &mut files);
    files.sort();
    files
        .iter()
        .map(|f| production(&read(f)).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
