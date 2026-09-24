use super::*;

fn rows(name: &str, src: &str) -> Vec<Row> {
    rows_of(name, src, &[], &[])
}

/// `(kind, text)` of every row, for compact assertions.
fn seen(rows: &[Row]) -> Vec<(&'static str, &str)> {
    rows.iter()
        .map(|r| (r.kind.label(), r.text.as_str()))
        .collect()
}

#[test]
fn static_attributes_and_text_nodes_are_rows_and_comments_are_not() {
    let html = r#"<button title="Refresh projects" aria-label="Refresh"
        placeholder="Search&hellip;">Go &amp; see</button>
<!-- <b>a comment</b> -->
<style>.x::after { content: "styled"; }</style>
<p>   </p><p>·</p>"#;
    assert_eq!(
        seen(&rows("index.html", html)),
        vec![
            ("title", "Refresh projects"),
            ("aria-label", "Refresh"),
            ("placeholder", "Search…"),
            ("text", "Go & see"),
        ]
    );
}

#[test]
fn a_multi_line_text_node_is_booked_to_its_first_line() {
    let html = "<div>\n\n  <p>\n    Open a project\n    to see its runs.\n  </p>\n</div>";
    let out = rows("index.html", html);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "Open a project to see its runs.");
    assert_eq!(out[0].line, 4);
}

#[test]
fn an_alpine_expression_gives_its_branches_and_never_its_condition() {
    let html = r#"<span :title="p.remote === 'github' ? 'on GitHub' : 'local only'"
      x-text="uptimeText || 'connecting…'" :class="'not-copy'"></span>"#;
    assert_eq!(
        seen(&rows("index.html", html)),
        vec![
            ("alpine", "on GitHub"),
            ("alpine", "local only"),
            ("alpine", "connecting…"),
        ]
    );
}

