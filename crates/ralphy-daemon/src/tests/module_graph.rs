//! The module graph of the workbench: scripts, event names, Alpine components and entry modules.

use super::*;

/// The `src` of each `<script type="module">` tag of a page.
fn module_tags(html: &str) -> Vec<String> {
    html.match_indices(r#"<script type="module" src=""#)
        .filter_map(|(at, open)| {
            let rest = &html[at + open.len()..];
            rest.find('"').map(|end| rest[..end].to_string())
        })
        .collect()
}

/// The first-party classic scripts not yet moved to TypeScript (ADR-0075
/// D9). A ratchet: a file leaves the list in the change that moves it, and
/// nothing joins it. Empty: every first-party `.js` file is refused.
const CLASSIC_SCRIPTS: &[&str] = &[];

/// The `window.WB*` names a module still sets. A name stays only while a
/// reader outside the modules reads it on `window`: the `index.html` markup,
/// another page's code (`popup.WBViewer`), or a browser check. A module reads
/// a module's export by `import`; a page instance (`WB`, `WBConsole`,
/// `WBDaemon`, `WBNotes`, `WBViewer`) is the entry module's to set (ADR-0075
/// D9). A ratchet like `CLASSIC_SCRIPTS`.
const MODULE_WINDOW_NAMES: &[&str] = &[
    "WB",
    "WBConsole",
    "WBConsoleName",
    "WBDaemon",
    "WBKanban",
    "WBNotes",
    "WBRuns",
    "WBSpend",
    "WBView",
    "WBViewer",
];

/// ADR-0075 D1, D2, D8 and D10: first-party workbench code is TypeScript. A
/// `.js` file in the source tree is a vendored file or a classic script on the
/// list that only gets shorter; a module sets no `window.WB*` name that no
/// reader outside the modules needs; the `file://` demo does not come back.
#[test]
fn first_party_scripts_move_to_typescript_and_never_back() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut classic = Vec::new();
    let mut modules = Vec::new();
    for entry in std::fs::read_dir(&src).expect("assets/ui is readable") {
        let name = entry
            .expect("an entry of assets/ui")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.ends_with(".js") {
            classic.push(name);
        } else if name.ends_with(".ts") && !name.ends_with(".d.ts") {
            modules.push(name);
        }
    }
    for name in &classic {
        assert!(
            CLASSIC_SCRIPTS.contains(&name.as_str()),
            "assets/ui/{name} is a new classic script: first-party code is a .ts module"
        );
    }
    for name in CLASSIC_SCRIPTS {
        assert!(
            classic.iter().any(|c| c == name),
            "{name} is no longer a classic script: remove it from CLASSIC_SCRIPTS"
        );
    }
    for name in &modules {
        let text = std::fs::read_to_string(src.join(name)).expect("a module is readable");
        for (at, _) in text.match_indices("window.WB") {
            let rest = &text[at + "window.".len()..];
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if rest[ident.len()..].trim_start().starts_with("= ") {
                assert!(
                    MODULE_WINDOW_NAMES.contains(&ident.as_str()),
                    "{name} sets window.{ident}: a module exports; it sets a window name only while markup, another page or a browser check reads it"
                );
            }
        }
    }
    assert!(
        !src.join("wb-mode.js").exists() && !src.parent().expect("assets").join("ui-demo").exists(),
        "the file:// demo is archived under the tag workbench-demo-archive"
    );
}

/// The `workbench:*` names written in `text`: each `workbench:` followed by a
/// lowercase name. `workbench:*` and the prose "the workbench: a …" name none.
fn workbench_event_names(text: &str) -> Vec<String> {
    text.match_indices("workbench:")
        .map(|(at, tag)| {
            text[at + tag.len()..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                .collect::<String>()
        })
        .filter(|name| name.starts_with(|c: char| c.is_ascii_lowercase()))
        .collect()
}

/// The names `globals.d.ts` lists in `interface <map> { … }`, one
/// `"workbench:<name>": CustomEvent<…>;` line each.
fn typed_event_names(globals: &str, map: &str) -> std::collections::BTreeSet<String> {
    let open = format!("interface {map} {{");
    let start = globals
        .find(&open)
        .unwrap_or_else(|| panic!("globals.d.ts must declare {open}"));
    let body = &globals[start + open.len()..];
    let body = &body[..body.find("\n}").expect("the interface closes")];
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("\"workbench:"))
        .filter_map(|rest| rest.split_once("\":").map(|(name, _)| name.to_string()))
        .collect()
}

