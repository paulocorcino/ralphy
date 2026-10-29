use super::*;

/// A `shell()` as `app.ts` writes it, with one section per group used.
const APP: &str = "\
import { x } from \"./x.ts\";
export function shell() {
  return shellData({
    // The open project.
    openSlug: null as string | null,
    waking: {} as Record<string, any>,
    // --plan-agent and --branch-mode new|current.
    toggle(ref: string) {
      this.openSlug = ref;
      this.kanbanSel = null;
      this.runsByProject[ref] = [];
    },

    // --- branch switcher --------------------------------------------------
    branchOpen: false,
    get writeLocked() {
      return `${this.runIsLive(this.openSlug)}`;
    },
    stopBranch: async () => {
      this.branchOpen = !this.branchOpen;
    },

    // ---- Runs panel ----
    kanbanSel: null,
    runsByProject: {},
    runIsLive(slug: string): Map<string, number> {
      this.kanbanSel++;
      return this.runsByProject[slug];
    },
  });
}
";

fn parsed() -> Shell {
    parse_shell(APP).expect("the fixture parses")
}

fn member<'a>(shell: &'a Shell, name: &str) -> &'a Member {
    shell
        .members
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no member {name}"))
}

#[test]
fn a_heading_needs_three_dashes_and_a_space() {
    assert_eq!(
        heading("    // --- branch switcher -----"),
        Some("branch switcher")
    );
    assert_eq!(
        heading("    // ---- write controls (#318) ------------"),
        Some("write controls (#318)")
    );
    assert_eq!(
        heading("    // --- the slot: pin a tab beside the active one --"),
        Some("the slot: pin a tab beside the active one")
    );
    assert_eq!(
        heading("    // --- move (issue #364)"),
        Some("move (issue #364)")
    );
    assert_eq!(
        heading("    // --plan-agent and --branch-mode new|current."),
        None
    );
    assert_eq!(heading("    // ---"), None);
    assert_eq!(heading("      // --- nested inside a member ---"), None);
}

#[test]
fn a_member_belongs_to_the_heading_it_sits_under() {
    let shell = parsed();
    let names: Vec<&str> = shell.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "openSlug",
            "waking",
            "toggle",
            "branchOpen",
            "writeLocked",
            "stopBranch",
            "kanbanSel",
            "runsByProject",
            "runIsLive"
        ]
    );
    let group = |name: &str| {
        let n = shell
            .members
            .iter()
            .position(|m| m.name == name)
            .unwrap_or(usize::MAX);
        shell.group(n)
    };
    // `toggle` sits under no heading, whatever the `// --plan-agent` comment says.
    assert_eq!(group("toggle"), Group::Core);
    assert_eq!(group("writeLocked"), Group::Git);
    assert_eq!(group("runIsLive"), Group::Board);
    assert_eq!(member(&shell, "writeLocked").kind, Kind::Getter);
    assert_eq!(member(&shell, "stopBranch").kind, Kind::Method);
    assert_eq!(member(&shell, "waking").kind, Kind::Field);
    // A member's lines run from the end of the one before it.
    let toggle = member(&shell, "toggle");
    assert_eq!((toggle.from, toggle.first, toggle.last), (7, 8, 12));
    assert_eq!(member(&shell, "runIsLive").last, 29);
}

#[test]
fn a_heading_with_no_group_is_an_error() {
    let app = APP.replace("Runs panel", "a new section");
    let err = parse_shell(&app)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("\"a new section\""), "{err}");
}

fn parse_error(app: &str) -> String {
    parse_shell(app)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default()
}

#[test]
fn app_ts_with_no_function_shell_is_an_error() {
    let err = parse_error("const s = shellData({ a: 1 });\n");
    assert_eq!(err, "app.ts has no `function shell`");
}

