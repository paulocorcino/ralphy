use super::super::check::CopyCall;
use super::super::{rows_of, CopyFns};
use super::scan;

fn seen_with(js: &str, fns: CopyFns) -> Vec<(&'static str, String)> {
    rows_of("x.js", js, &[], fns)
        .into_iter()
        .map(|r| (r.kind.label(), r.text))
        .collect()
}

fn seen(js: &str) -> Vec<(&'static str, String)> {
    seen_with(js, CopyFns::default())
}

fn row(kind: &'static str, text: &str) -> (&'static str, String) {
    (kind, text.to_string())
}

/// The sinks are calls: the same name used as a plain value, or declared, shows
/// nothing. Each source is read alone: a value has no closing bracket, so its
/// arguments would run to the end of the source.
#[test]
fn a_sink_name_that_is_not_called_is_not_a_sink() {
    for js in [
        r#"confirm = "Ask the operator first""#,
        r#"function confirm("Delete everything now") {}"#,
        r#"key = "esc", "Left side", "Left""#,
        r#"setAttribute = "title", "A title that is set""#,
        r#"term.write = "not a notice""#,
    ] {
        assert_eq!(seen(js), vec![], "{js}");
    }
}

/// `term.write` is the terminal; a `write` that only has `term` two tokens
/// before it is not, and `writeln` is a terminal write too.
#[test]
fn a_terminal_write_needs_the_dot_and_writeln_counts() {
    let js = r#"
      term.writeln("A line for the terminal");
      use(term, write("Not a terminal write"));
"#;
    assert_eq!(
        seen(js),
        vec![row("js:term-notice", "A line for the terminal")]
    );
}

/// The key bar is a call of a bare `key` whose first argument is one literal.
#[test]
fn only_a_bare_key_call_with_a_literal_name_is_the_key_bar() {
    let js = r#"
      bar.key("esc", "Escape key", "Escape");
      key(name, "A label", "A title");
      key("a" + n, "Another label", "Another title");
      function key("esc", "Left side", "Left") {}
"#;
    assert_eq!(seen(js), vec![]);
}

/// A declaration is not a call of its own name, and a name used as a value
/// is not a call of the function.
#[test]
fn a_declaration_or_a_value_is_not_a_call_of_a_listed_function() {
    let calls: Vec<CopyCall> =
        serde_json::from_str(r#"[{ "name": "paintState", "arg": 1 }]"#).expect("a literal");
    let fns = || CopyFns {
        helpers: &[],
        calls: &calls,
    };
    for js in [
        r#"function paintState(el, "Saved it now") {}"#,
        r#"paintState = 1, "Saved it""#,
    ] {
        assert_eq!(seen_with(js, fns()), vec![], "{js}");
    }
    assert_eq!(
        seen_with(r#"paintState(el, "Saved");"#, fns()),
        vec![row("js:call", "Saved")]
    );
}

/// A copy helper is a function that is declared with a body: a mention of its
/// name, a method of the same name, or a call that goes on to another bracket
/// opens no body.
#[test]
fn only_a_declared_body_of_a_copy_helper_returns_copy() {
    let js = r#"
      const shown = labelText
      if (ok) { return "must not be copy"; }
      view.titleText(1) { return "also not copy"; }
      const x = verbTitle(verb)(() => { return "inner text of a call"; });
"#;
    assert_eq!(seen(js), vec![]);
}

/// A key is shown only as a key of an object literal, with a colon.
#[test]
fn a_shown_key_needs_a_colon_and_an_object_around_it() {
    let js = r#"
      { title, "Not a shown key" }
      function f(mode: number, label: "A literal type") {}
"#;
    assert_eq!(seen(js), vec![]);
}

/// The dialog helpers of the workbench each take an object of shown keys.
#[test]
fn each_dialog_helper_gives_the_shown_keys_of_its_object() {
    let js = r#"
      askNotice({ message: "A notice for you" });
      askPrompt({ title: "A prompt for you" });
      askInShell({ text: "A shell question" });
"#;
    assert_eq!(
        seen(js),
        vec![
            row("js:confirm", "A notice for you"),
            row("js:confirm", "A prompt for you"),
            row("js:confirm", "A shell question"),
        ]
    );
}

/// A DOM property is a sink when it is assigned (`=` or `+=`), not when it
/// is a plain variable.
#[test]
fn a_dom_property_is_a_sink_only_when_assigned() {
    let js = r#"
      el.innerText = "Inner text here";
      el.placeholder = "Type a name";
      el.ariaLabel = "Close the panel";
      el.textContent += "More text added";
      let placeholder = "A plain variable text";
"#;
    assert_eq!(
        seen(js),
        vec![
            row("js:text-content", "Inner text here"),
            row("js:attribute", "Type a name"),
            row("js:attribute", "Close the panel"),
            row("js:text-content", "More text added"),
        ]
    );
}

/// Only three attributes carry copy, and only when named by one literal.
#[test]
fn set_attribute_reads_three_named_attributes() {
    let js = r#"
      el.setAttribute("title", "Close this panel");
      el.setAttribute("placeholder", "Type a name here");
      el.setAttribute("title" + n, "Not copy of an attribute");
"#;
    assert_eq!(
        seen(js),
        vec![
            row("js:attribute", "Close this panel"),
            row("js:attribute", "Type a name here"),
        ]
    );
}

/// `this.x = …` is state; `this` as an argument is not.
#[test]
fn a_this_that_is_not_followed_by_a_member_is_not_state() {
    let js = r#"
      setup(this, mode = "A default mode");
"#;
    assert_eq!(seen(js), vec![]);
}

/// A selector in a constant is not copy, whichever bracket it starts with.
#[test]
fn a_selector_constant_is_not_copy() {
    let js = r#"
      const SELECTOR = "[data-x] and more";
"#;
    assert_eq!(seen(js), vec![]);
}

/// The text just before a literal at its sink is kept, because a one-word text
/// is pinned only next to it.
#[test]
fn each_sink_keeps_the_text_that_comes_before_its_literal() {
    let js = r#"
      toast({ text: "Saved it now" });
      el.textContent = "Done now";
      el.setAttribute("title", "Closes it");
      const NEEDS_REPO = "Select a repo first.";
"#;
    let mut found = Vec::new();
    scan(js, 1, CopyFns::default(), &mut found);
    let anchors: Vec<Option<&str>> = found.iter().map(|f| f.anchor.as_deref()).collect();
    assert_eq!(
        anchors,
        vec![
            Some("text: "),
            Some("textContent = "),
            Some("\"title\", "),
            Some("NEEDS_REPO = "),
        ]
    );
}