#[test]
fn a_concatenated_sentence_is_flagged_and_its_holes_are_named() {
    let html = r#"<i :title="'Wake ' + g.environment + '. ' + g.diagnosis"></i>"#;
    let out = rows("index.html", html);
    assert_eq!(out[0].text, "Wake {g.environment}. {g.diagnosis}");
    assert!(out[0].concatenated);

    let js = "window.WBConsole.toast({ text: `at the ${NOTE_MAX}-note cap` });";
    let out = rows("wb-notes.js", js);
    assert_eq!(seen(&out), vec![("js:toast", "at the {NOTE_MAX}-note cap")]);
    assert!(out[0].concatenated);

    let plain = rows("wb-console.js", r#"cancel.textContent = "Cancel";"#);
    assert!(!plain[0].concatenated);
}

#[test]
fn a_dialog_object_spread_over_lines_gives_one_row_per_shown_key() {
    let js = r#"
      const ok = await askConfirm({
        title: "Stop this run?",
        message: p.name + " stops now.",
        confirmLabel: "Stop",
        danger: true,
        id: "not-shown",
      });
      const plain = window.confirm(`Reopen as ${label}?`);
      const other = this.confirm("not a dialog");
"#;
    let out = rows("app.js", js);
    assert_eq!(
        seen(&out),
        vec![
            ("js:confirm", "Stop this run?"),
            ("js:confirm", "{p.name} stops now."),
            ("js:confirm", "Stop"),
            ("js:confirm", "Reopen as {label}?"),
        ]
    );
    assert_eq!(
        out.iter().map(|r| r.line).collect::<Vec<_>>(),
        vec![3, 4, 5, 9]
    );
}

#[test]
fn each_dom_sink_is_its_own_kind() {
    let js = r#"
      cancel.textContent = "Cancel";
      close.title = "dismiss";
      btn.innerHTML = '<i class="bi bi-x"></i> Close all';
      modal.setAttribute("aria-label", "Confirm");
      modal.setAttribute("data-x", "not copy");
      state.label = { label: "Backlog" };
"#;
    assert_eq!(
        seen(&rows("wb-console.js", js)),
        vec![
            ("js:text-content", "Cancel"),
            ("js:attribute", "dismiss"),
            ("js:text-content", "Close all"),
            ("js:attribute", "Confirm"),
            ("js:property", "Backlog"),
        ]
    );
}

#[test]
fn a_terminal_notice_is_a_row_and_the_image_write_rpc_is_not() {
    let js = r#"
      term.write("\r\n[session closed]\r\n");
      win._term?.term.write("\r\n[close failed — connection unavailable]\r\n");
      await conn.write("image.write", { bytes });
"#;
    assert_eq!(
        seen(&rows("wb-console.js", js)),
        vec![
            ("js:term-notice", "[session closed]"),
            ("js:term-notice", "[close failed — connection unavailable]"),
        ]
    );
}

#[test]
fn the_key_bar_gives_its_label_and_its_title() {
    let js = r#"
      function key(name, text, title) { b.dataset.k = name; }
      key("esc", "esc", "Escape");
      key("left", '<i class="bi bi-arrow-left"></i>', "Left");
"#;
    assert_eq!(
        seen(&rows("wb-console.js", js)),
        vec![
            ("js:key-bar", "esc"),
            ("js:key-bar", "Escape"),
            ("js:key-bar", "Left"),
        ]
    );
}

#[test]
fn state_messages_and_label_helpers_are_rows_and_enum_values_are_not() {
    let js = r#"
      this.reposError = "could not load projects";
      this.syncBusy = "fetch";
      verbTitle(verb) {
        if (!verb) return "";
        return verb === "stage" ? "stage this path" : "bi-check";
      },
      helper(x) { return "not a label helper"; }
"#;
    assert_eq!(
        seen(&rows("app.js", js)),
        vec![
            ("js:state", "could not load projects"),
            ("js:helper", "stage this path"),
        ]
    );
}

#[test]
fn comments_and_regex_literals_do_not_derail_the_lexer() {
    let js = r#"
      // toast({ text: "in a line comment" });
      /* askConfirm({ title: "in a block comment" }); */
      const quote = /"/g;
      const cls = /[/"]/;
      el.textContent = "After the regex";
"#;
    assert_eq!(
        seen(&rows("wb-viewer.js", js)),
        vec![("js:text-content", "After the regex")]
    );
}

#[test]
fn an_inline_script_keeps_its_page_line_numbers() {
    let html =
        "<p>Page</p>\n<script>\n  el.textContent = \"From the script\";\n</script>\n<p>After</p>";
    let out = rows("detached.html", html);
    assert_eq!(
        out.iter()
            .map(|r| (r.line, r.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "Page"), (3, "From the script"), (5, "After")]
    );
}

#[test]
fn an_area_is_the_innermost_landmark_including_its_own_label() {
    let html = r#"<aside class="side">
  <div class="changes-view"><button title="Commit">x</button></div>
  <section class="small"><p>Inside a group</p></section>
</aside>
<div class="modal-scrim"><div class="modal" role="dialog" aria-label="Settings">
  <h2>General</h2></div></div>"#;
    let out = rows("index.html", html);
    assert_eq!(
        out.iter()
            .map(|r| (r.text.as_str(), r.area.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("Commit", "div.changes-view"),
            ("x", "div.changes-view"),
            ("Inside a group", "aside.side"),
            ("Settings", "Settings"),
            ("General", "Settings"),
        ]
    );
}

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
    let rows = inventory(&root.join(UI_DIR), &pins, &[]).expect("the inventory runs");
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

#[test]
fn a_screaming_const_with_prose_is_a_row_and_a_media_query_is_not() {
    let js = r#"
  const NEEDS_REPO = "Select a repo before launching an agent.";
  const PHONE_QUERY = "(max-width: 560px), (pointer: coarse)";
  const MAX_HITS = 200;
  const label = "a local with spaces";
  const KIND = "local";
"#;
    let out = rows("wb-agents.js", js);
    assert_eq!(
        seen(&out),
        vec![("js:const", "Select a repo before launching an agent.")]
    );
    assert_eq!(out[0].line, 2);
}

#[test]
fn a_function_named_in_copy_helpers_returns_copy() {
    let js = r#"
  function note({ hits, truncated }) {
    if (!hits.length) return "no matches";
    return "";
  }
  function other() { return "not a helper"; }
"#;
    let helpers = vec!["note".to_string()];
    let out = rows_of("wb-file-search.js", js, &[], &helpers);
    assert_eq!(seen(&out), vec![("js:helper", "no matches")]);
    assert!(seen(&rows("wb-file-search.js", js)).is_empty());
}

#[test]
fn an_inner_html_toolbar_gives_one_row_per_element() {
    let js = r#"
    el.innerHTML = `
      <div class="viewer-toolbar">
        <button title="Find" aria-label="Find"><span>Find</span></button>
        ${detachBtnHtml(rec)}
        <button title="Save as ${enc}"><i class="bi bi-save"></i></button>
      </div>`;
    btn.innerHTML = '<i class="bi bi-x"></i> Close all';
"#;
    let out = rows("wb-viewer.js", js);
    assert_eq!(
        out.iter()
            .map(|r| (r.line, r.kind.label(), r.text.as_str(), r.concatenated))
            .collect::<Vec<_>>(),
        vec![
            (4, "title", "Find", false),
            (4, "aria-label", "Find", false),
            (4, "js:text-content", "Find", false),
            (6, "title", "Save as {enc}", true),
            (8, "js:text-content", "Close all", false),
        ]
    );
}

#[test]
fn text_after_an_inline_element_goes_on_with_its_sentence() {
    let html = r#"<p>No runs in <b x-text="name"></b>. Click <b>run</b> to start one.</p>
<p><span x-text="n"></span> rows not shown.</p>
<button><i class="bi bi-x"></i> close</button>"#;
    let out = rows("index.html", html);
    assert_eq!(
        out.iter()
            .map(|r| (r.text.as_str(), r.continues))
            .collect::<Vec<_>>(),
        vec![
            ("No runs in", false),
            (". Click", true),
            ("run", true),
            ("to start one.", true),
            ("rows not shown.", true),
            ("close", false),
        ]
    );
}
