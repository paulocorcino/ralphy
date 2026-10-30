//! The plane under the stage: drags, taps, the viewport, the frame chrome and the view store.

use super::*;

/// A press is a DRAG only past a threshold (4px mouse, 10px finger): a tap
/// on a titlebar, a resize band or a fence handle moves nothing and persists
/// nothing. The threshold folds (`dragThreshold`, `dragBegins`) are driven by
/// `ui-tests/wb-console-input.test.mjs`; the four gesture handlers are DOM wiring
/// that no node test runs, so they are pinned here on the predicate they
/// consult.
#[test]
fn shell_drags_only_past_a_threshold() {
    // The console, its fence list, its window chrome and its fences, as one
    // text: the window gestures live in the chrome, the fence gestures in the
    // fences, the fence records in the fence list.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../../assets/ui/wb-stage-chrome.ts"),
        include_str!("../../assets/ui/wb-stage-fences.ts"),
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
    ]
    .concat();
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    for handler in [
        "function makeDraggable(",
        "function startResize(",
        "function startFenceMove(",
        "function startFenceResize(",
    ] {
        let b = body(handler);
        assert!(
            b.contains("dragThreshold(e.pointerType)") && b.contains("dragBegins("),
            "{handler} must arm only past dragThreshold"
        );
    }
    // The console gestures write only once armed: a bare tap moved nothing.
    // `persist()` is the hook, not `setWin` directly, since the note card
    // drops through the same gesture into its own collection (ADR-0064 §8).
    // BOTH halves are pinned: that only an armed gesture writes, AND what the
    // hook defaults to — the rect alone, computed at the act (ADR-0050
    // amendment 2026-10-04). Pinning the call alone would let a regression
    // bind the default to a no-op — every test green while an armed window
    // drag writes nothing and the layout is lost on the next reload.
    for handler in ["function makeDraggable(", "function startResize("] {
        let b = body(handler);
        assert!(
            b.contains("if (armed) persist()"),
            "{handler} must persist only an armed gesture"
        );
        assert!(
            b.contains(
                "opts?.onDrop || (() => setWin(win as ConsoleWin, { rect: restoreRect(win) }))"
            ),
            "{handler}'s drop hook must default to writing the window's rect"
        );
    }
    // The projection a card's lock is derived from. Pinned in Rust because
    // the module's fence array is not reachable from a ui-test: an
    // implementation that handed out the LIVE records, or that dropped
    // `rect`/`locked` from the copy, would break `lockedBy`'s "fence"
    // verdict with nothing on either side to catch it.
    assert!(
        body("function fenceRecords(").contains("fences().map((f: DeskFence) => ({ ...f }))"),
        "fenceRecords must answer copies of the whole record (ADR-0064 §8)"
    );
    // And the card must actually PASS both hooks: the default is correct
    // for a window and wrong for a card, whose node has no `_deskLocked`
    // and whose rect belongs to another collection.
    let notes_js = include_str!("../../assets/ui/wb-notes.ts");
    for pin in [
        "locked: () => !!el._noteLocked",
        "onDrop: () => persistCards(el)",
    ] {
        assert!(
            notes_js.contains(pin),
            "the card must hand the gesture its own {pin} (ADR-0064 §8)"
        );
    }
}

/// A finger on a console's titlebar: two taps maximize, a hold on the name
/// renames. `isDoubleTap` is driven by `ui-tests/wb-console-input.test.mjs`; the
/// wiring is DOM that no node test runs, so it is pinned here, and the
/// gestures themselves by `tests/browser/console/wb_console_touch.py`.
#[test]
fn titlebar_touch_double_taps_and_holds() {
    // The console, its window chrome and its title, as one text: the titlebar lives in the
    // chrome, the name's double-click in the title.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-chrome.ts"),
        include_str!("../../assets/ui/wb-console-title.ts"),
    ]
    .concat();
    let after = js
        .split_once("function wireTitleTouch(")
        .expect("the console must keep wireTitleTouch")
        .1;
    let b = &after[..after.find("\n  }").expect("the function must close")];
    for pin in [
        // Each `dblclick` listener needs the press's input type.
        "win._lastPointerType = e.pointerType;",
        // Without it a `mousedown` after the hold blurs the new name input.
        "e.preventDefault();",
        // Without it iOS Safari zooms the page on the double tap.
        r#"if (!(e.target as HTMLElement).closest("button, .session-name-input")) e.preventDefault();"#,
        "{ passive: false }",
        "dragBegins(pressed,",
        "HOLD_MS)",
        "isDoubleTap(win._lastTap, tap) && !isFull(win)",
        "startRename(win, span);",
        r#"document.addEventListener("pointercancel", onCancel);"#,
        r#"document.removeEventListener("pointercancel", onCancel);"#,
    ] {
        assert!(b.contains(pin), "wireTitleTouch must keep {pin}");
    }
    assert!(
        js.contains("wireTitleTouch(win, titlebar, maxOrRestore);"),
        "every console titlebar must get the touch gestures"
    );
    // Both `dblclick` listeners (bar and name) leave a finger's taps to
    // wireTitleTouch, or Chrome on Android toggles or renames twice.
    assert_eq!(
        js.matches(r#"if (win._lastPointerType !== "mouse") return;"#)
            .count(),
        2,
        "the titlebar and the name dblclick must both ignore a touch"
    );
}

