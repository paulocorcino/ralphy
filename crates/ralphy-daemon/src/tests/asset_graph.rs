//! Every shell links the whole cascade, and every asset is reachable from a shell.

use super::*;

/// Every document links every partial, in one order.
///
/// The partials are ONE cascade cut into thirteen files, so this is not a
/// convention — it is the thing that makes them equivalent to the file they
/// came from. A document that links eleven of them is missing rules; a
/// document that links them in a different order gets different winners for
/// every selector declared in two sections. Neither failure is visible to a
/// substring pin, and the second is not visible to a human reading a diff.
#[test]
fn every_shell_links_the_whole_cascade() {
    let mut expected: Vec<String> = UI
        .get_dir("styles")
        .expect("the stylesheet partials must be embedded")
        .files()
        .map(|f| f.path().to_string_lossy().replace('\\', "/"))
        // `.css` only, matching `served_css()`: anything else dropped into
        // `styles/` is not part of the cascade and must not be demanded as a
        // `<link>` in all three documents.
        .filter(|path| path.ends_with(".css"))
        .collect();
    expected.sort();

    for (shell, html) in [
        ("index.html", super::INDEX_HTML),
        ("detached.html", super::DETACHED_HTML),
        ("detached-fence.html", super::DETACHED_FENCE_HTML),
    ] {
        let linked: Vec<String> = tag_references(html)
            .into_iter()
            .filter(|r| r.starts_with("styles/"))
            .collect();
        assert_eq!(
            linked, expected,
            "{shell} must link every stylesheet partial in numeric order — \
             the files are one cascade, and order decides which \
             declaration wins"
        );
    }
}

/// The sweep set is not allowed to quietly shrink.
///
/// [`swept_ui_assets`] is derived, so nobody can forget to add a file to it —
/// but somebody could narrow the filter, or move an asset under `vendor/`,
/// and every sweep would go green over less. This states the floor: the
/// three shells, and a count no smaller than the tree had when the sweeps
/// stopped being hand-written lists.
#[test]
fn the_sweep_set_covers_every_asset_we_wrote() {
    let swept = swept_ui_assets();
    for shell in ["/", "/popup", "/fence"] {
        assert!(
            swept.iter().any(|p| p == shell),
            "{shell} must be swept — it is a served document this repo wrote"
        );
    }
    let js = swept.iter().filter(|p| p.ends_with(".js")).count();
    assert!(
        js >= 19,
        "expected at least the 19 non-vendor .js assets the tree held when \
         the sweeps became tree-driven, found {js} — did the filter narrow?"
    );
    // NEGATIVE CONTROL: a sweep set that swallowed the vendor bundles would
    // satisfy every count above while making each sweep meaningless (Monaco
    // says "mock" in its own test fixtures).
    assert!(
        !swept.iter().any(|p| p.starts_with("/vendor/")),
        "vendor bytes are not our copy and must stay out of the sweep set"
    );
}