#[test]
fn a_shell_that_does_not_return_shell_data_is_an_error() {
    let err = parse_error("function shell() {\n  return { a: 1 };\n}\n");
    assert_eq!(err, "shell() does not return shellData({ … })");
}

#[test]
fn an_object_literal_that_is_not_closed_is_an_error() {
    let err = parse_error("function shell() {\n  return shellData({\n    a: 1,\n");
    assert_eq!(err, "the object literal of shell() is not closed");
}

#[test]
fn a_member_with_no_plain_name_is_an_error() {
    let app = "function shell() {\n  return shellData({\n    a: 1,\n    [k]: 2,\n  });\n}\n";
    assert_eq!(
        parse_error(app),
        "app.ts:4: a member of shell() with no plain name"
    );
}

#[test]
fn reads_and_writes_from_outside_a_group_are_told_apart() {
    let shell = parsed();
    let by = stats(&shell, &[]);
    let names = |list: &BTreeMap<String, BTreeSet<Group>>| -> Vec<String> {
        list.keys().cloned().collect()
    };
    // `toggle` (core) assigns `kanbanSel` and writes inside `runsByProject`.
    assert_eq!(
        names(&by[&Group::Board].written_outside),
        ["kanbanSel", "runsByProject"]
    );
    // `writeLocked` (git) calls `runIsLive` in a template hole.
    assert_eq!(names(&by[&Group::Board].read_outside), ["runIsLive"]);
    assert_eq!(names(&by[&Group::Core].read_outside), ["openSlug"]);
    assert!(by[&Group::Core].written_outside.is_empty());
    // `stopBranch` reads and writes `branchOpen`, both inside its own group.
    assert!(by[&Group::Git].read_outside.is_empty());
    assert_eq!(by[&Group::Board].members.len(), 3);
}

#[test]
fn every_write_shape_is_a_write() {
    let known: BTreeMap<&str, usize> = [("x", 0)].into_iter().collect();
    let shape = |code: &str| {
        let mut out = Vec::new();
        scan_accesses(&lex(code, 1), &known, &mut out);
        out.first().map(|(_, w)| *w)
    };
    for code in [
        "this.x = 1;",
        "this.x += 1;",
        "this.x ||= {};",
        "this.x++;",
        "--this.x;",
        "this.x.push(1);",
        "this.x.y = 1;",
        "this.x[k] = 1;",
        "this.x?.y.z++;",
        "delete this.x[k];",
        "self.x = e;",
    ] {
        assert_eq!(shape(code), Some(true), "{code}");
    }
    for code in [
        "this.x == 1;",
        "this.x.y();",
        "f(this.x);",
        "this.x.push;",
        "this.x || y;",
        "delete this.x;",
    ] {
        assert_eq!(shape(code), Some(false), "{code}");
    }
    assert_eq!(shape("this.y = 1;"), None);
}

/// Every access of `x` that `scan_accesses` finds in `code`, as `(name, writes)`.
fn accesses_of_x(code: &str) -> Vec<(String, bool)> {
    let known: BTreeMap<&str, usize> = [("x", 0)].into_iter().collect();
    let mut out = Vec::new();
    scan_accesses(&lex(code, 1), &known, &mut out);
    out
}

#[test]
fn a_hole_inside_a_hole_is_one_read() {
    assert_eq!(
        accesses_of_x("show(`a ${ `b ${this.x}` }`);"),
        [("x".to_string(), false)]
    );
}

#[test]
fn this_followed_by_a_bracket_is_no_access() {
    assert!(accesses_of_x("this[x] = 1;").is_empty());
}

#[test]
fn an_indexed_member_is_a_read_and_a_step_on_it_is_a_write() {
    assert_eq!(accesses_of_x("this.x[k];"), [("x".to_string(), false)]);
    assert_eq!(accesses_of_x("this.x[k]++;"), [("x".to_string(), true)]);
}

