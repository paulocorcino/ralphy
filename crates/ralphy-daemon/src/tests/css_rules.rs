//! Rules over the stylesheet itself: colour tokens, bounded feeds, hover and duplicate properties.

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
    &css[start..end]
}

/// Each marked block of new chrome speaks the shell's token language
/// (ADR-0035): `var(--…)` tokens only, never a hex literal. Each block has its
/// own marker pair, so a pin never widens over another issue's CSS. A block
/// emptied by mistake would pass the negative check, so each must also have
/// content.
#[test]
fn marked_css_blocks_add_no_colour_outside_the_token_set() {
    let css = served_css();
    for name in [
        "#331 runs chrome",
        "#319 discard",
        "#318 write controls",
        "#317 rail view",
    ] {
        let block = css_block(&css, name);
        assert!(
            !block.trim().is_empty(),
            "the {name} block must not be empty"
        );
        assert!(
            !block.contains('#'),
            "the {name} CSS must reference var(--…) tokens only, no hex literals"
        );
    }
}

/// The runs chrome's declarations that actually DO the bounding. The markup
/// pins above prove the box exists; only these
/// prove it is bounded, and `max-height` is a single line whose deletion
/// restores the original defect with every other pin still green.
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
        block[narrow..].contains("max-height: 22vh"),
        "the narrow-width rule must still cap the feed"
    );
}

/// The write and discard controls are reachable on a touch screen: no
/// `:hover` rule reveals them. The discard block also keeps its `@media`
/// rule: the touch de-emphasis IS the criterion, and a block that lost it
/// would pass the rest vacuously.
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
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        match rest[at + 2..].find("*/") {
            Some(end_at) => rest = &rest[at + 2 + end_at + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// No top-level selector declares the same property twice with two values.
///
/// `styles.css` is 6,400 lines and a selector is free to appear in several
/// sections — that is normal and additive, and this gate allows it. What it
/// forbids is the same selector setting the same PROPERTY twice: source
/// order silently picks a winner, and the loser sits in the file reading
/// like an intention that someone can maintain. `.run-verb:disabled` carried
/// `opacity: 0.45` at line 1041 and `opacity: 0.4` at 5451 for as long as
/// both existed; the 0.45 never rendered once.
///
/// It is also the precondition for splitting this file into partials: rules
/// can be regrouped safely only while no pair of them is deciding an outcome
/// by which one comes last.
///
/// Top-level only, and by design. A declaration inside `@media` is SUPPOSED
/// to override the base one — that is the mechanism, not a collision — so
/// anything nested is skipped rather than reported.
#[test]
fn no_selector_sets_one_property_twice() {
    let css = strip_css_comments(&served_css());
    let mut seen: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();
    let mut depth = 0usize;
    let mut selector = String::new();
    let mut body = String::new();
    let mut in_body = false;

    // A quoted value can contain `{`, `}` or `;` — `content: "{"` is legal —
    // and an unaware walk desyncs `depth` and silently drops every rule after
    // it. The only guard is the `seen.len()` floor, which a truncated walk
    // still clears, so the under-report would be invisible. No such value
    // exists today; this keeps it that way.
    let mut in_string: Option<char> = None;
    for ch in css.chars() {
        if in_string.is_some() || ch == '"' || ch == '\'' {
            match in_string {
                Some(quote) if ch == quote => in_string = None,
                Some(_) => {}
                None => in_string = Some(ch),
            }
            // The character still belongs to whatever encloses it — a quoted
            // attribute value is part of the SELECTOR (`[data-dir="n"]`), a
            // quoted value is part of the body. Only the brace/semicolon
            // MEANING is suspended inside the quotes.
            if depth == 0 {
                selector.push(ch);
            } else if depth == 1 && in_body {
                body.push(ch);
            }
            continue;
        }
        match ch {
            '{' => {
                depth += 1;
                if depth == 1 {
                    in_body = true;
                    body.clear();
                }
            }
            '}' => {
                if depth == 1 && in_body {
                    // An at-rule (`@media`, `@supports`) holds nested rules
                    // rather than declarations; its overrides are the point.
                    if !selector.trim().starts_with('@') {
                        let sel = selector.split_whitespace().collect::<Vec<_>>().join(" ");
                        // One entry per MEMBER of a comma group. Keying on
                        // the raw selector text made `.a { opacity: 1 }` and
                        // `.a, .b { opacity: 0.5 }` two different keys —
                        // which is the commonest real shape of the very
                        // collision this gate was written for.
                        // NOT split on commas, and this was measured rather
                        // than assumed. A review asked for the split: keying
                        // on the raw selector text means `.a { … }` and
                        // `.a, .b { … }` are different keys, so a collision
                        // between them is missed. Splitting the group does
                        // find those — and it also reds on the ordinary CSS
                        // idiom of a base rule for a group followed by a
                        // refinement for one member, which this stylesheet
                        // uses correctly: `.md-body h1..h4` set
                        // `letter-spacing: -0.011em`, then `.md-body h1` sets
                        // `-0.02em`. A gate that fails conformant code is
                        // worse than one with a known blind spot, so the
                        // blind spot is stated instead — a collision is
                        // reported only between two blocks whose selector
                        // text is identical.
                        // Within ONE block, re-declaring a property is the
                        // documented CSS fallback idiom — `height: 100vh`
                        // then `height: 100dvh` is how a browser without
                        // `dvh` still gets a height. So each block is folded
                        // to its own last-wins map first, and only the
                        // ACROSS-block collisions are reported.
                        let mut block: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        for decl in body.split(';') {
                            let Some((prop, value)) = decl.split_once(':') else {
                                continue;
                            };
                            let prop = prop.trim().to_string();
                            let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
                            if prop.is_empty() || prop.starts_with("--") {
                                continue;
                            }
                            block.insert(prop, value);
                        }
                        for (prop, value) in block {
                            if let Some(first) =
                                seen.insert((sel.clone(), prop.clone()), value.clone())
                            {
                                assert_eq!(
                                    first, value,
                                    "the stylesheet declares `{prop}` on `{sel}` in two \
                                     separate blocks with different values ({first} then \
                                     {value}) — source order decides which one renders, and \
                                     the other is dead"
                                );
                            }
                        }
                    }
                    in_body = false;
                    selector.clear();
                }
                depth = depth.saturating_sub(1);
            }
            _ if depth == 0 => selector.push(ch),
            _ if depth == 1 => body.push(ch),
            _ => {}
        }
    }

    // NEGATIVE CONTROL: a parse that found nothing would pass silently. The
    // stylesheet is thousands of declarations; this states that the walk
    // actually reached them.
    assert!(
        seen.len() > 1_000,
        "the stylesheet walk collected only {} declarations — it is not \
         parsing the file",
        seen.len()
    );
}