/// The three shells and the embedded tree agree, in both directions.
///
/// This is the regression a file split actually causes, and until now
/// NOTHING caught it: you move a fold out of `app.ts` into `wb-foo.js`, and
/// you forget the `<script>` tag — or you add it to `index.html` and not to
/// `detached-fence.html`, which loads its own subset of the same modules.
/// Every substring pin in this file still passes, every `node --test` still
/// passes, and the workbench is broken in the browser.
///
/// Both directions matter, and they fail differently:
///
/// - A tag pointing at nothing is a 404 at boot — the module never defines
///   its global and the first caller dies on `undefined`.
/// - An asset no shell references is dead weight shipped in the binary, and
///   more often it means the tag was DROPPED rather than the file added.
#[test]
fn every_shell_tag_resolves_and_every_asset_is_reachable() {
    const SHELLS: [(&str, &str); 3] = [
        ("index.html", super::INDEX_HTML),
        ("detached.html", super::DETACHED_HTML),
        ("detached-fence.html", super::DETACHED_FENCE_HTML),
    ];
    let embedded = embedded_ui_paths();
    let mut referenced: Vec<String> = Vec::new();

    for (shell, html) in SHELLS {
        for reference in tag_references(html) {
            // The seed loader is a template literal resolved at runtime, and
            // it points OUTSIDE the embedded tree on purpose (#300) — the
            // `the_served_ui_carries_no_seed` sweep owns that path.
            if reference.contains("${") || reference.starts_with("../") {
                continue;
            }
            assert!(
                embedded.iter().any(|p| p == &reference),
                "{shell} references {reference}, which is not in the embedded \
                 tree — the browser gets a 404 for it at boot"
            );
            referenced.push(reference);
        }
    }

    // The other direction, and it must be PER SHELL. A union-wide "somebody
    // references it" is too weak to catch the split regression: drop
    // `wb-fleet.js` from index.html and the union is still satisfied by the
    // two popups, which is exactly the shape that ships a broken desk.
    //
    // `index.html` IS the desk, so its required set is derived and can never
    // go stale: every non-vendor asset this repo writes is loaded there. A
    // new module is protected the moment it is embedded, with no list to
    // remember.
    // A module reached by an `import` needs no tag of its own (ADR-0075 D5).
    let index = SHELLS[0].1;
    let index_refs = with_module_imports(tag_references(index));
    // A popup's entry module, and the page script that entry calls, are the
    // assets only their own page loads.
    let popup_entries = [
        "detached-main.js",
        "wb-detached.js",
        "detached-fence-main.js",
        "wb-detached-fence.js",
    ];
    for path in embedded_ui_paths() {
        if path.starts_with("vendor/") || popup_entries.contains(&path.as_str()) {
            continue;
        }
        if !path.ends_with(".js") && !path.ends_with(".css") {
            continue;
        }
        assert!(
            index_refs.iter().any(|r| r == &path),
            "{path} is embedded but index.html has no tag for it — either the \
             tag was dropped, or the asset is dead weight in the binary"
        );
    }

    // The two popups load SUBSETS, so theirs cannot be derived from the tree
    // — a module they legitimately do not need is not a defect. What each
    // one needs is stated here, and it is the floor: adding a module to a
    // popup never reds this, dropping one always does.
    for (shell, required) in [
        (
            "detached.html",
            &[
                "wb-fleet.js",
                "wb-detached.js",
                "wb-monaco.js",
                "wb-file-viewer.js",
                "wb-events.js",
                "wb-messages.js",
            ][..],
        ),
        (
            "detached-fence.html",
            &[
                "wb-fleet.js",
                "wb-fail.js",
                "wb-detached-fence.js",
                "wb-desk-sink.js",
                "wb-desk-sync.js",
                "wb-detach-link.js",
                "wb-session-route.js",
                "wb-resume.js",
                "wb-daemon.js",
                "wb-events.js",
                // `wb-console.ts` imports the desk folds, the GPU budget, the
                // object that names its members (`wb-console-api.ts`, which
                // imports the geometry, the session folds, the input folds
                // and the resume rule), the title, the fence list, the detach, the view, the
                // terminal, the window chrome, the popup registry, the fences, the desk and its records and
                // restore, the console name,
                // the input folds, the key bar, the feeds, the dormancy, the window birth and the
                // placeholder, and they import the window state; the terminal imports its touch
                // gesture and its paste; the title imports the worktree switcher, the fences
                // import the fence gestures and the chrome the window gestures; the entry
                // imports the console, the door for operator messages, and the z stack and
                // the gestures. Stated HERE because this set is a hardcoded floor:
                // nothing derives the popup's needs from the tree, so an
                // import dropped from the entry breaks the second monitor
                // with no other signal.
                "wb-geometry.js",
                "wb-window-state.js",
                "wb-console-name.js",
                "wb-console-api.js",
                "wb-console-input.js",
                "wb-console-keys.js",
                "wb-console-feeds.js",
                "wb-console-dormancy.js",
                "wb-console-spawn.js",
                "wb-console-placeholder.js",
                "wb-console-session.js",
                "wb-desk-folds.js",
                "wb-console-gpu.js",
                "wb-console-fit.js",
                "wb-console-frames.js",
                "wb-console-press.js",
                "wb-console-terminal.js",
                "wb-console-touch.js",
                "wb-console-paste.js",
                "wb-console-title.js",
                "wb-console-checkout.js",
                "wb-stage-fence-gestures.js",
                "wb-stage-fence-list.js",
                "wb-desk-detach.js",
                "wb-stage-view.js",
                "wb-stage-chrome.js",
                "wb-stage-window-gestures.js",
                "wb-desk-popups.js",
                "wb-desk-records.js",
                "wb-desk-restore.js",
                "wb-stage-fences.js",
                "wb-stage-window.js",
                "wb-stage-columns.js",
                "wb-stage-stack.js",
                "wb-desk.js",
                "wb-console.js",
                "wb-messages.js",
            ][..],
        ),
    ] {
        let (_, html) = SHELLS
            .iter()
            .find(|(name, _)| *name == shell)
            .expect("the shell was just listed in SHELLS");
        let refs = with_module_imports(tag_references(html));
        for module in required {
            assert!(
                refs.iter().any(|r| r == module),
                "{shell} must load {module} — the popup is inert without it"
            );
        }
        // The stylesheet is thirteen partials now, and "links all of them, in
        // order" is a stronger statement than this one was — so it is made
        // once, for all three documents, by
        // `every_shell_links_the_whole_cascade`. What stays here is the
        // floor it rests on: a popup that links NO stylesheet at all.
        assert!(
            refs.iter().any(|r| r.starts_with("styles/")),
            "{shell} must load the stylesheet — an unstyled popup is a broken one"
        );
    }
}

