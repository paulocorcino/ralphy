//! Each limit the UI repeats equals its Rust constant.

use super::*;

/// A limit the UI repeats, and the Rust constant it repeats:
/// `(js file, js name, rust file, rust name)`, files under
/// `crates/ralphy-daemon/assets/ui/` and `crates/ralphy-daemon/src/`.
///
/// Not here, because one side has no named value: the note size cap
/// (`tree::MAX_READ_BYTES`, no JS copy), `FENCE_NAME_MAX` in wb-console.ts
/// (no Rust copy), and the file-search `MAX_HITS` (a field of the Rust
/// `SearchBudget::default` literal, not a constant).
const MIRRORS: &[(&str, &str, &str, &str)] = &[
    ("wb-desk-folds.ts", "DESK_MAX", "desk.rs", "DESK_MAX"),
    ("wb-desk-folds.ts", "FENCE_MAX", "desk.rs", "FENCE_MAX"),
    ("wb-desk-folds.ts", "NOTE_MAX", "desk.rs", "NOTE_MAX"),
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
    (
        "wb-file-paths.ts",
        "PROTECTED_DIRS",
        "fswrite.rs",
        "PROTECTED_DIRS",
    ),
    (
        "wb-file-search.ts",
        "MIN_CHARS",
        "tree/search.rs",
        "MIN_QUERY_CHARS",
    ),
    ("wb-notes-folds.ts", "DEFAULT_DIR", "note.rs", "DIR"),
];

/// The note exception to the write denylist, as a pair of functions
/// `(js file, js function, rust file, rust function)`: both bodies must hold
/// the same path words.
const NOTE_EXCEPTION: (&str, &str, &str, &str) = (
    "wb-file-paths.ts",
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
