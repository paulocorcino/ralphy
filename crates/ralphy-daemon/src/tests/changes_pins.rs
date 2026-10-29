//! The Monaco swap, the changes list, the discard control and two shell bindings, pinned over the served assets.

use super::*;

/// #308 pins the editor swap where it can actually regress: the embedded
/// asset tree. Monaco is vendored, CodeMirror is gone, and the four heavy
/// language workers stay excluded (the exclusion rule is prefix-based
/// because Monaco 0.56 ships content-hashed chunk names).
#[test]
fn monaco_replaced_codemirror_in_the_embedded_ui() {
    let paths = embedded_ui_paths();

    for required in [
        "vendor/monaco/vs/loader.js",
        "vendor/monaco/vs/editor/editor.main.js",
    ] {
        assert!(
            paths.iter().any(|p| p == required),
            "{required} must be embedded in the UI assets"
        );
    }

    let leftover: Vec<_> = paths
        .iter()
        .filter(|p| p.starts_with("vendor/codemirror/"))
        .collect();
    assert!(
        leftover.is_empty(),
        "CodeMirror must be deleted from the tree, found: {leftover:?}"
    );

    for worker in [
        "vs/assets/ts.worker",
        "vs/assets/css.worker",
        "vs/assets/html.worker",
        "vs/assets/json.worker",
    ] {
        let hits: Vec<_> = paths.iter().filter(|p| p.contains(worker)).collect();
        assert!(
            hits.is_empty(),
            "language worker {worker} must not be vendored, found: {hits:?}"
        );
    }

    // The other three exclusion rows of docs/WORKBENCH-BUILD-GUIDE.md, so a
    // future Monaco bump that re-copies the tarball wholesale fails here
    // rather than silently doubling the payload.
    for excluded in ["vendor/monaco/vs/language/", "vendor/monaco/vs/nls/"] {
        let hits: Vec<_> = paths.iter().filter(|p| p.starts_with(excluded)).collect();
        assert!(
            hits.is_empty(),
            "{excluded} must not be vendored, found: {hits:?}"
        );
    }
    let typed: Vec<_> = paths
        .iter()
        .filter(|p| {
            p.starts_with("vendor/monaco/") && (p.ends_with(".d.ts") || p.ends_with(".map"))
        })
        .collect();
    assert!(
        typed.is_empty(),
        "no .d.ts / .map may be vendored with Monaco, found: {typed:?}"
    );

    assert!(
        include_str!("../../assets/ui/wb-monaco.ts").contains("monaco.editor.create"),
        "wb-monaco.ts must build the editor through Monaco's own factory"
    );

    let viewer = include_str!("../../assets/ui/wb-file-viewer.ts");
    assert!(
        viewer.contains("WBMonaco.create"),
        "wb-file-viewer.ts must mount its editor through WBMonaco"
    );
    // The mirror pane (ADR-0037 §3c) is a SECOND editor over the pane's
    // model, never a second model: `wb_monaco_308.py` counts models per
    // open pane and a mirror must not move that count.
    assert!(
        viewer.contains("WBMonaco.createOver"),
        "wb-file-viewer.ts must mount the mirror through WBMonaco.createOver"
    );
    // Built from parts so this pin cannot trip on its own source text.
    let outgoing = concat!("Code", "Mirror(");
    assert!(
        !viewer.contains(outgoing),
        "wb-file-viewer.ts must not construct a {outgoing} editor"
    );
}

