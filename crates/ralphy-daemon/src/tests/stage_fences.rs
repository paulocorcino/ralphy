//! The stage's fences: how they are drawn, locked, grouped and arranged.

use super::*;

/// The fence floor (#340): the DOM half, the CSS and the wiring. The pure
/// folds (`nextFenceSlot`, `fenceSpawnRect`, `nextFenceName`, the cap) are
/// driven by `ui-tests/wb-console.test.mjs` and `wb-geometry.test.mjs`; the
/// node suite renders no DOM or CSS, so this test holds that half.
#[test]
fn shell_draws_fences_below_the_windows() {
    // The console, its fence list, its fences and its title, as one text: the
    // fence element lives in the fences, the fence list and its rendering in
    // the fence list, the console rename in the title.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-window.ts"),
        include_str!("../../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../../assets/ui/wb-stage-fences.ts"),
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../../assets/ui/wb-console-title.ts"),
    ]
    .concat();
    for pin in [
        "function renderFences(",
        "function renameFence(",
        "function removeFence(",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #340 pin {pin}"
        );
    }
    // The plane is sized to windows AND fences (ADR-0051 §2) AND note cards
    // (ADR-0064 §8). Reverting this ONE selector leaves every other test
    // green while a fence or a card past the last window becomes
    // unreachable — the stage never grows to hold it.
    assert!(
        js.contains(r#"querySelectorAll<HTMLElement>(".session-window, .fence, .note-card")"#),
        "applyExtent must fold the fences and the cards into the stage extent (#340, ADR-0064)"
    );
    let tab = include_str!("../../assets/ui/wb-consoles-tab.ts");
    assert!(
        tab.contains("newFence("),
        "wb-consoles-tab.ts must wire the toolbar act"
    );
    let html = include_str!("../../assets/ui/index.html");
    assert!(
        html.contains("newFence()"),
        "index.html must carry the Fence button (#340)"
    );

    // Scoped to the `.fence` rule's OWN body: `pointer-events` and a small
    // `z-index` both occur elsewhere in the sheet, so an unscoped substring
    // would pass with the fence tier deleted.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    let fence = rule("\n.fence {");
    assert!(
        fence.contains("pointer-events: none"),
        "the fence floor is inert — it may never swallow a window gesture (#340)"
    );
    // The SEMICOLON is load-bearing: a bare `z-index: 1` substring is also
    // satisfied by `10`, `100` and `1000` — a fence raised over `Z_BASE`
    // (60), which is the exact defect this pin names.
    assert!(
        fence.contains("z-index: 1;"),
        "a fence draws BELOW every console window (#340)"
    );
    // NEGATIVE CONTROL: "make the whole thing inert" would delete the rename
    // affordance, and the two pins above would still be green.
    assert!(
        rule("\n.fence-name {").contains("pointer-events: auto"),
        "the name field must stay clickable — a fence is renamed in place (#340)"
    );
    // The name is inert until it is EDITED, and an edit is cancellable. The
    // press is cancelled in `buildFence` (no focus ring, no caret) and the
    // drag half is here, so a sweep across the head cannot paint the name.
    assert!(
        rule("\n.fence-name[readonly] {").contains("user-select: none"),
        "a read-only fence name must not be selectable by a sweep"
    );
    let squeezed: String = js.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squeezed.contains(r#"name.addEventListener("mousedown", (e: MouseEvent) => { if (name.readOnly) e.preventDefault(); });"#),
        "a single click on a read-only fence name must leave no trace at all"
    );
    // Enter is the ONLY commit. `change` fires on blur, so committing there
    // made "click away" mean SAVE — with nothing able to undo it.
    assert!(
        !js.contains(r#"name.addEventListener("change""#),
        "the fence name must not commit on `change` — leaving the field CANCELS"
    );
    assert!(
        squeezed.contains("if (!commit) name.value = pristine;"),
        "ending an edit without a commit must restore the name"
    );
    // …and must leave the name UNSELECTED. MEASURED: neither `blur()` nor
    // re-assigning the same string clears the range `select()` made, so an
    // abandoned edit left the name highlighted on a field nobody was editing.
    // The commit path only looked right because `renameFence` replaces the
    // input outright.
    assert!(
        squeezed.contains("name.setSelectionRange(0, 0);"),
        "ending an edit must collapse the selection its `select()` made"
    );
    // THE CAP: refused, not absorbed (the refusal is driven by
    // `wb-console.test.mjs`). A 13th fence once cost the operator a DIFFERENT
    // one, dropped by its age; nothing on the page drops a record now
    // (ADR-0050 amendment 2026-10-04), and the daemon refuses a create past
    // the cap.
    assert!(
        squeezed.contains("if (atFenceCap()) return false;"),
        "a fence past the cap must be refused before it is born"
    );
    // …and the name must not be counted. MEASURED against the running shell:
    // `Fence ${fences.length + 1}` froze at the cap, so the 13th fence and every
    // one after were all born "Fence 13" — duplicates in the list that IS the
    // plane's map (there is no minimap, and the Alt+Shift+F<n> rows are picked
    // out by name).
    assert!(
        squeezed.contains("name: nextFenceName(fences()),"),
        "a new fence must be numbered from the names present, not from the count"
    );
    assert!(
        !squeezed.contains("name: `Fence ${fences.length + 1}`")
            && !squeezed.contains("name: `Fence ${fences().length + 1}`"),
        "numbering by the fence COUNT collides at the cap — measured"
    );

    // The console menus (wb-consoles-tab.ts) hold the toolbar acts and keys.
    let tab_js = include_str!("../../assets/ui/wb-consoles-tab.ts");
    let app: String = tab_js.split_whitespace().collect::<Vec<_>>().join(" ");
    // The toolbar says why before the click and again if one gets through — the
    // #318 idiom, since `wb-console.ts` reaches no shell and can only refuse.
    assert!(
        app.contains("if (WBConsole.createFence() === false) this.$store.modals.flash(this.fenceCapMessage());"),
        "newFence must let the module refuse, and say so when it does"
    );
    // The dimming reads the REACTIVE snapshot, not the module's array. MEASURED:
    // a binding over `WBConsole.atFenceCap()` never re-evaluated (the array is
    // not Alpine state), so the row stayed enabled on a full plane.
    assert!(
        app.contains("return this.fenceItems.length >= window.WBConsole.FENCE_MAX;"),
        "fenceAtCap must read the snapshot Alpine can observe changing"
    );
    let shell = include_str!("../../assets/ui/index.html");
    assert!(
        shell.contains(r#":disabled="fenceAtCap()""#),
        "the New fence row must be disabled at the cap, not merely refused"
    );
    // The key range IS the cap, so no fence the plane can hold is unreachable
    // and no menu row lacks an accelerator. MEASURED (Chromium 148): with
    // Alt+Shift held, F10/F11/F12 all reach the document — their reserved
    // neighbours (Shift+F10, F11, F12) each want their own exact combo.
    assert!(
        app.contains(r"if (!/^F(?:[1-9]|1[0-2])$/.test(e.code)) return;"),
        "the fence accelerators must run F1..F12, matching the cap"
    );
    assert!(
        !shell.contains(r#"x-show="i < 9""#),
        "every fence row now carries an accelerator — the F9 label cap is gone"
    );
    // The row carries the KEY, the head carries the MODIFIER. The pair never
    // varies across the twelve rows, and spelling it in each one crowded the
    // panel until the name wrapped mid-word beside `Alt+Shift+F10`.
    assert!(
        app.contains("fenceShortcutLabel(n: number) { return `F${n}`; }"),
        "a fence row's label must be the bare key"
    );
    assert!(
        shell.contains(r#"x-text="fenceShortcutHint()""#),
        "the fence menu's head must state the modifier pattern once"
    );
    // …and the cancel must not rely on `blur`: the plane's pan handler
    // `preventDefault()`s mousedown, so pressing the stage does not move focus
    // at all (measured — the field kept its caret and the half-typed name).
    assert!(
        squeezed.contains(r#"document.addEventListener("pointerdown", stopOutside, true);"#),
        "a press outside the field must end the edit, since blur cannot be trusted here"
    );
}

/// A console and a fence can be LOCKED in place (ADR-0050 / ADR-0051 lock
/// amendment, 2026-09-20): the four gesture handlers and the tiler consult
/// the lock and refuse, the chrome carries the toggle, and the stylesheet
/// drops the bands. Every pin is an expression, as above.
#[test]
fn shell_locks_consoles_and_fences() {
    // The whole console family, as one text: the lock, the fence's lock, the
    // lock buttons, the gestures and the desk records live in its modules.
    let js = &console_family();
    for pin in [
        "function fenceLocked(",
        "function isLocked(",
        "function applyLock(",
        "function toggleLock(",
        "function paintFenceLock(",
        "function setFenceLock(",
        "function converge(",
        "actions.append(colBtn, fullBtn, maxBtn, restartBtn, lockBtn, closeBtn)",
        "tools.append(tile, columns, lock, detach, drop)",
    ] {
        assert!(js.contains(pin), "the console must keep the lock pin {pin}");
    }
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    // `heldFast()` defaults to `isLocked(win)` and is the note card's own
    // lock when a card is dragging (ADR-0064 §8); the refusal is the same.
    for handler in ["function makeDraggable(", "function startResize("] {
        let b = body(handler);
        assert!(
            b.contains("if (heldFast()) return")
                && b.contains("opts?.locked || (() => isLocked(win as ConsoleWin))"),
            "{handler} must refuse a locked console"
        );
    }
    for handler in ["function startFenceMove(", "function startFenceResize("] {
        assert!(
            body(handler).contains("if (fenceLocked(f.id)) return"),
            "{handler} must refuse a locked fence"
        );
    }
    assert!(
        body("function arrangeFence(").contains("if (fenceLocked(id)) return"),
        "tiling a locked fence must be a no-op"
    );
    assert!(
        body("function recordOf(").contains("locked: !!win._deskLocked"),
        "recordOf must write the lock as a bool"
    );
    assert!(
        body("function deskOf(").contains("locked: !!win._deskLocked"),
        "deskOf must carry the lock into rebuilds and the popup"
    );
    assert!(
        body("function renderFences(").contains("paintFenceLock(el, !!f.locked)"),
        "renderFences is the one place a fence's lock reaches the DOM"
    );
    assert!(
        body("function ingestDesk(").contains("converge()")
            && body("function converge(").contains("applyLock(w, !!r.locked)"),
        "a lock set on another device must reach this page's windows on its next GET"
    );
    let css = served_css();
    assert!(
        css_rule_body(&css, ".fence-lock {").contains("pointer-events: auto"),
        ".fence-lock must opt back into pointer events — the cluster is inert"
    );
    assert!(
        css_rule_body(&css, ".session-window.held .session-handle {").contains("display: none"),
        "a locked console shows no resize bands"
    );
    assert!(
        css_rule_body(&css, ".fence.locked .fence-edge {").contains("display: none"),
        "a locked fence shows no resize bands"
    );
    assert!(
        css_rule_body(&css, ".fence-head {").contains("10.5rem"),
        "the head's reserve must make room for the fifth tool"
    );
    // The title bar's controls stack ABOVE the resize bands: the NE corner
    // band (26px under a coarse pointer) covered four fifths of the close
    // button, and a finger had one sliver to hit.
    let actions = css_rule_body(&css, ".session-actions {");
    assert!(
        actions.contains("position: relative") && actions.contains("z-index: 3"),
        "the actions cluster must sit above the resize bands"
    );
}

/// A fence is a GROUP (#341): derived membership, non-overlap, and the two
/// gestures that carry it. The three folds (`fenceMembership`, `fenceFits`,
/// `fenceMoveDelta`) are driven by `ui-tests/wb-geometry.test.mjs`; the
/// gestures and the CSS hit-test contract are pinned here, because the node
/// suite runs no DOM and no CSS.
#[test]
fn shell_fences_are_a_group() {
    // The console, its fences and its desk records, as one text: the two
    // gestures live in the fences, the window record in the desk records.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-fences.ts"),
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../../assets/ui/wb-desk-records.ts"),
    ]
    .concat();
    for pin in ["function startFenceMove(", "function startFenceResize("] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #341 pin {pin}"
        );
    }
    // Membership is DERIVED, never stored: the only fence id in the shell is
    // the fence element's OWN `data-fence-id`. A desk record that carried one
    // is exactly the state that can disagree with the geometry.
    let record = js
        .split_once("function recordOf(")
        .expect("wb-console.ts must keep recordOf")
        .1;
    assert!(
        !record[..record.find("\n  }").expect("recordOf must close")].contains("fence"),
        "no window record may carry a stored fence id (#341)"
    );
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    // Both handles opt back IN, against a `.fence`/`.fence-head` that stay
    // inert — that pair is the whole hit-test contract of this slice. The
    // resize handle is `.fence-edge` since ADR-0051 §7a made every border a
    // handle; `.fence-grip` is the SE one's second class and no longer
    // carries the opt-in itself.
    for head in ["\n.fence-grab {", "\n.fence-edge {"] {
        assert!(
            rule(head).contains("pointer-events: auto"),
            "{head} must take pointer events — it is a gesture handle (#341)"
        );
    }
    assert!(
        rule("\n.fence-head {").contains("width: max-content"),
        "the head stays shrink-wrapped — a full-width band swallows the floor pan (#340/#341)"
    );
    // The STACKING ORDER of the head against those bands. Measured when §7a
    // landed: the bands are `position:absolute` and the head is static, so
    // they paint over it whatever the DOM order — the NW corner covered the
    // `⠿` grab and a fence MOVE silently became a resize toward the origin,
    // leaving every member behind. DOM order alone cannot express this, so
    // the lift is pinned here.
    for head in ["\n.fence-head {", "\n.fence-tools {"] {
        assert!(
            rule(head).contains("z-index: 2"),
            "{head} must sit ABOVE the resize bands, or they swallow its controls (§7a)"
        );
    }
    // The refusal must be VISIBLE: a silent revert reads as a dropped drag.
    assert!(
        rule("\n.fence-invalid {").contains("var(--danger)"),
        "a refused fence drop must show feedback, not just revert (#341)"
    );
}

/// Arrange moved INTO the fence (#342): the global control is retired and
/// tiling is a per-fence act over a pure fold. The fold (`tileIntoRect`) and
/// `fenceRepos` are driven by the node suite; the retirement, the fence
/// chrome and the CSS are pinned here, where a revert of either half fails.
#[test]
fn shell_arranges_into_the_fence() {
    // The console, its fence list and its fences, as one text: tiling lives
    // in the fences, the fence chrome readout in the fence list.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../../assets/ui/wb-stage-fences.ts"),
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
    ]
    .concat();
    for pin in [
        "function arrangeFence(",
        "function refreshFenceChrome(",
        "fence-arrange",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #342 pin {pin}"
        );
    }
    // The global act is GONE, not wrapped: a surviving entry point is a
    // second meaning of "arrange" (ADR-0051 §7). Safe against the pin above
    // — `"function arrangeFence("` does not contain `"function arrange("`.
    assert!(
        !js.contains("function arrange("),
        "the global arrange must be retired, not kept as a wrapper (#342)"
    );
    // The #338 rule — a maximized console is never tiled — has no other home
    // now that the global arrange is gone. `.maximized` overrides all four
    // offsets with `!important`, so a tile rect written onto one is
    // invisible while it silently replaces the rect the restore reads back.
    let fence_arrange = js
        .split_once("function arrangeFence(")
        .expect("wb-console.ts must keep arrangeFence")
        .1;
    // The EXPRESSION, not the bare word: `"maximized"` alone is satisfied by
    // the comment that explains the rule, so deleting the filter leaves this
    // green — measured. A pin a comment can satisfy is not a pin.
    assert!(
        fence_arrange[..fence_arrange
            .find("\n  }")
            .expect("arrangeFence must close")]
            .contains(r#"!m.el.classList.contains("maximized")"#),
        "arrangeFence must exclude a maximized member from the grid (#338/#342)"
    );
    assert!(
        fence_arrange[..fence_arrange
            .find("\n  }")
            .expect("arrangeFence must close")]
            .contains("minWidth"),
        "arrangeFence must relax the CSS floor for a tile below it (#342)"
    );
    let app = shell_family();
    assert!(
        !app.contains("arrangeConsoles"),
        "the shell's global arrange action must be gone (#342)"
    );
    let html = include_str!("../../assets/ui/index.html");
    assert!(
        !html.contains("Arrange"),
        "the canvas toolbar must carry no Arrange control (#342)"
    );
    // The fence's arrange button opts back INTO pointer events, against a
    // `.fence`/`.fence-head` that stay inert — without this the control is
    // drawn and unclickable, and every other pin here stays green.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    assert!(
        rule("\n.fence-arrange {").contains("pointer-events: auto"),
        "the fence's arrange button must take pointer events (#342)"
    );
    // The same trap for the button that opens the fence as columns
    // (ADR-0051 §5, 2026-09-30).
    assert!(
        rule("\n.fence-columns {").contains("pointer-events: auto"),
        "the fence's columns button must take pointer events"
    );
    // The window floor lives in TWO files — the CSS declaration and the
    // constants `arrangeFence` relaxes it against. Nothing else notices when
    // they diverge: the tiles would silently render past the fence again.
    for (konst, decl) in [
        ("WIN_MIN_W = 240", "min-width: 240px"),
        ("WIN_MIN_H = 150", "min-height: 150px"),
    ] {
        assert!(
            include_str!("../../assets/ui/wb-geometry.ts").contains(konst),
            "wb-geometry.ts must mirror the window floor as {konst} (#342)"
        );
        assert!(
            rule("\n.session-window {").contains(decl),
            "styles.css's window floor must still be `{decl}` — wb-console.ts mirrors it (#342)"
        );
    }
}
