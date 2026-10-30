//! Every error literal the UI compares is produced by Rust code.

use super::*;

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
/// (wb-file-viewer-read-only.ts) tables.
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
