//! The chrome of one console: the whole-screen control and the three rules of its frame.

use super::*;

/// Per-console fullscreen, and the tablet's way out of it.
///
/// Everything here is a SAFETY pin, not a feature pin. Fullscreen puts the
/// window in the top layer, where the browser sizes it to the display and
/// outranks every author rule — so the two places that read a window's box
/// (the stage extent and the desk record) must read the INLINE rect instead,
/// or a reload brings the console back the size of a monitor. And the
/// control's look must be derived from the browser's own event, because on a
/// tablet there is no Esc: a stale "exit" icon over a window that already
/// left fullscreen is the operator's only exit, pointing at nothing.
///
/// `fullscreenOffered` is driven by the node suite; the rest is DOM wiring
/// and CSS that no node test runs, so it fails here or nowhere.
#[test]
fn a_console_can_take_the_whole_screen() {
    // The console, its window chrome and its view, as one text: the full-screen state lives
    // in the console, its button and the gesture guards in the chrome, the registration of
    // the browser's event in the view.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-window.ts"),
        include_str!("../../assets/ui/wb-stage-chrome.ts"),
        include_str!("../../assets/ui/wb-stage-view.ts"),
    ]
    .concat();
    for pin in [
        "function toggleFull(",
        "function syncFullState(",
        "function isFull(",
        // The registration, not the function: without it nothing re-derives
        // the control after an Esc, a system swipe, or the browser dropping
        // fullscreen on its own.
        r#"document.addEventListener("fullscreenchange", syncFullState)"#,
        // Built only where the browser can HOLD it, which is two
        // questions, not one: is the API there at all, and is this the
        // engine that hands fullscreen back the moment the keyboard
        // rises. A control the next tap cancels is worse than no control.
        "fullBtn.hidden = !fullscreenOffered(document.fullscreenEnabled, navigator.vendor)",
        "win.requestFullscreen()",
        "document.exitFullscreen()",
    ] {
        assert!(
            js.contains(pin),
            "the console must keep the fullscreen pin {pin}"
        );
    }
    // The guards that keep the inline rect honest while the top layer owns
    // the geometry: the rect read skips a fullscreen window, and the two
    // gestures refuse one. Each is found in its own function, in any layout.
    let fn_body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        squeeze(&after[..after.find("\n  }").expect("the function must close")])
    };
    assert!(
        fn_body("function restoreRect(").contains("!isFull(win)"),
        "restoreRect must not read a fullscreen window's box"
    );
    for gesture in ["function makeDraggable(", "function startResize("] {
        assert!(
            fn_body(gesture).contains("isFull(win)"),
            "{gesture} must refuse a fullscreen window"
        );
    }
    // The click handler must NOT paint the icon: that is `syncFullState`'s
    // single job, and a second writer is exactly how the stale-exit bug
    // comes back. `toggleFull` is bounded by the next function comment.
    let body = js
        .split_once("function toggleFull(")
        .expect("toggleFull must exist")
        .1;
    let body = &body[..body
        .find("\n  // The fullscreen control's LOOK")
        .unwrap_or(body.len())];
    assert!(
        !body.contains("bi-fullscreen-exit"),
        "toggleFull must not write the control's icon — syncFullState derives it"
    );

    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    // The touch target IS the exit on a tablet. 44px is Apple's HIG floor;
    // the 22px resting chrome is a mouse-only size.
    assert!(
        rule("\n.session-window.fullscreen .session-actions button {").contains("width: 44px"),
        "the fullscreen titlebar must offer a 44px touch target — a tablet has no Esc"
    );
    // The home indicator overlays the bottom of a fullscreen element.
    assert!(
        rule("\n.session-window:fullscreen {").contains("env(safe-area-inset-bottom"),
        "a fullscreen console must inset for the home indicator"
    );

    // The iPad's OTHER answer, which needs no fullscreen state at all:
    // installed to the home screen the shell runs chrome-less for good.
    let html = super::INDEX_HTML;
    for pin in [
        r#"<link rel="manifest" href="manifest.webmanifest" />"#,
        r#"<link rel="apple-touch-icon" href="icon-192.png" />"#,
        r#"<meta name="apple-mobile-web-app-capable" content="yes" />"#,
    ] {
        assert!(
            html.contains(pin),
            "index.html must carry the install pin {pin}"
        );
    }
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../assets/ui/manifest.webmanifest"))
            .expect("the manifest must be valid JSON — a broken one is ignored silently");
    assert_eq!(
        manifest["display"], "standalone",
        "the installed shell must launch without browser chrome"
    );
    // Served as octet-stream the manifest is ignored by every browser, and
    // the failure is silent: the shell renders, the install never offers.
    assert_eq!(
        content_type("manifest.webmanifest"),
        "application/manifest+json"
    );
    for icon in ["icon-192.png", "icon-512.png"] {
        assert!(
            UI.get_file(icon).is_some(),
            "{icon} must be embedded — the manifest and apple-touch-icon both name it"
        );
    }
}

