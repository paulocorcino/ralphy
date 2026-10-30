//! The run picker, the board's plan blocks and the runs feed.

use super::*;

/// The run picker answers "what is running?" with the MODEL, and "for how
/// long?" with a clock counting from the document's own phase anchor.
///
/// Both facts already existed on the wire and the panel was discarding them:
/// `fromSnapshot` dropped `model`/`effort`/`budget_min`, and the title bound
/// the vendor. A vendor name does not say which of sol/terra/luna is burning
/// quota, and nothing said whether a phase was two minutes or forty in.
#[test]
fn the_run_picker_names_the_model_and_clocks_the_phase() {
    // The folds behind it (`fromSnapshot`, `modelEffort`, `fmtClock`,
    // `phaseClock`, `runTitle`) are driven by `ui-tests/wb-runs.test.mjs`.
    let shell = super::INDEX_HTML;
    for pin in [
        r#"<span class="run-select-title" x-text="runTitle($store.runs.currentRun())"></span>"#,
        r#"<span class="run-sub-phase" x-text="runIdentity($store.runs.currentRun())"></span>"#,
        r#"<span class="run-clock" x-show="runClock($store.runs.currentRun())""#,
    ] {
        assert!(
            shell.contains(pin),
            "index.html must keep the picker pin {pin}"
        );
    }
    // The vendor MOVED, it was not deleted: `runIdentity` is what carries it,
    // and the negative pin is what stops the title reverting to it.
    assert!(
        !shell.contains(r#"x-text="$store.runs.currentRun()?.agent""#),
        "the picker's headline must be the model, not the vendor"
    );
    assert!(
        !shell.contains(r#"x-text="currentRun()?.agent""#),
        "the picker's headline must be the model, not the vendor"
    );

    let app_js = shell_family();
    let app: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
    // The tick is what makes the clock live, and reading `nowMs` in the getter
    // is what subscribes the binding to it — a `Date.now()` inside `runClock`
    // would leave the clock frozen with every pin above still green.
    assert!(
        app.contains("return WBRun.phaseClock(run, this.nowMs);"),
        "runClock must read the reactive `nowMs`, not the clock directly"
    );
    // The GUARD and the period, not the whole interval body: the tick is a
    // shared seam (presence staleness rides it too), and quoting every
    // statement in it made this run-panel test fail for a change that had
    // nothing to do with the run panel.
    assert!(
        app.contains("this._clockTick = setInterval(() => { if (this.runsOpen)"),
        "the shell must own a one-second tick for the phase clock"
    );
    assert!(
        app.contains("if (this.runsOpen) this.nowMs = Date.now();"),
        "the phase clock must advance only while the panel is open"
    );
    // The period of the shell's own tick, the one that holds the phase
    // guard: the store `connection` has a one-second presence tick of its
    // own, which the family also holds.
    let phase_tick = app
        .split("this._clockTick = setInterval(")
        .find(|tick| tick.starts_with("() => { if (this.runsOpen)"))
        .unwrap_or_default();
    assert!(
        phase_tick
            .find("}, ")
            .is_some_and(|end| phase_tick[end..].starts_with("}, 1000);")),
        "the phase clock must tick once a second"
    );
}

/// The design-system scrollbar is the DEFAULT, not a list of opted-in
/// selectors. It was the latter for four rounds, and every round shipped one
/// more surface wearing the platform's chrome bar — the operator reported the
/// same defect three times. This pin protects the inversion, so the next
/// scrolling surface is born correct instead of born reported.
///
/// It also protects two MEASURED facts that are easy to "tidy" back into a
/// bug (the browser-side numbers live in `tests/wb_scrollbars.py`):
///   * setting `scrollbar-color` makes the browser IGNORE that element's
///     `::-webkit-scrollbar` rules and fall back to the platform width, so the
///     two mechanisms cannot be combined — every such block this file used to
///     carry was dead code drawing a platform-width bar in ralphy's colours;
///   * `scrollbar-color` inherits but `scrollbar-width` DOES NOT, so the rule
///     must be `*` and not `html`, or the width silently stays `auto`.
#[test]
fn the_design_system_scrollbar_is_the_default_not_a_list() {
    let css = served_css();
    let squeezed: String = css.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed
            .contains("* { scrollbar-width: thin; scrollbar-color: var(--border) transparent; }"),
        "the default must be `*` — `scrollbar-width` does not inherit"
    );
    assert!(
        !squeezed.contains("html { scrollbar-width"),
        "a root rule themes the colour and leaves the width `auto` — measured"
    );
    // THE CONTRACT, as a negative, and the reason the mechanisms must not be
    // mixed: NO `::-webkit-scrollbar` RULE may return to this file. Pinning the
    // covered surfaces by name would instead be satisfied by re-adding the very
    // list being removed.
    //
    // Comments are stripped first, because a comment is exactly where this
    // pseudo-element SHOULD still be named — the explanation above the default
    // names it three times, and a pin that forbade the words would forbid the
    // reasoning along with the code.
    let mut code = String::with_capacity(css.len());
    let mut rest = css.as_str();
    while let Some(start) = rest.find("/*") {
        code.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    code.push_str(rest);
    assert!(
        !code.contains("::-webkit-scrollbar"),
        "a ::-webkit-scrollbar rule is inert beside `scrollbar-color` — do not re-add one"
    );
    // Monaco is the documented exception: it paints its own slider, so it is
    // the one surface no scrollbar property reaches.
    assert!(
        css.contains(".monaco-scrollable-element > .scrollbar > .slider {"),
        "Monaco's own slider must keep its palette rule"
    );
}

/// The board's label editor: not clipped, and not inviting a refused click.
///
/// Two independent defects. The menu was a `.dropdown` (absolutely
/// positioned) inside `.kd-inner`'s scroll inside `.kanban-detail`'s
/// `overflow: hidden`, so it was clipped on every card past the middle of the
/// drawer. And `label.set` is a run-lock-aware Mutate (`mutate.rs`'s
/// `guard_run_lock(&ws, "label set", …)`), so with a live run every toggle was
/// refused, the optimistic chip snapped back, and the editor was the ONE write
/// control in the shell with no gate.
#[test]
fn the_label_editor_is_unclipped_and_closed_under_a_live_run() {
    let shell = super::INDEX_HTML;
    // The clip fix is structural: it must stop being a floating dropdown.
    assert!(
        shell.contains(r#"<div class="kd-label-menu" x-show="labelMenuOpen""#),
        "the label menu must be in flow, not a `.dropdown`"
    );
    assert!(
        !shell.contains("dropdown kd-label-menu"),
        "a `.dropdown` label menu is clipped by the drawer — that is the defect"
    );
    let css = served_css();
    let squeezed: String = css.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed.contains(".kd-label-menu { flex-basis: 100%;"),
        "the label menu must claim its own row inside the wrapping .kd-labels"
    );
    // The gate, on the opener AND on every row: a menu that opens onto sixteen
    // live-looking options is the same invitation one click deeper.
    for pin in [
        r#":disabled="labelsLocked()""#,
        r#":title="labelLockReason() || 'Add or remove labels on this issue'""#,
    ] {
        assert!(
            shell.contains(pin),
            "index.html must keep the label gate {pin}"
        );
    }
    assert_eq!(
        shell.matches(r#":disabled="labelsLocked()""#).count(),
        2,
        "both the edit button and the option rows must be gated"
    );

    let app_js = shell_family();
    // ONE predicate, two subjects — the drift #318 avoided. A second
    // "does this repo have a live run" test is how the gate and the controls
    // beside it start disagreeing. The sentence itself is driven by
    // `ui-tests/wb-changes.test.mjs`.
    assert!(
        js_method_body(&app_js, "labelLockReason() {").contains("WBChanges.writeLockReason("),
        "the label reason must reuse writeLockReason, not parallel it"
    );
    assert!(
        squeeze(&app_js).contains("if(this.labelsLocked())return;"),
        "toggleLabel must refuse behind the disabled rows too"
    );
}

/// Stopping a run — the one control in the Runs panel that throws away work
/// in progress (ADR-0054, ADR-0032 §6) — asks through the shell's own dialog.
/// `window.confirm` names the origin, ignores the theme and blocks the page,
/// which is the wrong furniture for the panel's most consequential click.
#[test]
fn stopping_a_run_confirms_through_the_design_system_dialog() {
    let code = squeeze(&shell_family());
    let start = code
        .find("asyncstopRun(")
        .expect("the shell must keep stopRun");
    let body = &code[start..];
    let body = &body[..body[1..].find("async").map_or(body.len(), |i| i + 1)];
    assert!(
        body.contains(r#"constok=awaitthis.$store.modals.askConfirm({title:"Stopthisrun?","#),
        "stopRun must confirm through askConfirm: {body}"
    );
    assert!(
        !body.contains("window.confirm("),
        "stopRun must not confirm through window.confirm"
    );
}

/// The board can see, read and throw away the plan that the NEXT RUN will
/// execute. Same CI bargain as the pins around it; the structural half of
/// the slice, since Playwright — which renders it — does not run in CI.
#[test]
fn the_board_surfaces_the_plan_the_next_run_would_execute() {
    // The plan folds (`planSummary`, the pill, `isBundleReason`, and the
    // zero-open-steps verdict that mirrors `plan::count_open_steps`) are
    // driven by `ui-tests/wb-runs.test.mjs`.
    let app_js = shell_family();
    for pin in [
        r#"path: ".ralphy/plan.md","#,
        r#"window.WBDaemon.write("plan.discard", { repo: slug })"#,
    ] {
        assert!(app_js.contains(pin), "the shell must keep {pin}");
    }
    // The discard is confirmed, and the plan is only ever shown against the
    // issue its trailer names (`planFor`) — a plan offered on the wrong card
    // would invite a discard of the wrong work.
    let app_squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        app_squeezed.contains(
            r#"const ok = await this.$store.modals.askConfirm({ title: "Discard this plan?","#
        ),
        "discardPlan must confirm first"
    );
    assert!(
        app_squeezed.contains("return held && held.summary.issue === number ? held : null;"),
        "a plan is shown against the issue it names, and no other"
    );

    let html = super::INDEX_HTML;
    for pin in [
        r#"class="kc-plan""#,
        r#"class="kanban-plan-chip""#,
        r#"class="modal plan-modal""#,
        r#"data-act="plan-discard""#,
    ] {
        assert!(html.contains(pin), "index.html must keep {pin}");
    }
    // Gated while a run holds the repo: a run owns the plan it is executing.
    let discard = html
        .find(r#"data-act="plan-discard""#)
        .expect("the discard control must exist");
    assert!(
        html[discard..discard + 220].contains(":disabled=\"$store.connection.writeLocked()\""),
        "the discard must be gated while a run holds the repo"
    );
    // A dialog a MODAL asks for must sit above it. Every `.modal-scrim` shares
    // `z-index: 500`, so the winner was DOM order — the plan modal's scrim
    // covered the confirm it had just raised, leaving Escape as the only
    // reachable control. Measured with Playwright, which named the plan scrim
    // as the interceptor; the fix raises the ASKED-FOR dialog rather than
    // reordering the markup, so it cannot regress by a paste in the wrong spot.
    let css = served_css();
    assert!(
        css.contains(".modal-scrim:has(> .confirm-modal),")
            && css.contains(".modal-scrim:has(> .prompt-modal)"),
        "the confirm/prompt dialogs must outrank the modal that raised them"
    );
}

/// The plan blocks' chrome: the note explains the list from ABOVE it, and the
/// section picker looks like the dropdown it is. Structural, not cosmetic —
/// both defects are invisible to every other test in this file.
#[test]
fn the_plan_blocks_explain_themselves_before_the_space_they_describe() {
    let html = super::INDEX_HTML;
    let note = html
        .find(r#"class="plan-steps-note""#)
        .expect("index.html must keep the steps note");
    let list = html
        .find(r#"<ul class="plan-steps">"#)
        .expect("index.html must keep the steps list");
    assert!(
        note < list,
        "the steps note must precede the list it explains — under an empty \
         list it sits at the bottom of the block, away from the space it is about"
    );
    // `all: unset` on `.plan-picker` removes the native arrow, so the caret is
    // the only thing saying the head opens. Its inertness is the other half:
    // a caret that swallows the click advertises an act it then prevents.
    assert!(
        html.contains(r#"class="plan-picker-caret" x-icon="'chevron-down'""#),
        "the section picker must carry a caret (`all: unset` drops the native one)"
    );
    let css = served_css();
    let caret = css
        .find(".plan-picker-caret {")
        .expect("styles.css must style the picker caret");
    assert!(
        css[caret..caret + 200].contains("pointer-events: none"),
        "the caret must be inert to the pointer so the click reaches the select"
    );
}

/// The runs panel's chrome (#331). The suite CI runs calls functions and
/// never renders markup, and Playwright does not run there — so these
/// substrings are the only CI-visible gate over this markup, the same
/// bargain #318/#319 struck for the write controls.
#[test]
fn the_runs_feed_is_contained_in_the_markup() {
    let html = super::INDEX_HTML;
    for pin in [
        r#"class="runs-feed""#,
        r#"data-act="feed-collapse""#,
        r#"data-act="feed-dismiss""#,
        r#"x-show="rawFeedOpen""#,
        r#"class="runs-verb-error""#,
        "verbLocked()",
        "verbTitle('triage')",
    ] {
        assert!(
            html.contains(pin),
            "index.html must keep the #331 pin {pin}"
        );
    }
    // The NEGATED pin, written STRUCTURALLY rather than as one spelling of
    // the old tag: `<pre x-show="rawFeed" class="runs-raw">` is the same
    // defect with the attributes swapped. The invariant is that the feed
    // occurs exactly once and is INSIDE the sized box, so the box's class
    // must appear before it.
    assert_eq!(
        html.matches("runs-raw").count(),
        1,
        "the raw feed must occur exactly once in index.html (#331)"
    );
    let feed_box = html
        .find(r#"class="runs-feed""#)
        .expect("index.html must keep the .runs-feed box");
    let raw = html
        .find("runs-raw")
        .expect("index.html must keep the raw feed");
    assert!(
        feed_box < raw,
        "the raw feed must stay INSIDE its sized .runs-feed box (#331)"
    );

    // The toolbar states the run lock in each disabled control's `title` and
    // NOWHERE else: the visible note was removed on the operator's own
    // request, so its absence is the pin. Negated, because a reflex to
    // "explain the dimmed button on screen" is exactly what would put it back.
    assert!(
        !html.contains("runs-lock-note"),
        "the runs toolbar carries no standing lock message — the reason rides \
         each disabled verb's title (`verbTitle`)"
    );

    let app_js = shell_family();
    // `rawFeedOpen: false` is the DEFAULT, not an incidental initialiser: the
    // feed can take 30vh of a panel whose job is the trail and the plan, so
    // the bytes are opt-in and only the head arrives with the output.
    for pin in ["dismissFeed()", "runVerbFailed(", "rawFeedOpen: false"] {
        assert!(
            app_js.contains(pin),
            "the shell must keep the #331 pin {pin}"
        );
    }
    assert!(
        !app_js.contains("rawFeedOpen = true"),
        "no reset may re-open the feed: dismiss and every verb click return it \
         to the collapsed default"
    );
    // The gate REUSES the Changes derivation rather than paralleling it —
    // that reuse is the acceptance criterion, so it is pinned. Whitespace
    // is collapsed first so the pin judges the CODE and not its layout: it
    // must survive a reformat and a CRLF checkout (this host holds LF in
    // the blob and CRLF on disk) without blaming a design rule for either.
    let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed.contains("verbLocked() { return this.writeLocked(); }"),
        "verbLocked() must be literally writeLocked(), not a second predicate (#331)"
    );
    // `verbLockTitle` and `exitNote` are driven by `ui-tests/wb-runs.test.mjs`.
    // Both terminal frames, the refusal and the exit, go to the panel.
    let daemon_js = squeeze(include_str!("../../assets/ui/wb-daemon.ts"));
    assert_eq!(
        daemon_js
            .matches(r#"sendWindow(window,"workbench:run-verb-failed",{message:"#)
            .count(),
        2,
        "wb-daemon.ts must route a terminal verb frame to the panel (#331)"
    );
    assert!(
        squeeze(&app_js).contains(concat!(
            r#"addEventListener("workbench:run-verb-failed","#,
            r#"(e)=>this.runVerbFailed(e.detail.message),);"#
        )),
        "the Runs panel must hear a terminal verb frame (#331)"
    );
}
