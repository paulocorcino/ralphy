//! Rules over the stylesheet itself: bounded feeds and hover.

use super::*;

/// The CSS between a marked block's `/* <name> */` and `/* <name> end */`
/// comments.
fn css_block<'a>(css: &'a str, name: &str) -> &'a str {
    let open = format!("/* {name} */");
    let close = format!("/* {name} end */");
    let start = css
        .find(&open)
        .unwrap_or_else(|| panic!("styles.css must keep the {name} opening marker"))
        + open.len();
    let end = css
        .find(&close)
        .unwrap_or_else(|| panic!("styles.css must keep the {name} closing marker"));
    css.get(start..end)
        .expect("both markers are ASCII and the closing one follows the opening one")
}

/// The runs chrome's declarations that actually DO the bounding. The markup
/// pins above prove the box exists; only these
/// prove it is bounded, and `max-height` is a single line whose deletion
/// restores the original defect with every other pin still green.
// pin: the runs feed is bounded and wraps —
// node computes no style, so only the stylesheet text shows it.
#[test]
fn the_runs_feed_is_bounded_and_wraps() {
    let css = served_css();
    let block = css_block(&css, "#331 runs chrome");
    // The bound, the wrap, and the containment: the three declarations the
    // issue's criteria rest on. The browser pass measures them, and that
    // pass — Playwright — does not run in CI (lib.rs doc above).
    for decl in [
        "max-height: 30vh",
        "overflow-wrap: anywhere",
        "white-space: pre-wrap",
        "flex: 0 0 auto",
    ] {
        assert!(
            block.contains(decl),
            "the feed's containment rests on `{decl}` — it must stay in the #331 block"
        );
    }
    // The phone width is a criterion, so the narrow rule is pinned by its
    // BREAKPOINT and its payload: a bare `@media` would be satisfied by
    // `@media print {}` while the narrow cap was deleted.
    let narrow = block
        .find("@media (max-width: 560px)")
        .expect("the phone width is a criterion — keep the (max-width: 560px) rule");
    assert!(
        block.split_at(narrow).1.contains("max-height: 22vh"),
        "the narrow-width rule must still cap the feed"
    );
}

/// The write and discard controls are reachable on a touch screen: no
/// `:hover` rule reveals them. The discard block also keeps its `@media`
/// rule: the touch de-emphasis IS the criterion, and a block that lost it
/// would pass the rest vacuously.
// pin: the write and discard controls are not hover gated —
// node computes no style, so only the stylesheet text shows it.
#[test]
fn the_write_and_discard_controls_are_not_hover_gated() {
    let css = served_css();
    assert!(
        css_block(&css, "#319 discard").contains("@media"),
        "the touch de-emphasis is the criterion — the discard block must keep its @media rule"
    );
    // (block, the control it styles)
    for (name, control) in [
        ("#318 write controls", "write control"),
        ("#319 discard", "discard control"),
    ] {
        // The touch criterion, pinned where CI can see it: a hover-gated
        // `opacity`/`visibility` is exactly the affordance a phone cannot find.
        // Split on `}` so each chunk is one rule — selector, then its body —
        // and judge the BODY of any rule whose selector mentions `:hover`.
        // Comments are stripped FIRST: one of them names
        // `.branch-chip.disabled:hover` as prior art, and a raw split would
        // read that prose as a selector.
        let declarations = strip_css_comments(css_block(&css, name));
        let mut hover_rules = 0;
        for rule in declarations.split('}') {
            let Some((selector, body)) = rule.split_once('{') else {
                continue;
            };
            if !selector.contains(":hover") {
                continue;
            }
            hover_rules += 1;
            // `display` and `max-height` are in the list because the TEXTBOOK
            // hover-gated affordance is `display: none` + `:hover { display:
            // … }` — banning only `opacity`/`visibility` would leave the most
            // obvious spelling of the defect green.
            for banned in ["opacity", "visibility", "display", "max-height"] {
                assert!(
                    !body.contains(banned),
                    "a {control} must not be hover-gated on {banned}: {selector}"
                );
            }
        }
        // …and the scan must have had something to judge: a block that
        // stopped carrying `:hover` rules would satisfy the loop vacuously.
        assert!(
            hover_rules >= 2,
            "the {name} block must still carry its :hover rules, found {hover_rules}"
        );
    }
}

/// CSS text with every `/* … */` comment removed, so a rule scan judges
/// selectors and not the prose that names one as prior art.
fn strip_css_comments(block: &str) -> String {
    let mut out = String::new();
    let mut rest = block;
    while let Some((before, after)) = rest.split_once("/*") {
        out.push_str(before);
        let Some((_, tail)) = after.split_once("*/") else {
            rest = "";
            break;
        };
        rest = tail;
    }
    out.push_str(rest);
    out
}