#[test]
fn a_binding_names_only_free_members_of_shell_scope() {
    let known: BTreeSet<String> = ["openSlug", "tabs", "rowOpen", "branchOpen", "item", "label"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let html = "\
<head><script>let openSlug = 1;</script></head>
<body x-data=\"shell\" :class=\"{ open: openSlug }\">
  <!-- <div x-show=\"tabs\"></div> -->
  <template x-for=\"(item, i) in tabs\">
    <span :title=\"item.label + rowOpen(i)\" x-ref=\"tabs\" x-cloak></span>
  </template>
  <div @click=\"(e) => e.label\"></div>
  <div class=\"modal-scrim\" x-bind=\"scrim('branchOpen', () => 0)\"></div>
  <input x-model=\"$store.label\">
  <div x-data=\"wbSettingsDialog\" @open.window=\"tabs\">
    <span x-text=\"openSlug\"></span>
  </div>
  <span x-text=\"label\"></span>
</body>";
    let bindings = scan_bindings(html, &known);
    let found: Vec<(usize, Vec<&str>)> = bindings
        .iter()
        .map(|b| (b.line, b.names.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        found,
        [
            (2, vec!["openSlug"]),
            (4, vec!["tabs"]),
            (5, vec!["rowOpen"]),
            (7, vec![]),
            (8, vec!["branchOpen"]),
            (9, vec![]),
            (13, vec!["label"]),
        ]
    );
}

fn binding_lines(html: &str) -> Vec<(usize, Vec<String>)> {
    let known: BTreeSet<String> = ["openSlug", "tabs"].iter().map(|s| s.to_string()).collect();
    scan_bindings(html, &known)
        .into_iter()
        .map(|b| (b.line, b.names.into_iter().collect()))
        .collect()
}

#[test]
fn a_doctype_ends_at_its_own_closing_bracket() {
    let html = "<!DOCTYPE html>\n<body x-data=\"shell\" :class=\"openSlug\"></body>";
    assert_eq!(binding_lines(html), [(2, vec!["openSlug".to_string()])]);
}

#[test]
fn a_self_closing_tag_holds_no_scope() {
    let html = "\
<div x-data=\"other\">
  <b x-data=\"shell\" />
  <span x-text=\"openSlug\"></span>
</div>";
    assert_eq!(binding_lines(html), []);
}

#[test]
fn an_unquoted_value_ends_at_white_space() {
    let html = "<body x-data=shell :title=tabs></body>";
    assert_eq!(binding_lines(html), [(1, vec!["tabs".to_string()])]);
}

/// The names the one `x-text` expression `expr` reads, out of `known`.
fn names_read(expr: &str, known: &[&str]) -> Vec<String> {
    let known: BTreeSet<String> = known.iter().map(|s| s.to_string()).collect();
    let html = format!("<body x-data=\"shell\"><i x-text=\"{expr}\"></i></body>");
    scan_bindings(&html, &known)
        .into_iter()
        .flat_map(|b| b.names)
        .collect()
}

#[test]
fn an_arrow_with_one_bare_parameter_declares_it() {
    assert_eq!(
        names_read("list.map(x => x + openSlug)", &["x", "openSlug"]),
        ["openSlug"]
    );
}

#[test]
fn let_const_and_var_declare_the_name_after_them() {
    assert_eq!(
        names_read(
            "{ let tabs = 1; const x = 2; var y = 3; tabs + x + y + openSlug }",
            &["tabs", "x", "y", "openSlug"]
        ),
        ["openSlug"]
    );
}

#[test]
fn a_name_after_an_optional_chain_dot_is_a_property_not_a_read() {
    assert_eq!(
        names_read("tabs?.openSlug", &["tabs", "openSlug"]),
        ["tabs"]
    );
}

#[test]
fn an_object_key_after_a_comma_is_not_a_read() {
    assert_eq!(
        names_read("call({ tabs: 1, openSlug: 2 })", &["tabs", "openSlug"]),
        Vec::<String>::new()
    );
}