/// A phone-width pane folds its chrome by the PANE's width, not the
/// viewport's — a viewer is narrow in a desktop split and in a detached
/// popup too. That criterion is a `@container` query on the pane, and it
/// only works if the pane declares itself a container: drop the
/// `container-type` and every rule inside the query goes silently inert,
/// with nothing on the JS side to notice. Pinned here for that reason, and
/// the threshold is pinned at ONE number because `wb-file-viewer.ts` measures
/// the same 560 to trim Monaco's gutter — two numbers would fold the
/// captions and the gutter at different widths.
#[test]
fn the_narrow_pane_criterion_is_the_panes_own_width() {
    let css = served_css();
    // Anchored at a line start: `#viewers.split > .viewer {` (the slot's
    // two-column override) contains the same characters and must not be
    // the block this reads.
    for (pane, name) in [("\n.viewer {", "viewer"), ("\n.kanban {", "kanban")] {
        let at = css
            .find(pane)
            .unwrap_or_else(|| panic!("{pane} must be styled"));
        let block = &css[at..at + css[at..].find('}').expect("a rule block closes")];
        assert!(
            block.contains("container-type: inline-size")
                && block.contains(&format!("container-name: {name}")),
            "{pane} must declare itself the `{name}` container, or its @container rules never fire"
        );
        let query = format!("@container {name} (max-width: 560px)");
        assert!(
            css.contains(&query),
            "the stylesheet must fold `{name}` at 560px: `{query}`"
        );
    }
    let js = include_str!("../../assets/ui/wb-file-viewer.ts");
    assert!(
        js.contains("const NARROW_PX = 560;"),
        "wb-file-viewer.ts must trim the gutter at the SAME 560px the stylesheet folds the captions"
    );
}

/// The UI suite's barrel cannot lie about what it runs.
///
/// `node --test <dir>` given a BARE directory resolves `package.json#main`
/// and runs that one file — it does not recurse. So `ui-tests/index.mjs` is
/// the whole suite, and a `*.test.mjs` that nobody imports there is not a
/// failing test: it is a file the runner never opens, silently. That is the
/// worst shape a test can have, and it is invisible to the JS side by
/// construction — the suite cannot notice a file it never loads. Only a
/// reader of the DIRECTORY can, which is why this gate is in Rust.
#[test]
fn every_ui_test_file_is_imported_by_the_barrel() {
    // Comment-stripped: an import parked behind `//` does not run, and the
    // whole point of this gate is that a file the runner never opens is not
    // a passing test.
    let barrel: String = include_str!("../../ui-tests/index.mjs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui-tests");
    let mut found = 0;
    for entry in std::fs::read_dir(&dir).expect("the ui-tests directory ships with the crate") {
        let name = entry
            .expect("a directory entry the read_dir call just yielded")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if !name.ends_with(".test.mjs") {
            continue;
        }
        found += 1;
        assert!(
            barrel.contains(&format!("import \"./{name}\";")),
            "ui-tests/{name} is never imported by ui-tests/index.mjs, so \
             `node --test crates/ralphy-daemon/ui-tests` never runs it"
        );
    }
    assert!(
        found >= 10,
        "expected the UI suite to hold at least the ten test files it had \
         when this gate was written, found {found} — did the glob break?"
    );
}

/// A part is put into its page at build time and is never served (ADR-0080
/// D2): no path of the embedded tree is a `*.part.html` file.
#[test]
fn no_part_file_is_served() {
    let paths = embedded_ui_paths();
    assert!(
        paths.iter().any(|p| p == "index.html"),
        "the embedded tree must hold the desk page: {} paths read",
        paths.len()
    );
    let parts: Vec<&String> = paths.iter().filter(|p| p.ends_with(".part.html")).collect();
    assert!(
        parts.is_empty(),
        "a part is put into its page and never served: {parts:?}"
    );
}