/// #309's deliverable is JS/HTML no Rust gate compiles — this pin over the
/// served assets is the same reason the #308 Monaco pin above and
/// `usage.rs:544` exist, so `cargo test` reds after a deletion.
#[test]
fn the_changes_section_renders_a_status_marked_list() {
    let html = super::INDEX_HTML;
    assert!(
        html.contains(r#"class="changes-list""#),
        "index.html must render the changes-list"
    );
    assert!(
        html.contains(r#"class="chg-mark""#),
        "index.html must render a chg-mark per row"
    );
    assert!(
        html.contains("showSideView('changes')"),
        "index.html must wire the rail's Changes button to showSideView"
    );

    // The status markers, the diff target, the staged/unstaged split and the
    // write-control folds of `wb-changes.js` are driven by
    // `ui-tests/wb-changes.test.mjs`; this test holds the markup they feed.

    // The diff tab (#311): the row's click wiring and the diff editor's
    // factory.
    assert!(
        html.contains("openDiff($store.projects.openSlug, c)"),
        "index.html must open a diff from a changes row"
    );
    assert!(
        include_str!("../../assets/ui/wb-file-viewer.ts").contains("WBMonaco.createDiff"),
        "wb-file-viewer.ts must mount the diff through WBMonaco.createDiff"
    );

    // The staged/unstaged split (#315): the row's two halves, the group
    // headline, and the per-group `:key` prefix without which Alpine collides
    // a staged-then-modified path's two rows.
    for pin in [
        r#"class="chg-name""#,
        r#"class="chg-dir""#,
        r#"class="chg-group-head""#,
        ">Staged changes<",
        // BOTH keys: pinning only the staged one stays green if the two
        // templates are keyed identically.
        "'s:' + c.path",
        "'u:' + c.path",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the changes-group pin {pin}"
        );
    }

    // The promotion to a rail view (#317): the view's root, the rail icon
    // that reaches it, and the per-project indicator that keeps the count
    // reachable with no navigation.
    for pin in [
        r#"class="changes-view""#,
        r#"x-icon="'git-compare'""#,
        r#"class="chg-badge""#,
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the rail-view pin {pin}"
        );
    }
    // NEGATED: the accordion is removed, not left standing beside the view.
    // Both strings occurred ONLY in the block #317 deleted, so either one
    // reappearing means the section came back.
    for gone in ["changes-sec", "toggleChanges"] {
        assert!(
            !html.contains(gone),
            "index.html must not resurrect the Changes accordion ({gone})"
        );
    }

    // The write controls (#318). The `node --test` suite CI runs never
    // renders markup and Playwright does not run there, so these substring
    // pins are the only CI-visible check of this markup — every control the
    // panel's write gesture needs is named.
    for pin in [
        r#"data-act="stage""#,
        r#"data-act="unstage""#,
        r#"data-act="stage-all""#,
        r#"data-act="unstage-all""#,
        r#"class="chg-commit""#,
        r#"x-model="commitMsg""#,
        "writeLocked()",
        "commitTarget().label",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the write-control pin {pin}"
        );
    }
    // NEGATED: the message box is no longer inert. That title string existed
    // ONLY on the disabled `.chg-msg`, so its reappearance means the box
    // regressed to a placeholder.
    assert!(
        !html.contains("inert until the write controls land"),
        "the commit message box must no longer declare itself inert (#318)"
    );
}

