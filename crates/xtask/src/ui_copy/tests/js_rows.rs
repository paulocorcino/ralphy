use super::*;

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
        seen(&rows("wb-file-viewer.js", js)),
        vec![("js:text-content", "After the regex")]
    );
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
    let fns = CopyFns {
        helpers: &helpers,
        calls: &[],
    };
    let out = rows_of("wb-file-search.js", js, &[], fns);
    assert_eq!(seen(&out), vec![("js:helper", "no matches")]);
    assert!(seen(&rows("wb-file-search.js", js)).is_empty());
}

#[test]
fn a_typescript_helper_with_a_return_type_returns_copy() {
    let ts = r#"
  export function addedText(s: State): string {
    return "Added the host.";
  }
  export function kindText(s: State): "a" | "b" {
    return "a";
  }
"#;
    assert_eq!(
        seen(&rows("wb-hosts.ts", ts)),
        vec![("js:helper", "Added the host."), ("js:helper", "a")]
    );
}

#[test]
fn a_call_named_in_copy_calls_gives_its_listed_argument() {
    let js = r#"
  function paintState(el, text) { el.dataset.state = text; }
  paintState(el, "Could not save: " + why);
  paintState(el, ok ? "Saved" : "Renamed");
  paintState("not copy", String(err?.message || err));
  view.paintState(el, "a method is not the listed function");
"#;
    let calls: Vec<CopyCall> =
        serde_json::from_str(r#"[{ "name": "paintState", "arg": 1 }]"#).expect("a literal");
    let fns = CopyFns {
        helpers: &[],
        calls: &calls,
    };
    let out = rows_of("wb-notes.js", js, &[], fns);
    assert_eq!(
        seen(&out),
        vec![
            ("js:call", "Could not save: {why}"),
            ("js:call", "Saved"),
            ("js:call", "Renamed"),
        ]
    );
    assert!(seen(&rows("wb-notes.js", js)).is_empty());
}

#[test]
fn an_owner_method_in_copy_calls_matches_only_on_that_owner() {
    let js = r#"
  window.WBFail.failed(reply, "Could not stage: the daemon gave no reason.");
  WBFail.failed(reply, `Could not create ${name}: the daemon gave no reason.`);
  window.WBFail?.failed(reply, "Could not pull: the daemon gave no reason.");
  window.WBFail.failed(
    reply,
    verb === "branch.create"
      ? "Could not create the branch: the daemon gave no reason."
      : "Could not switch branch: the daemon gave no reason.",
  );
  other.failed(reply, "another owner is not read");
  failed(reply, "a bare call is not read");
"#;
    let calls: Vec<CopyCall> =
        serde_json::from_str(r#"[{ "name": "WBFail.failed", "arg": 1 }]"#).expect("a literal");
    let fns = CopyFns {
        helpers: &[],
        calls: &calls,
    };
    assert_eq!(
        seen(&rows_of("app.js", js, &[], fns)),
        vec![
            ("js:call", "Could not stage: the daemon gave no reason."),
            (
                "js:call",
                "Could not create {name}: the daemon gave no reason."
            ),
            ("js:call", "Could not pull: the daemon gave no reason."),
            (
                "js:call",
                "Could not create the branch: the daemon gave no reason."
            ),
            (
                "js:call",
                "Could not switch branch: the daemon gave no reason."
            ),
        ]
    );
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
    let out = rows("wb-file-viewer.js", js);
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
fn a_setting_gives_its_label_help_and_section_blurb() {
    let js = r#"
window.WB_SETTINGS = [
  {
    id: "queue",
    title: "Queue",
    blurb: "How issues are picked up.",
    items: [
      { key: "queue.assignee", label: "Assignee filter", help: "Only issues assigned to this login." },
    ],
  },
];
"#;
    assert_eq!(
        seen(&rows("wb-settings.js", js)),
        vec![
            ("js:property", "Queue"),
            ("js:property", "How issues are picked up."),
            ("js:property", "Assignee filter"),
            ("js:property", "Only issues assigned to this login."),
        ]
    );
}

#[test]
fn a_note_key_is_copy_and_an_icon_class_list_is_not() {
    let js = r#"
const sync = { note: "no upstream", fetched: "never fetched" };
const icons = { toml: "bi bi-gear", note: "bi bi-sticky" };
"#;
    assert_eq!(
        seen(&rows("wb-changes.js", js)),
        vec![("js:property", "no upstream")]
    );
}

#[test]
fn a_ternary_in_parentheses_gives_both_branches_in_order() {
    let js = r#"
      save.textContent = (ok ? "Saved" : "Failed");
"#;
    assert_eq!(
        seen(&rows("wb-console.js", js)),
        vec![("js:text-content", "Saved"), ("js:text-content", "Failed")]
    );
}

#[test]
fn a_nested_ternary_gives_its_branches_in_source_order() {
    let js = r#"
      save.textContent = a ? b ? "One" : "Two" : "Three";
      stop.textContent = a ? "Four" : b ? "Five" : "Six";
"#;
    assert_eq!(
        seen(&rows("wb-console.js", js)),
        vec![
            ("js:text-content", "One"),
            ("js:text-content", "Two"),
            ("js:text-content", "Three"),
            ("js:text-content", "Four"),
            ("js:text-content", "Five"),
            ("js:text-content", "Six"),
        ]
    );
}

#[test]
fn an_and_gives_its_right_side_and_not_its_condition() {
    let js = r#"
      wait.textContent = busy && "Wait here";
"#;
    assert_eq!(
        seen(&rows("wb-console.js", js)),
        vec![("js:text-content", "Wait here")]
    );
}

#[test]
fn a_hole_over_several_lines_keeps_the_line_of_a_later_attribute() {
    // (case, source, line of the button)
    let cases = [
        (
            "a newline inside the hole",
            r#"
el.innerHTML = `<p>${a +
  b}</p><button title="Later one">Go</button>`;
"#,
            3,
        ),
        (
            "newlines at the start and the end of the hole",
            r#"
el.innerHTML = `<p>${
  a
}</p><button title="Later one">Go</button>`;
"#,
            4,
        ),
    ];
    for (case, js, line) in cases {
        let out = rows("wb-console.js", js);
        assert_eq!(
            out.iter()
                .map(|r| (r.line, r.kind.label(), r.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (line, "title", "Later one"),
                (line, "js:text-content", "Go")
            ],
            "{case}"
        );
    }
}
