//! The fence list: the toolbar picker, the jump and the birth of a console in the focused fence.

use super::*;

/// The fence list is the MAP (#343): the toolbar picker, the jump that
/// reuses #337's arithmetic, and the birth of a console inside the focused
/// fence. The pure folds (`rectHolds`, `fenceSummaries`, `spawnRectIn`,
/// `fenceCycle`) are driven by the node suite; the DOM wiring that calls
/// them runs in no node test, so a deletion there fails HERE or nowhere.
/// Every pin below is an EXPRESSION, not a bare noun: #342 measured that a
/// function's own explanatory comment satisfies a noun pin, leaving it green
/// over deleted code.
#[test]
fn shell_lists_the_fences() {
    // The console, its fence list, its window chrome and its view, as one text: the fence
    // list and the focused fence live in the fence list, the birth in the focused fence in the
    // chrome, the jumps and the slide in the view.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../../assets/ui/wb-stage-chrome.ts"),
        include_str!("../../assets/ui/wb-stage-view.ts"),
    ]
    .concat();
    let geometry = include_str!("../../assets/ui/wb-geometry.ts");
    for pin in [
        "function fenceList(",
        "function jumpToFence(",
        "function focusFence(",
        "function clearFenceFocus(",
    ] {
        assert!(js.contains(pin), "the console must keep the #343 pin {pin}");
    }
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    // The issue's own rule: the jump REUSES a shared fold, it does not
    // re-derive the arithmetic. Both halves are load-bearing — the call
    // site, and the fact that there is exactly one definition to call. A
    // second copy would satisfy the first assertion alone.
    //
    // ADR-0051 §7 (amended) changed WHICH fold: the jump ANCHORS the fence's
    // top-left corner rather than centring it. `bringIntoView` still exists
    // and still centres for the Go-to picker (#337), so both are pinned —
    // routing the jump back through the centring one is the regression this
    // catches.
    // Since ADR-0064 §10 a NOTE CARD jumps too, so the slide — and with it
    // the stored-offset invariant the jump learned the hard way — lives in
    // one `jumpToEl` that takes the fold. The two folds are NOT
    // interchangeable and the ADRs draw the contrast on purpose: a fence
    // is a region and anchors its corner (ADR-0051 §7 amended), a card is
    // a point of interest and is centred (ADR-0064 §10). Each jump is
    // pinned on its own fold AND on its own focus call, which is the other
    // half that differs between them.
    assert!(
        body("function jumpToEl(").contains("fold(restoreRect(el)"),
        "the jump must place the element through the fold it was handed (#343, §7)"
    );
    for (jump, fold, focus) in [
        (
            "function jumpToFence(",
            "jumpToEl(el, anchorIntoView)",
            "focusFence(id)",
        ),
        (
            "function jumpToNote(",
            "jumpToEl(el, bringIntoView)",
            "focusWin(el)",
        ),
    ] {
        let b = body(jump);
        assert!(
            b.contains(fold),
            "{jump} must route through the one slide with {fold} (#343, ADR-0064 §10)"
        );
        assert!(
            b.contains(focus),
            "{jump} must focus what it jumped to, not only slide to it"
        );
        assert!(
            !b.contains("anchorIntoView(restoreRect") && !b.contains("bringIntoView(restoreRect"),
            "{jump} must not carry a second copy of the arithmetic"
        );
    }
    for (name, what) in [
        ("function bringIntoView(", "bring-into-view"),
        ("function anchorIntoView(", "anchor-into-view"),
    ] {
        assert_eq!(
            geometry.matches(name).count(),
            1,
            "there may be exactly ONE {what} implementation, in wb-geometry.ts (#343)"
        );
        assert!(
            !js.contains(name),
            "wb-console.ts must not define {what} again (#343)"
        );
    }
    // The slide is a VIEW effect layered on top, never a substitute for the
    // committed offsets: `slideTo` is cancellable, and both the floor's pan
    // and the wheel take the view back from a jump still in flight.
    assert!(
        body("function jumpToEl(").contains("slideTo(ws, to)"),
        "the jump must travel through the cancellable slide (§7)"
    );
    for owner in ["function onFloorDown(", "function onWheel("] {
        assert!(
            body(owner).contains("cancelSlide()"),
            "{owner} must abandon a slide in flight — the operator's hand outranks it (§7)"
        );
    }
    // The keyboard walk, and the rule that makes it a sweep rather than a
    // teleport: READING ORDER off the geometry, not the desk array's order.
    let desk_folds = include_str!("../../assets/ui/wb-desk-folds.ts");
    let cycle = desk_folds
        .split_once("function fenceCycle(")
        .expect("wb-desk-folds.ts must keep fenceCycle")
        .1;
    assert!(
        cycle[..cycle.find("\n}").expect("the function must close")].contains("fenceOrder("),
        "the walk must step through the reading-order fold, not the desk array (§7)"
    );
    assert!(
        body("function stepFence(").contains("jumpToFence("),
        "a keyboard step must be a real jump, not a bare focus flip (§7)"
    );
    // The focus STATE MACHINE, not just its function names: measured, a
    // `focusFence` gutted to an empty body leaves every other pin here, the
    // whole node table and clippy green — while no ring renders and every
    // console is born free, which is this issue's headline behaviour. The
    // module variable and the class are what make focus real.
    let focus = body("function focusFence(");
    assert!(
        focus.contains("focusedFence = id") && focus.contains("is-focused"),
        "focusFence must set the module's focused id AND mark the element (#343)"
    );
    assert!(
        body("function clearFenceFocus(").contains("focusFence(null)"),
        "clearing focus must go through the same one setter (#343)"
    );
    assert!(
        body("function jumpToFence(").contains("focusFence(id)"),
        "the jump must FOCUS the fence it lands on — that is what the birth path reads (#343)"
    );
    // The focus comes BEFORE the maximize guard: under a maximize only the
    // slide is skipped, and the birth path still reads the focused fence.
    let jump = body("function jumpToFence(");
    let guard = jump
        .find(r#"querySelector(".session-window.maximized")) return el"#)
        .expect("under a maximize the fence jump must not move the view (ADR-0051 §7)");
    assert!(
        jump.find("focusFence(id)").is_some_and(|at| at < guard),
        "the fence must take the focus even when the view does not move (ADR-0051 §7)"
    );
    assert!(
        body("function onFloorDown(").contains("clearFenceFocus()"),
        "a bare-floor press outside the focused fence must clear it (#343)"
    );
    // The containment predicate is REUSED, not re-spelled — the same rule
    // the issue states for `bringIntoView`. Pinning only the definition
    // lets an inlined comparison sit beside it as exported dead code.
    // The two owners now live in two files — `fenceMembership` went to
    // `wb-geometry.ts` with the predicate it shares, the floor's hit test
    // stayed with the DOM it reads — so the slicer is told which source to
    // carve. That is the whole change: the invariant ("one containment
    // predicate, two callers") is exactly what it was, and it is now stated
    // ACROSS the seam, which is where it can actually break.
    let geometry_body = |name: &str| -> String {
        let after = geometry
            .split_once(name)
            .unwrap_or_else(|| panic!("wb-geometry.ts must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    for (owner, what, found) in [
        (
            "function fenceMembership(",
            "membership",
            geometry_body("function fenceMembership("),
        ),
        (
            "function onFloorDown(",
            "the floor's focus hit test",
            body("function onFloorDown("),
        ),
    ] {
        let _ = owner;
        assert!(
            found.contains("rectHolds("),
            "{what} must go through the one containment predicate (#343)"
        );
    }
    // The birth site goes through the pure box, so "a console opened while a
    // fence is focused lands inside it" is arithmetic, not a hand-placed
    // rect that drifts from the fence's own geometry.
    assert!(
        body("function spawnInFence(").contains("spawnRectIn("),
        "spawnInFence must place a fence-born console through spawnRectIn (#343)"
    );
    // One fold feeds BOTH readouts: the fence's own chrome and the toolbar
    // row can never disagree about a count.
    assert!(
        body("function refreshFenceChrome(").contains("fenceSummaries("),
        "the fence chrome must read the same fold the list does (#343)"
    );
    let tab = include_str!("../../assets/ui/wb-consoles-tab.ts");
    for pin in ["jumpFence(", "fenceList()"] {
        assert!(
            tab.contains(pin),
            "wb-consoles-tab.ts must keep the #343 pin {pin}"
        );
    }
    let html = include_str!("../../assets/ui/index.html");
    // `class="fence-item"`, not the bare noun: the markup's own comment
    // names the class, so a rename on the real element would leave a bare
    // `"fence-item"` pin green while every row loses its styling.
    for pin in ["jumpFence(", r#"class="fence-item""#] {
        assert!(
            html.contains(pin),
            "index.html must keep the #343 pin {pin}"
        );
    }
    // The focused fence must be VISIBLE — an invisible focus makes "the next
    // console is born over there" unexplainable to the operator.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    // A DEFINED token: an `outline` shorthand naming an undefined custom
    // property is invalid as a whole, so the ring silently never renders —
    // measured, with `--accent`, which this palette does not have.
    let focused = rule("\n.fence.is-focused {");
    assert!(
        focused.contains("outline: 2px solid var(--console-text)"),
        "the focused fence must carry a visible ring (#343)"
    );
    assert!(
        css.contains("--console-text:"),
        "the focus ring's colour token must exist — an undefined one voids the whole shorthand (#343)"
    );
}