/// #633: the `workbench:*` events are one typed list, in `globals.d.ts`. A
/// send through `wb-events.ts` with a name not in it fails `tsc`; a listener,
/// a `$dispatch` in the markup or a `.window` listener does not, so every
/// name in a module and in `index.html` is read here, both names of a ternary
/// included.
#[test]
fn every_workbench_event_name_is_in_the_typed_list() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let globals =
        std::fs::read_to_string(src.join("globals.d.ts")).expect("globals.d.ts is readable");
    let window = typed_event_names(&globals, "WindowEventMap");
    let document = typed_event_names(&globals, "DocumentEventMap");
    // NEGATIVE CONTROL: the lists are read. An empty list would pass nothing,
    // and a parser that read nothing would report every name.
    assert!(
        window.contains("project-changed") && document.contains("action"),
        "the typed lists were not read from globals.d.ts: {window:?} {document:?}"
    );
    let mut used = std::collections::BTreeSet::new();
    let mut unknown = Vec::new();
    for entry in std::fs::read_dir(&src).expect("assets/ui is readable") {
        let name = entry
            .expect("an entry of assets/ui")
            .file_name()
            .to_string_lossy()
            .into_owned();
        let module = name.ends_with(".ts") && !name.ends_with(".d.ts");
        if !module && name != "index.html" {
            continue;
        }
        let text = std::fs::read_to_string(src.join(&name)).expect("a module is readable");
        for event in workbench_event_names(&text) {
            if !window.contains(&event) && !document.contains(&event) {
                unknown.push(format!("{name}: workbench:{event}"));
            }
            used.insert(event);
        }
    }
    assert!(
        unknown.is_empty(),
        "a workbench event name that globals.d.ts does not list (WindowEventMap or DocumentEventMap):\n{}",
        unknown.join("\n")
    );
    let unused: Vec<_> = window
        .iter()
        .chain(&document)
        .filter(|n| !used.contains(*n))
        .collect();
    assert!(
        unused.is_empty(),
        "globals.d.ts lists workbench events that no module and no markup uses: {unused:?}"
    );
    // A dialog hears its `-open` event with `.window`: that name is a window event.
    let html = include_str!("../../assets/ui/index.html");
    for (at, _) in html.match_indices("@workbench:") {
        let rest = &html[at + 1..];
        let event = workbench_event_names(rest)
            .into_iter()
            .next()
            .unwrap_or_default();
        if rest["workbench:".len() + event.len()..].starts_with(".window") {
            assert!(
                window.contains(&event),
                "index.html hears workbench:{event} on the window, and WindowEventMap does not list it"
            );
        }
    }
}

/// ADR-0073 D3: an Alpine component can be built without Alpine. Each
/// component `main.ts` registers is the exported factory of an ES module
/// (ADR-0075 D2), and that module has its own `ui-tests/<file>.test.mjs`,
/// where `loadComponent` checks the `uses` list of ADR-0073 D4. An Alpine
/// store (D6) is built the same way.
#[test]
fn every_alpine_component_can_be_built_and_has_a_test() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui-tests");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let main = include_str!("../../assets/ui/main.ts");
    let mut found = 0;
    for (name, factory) in alpine_data_calls(main)
        .into_iter()
        .chain(alpine_store_calls(main))
    {
        if name == "shell" {
            continue;
        }
        found += 1;
        let stem = main
            .lines()
            .filter(|l| l.starts_with("import {"))
            .find(|l| l.contains(&format!(" {factory} ")) || l.contains(&format!(" {factory},")))
            .and_then(|l| l.split_once("from \"./")?.1.strip_suffix(".ts\";"))
            .unwrap_or_else(|| {
                panic!("main.ts registers {name} and imports no {factory} from a sibling")
            });
        let module = std::fs::read_to_string(src.join(format!("{stem}.ts")))
            .unwrap_or_else(|e| panic!("read {stem}.ts: {e}"));
        assert!(
            module.contains(&format!("export function {factory}(")),
            "{stem}.ts must export the factory {factory}, so node --test can build {name}"
        );
        assert!(
            tests.join(format!("{stem}.test.mjs")).is_file(),
            "{stem}.ts holds the component {name} and has no ui-tests/{stem}.test.mjs"
        );
    }
    assert!(
        found >= 7,
        "expected at least the six dialog and section components and the projects store, found {found}"
    );
}

/// The `(name, factory)` of each `Alpine.store("name", factory());` line.
fn alpine_store_calls(js: &str) -> Vec<(&str, &str)> {
    js.lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("Alpine.store(\"")?
                .strip_suffix("());")
        })
        .filter_map(|l| l.split_once("\", "))
        .collect()
}

