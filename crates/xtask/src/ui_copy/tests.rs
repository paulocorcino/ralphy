use super::*;

pub(super) fn rows(name: &str, src: &str) -> Vec<Row> {
    rows_of(name, src, &[], CopyFns::default())
}

/// `(kind, text)` of every row, for compact assertions.
pub(super) fn seen(rows: &[Row]) -> Vec<(&'static str, &str)> {
    rows.iter()
        .map(|r| (r.kind.label(), r.text.as_str()))
        .collect()
}

mod html_rows;
mod js_rows;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xtask-ui-copy-{}-{tag}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clearing the scratch dir");
    }
    dir
}

fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    let parent = path.parent().expect("a fixture path has a parent");
    std::fs::create_dir_all(parent).expect("creating a fixture dir");
    std::fs::write(&path, text).expect("writing a fixture");
}

/// The whole path: files discovered, `vendor/` never read, and the pinned
/// cross-reference built from `asset_pins::gather` over a fixture test file.
#[test]
fn the_inventory_skips_vendor_and_cross_references_the_pins() {
    let root = scratch("inventory");
    put(
        &root,
        "crates/ralphy-daemon/src/tests.rs",
        r##"
    fn t() {
        let html = include_str!("../assets/ui/index.html");
        for pin in [r#"title="close""#, r#"data-act="stage""#] {
            assert!(html.contains(pin), "msg");
        }
        assert!(html.contains("stage"), "the close button says close");
    }
"##,
    );
    put(
        &root,
        "crates/ralphy-daemon/assets/ui/index.html",
        r#"<button title="close" data-act="stage">stage</button>"#,
    );
    put(
        &root,
        "crates/ralphy-daemon/assets/ui/wb-a.js",
        r#"toast({ text: "from a module" });"#,
    );
    put(
        &root,
        "crates/ralphy-daemon/assets/ui/other.js",
        r#"toast({ text: "not a workbench module" });"#,
    );
    put(
        &root,
        "crates/ralphy-daemon/assets/ui/vendor/wb-lib.js",
        r#"toast({ text: "vendored" });"#,
    );

    let pins = asset_pins::gather(&root).expect("gathering the fixture pins");
    let rows =
        inventory(&root.join(UI_DIR), &pins, CopyFns::default()).expect("the inventory runs");
    let got: Vec<(&str, &str, &[String])> = rows
        .iter()
        .map(|r| (r.file.as_str(), r.text.as_str(), r.pinned_by.as_slice()))
        .collect();
    let pinned = vec!["crates/ralphy-daemon/src/tests.rs:4".to_string()];
    assert_eq!(
        got,
        vec![
            ("index.html", "close", pinned.as_slice()),
            ("index.html", "stage", &[][..]),
            ("wb-a.js", "from a module", &[][..]),
        ]
    );

    let json: serde_json::Value =
        serde_json::from_str(&to_json(&rows).expect("the inventory serializes"))
            .expect("the output is JSON");
    assert_eq!(json["totals"]["rows"], 3);
    assert_eq!(json["totals"]["pinned"], 1);
    assert_eq!(json["rows"][0]["kind"], "title");
    assert_eq!(json["totals"]["by_kind"]["js:toast"], 1);

    let md = to_markdown(&rows);
    assert!(md.contains("| 1 | title | close |  | crates/ralphy-daemon/src/tests.rs:4 |"));
    std::fs::remove_dir_all(&root).expect("removing the scratch dir");
}

#[test]
fn the_markdown_has_a_section_per_file_and_a_table_per_area() {
    let row = |file: &str, area: &str, text: &str, concatenated: bool| Row {
        text: text.to_string(),
        file: file.to_string(),
        line: 7,
        kind: Kind::Text,
        area: area.to_string(),
        concatenated,
        pinned_by: Vec::new(),
        continues: false,
    };
    let md = to_markdown(&[
        row("a.html", "", "a | b", true),
        row("a.html", "Hosts", "c", false),
        row("b.ts", "Hosts", "d", false),
    ]);
    let headings: Vec<&str> = md.lines().filter(|l| l.starts_with('#')).collect();
    assert_eq!(
        headings,
        [
            "# UI copy inventory",
            "## Totals",
            "## a.html",
            "### (top level)",
            "### Hosts",
            "## b.ts",
            "### Hosts",
        ]
    );
    assert!(md.contains("| 7 | text | a \\| b | yes |  |"));
    assert!(md.contains("| 7 | text | c |  |  |"));
}

#[test]
fn a_one_word_text_is_pinned_only_next_to_its_sink() {
    let anchor = Some("title=");
    assert!(pins_form(r#"<b title="close">"#, "close", anchor, true));
    assert!(!pins_form(r#"data-act="close""#, "close", anchor, true));
    assert!(pins_form(
        r#"x-text="open ? 'Hide' : 'Show'""#,
        "Show",
        Some("x-text=\""),
        true
    ));
    assert!(!pins_form(
        r#":class="open ? 'Show' : ''""#,
        "Show",
        Some("x-text=\""),
        true
    ));
    // Prose in an assertion message is not a pin until it is long enough to be
    // specific.
    assert!(!pins_form(
        "(empty = the repo root), not early",
        "the repo root",
        None,
        true
    ));
    assert!(pins_form(
        "must say: open a project to start",
        "open a project to start",
        None,
        false
    ));
}

/// The command, not only `check()`: a failing report makes `--check` an error,
/// and a clean one does not.
#[test]
fn check_is_an_error_when_the_report_fails_and_ok_when_it_is_clean() {
    let root = scratch("check-exit");
    put(
        &root,
        "docs/ui-copy-rules.json",
        r#"{
  "casing": { "exempt_kinds": [], "state_words": [], "key_names": [] },
  "proper_nouns": [],
  "banned": [],
  "punctuation": [],
  "sentence_max_words": 20,
  "plain": [],
  "contractions": { "banned": [], "use": "u", "reason": "r" }
}"#,
    );
    put(&root, "crates/ralphy-daemon/src/tests.rs", "");
    let page = "crates/ralphy-daemon/assets/ui/index.html";
    let args = [
        "--root",
        root.to_str().expect("a UTF-8 temp path"),
        "--check",
    ]
    .map(String::from);

    put(&root, page, r#"<button title="close">X</button>"#);
    let err = ui_copy_cmd(&args).expect_err("a violation fails --check");
    assert!(err.to_string().contains("1 violations"), "{err}");

    put(&root, page, r#"<button title="Close">X</button>"#);
    ui_copy_cmd(&args).expect("a clean tree passes --check");
    std::fs::remove_dir_all(&root).expect("removing the scratch dir");
}
