//! Pins on the page's wiring: nudges, the explorer, the changes row, icons and the project row.

use super::*;

/// The run-completion nudge (#310) across the assets. `shouldReload` and
/// `subscribeChanges` are driven by `wb-changes.test.mjs` and
/// `wb-daemon.test.mjs`, and `toggle` remounting the subscription by
/// `wb-changes-panel.test.mjs`. No node test reaches the export
/// `wb-changes-panel.ts` guards on or the filter call inside
/// `mountChangesSub`, so those two are pinned here.
#[test]
fn the_run_completion_nudge_is_wired_through_the_ui_assets() {
    assert!(
        include_str!("../../assets/ui/wb-daemon.ts").contains("subscribeChanges,"),
        "wb-daemon.ts must EXPORT subscribeChanges — app.ts guards on it (#310)"
    );
    assert!(
        shell_family().contains("WBChanges?.shouldReload?.(frame, this.$store.projects.openSlug)"),
        "the shell must filter each nudge through shouldReload (#310)"
    );
}

/// The wake affordance, pinned from CI. `toggle` calling `wakePeerFor` is
/// driven by `app.test.mjs`, and `wakePeerFor` calling `wakePeer` by
/// `wb-projects-store.test.mjs`; `wakeable`
/// by `wb-fleet.test.mjs`. The markup and the body of `wakePeer` are reached
/// by no node test, and Playwright does not run in CI — these substrings are
/// the only CI-visible gate over the one consumer `/api/fleet/nudge` has, and
/// a route with no caller is a route that rots.
#[test]
fn the_peer_wake_is_wired_through_the_ui_assets() {
    let app_js = shell_family();
    for symbol in [
        r#"apiFetch("POST /api/fleet/nudge", { query: { daemon_id: daemonId } })"#,
        "async wakePeer(",
        // Readiness, not the spawn: a caller that acted on `nudged` would be
        // back to reporting a peer as woken while it is still booting.
        "woke?.ready",
    ] {
        assert!(
            app_js.contains(symbol),
            "the shell must keep {symbol} to wake a peer"
        );
    }
    let html = super::INDEX_HTML;
    assert!(
        html.contains(r#"@click.stop="$store.projects.wakePeer(g.daemon)""#),
        "the group header must carry the wake control"
    );
    assert!(
        html.contains(r#"x-show="$store.projects.peerWakeable(g)""#),
        "the wake control must be withheld from states a nudge cannot fix"
    );
    // Alpine stringifies an object nested inside a `:class` ARRAY, so the
    // object form emits a literal `[object Object]` class and the state it
    // meant to set never applies. Measured in the browser, since no unit test
    // in this tree renders Alpine.
    assert!(
        html.contains(
            r#":class="[g.state, $store.projects.peerFault(g) ? 'fault' : '', $store.projects.waking[g.daemon] ? 'waking' : '']""#
        ),
        "the wake glyph's :class must bind plain strings, never a nested object"
    );
    // `asleep` is the ordinary course of a day, not a fault. The rule is in
    // `stateFault` (driven by ui-tests/wb-fleet.test.mjs); the CSS only
    // colours what it marks.
    assert!(
        served_css().contains(".env-group .peer-state.fault {"),
        "styles must colour the state glyph `stateFault` marks"
    );
}

/// Creating must be reachable for every target the operator can point at:
/// a folder, a file (meaning its parent), and the repo root. The root has
/// no node — a right-click on empty tree space resolves to `null` — so the
/// context handler must NOT bail on a missing node, or a top-level file is
/// uncreatable. The Files header carries the same two actions, because
/// right-clicking empty space is an affordance nothing on screen advertises.
/// The directory `emitCreate` sends is driven by `wb-files.test.mjs`.
#[test]
fn the_explorer_can_create_at_every_target_including_the_repo_root() {
    let js = include_str!("../../assets/ui/wb-files.ts");
    let mount = include_str!("../../assets/ui/wb-files-mount.ts");
    assert!(
        mount.contains("files.showMenu(ev.clientX, ev.clientY, node || null)"),
        "the tree's contextmenu handler must open the menu for a NULL node \
         (empty space = the repo root), not return early"
    );
    assert!(
        js.contains("createHere(kind: \"file\" | \"folder\") {"),
        "wb-files.ts must keep createHere(kind) — the Files header calls it"
    );
    let html = super::INDEX_HTML;
    for symbol in ["createHere('file')", "createHere('folder')"] {
        assert!(
            html.contains(symbol),
            "index.html's Files header must wire {symbol}"
        );
    }
}

/// Naming a new entry goes through the design-system prompt, not the
/// browser's: `window.prompt` is unstyled, is suppressible for the whole
/// origin by one "prevent this page from creating more dialogues" tick, and
/// never renders in a detached popup. It stays only as the fallback for a
/// shell that cannot be reached. The create action calling `askPrompt`, and
/// `askPrompt` settling with the name `promptSubmit` accepts, are driven by
/// `app.test.mjs`; this pins the markup the dialog renders into.
#[test]
fn naming_a_new_entry_uses_the_design_system_prompt() {
    let html = super::INDEX_HTML;
    for symbol in ["prompt-modal", "id=\"prompt-input\"", "promptSubmit()"] {
        assert!(
            html.contains(symbol),
            "index.html must render the prompt dialog ({symbol})"
        );
    }
}

/// A Changes row's action buttons must sit OUTSIDE the clipped region.
///
/// `.chg-name` is frozen on purpose (`flex: 0 0 auto`, so an absurd name is
/// never ellipsized while a directory can still drain), which means a long
/// name genuinely overflows and something must clip. When that clip lived on
/// `.chg-row` and the buttons were the row's LAST children, the clip ate the
/// buttons: `docs/adr/0032-daemon-mode-supervised-launcher.md` pushed `+`/`×`
/// to x=384/414 against a row edge of 347, so a path with a long file name
/// could not be staged or discarded from the UI at all. `.chg-face` owns the
/// overflow now and the controls are its siblings.
///
/// Only a browser computes that geometry and CI runs none, so what is pinned
/// here is the structure the geometry follows from: the face wraps every
/// descriptive span, closes, and only then come the actions.
#[test]
fn a_changes_row_keeps_its_actions_outside_the_clipped_face() {
    let html = super::INDEX_HTML;
    let rows: Vec<&str> = html.matches("class=\"chg-row\"").collect();
    assert_eq!(
        rows.len(),
        2,
        "expected the staged and unstaged row templates; the count changed, \
         so re-check that each still keeps its actions outside the face"
    );
    for (i, block) in html.split("class=\"chg-row\"").skip(1).enumerate() {
        let row = block.split_once("</li>").map_or(block, |(head, _)| head);
        let face = row
            .find("class=\"chg-face\"")
            .unwrap_or_else(|| panic!("row {i} must wrap its spans in .chg-face"));
        let act = row
            .find("class=\"chg-act")
            .unwrap_or_else(|| panic!("row {i} must carry at least one .chg-act"));
        assert!(
            face < act,
            "row {i}: the face must open BEFORE the actions — an action inside \
             the overflow region is an action a long file name can clip away"
        );
        // The face must CLOSE before the first action. Counting `</span>`
        // alone cannot see that — each descriptive child closes itself — so
        // balance the tags: inside the face, every child pairs up, and the ONE
        // unmatched close is the face's own. Equal counts mean the face is
        // still open when the button arrives, which is the bug's exact shape.
        let inner = {
            let after_tag = row[face..]
                .find('>')
                .map(|gt| face + gt + 1)
                .expect("the .chg-face opening tag must close");
            &row[after_tag..act]
        };
        let opens = inner.matches("<span").count();
        let closes = inner.matches("</span>").count();
        assert!(
            inner.contains("chg-name"),
            "row {i}: the file name belongs inside the face; found: {inner:?}"
        );
        assert_eq!(
            closes,
            opens + 1,
            "row {i}: the face must close before the actions — {opens} span(s) \
             opened and {closes} closed, so the button is INSIDE the clipped \
             region a long file name overflows; found: {inner:?}"
        );
    }

    // The overflow belongs to the face. `.chg-row` keeps one only as a
    // backstop, and the face is what may shrink (`min-width: 0`).
    let css = served_css();
    let face = css
        .split_once(".chg-face {")
        .expect("styles.css must define .chg-face")
        .1
        .split_once('}')
        .expect("the .chg-face block must close")
        .0;
    for decl in ["overflow: hidden", "min-width: 0", "flex: 1 1 auto"] {
        assert!(
            face.contains(decl),
            ".chg-face must declare {decl} — it is the clipping, shrinkable \
             region; found: {face:?}"
        );
    }
}

/// Icons are drawn by the `x-icon` directive, which Alpine runs wherever it
/// initializes an element (#487). Nothing scans the document for lucide
/// placeholders any more, so a `data-lucide` attribute in the markup would
/// ship as an empty icon, and a `createIcons()` call would bring back the
/// scan that missed every `x-if` and `x-for` built after it ran (#332).
/// `wb_icons_487.py` owns the rendering; this pin keeps the markup on it.
#[test]
fn every_icon_is_drawn_by_the_x_icon_directive() {
    let html = super::INDEX_HTML;
    let js = shell_family();
    assert!(
        js.contains("export function iconDirective(")
            && include_str!("../../assets/ui/main.ts")
                .contains(r#"Alpine.directive("icon", iconDirective);"#),
        "main.ts must register the x-icon directive of the shell family"
    );
    assert!(
        html.matches("x-icon=").count() >= 40,
        "index.html must draw its icons with x-icon"
    );
    for (file, text) in [("index.html", html)]
        .into_iter()
        .chain(SHELL_FAMILY.iter().copied())
    {
        assert!(
            !text.contains("data-lucide=") && !text.contains("createIcons()"),
            "{file} must not use lucide placeholders or the createIcons scan"
        );
    }
}

/// A repo with no `origin` is keyed `path-<hash>` (ADR-0008 D7). That stays
/// the identity; only the LABEL becomes the directory basename (#332).
#[test]
fn a_remoteless_project_is_labelled_by_its_directory() {
    let js = shell_family();
    let load = js_method_body(&js, "async loadRepos({ git = true } = {}) {");
    assert!(
        load.contains("name: x.name"),
        "`loadRepos` must keep `/api/repos`'s name — the label reads it; \
         found: {load:?}"
    );

    // The label fold (`repoLabel`) is driven by `ui-tests/wb-project.test.mjs`,
    // except one clause no row reaches: an OWNER that starts with `path-`
    // (`path-org/tool`) is a GitHub repo and must not be relabelled off disk.
    let project = include_str!("../../assets/ui/wb-project.ts");
    let label = project
        .split_once("function repoLabel(p: Project) {")
        .expect("wb-project.ts must define repoLabel")
        .1
        .split_once("\n}")
        .expect("repoLabel must close at module indent")
        .0;
    assert!(
        label.contains("projectName(p)"),
        "the label is built on the daemon's project name; found: {label:?}"
    );

    let filter = js_method_body(&js, "filteredProjects() {");
    assert!(
        filter.contains("repoLabel("),
        "the filter must match the VISIBLE label: typing what the row prints \
         and getting an empty list is the defect a directory label would \
         otherwise introduce; found: {filter:?}"
    );

    let html = super::INDEX_HTML;
    assert!(
        html.contains(r#"x-text="repoLabel(p)""#),
        "the row must render the label through `repoLabel`"
    );
    assert!(
        html.contains(r#":data-slug="p.slug""#),
        "the label is a view concern; `.project-slug` must keep the canonical \
         ADR-0008 D7 slug in `data-slug` (the browser tests locate a row by it)"
    );
}

/// A twenty-character label in a column fixed at 300px wrapped the row to
/// two lines (#332).
#[test]
fn the_project_name_truncates_instead_of_wrapping() {
    let css = served_css();
    let body = css_rule_body(&css, ".project-slug {");
    for (decl, why) in [
        (
            "flex: 1 1 auto",
            "the name is the row's ONE growing child — it is what pushes \
             the branch chip to the right edge",
        ),
        (
            "min-width: 0",
            "a flex item does not shrink below its content without it; that, \
             not the missing ellipsis, is what produced the wrap",
        ),
        ("overflow: hidden", "the clip the ellipsis needs"),
        ("text-overflow: ellipsis", "the truncation marker"),
        ("white-space: nowrap", "one line"),
    ] {
        assert!(
            body.contains(decl),
            ".project-slug must declare {decl} — {why}; found: {body:?}"
        );
    }
}

/// The chip sits on the project row, merged with the change count: collapsed
/// it is only the count, open it grows the branch and switches it. The Files
/// bar is left to the checkout chip. Neither block nests a `<div>`, so slicing
/// each to its first `</div>` is exact.
#[test]
fn the_branch_chip_carries_the_change_count_on_the_project_row() {
    let html = super::INDEX_HTML;
    let slice = |open: &str| -> String {
        html.split_once(open)
            .unwrap_or_else(|| panic!("index.html must carry `{open}`"))
            .1
            .split_once("</div>")
            .unwrap_or_else(|| panic!("the `{open}` block must close"))
            .0
            .to_string()
    };

    let row = slice(r#"class="project-head""#);
    // Proves the slice LANDED. Without it the negative below passes
    // vacuously on any mis-sliced or empty string.
    assert!(
        row.contains("project-slug"),
        "the .project-head slice must contain the project name; found: {row:?}"
    );
    for needle in [
        r#"class="branch-chip""#,
        r#"class="chg-badge""#,
        "branchChipClick(p, $event)",
        r#"x-show="$store.projects.rowOpen(p)""#,
    ] {
        assert!(
            row.contains(needle),
            "the row's chip must carry {needle:?} — the count, and the branch \
             switcher once the row is open; found: {row:?}"
        );
    }
    assert!(
        row.contains("rowTitle(p)"),
        "a collapsed row shows no branch while `filteredProjects` still \
         matches on branch, so it must name the branch in its title; \
         found: {row:?}"
    );

    let files = slice(r#"class="side-head files-sec""#);
    assert!(
        files.contains("createHere('file')"),
        "the Files bar must keep its own actions; found: {files:?}"
    );
    assert!(
        !files.contains("branch-chip"),
        "the Files bar is the checkout chip's — a worktree name needs the \
         width; found: {files:?}"
    );
    for (pin, what) in [
        (r#"class="branch-chip""#, "chip"),
        (r#"class="chg-badge""#, "count"),
    ] {
        assert_eq!(
            html.matches(pin).count(),
            1,
            "one {what} per row — two would let the count and the branch \
             disagree about which tree they describe"
        );
    }

    // Capped at 48% on every row, the chip cost the name half its width
    // (#332). Now the name wins: the chip takes the squeeze first, and its
    // bound sits on the branch name — a `max-width` on the chip freezes it at
    // the cap (flex shrinks from the unclamped size) and squeezes the name.
    let css = served_css();
    let chip = css_rule_body(&css, ".branch-chip {");
    assert!(
        chip.contains("flex: 0 10000 auto") && !chip.contains("max-width"),
        ".branch-chip must shrink first and carry no max-width; found: {chip:?}"
    );
    let name = css_rule_body(&css, ".branch-chip-name {");
    assert!(
        name.contains("max-width:"),
        ".branch-chip-name must bound the chip; found: {name:?}"
    );
    let collapsed = css_rule_body(&css, ".branch-chip.collapsed {");
    assert!(
        collapsed.contains("padding: 0"),
        "a collapsed chip is only its count; found: {collapsed:?}"
    );

    let js = shell_family();
    for pin in [
        "rowTitle(p: Project) {",
        "branchChipClick(p: Project, ev: Event) {",
    ] {
        assert!(js.contains(pin), "the shell must define {pin}");
    }
}

/// The sidebar's left edge was ragged (0.8rem for the headers, search box
/// and rows; 0.5rem for the Changes toolbar and compose box), so tightening
/// only the project row would have made it worse. One token, six rules
/// (#332). `.runs-head` keeps its own value: it is not this column.
#[test]
fn the_sidebar_column_keeps_one_gutter() {
    let css = served_css();
    assert!(
        css.contains("--side-gutter:"),
        "the column's gutter must be a token, so it moves once"
    );
    for selector in [
        ".side-head {",
        ".side-search {",
        ".project-head {",
        ".chg-toolbar {",
        ".side-empty {",
        ".chg-compose {",
    ] {
        let body = css_rule_body(&css, selector);
        assert!(
            body.contains("var(--side-gutter)"),
            "`{selector}` shares the sidebar's left edge — a literal value \
             here re-rags the column; found: {body:?}"
        );
    }
}