/// The `(name, factory)` of each `Alpine.data("name", factory);` line.
fn alpine_data_calls(js: &str) -> Vec<(&str, &str)> {
    js.lines()
        .filter_map(|l| l.trim().strip_prefix("Alpine.data(\"")?.strip_suffix(");"))
        .filter_map(|l| l.split_once("\", "))
        .collect()
}

/// ADR-0075 D5: each page loads one entry module, and only the entry of
/// `index.html` registers Alpine components and starts Alpine, once. A
/// name in an `x-data` that `main.ts` does not register is an element
/// Alpine cannot build, which no node test renders.
#[test]
fn each_page_starts_from_one_entry_module() {
    for (page, html, entry) in [
        (
            "index.html",
            include_str!("../../assets/ui/index.html"),
            "main.js",
        ),
        (
            "detached.html",
            include_str!("../../assets/ui/detached.html"),
            "detached-main.js",
        ),
        (
            "detached-fence.html",
            include_str!("../../assets/ui/detached-fence.html"),
            "detached-fence-main.js",
        ),
    ] {
        assert_eq!(
            module_tags(html),
            vec![entry.to_string()],
            "{page} loads one entry module"
        );
        assert!(
            !tag_references(html)
                .iter()
                .any(|r| r.starts_with("vendor/alpine")),
            "{page} loads Alpine only through main.ts"
        );
    }
    let main = UI
        .get_file("main.js")
        .and_then(|f| f.contents_utf8())
        .expect("main.js is embedded UTF-8");
    assert!(
        main.contains(r#"import Alpine from "./vendor/alpine.esm.js";"#),
        "main.js imports the vendored Alpine ES module build, at its own path"
    );
    for path in embedded_ui_paths() {
        if path.starts_with("vendor/") || !path.ends_with(".js") || path == "main.js" {
            continue;
        }
        let text = UI
            .get_file(&path)
            .and_then(|f| f.contents_utf8())
            .unwrap_or_else(|| panic!("{path} is embedded UTF-8"));
        for call in ["Alpine.start(", "Alpine.data(", "Alpine.directive("] {
            assert!(
                !text.contains(call),
                "{path} calls {call}: only main.ts does"
            );
        }
    }
    assert_eq!(
        main.matches("Alpine.start()").count(),
        1,
        "main.ts starts Alpine once"
    );

    let index = include_str!("../../assets/ui/index.html");
    let mut bound: Vec<&str> = index
        .split("x-data=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(name, _)| name))
        .collect();
    bound.sort_unstable();
    let mut registered: Vec<&str> = alpine_data_calls(main)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    registered.sort_unstable();
    assert_eq!(
        registered, bound,
        "main.ts registers exactly the x-data names of index.html"
    );
}

/// The order inside each entry module is a load order: `app.ts` and the
/// page modules read the instances on `window`, a torn-off page posts its
/// "ready" message from its wire function (the opener's reply reads the
/// instances at once), and the consoles boot after Alpine has started, as
/// they did when `DOMContentLoaded` triggered the boot (ADR-0075 phase 5).
/// No unit test runs an entry module, so the order is pinned here.
#[test]
fn each_entry_module_creates_the_instances_before_it_wires_the_page() {
    for (entry, text, order) in [
        (
            "main.ts",
            include_str!("../../assets/ui/main.ts"),
            &[
                "const messages = createMessages(",
                "window.WBConsole = createConsole(",
                "window.WBDaemon = createDaemon(",
                "window.WBViewer = createViewer(",
                "window.WBNotes = createNotes(",
                "wire(window, document, { messages });",
                "Alpine.start();",
                "window.WBConsole.boot();",
            ][..],
        ),
        (
            "detached-main.ts",
            include_str!("../../assets/ui/detached-main.ts"),
            &[
                "const messages = createMessages(",
                "window.WBViewer = createViewer(",
                "wireDetached(window, document);",
            ][..],
        ),
        (
            "detached-fence-main.ts",
            include_str!("../../assets/ui/detached-fence-main.ts"),
            &[
                "const messages = createMessages(",
                "window.WBConsole = createConsole(",
                "window.WBNotes = createNotes(",
                "wireDetachedFence(window, document);",
                "window.WBConsole.boot();",
            ][..],
        ),
    ] {
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        let mut last = 0;
        for step in order {
            let at = code
                .find(step)
                .unwrap_or_else(|| panic!("{entry} must contain `{step}`"));
            assert!(
                at >= last,
                "{entry}: `{step}` must come after the steps listed before it"
            );
            last = at;
        }
    }
}
