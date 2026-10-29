use super::*;

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

#[test]
fn input_cut_short_gives_rows_and_never_panics() {
    // Text before a cut tag is still copy; a string cut inside an escape is
    // not a whole string, so it gives no row.
    assert_eq!(
        seen(&rows("index.html", "<p>Hello there</")),
        vec![("text", "Hello there")]
    );
    for (file, js) in [
        ("wb-c.js", r#"el.textContent = f("\x"#),
        ("wb-d.js", r#"el.textContent = g(`${"\u{"#),
        ("wb-e.js", r#"el.textContent = h("\u12"#),
    ] {
        assert_eq!(seen(&rows(file, js)), vec![], "{file}");
    }
}

#[test]
fn a_void_element_names_no_area_for_the_text_after_it() {
    let out = rows(
        "index.html",
        r#"<p><img class="hero-view" src="a.png"> Caption</p>"#,
    );
    assert_eq!(
        out.iter()
            .map(|r| (r.text.as_str(), r.area.as_str()))
            .collect::<Vec<_>>(),
        vec![("Caption", "")]
    );
}

#[test]
fn text_with_no_tag_after_it_is_still_a_row() {
    let out = rows("index.html", "<p>Tail");
    assert_eq!(seen(&out), vec![("text", "Tail")]);
}