/// The stage/viewport shell (#336). A clamp lives in CSS and markup, which
/// the `node --test` suite never renders and Playwright — which does — does
/// not run in CI. So this is the only CI-visible gate that the deletion
/// stays deleted: a re-added clamp would pass every unit test in the tree.
#[test]
fn shell_has_no_clamp_and_carries_the_stage() {
    // The whole console family; the terminal holds the fit observer.
    let js = console_family();
    assert!(
        !js.contains("clampAll"),
        "the clamp-and-refit is deleted, not renamed (#336)"
    );
    assert!(
        !js.contains("observeWorkspace"),
        "nothing observes the viewport to reposition a window (#336)"
    );
    // The NEGATIVE control for the two pins above: the per-window fit
    // observer must SURVIVE. It resizes a terminal, never a window rect, so
    // a blanket "no ResizeObserver" edit would be the wrong fix.
    assert!(
        js.contains("new ResizeObserver"),
        "the per-window terminal fit observer must survive the deletion (#336)"
    );
    // The extent itself (`stageExtent`) is driven by the node suite.

    let html = super::INDEX_HTML;
    assert!(
        html.contains(r#"id="stage""#),
        "index.html must carry the stage plane (#336)"
    );
    assert!(
        !html.contains(r#"class="stage""#),
        "the old tab-body class is renamed .tabbody — one meaning per name (#336)"
    );

    let css = served_css();
    for pin in ["#stage {", "#workspace.maxlock {", ".tabbody {"] {
        assert!(css.contains(pin), "styles.css must keep the #336 pin {pin}");
    }
}

/// The navigation layer over that plane (#337). The folds (`bringIntoView`,
/// `panNudge`) are driven by `ui-tests/wb-console.test.mjs`; the gestures
/// that use them are DOM wiring that no node test runs, so they are pinned
/// here, each inside the function that must hold it.
#[test]
fn shell_navigates_the_plane() {
    // The console, its window chrome, its fences and its view, as one text:
    // the plane lives in the console, the window drag in the chrome, the
    // fence drag in the fences, the reveal, the pan and the wheel in the view.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-chrome.ts"),
        include_str!("../../assets/ui/wb-stage-fences.ts"),
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../../assets/ui/wb-stage-view.ts"),
    ]
    .concat();
    for pin in [
        "function reveal(",
        "function onFloorDown(",
        "function onWheel(",
        // The REGISTRATION, not the phrase: pinning a bare `passive: false`
        // is satisfied by the comment that explains it, so the option could
        // be deleted with this gate still green.
        r#"addEventListener("wheel", onWheel, { passive: false })"#,
    ] {
        assert!(js.contains(pin), "the console must keep the #337 pin {pin}");
    }
    // Scoped to each gesture: the file carries each of these statements
    // several times, so a whole-file match stays green when one gesture
    // loses its own copy.
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        squeeze(&after[..after.find("\n  }").expect("the function must close")])
    };
    // The auto-pan loop's teardown: an uncancelled rAF pans forever after
    // the button is released. One loop cancels it, and each gesture stops it.
    assert!(
        body("function autoPan(").contains("cancelAnimationFrame(panRaf)"),
        "autoPan must cancel its loop (#337)"
    );
    for gesture in ["function makeDraggable(", "function startFenceMove("] {
        assert!(
            body(gesture).contains("pan.stop();"),
            "{gesture} must stop its auto-pan loop (#337)"
        );
    }
    // …and the two lost-mouseup recoveries, which are the only reason that
    // teardown is reachable when the release never arrives.
    for gesture in [
        "function makeDraggable(",
        "function startFenceMove(",
        "function startFenceResize(",
    ] {
        let b = body(gesture);
        assert!(
            b.contains("if(ev.buttons===0){onUp();")
                && b.contains(r#"window.addEventListener("blur",onUp)"#),
            "{gesture} must end on a lost mouseup (#337)"
        );
    }
    assert!(
        !js.contains("scrollIntoView"),
        "the centring is a tabled pure function, not the browser's heuristic (#337)"
    );

    let css = served_css();
    for pin in [
        // The VALUE, not the property: `overscroll-behavior: auto` is the
        // exact mutation `wb_pan_337.py` measures as chaining the wheel out
        // of the terminal (plane 200 -> 0), and a property-name pin passes
        // straight through it.
        "overscroll-behavior: contain",
        "#stage.panning {",
        "cursor: grabbing",
    ] {
        assert!(css.contains(pin), "styles.css must keep the #337 pin {pin}");
    }
    // `cursor: grab` alone is satisfied by `.session-titlebar`, so scope the
    // pin to the stage's OWN rule — the floor advertising itself as the pan
    // surface is the affordance this issue added.
    let stage_rule = css
        .split_once("\n#stage {")
        .expect("styles.css must keep the #stage rule")
        .1;
    assert!(
        stage_rule[..stage_rule.find('}').expect("the #stage rule must close")]
            .contains("cursor: grab"),
        "the stage itself must advertise the grab cursor (#337)"
    );

    // The picker's wiring lives in wb-consoles-tab-menus.ts; without this the
    // `@click` handlers in index.html can go dangling with every test still green.
    let tab = include_str!("../../assets/ui/wb-consoles-tab-menus.ts");
    for pin in ["toggleWindowMenu(", "revealWindow(", "windowList"] {
        assert!(
            tab.contains(pin),
            "wb-consoles-tab-menus.ts must keep the #337 pin {pin}"
        );
    }

    let html = super::INDEX_HTML;
    for pin in [r#"class="dropdown window-menu""#, r#"class="window-item""#] {
        assert!(
            html.contains(pin),
            "index.html must keep the #337 pin {pin}"
        );
    }
}

/// The FRAME chrome over that plane (#338): the footer pills and the
/// empty-stage hint belong to `.consoles-tab`, never to `#stage`, and the
/// maximize pin is a DERIVED fact. Same bargain as the two above — the CI
/// suites do not render this markup — so this is the only gate that notices
/// the chrome sliding back onto the plane.
#[test]
fn shell_pins_the_frame_chrome() {
    let html = super::INDEX_HTML;
    // The stage element is EMPTY in the markup, so the chrome physically
    // cannot be a child of it — a stronger pin than "is not inside" prose.
    assert!(
        html.contains(r#"<div id="stage"></div>"#),
        "the stage must stay an empty element the shell fills (#338)"
    );
    let tab = html
        .find(r#"class="consoles-tab""#)
        .expect("index.html must keep the consoles tab (#338)");
    // The section's OWN close, not a later landmark: bounding by `#viewers`
    // would still pass for chrome that escaped the tab entirely.
    let close = tab
        + html[tab..]
            .find("</section>")
            .expect("the consoles tab must close (#338)");
    // The viewport as a CLOSED element: an offset past its `</div>` cannot be
    // inside `#workspace` — and therefore cannot be inside `#stage` either.
    // Chrome inside the scrolling box pans away just as surely as chrome on
    // the plane, so "after the stage" alone would not be enough.
    let viewport = r#"<div id="workspace"><div id="stage"></div></div>"#;
    let ws = html
        .find(viewport)
        .expect("index.html must keep the closed viewport element (#338)")
        + viewport.len();
    assert!(
        ws > tab,
        "the viewport must live inside the consoles tab (#338)"
    );
    // `canvas-empty` was the second piece of frame chrome here. The
    // empty-stage caption is gone — the toolbar's New-console button says
    // the same thing where the operator acts — so the footer carries the
    // rule alone, and the caption's absence is asserted below.
    let pin = r#"class="canvas-foot""#;
    let at = html
        .find(pin)
        .unwrap_or_else(|| panic!("index.html must carry the frame chrome {pin} (#338)"));
    assert!(
        at > ws && at < close,
        "{pin} must be a SIBLING of the viewport inside .consoles-tab (#338)"
    );
    // The ELEMENT, not the words: the comment that stands where the caption
    // did quotes them, and a pin that cannot tell prose from markup would
    // make documenting the removal the thing that fails.
    assert!(
        !html.contains(r#"class="canvas-empty""#),
        "the empty-stage caption is not to come back"
    );

    // Whitespace-free text: the code, not its layout. The console and its
    // view, as one text: the pin and the maximize lock live in the console,
    // the scroll registration and the reveal in the view, so the negative
    // pin below covers both.
    let js = squeeze(
        &[
            include_str!("../../assets/ui/wb-console.ts"),
            include_str!("../../assets/ui/wb-stage-window.ts"),
            include_str!("../../assets/ui/wb-stage-view.ts"),
        ]
        .concat(),
    );
    for pin in [
        "functionsyncMaxPin(",
        // The REGISTRATION, not the function: without it the pin is only
        // re-derived on a maximize and a programmatic pan desyncs it.
        r#"ws.addEventListener("scroll",syncMaxPin)"#,
        "workbench:stage-extent",
        // The NEGATIVE control: the scroll freeze must SURVIVE. A blanket
        // deletion of the maximize machinery would satisfy every "no longer
        // contains" pin below and must be red, not green.
        "functionsyncMaxLock(",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts, wb-stage-window.ts or wb-stage-view.ts must keep the #338 pin {pin}"
        );
    }
    // The POSITIVE half of the `reveal()` change, in either operand order:
    // the negative pin below is one spelling and a requote would slip past
    // it, and scenario 4 (the only behavioural gate) does not run in CI.
    let maximized = r#"it.classList.contains("maximized")"#;
    let column = r#"it.classList.contains("column")"#;
    assert!(
        js.contains(&format!("if({maximized}||{column})returnit;"))
            || js.contains(&format!("if({column}||{maximized})returnit;")),
        "reveal() must return a maximized or column window as it is (#338)"
    );
    assert!(
        !js.contains(r#"if(ws.classList.contains("maxlock"))returnit;"#),
        "reveal() must pan the plane while maximized — Go-to is the path (#338)"
    );

    let css = served_css();
    let foot = css
        .split_once("\n.canvas-foot {")
        .expect("styles.css must keep the .canvas-foot rule (#338)")
        .1;
    let foot = &foot[..foot.find('}').expect("the .canvas-foot rule must close")];
    for pin in ["pointer-events: none", "z-index: 130"] {
        assert!(
            foot.contains(pin),
            "the .canvas-foot rule must keep {pin} — above the consoles, inert to the pointer (#338)"
        );
    }
}

/// The per-client view (#339) and — the part that can silently regress — the
/// ONE place allowed to name `localStorage`. ADR-0050 §3 dropped the browser
/// desk store; ADR-0051 §8 narrows that to "no *desk* in browser storage",
/// which is only honest while the view store stays a single module holding a
/// single key. Same bargain as the pins above — and this one is a TREE-WIDE
/// negative, which no per-module `node --test` file can state — so this is
/// the gate that notices the desk creeping back into the browser.
#[test]
fn shell_stores_only_the_view_in_the_browser() {
    // The console family and the shell own the state being persisted, and
    // must reach it only through `WBView` — a direct write from any of them
    // is how a second store starts.
    for (name, src) in [CONSOLE_FAMILY, SHELL_FAMILY].concat() {
        assert!(
            !src.contains("localStorage"),
            "{name} must not touch localStorage — wb-client-view.ts owns the store (#339)"
        );
    }

    let view = include_str!("../../assets/ui/wb-client-view.ts");
    // NEGATIVE CONTROL: deleting the store wholesale would satisfy every
    // "does not contain" assertion above. It must be red, not green.
    assert!(
        view.contains("localStorage"),
        "wb-client-view.ts IS the browser store — deleting it is not how #339 stays green"
    );
    assert!(
        view.contains(r#"const KEY = "wb.view.v1""#),
        "wb-client-view.ts must keep the one view key (#339)"
    );

    let js = include_str!("../../assets/ui/wb-stage-view.ts");
    // `viewLanding` is driven by `ui-tests/wb-geometry.test.mjs`, the landing
    // by `ui-tests/wb-stage-view.test.mjs`.
    for pin in [
        "function applyLanding(",
        // The REGISTRATION, not the function: without it the offset is never
        // persisted and the landing has nothing to restore.
        r#"ws.addEventListener("scroll", saveOffset)"#,
    ] {
        assert!(
            js.contains(pin),
            "wb-stage-view.ts must keep the #339 pin {pin}"
        );
    }

    // Tree-wide, not just the two modules above: any non-vendor asset that
    // starts naming `localStorage` is a second store by definition. `.html`
    // is swept too, so a store cannot come back as an inline `<script>`
    // block without touching a single `.js`.
    for path in embedded_ui_paths() {
        let scanned = path.ends_with(".js") || path.ends_with(".html");
        if !scanned || path.starts_with("vendor/") || path == "wb-client-view.js" {
            continue;
        }
        let src = UI
            .get_file(&path)
            .and_then(|f| f.contents_utf8())
            .unwrap_or_else(|| panic!("{path} must be embedded as UTF-8"));
        assert!(
            !src.contains("localStorage"),
            "{path} must not touch localStorage — wb-client-view.js is the only store (#339)"
        );
    }
}