/// The discard control (#319) — the same CI-visible substring gate the write
/// controls get, plus the NEGATED pin that keeps a group-level "discard all"
/// out: the issue asks for one file at a time, and a one-tap discard of every
/// path is precisely the mis-tap PRD #297 refused to ship.
#[test]
fn the_discard_control_is_pinned_in_the_markup() {
    let html = super::INDEX_HTML;

    for pin in [
        r#"data-act="discard""#,
        r#"class="chg-group-note""#,
        "groupNote('unstaged')",
        "groupNote('staged')",
        "discardRow($store.projects.openSlug, c)",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the discard pin {pin}"
        );
    }
    assert!(
        !html.contains(r#"data-act="discard-all""#),
        "there is no group-level discard: one file at a time (#319)"
    );
    // A conflicted row is all worktree work, so it lands in the UNSTAGED
    // group — but `restore --worktree` refuses an unmerged path, so the
    // control must not be offered there either.
    assert!(
        html.contains(r#"x-show="c.status !== 'conflicted'""#),
        "the discard control must be withheld from a conflicted row (#319)"
    );
    // The "unstaged rows ONLY" invariant, as a CI-VISIBLE oracle: this is
    // markup, which the `node --test` suite does not render, and Playwright
    // does not run in CI — so scenario 2 of `wb_changes_319.py` cannot be
    // the only thing proving it. The staged
    // list is the block between its `x-for` key and that list's close.
    let staged_from = html
        .find(r#"'s:' + c.path"#)
        .expect("index.html must keep the staged group's x-for key");
    let staged_block = &html[staged_from..];
    let staged_end = staged_block
        .find("</ul>")
        .expect("the staged group's list must close");
    assert!(
        !staged_block[..staged_end].contains(r#"data-act="discard""#),
        "the staged group must carry NO discard control — unstage comes first (#319)"
    );
}

/// A liveness flag has to be derived on a CLOCK, never inside the binding.
///
/// The uptime's `stale` class was bound to an inline
/// `Date.now() - _lastHeartbeat > 6000`. That expression can never fire: the
/// only reactive value in it is written by the heartbeat, so ticks STOPPING
/// — the very event it exists to report — is the one thing that does not
/// re-render it. A dead daemon read exactly like a live one, and the class
/// had no CSS either, so nothing on screen ever disagreed with the bug.
///
/// Pinned here because no node test renders this binding, and the failure is
/// silent by construction: the binding is present, the class is spelled correctly, and
/// the only symptom is an alarm that stays quiet.
#[test]
fn presence_staleness_is_derived_on_a_clock_not_inside_the_binding() {
    let html = super::INDEX_HTML;
    assert!(
        html.contains(r#":class="{ stale: $store.connection.presenceStale }""#),
        "the uptime must bind the DERIVED flag"
    );
    assert!(
        !html.contains("Date.now() - _lastHeartbeat"),
        "staleness must not be computed inside a binding — it cannot re-fire"
    );
    let app = shell_family();
    assert!(
        app.contains("presenceStale: false,"),
        "the shell must declare the flag as reactive state"
    );
    // Inside the tick, not merely somewhere in the file: a computation that
    // is not on a clock is the bug this test is named after. The shell and
    // the store `connection` each keep a one-second tick: one of them.
    let ticks: Vec<&str> = app
        .split("this._clockTick = setInterval(")
        .skip(1)
        .map(|tick| &tick[..tick.find("}, 1000);").expect("the clock tick must close")])
        .collect();
    assert!(!ticks.is_empty(), "the shell must keep its clock tick");
    assert!(
        ticks
            .iter()
            .any(|tick| tick.contains("this.presenceStale =")),
        "the clock tick must recompute staleness — that is what makes it observable"
    );
    // The class must also LAND: it was bound and styled nowhere, so the flag
    // being right would still have shown the operator nothing.
    let css = served_css();
    assert!(
        css.contains(".uptime.stale {"),
        "styles.css must give the stale uptime a visible state"
    );
}

/// The routing head never reaches an operator-facing string. A peer repo is
/// `<daemon_id>/<owner>/<repo>` on the wire (ADR-0052 §5); the ULID is how
/// the fleet routes, not what the repo is called, and rendering it raw is
/// how a WSL project came to be titled `01KY…/paulocorcino/vibeforge`.
///
/// The fold itself (`refSlug`, `refLabel`) is driven by
/// `ui-tests/wb-fleet.test.mjs`. This test holds what no node test renders:
/// the script tags and the markup that must call it.
#[test]
fn the_workbench_never_titles_a_repo_with_its_routing_head() {
    // The popups load `wb-file-viewer.ts`/`wb-console.ts`, which now call the
    // fold — without the import or tag the label silently falls back to the
    // ref in exactly the two windows nobody tests by hand.
    for page in [super::DETACHED_HTML, super::DETACHED_FENCE_HTML] {
        assert!(
            with_module_imports(tag_references(page)).contains(&"wb-fleet.js".to_string()),
            "a detached popup must load the fold it calls"
        );
    }
    let store = include_str!("../../assets/ui/wb-projects-store.ts");
    assert!(
        store.contains("projectLabel(ref: string | null | undefined) {"),
        "wb-projects-store.ts must keep the label helper the markup binds to"
    );
    // The crumb is the surface that STARTED this, and it no longer exists:
    // it went with the top bar, because the sidebar already names the open
    // project on its top row. So the pin moves to the surfaces that still
    // put the project's name on screen. It is deliberately NOT
    // `contains("$store.projects.projectLabel($store.projects.openSlug)")` any more — ten call sites satisfy
    // that substring, so it would stay green with every one of these
    // surfaces reverted to the raw ref.
    let html = super::INDEX_HTML;
    for pin in [
        r#"<span class="kanban-scope" x-text="$store.projects.openSlug ? $store.projects.projectLabel("#,
        r#"<span class="spend-project" x-text="$store.projects.projectLabel($store.projects.openSlug)""#,
    ] {
        assert!(
            html.contains(pin),
            "a surface that names the open project must render the LABEL: {pin}"
        );
    }
    // The two raw-ref spellings, banned file-wide: the crumb's own (kept so
    // the regression that named this test can never return by that route)
    // and the bare binding any new surface would reach for first.
    for anti in [
        r#"x-text="$store.projects.openSlug ?? 'no project'""#,
        r#"x-text="$store.projects.openSlug""#,
    ] {
        assert!(
            !html.contains(anti),
            "index.html must never print the routing head raw: {anti}"
        );
    }
    // The console name's prefix is the project name's last segment (ADR-0066
    // §2 and its 2026-10-02 amendment); an unnamed ref falls back to the
    // slug. Taken from the ref, a peer console would be named after its ULID.
    // The prefix is in the title, the fallback in the console.
    assert!(
        include_str!("../../assets/ui/wb-console-title.ts")
            .contains("prefixOf(projectNameOf(repo))")
            && console_family().contains("WBFleet.refSlug(ref)"),
        "a console name prefix must come from the project name or the slug, not the ref"
    );
}