/// Three pieces of chrome that only a browser can really prove, pinned here
/// because the browser pass — Playwright — does not run in CI.
#[test]
fn the_console_chrome_holds_its_three_rules() {
    let app = shell_family();
    let tab = include_str!("../../assets/ui/wb-consoles-tab.ts");
    let html = super::INDEX_HTML;
    // ONE dropdown at a time. Every toggler sends ONE event,
    // `workbench:menus-close`, and the shell and the two consoles components
    // each close their own menus on it — the account menu and the toolbar's
    // pickers used to enumerate each other and left both open, overlapping.
    // wb-consoles-tab.test.mjs and app.test.mjs drive the event.
    let close = r#"sendWindow(window, "workbench:menus-close");"#;
    let hear = r#"addEventListener("workbench:menus-close", "#;
    // Counted, not `contains`: the family also holds the two consoles
    // components, so a bare `contains` stays green when the shell's own send
    // or hear is gone. Five is the shell's one, their two, the + menu's, and
    // the attention panel's (wb-attention.ts).
    assert!(
        app.matches(close).count() == 5
            && app.matches(hear).count() == 5
            && app.contains("this.avatarMenu = false;"),
        "the shell must close the account menu on the one menus event, and send it"
    );
    // The account menu's send and hear are the third of each above.
    let account = include_str!("../../assets/ui/wb-account.ts");
    assert!(
        account.matches(close).count() == 1
            && account.matches(hear).count() == 1
            && account.contains("this.avatarMenu = false;"),
        "the account menu (wb-account.ts) must close on the one menus event, and send it"
    );
    assert!(
        tab.matches(close).count() == 2 && tab.matches(hear).count() == 2,
        "both consoles components must send the one menus event and hear it"
    );
    for pin in ["toggleAvatarMenu()", "toggleAgentMenu()"] {
        assert!(html.contains(pin), "index.html must toggle through {pin}");
    }
    assert!(
        !html.contains("avatarMenu = !avatarMenu"),
        "the account button must not toggle its own flag past the others"
    );
    // One picture for one thing: the Consoles tab, the New-console button and
    // the rows in its menu all wear the same terminal glyph.
    assert!(
        app.contains(r#"icon: "bi bi-terminal""#) && !app.contains("bi-robot"),
        "the Consoles tab must wear the terminal glyph, not a robot"
    );
    // A fence name is read-only until asked for twice: its title bar is also
    // what the operator clicks to reach the fence, and an always-live input
    // turned every such slip into a rename.
    let js = include_str!("../../assets/ui/wb-stage-fences.ts");
    assert!(
        js.contains("name.readOnly = true;") && js.contains(r#"name.addEventListener("dblclick""#),
        "the fence name must open on a double click and close on blur"
    );
}
