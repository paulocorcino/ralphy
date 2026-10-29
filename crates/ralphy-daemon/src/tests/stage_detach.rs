//! A fence or a file in its own window: the detach, the way home and the reload.

use super::*;

/// A detached file comes home however its popup closes: the popup sends its
/// edited bytes on unload, and the shell polls `closed` for one that dies
/// without an unload. No node test fires an unload on the popup's page
/// script (`wb-detached.ts`), so a deletion fails HERE or nowhere. Pins are
/// expressions, not nouns (#342).
#[test]
fn a_detached_file_comes_home_when_its_popup_closes() {
    let popup = include_str!("../../assets/ui/wb-detached.ts");
    for pin in [
        r#"window.addEventListener("beforeunload", sendHome)"#,
        r#"window.addEventListener("pagehide", sendHome)"#,
        r#"window.WBViewer?.descOf("detached")"#,
    ] {
        assert!(
            popup.contains(pin),
            "wb-detached.ts must keep the unload re-attach {pin}"
        );
    }
    let app = shell_family();
    for pin in [
        "detached.watch(win, desc);",
        "window.setInterval(pollDetached, 500)",
        "reattachFile(e.source as Window, m.desc);",
    ] {
        assert!(
            app.contains(pin),
            "the shell must keep the detached-file re-attach {pin}"
        );
    }
}

/// A fence detaches into its own window, and comes home (#346). The fold
/// (`detachFold`, with its cap of four) is driven by
/// `ui-tests/wb-console.test.mjs`; the DOM half, the popup document and the
/// CSS run in no node test, so a deletion there fails HERE or nowhere. Every
/// pin is an EXPRESSION, not a bare noun: #342 measured that a function's
/// own explanatory comment satisfies a noun pin, leaving it green over
/// deleted code.
#[test]
fn shell_detaches_a_fence() {
    // The console, its fence list, its fences, its desk and its detach, as
    // one text: the detach lives in the fences, the popup's mount and the
    // upload in the desk, the re-attach and the lifecycle channel in the
    // opener's side of the detach, the refused remove in the fence list.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-stage-fence-list.ts"),
        include_str!("../../assets/ui/wb-stage-fences.ts"),
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../../assets/ui/wb-desk.ts"),
        include_str!("../../assets/ui/wb-desk-restore.ts"),
        include_str!("../../assets/ui/wb-desk-detach.ts"),
    ]
    .concat();
    for pin in [
        "function detachFence(",
        "function reattachFence(",
        "function mountDetached(",
        "fence-detach",
        "fence-detached",
    ] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #346 pin {pin}"
        );
    }
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("wb-console.ts must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    // THE SINK SEAM. The persistence call sites must reach the desk through
    // the injected sink and carry NO `detached` branch of their own —
    // "incapable of writing", not "careful not to".
    assert!(
        squeeze(&body("function flushDesk(")).contains(".put(JSON.stringify(body))"),
        "flushDesk must write through the injected sink (#346)"
    );
    assert!(
        !body("function flushDesk(").contains("detach"),
        "flushDesk must carry no detached branch — the sink is the seam (#346)"
    );
    assert!(
        js.contains("deskSink.putSync("),
        "the pagehide flush must write through the injected sink (#346)"
    );
    // The PUT literal now lives in exactly one file. The second half is the
    // NEGATIVE CONTROL: deleting the write wholesale would satisfy the first
    // assertion alone.
    // The family holds the sink itself, which is the one place that writes.
    let outside_sink: String = CONSOLE_FAMILY
        .iter()
        .filter(|(name, _)| *name != "wb-desk-sink.ts")
        .map(|(_, text)| *text)
        .collect();
    assert!(
        !js.contains(r#""/api/desk", {"#)
            && !js.contains(r#""PUT /api/desk""#)
            && !outside_sink.contains(r#""/api/desk", {"#)
            && !outside_sink.contains(r#""PUT /api/desk""#),
        "the desk PUT must live only in wb-desk-sink.ts (#346)"
    );
    let sink = include_str!("../../assets/ui/wb-desk-sink.ts");
    assert!(
        sink.contains(r#"apiFetch("PUT /api/desk", {"#) && sink.contains("query: { tab: TAB },"),
        "wb-desk-sink.ts must still perform the desk PUT (#346)"
    );
    assert!(
        sink.contains("keepalive: true"),
        "the sink's putSync must outlive the closing document (#346)"
    );
    // Arrange is a no-op on a detached fence (ADR-0051 §7a): its consoles are
    // in another window, and tiling the empty box would rewrite the very
    // rects the re-attach restores from.
    assert!(
        body("function arrangeFence(").contains("if (popups.isDetached(id)) return;"),
        "arrangeFence must bail on a detached fence (#346, §7a)"
    );
    // Removing a detached fence would destroy the glyph that is the only way
    // home and keep its DETACH_MAX slot consumed — refuse it, do not silently
    // strand the popup.
    assert!(
        body("function removeFence(").contains("if (isDetached(id)) {"),
        "removeFence must refuse a detached fence (#346, §7a)"
    );
    // The re-attach message must trust the SOURCE lookup, not the payload:
    // `m.fenceId` would let a popup for fence A re-attach fence B.
    assert!(
        !js.contains("reattachFence(m.fenceId"),
        "the re-attach message must use the proven owner, never its payload (#346)"
    );
    // The INVARIANT: either the popup exists and the members are torn down,
    // or neither. `window.open` must therefore be reached before a single
    // window is touched, and a null handle must bail.
    let detach = body("function detachFence(");
    assert!(
        detach.contains("window.open(\"fence\""),
        "detachFence must open the popup document (#346)"
    );
    assert!(
        detach.contains("if (!handle)"),
        "a blocked popup must bail BEFORE anything is torn down (#346)"
    );
    assert!(
        detach
            .find("window.open(")
            .expect("detachFence must open a popup")
            < detach
                .find("tearDownMember(")
                .expect("detachFence must tear its members down"),
        "the popup must be open before a member is torn down — the #346 invariant"
    );
    // A member leaves the plane WITHOUT losing its desk record and WITHOUT
    // closing its daemon session: the record is shared state a second client
    // still renders, and the socket close is the writer-slot release (§9).
    let teardown = body("function tearDownMember(");
    assert!(
        teardown.contains("win._term?.dispose(") && teardown.contains("wins.delete(win)"),
        "tearDownMember must dispose the terminal and drop the window (#346, §9)"
    );
    assert!(
        !teardown.contains("forgetRecord") && !teardown.contains("sessions/close"),
        "a detached member must keep its desk record and its daemon session (#346)"
    );
    // THE POPUP'S DENIED CAPABILITIES. Each is what stops a window holding a
    // FRAGMENT of the plane from writing the whole desk or the shell's view.
    // The options are built by the page's entry module,
    // `detached-fence-main.ts`; the handshake is `wb-detached-fence.ts`.
    let html = super::DETACHED_FENCE_HTML;
    let entry = include_str!("../../assets/ui/detached-fence-main.ts");
    let page = include_str!("../../assets/ui/wb-detached-fence.ts");
    for pin in [
        "WBDeskSink.none()",
        "autoBoot: false",
        "canLaunch: false",
        "read: () => null",
    ] {
        assert!(
            entry.contains(pin),
            "detached-fence-main.ts must keep the #346 pin {pin}"
        );
    }
    for pin in [
        "WBConsole.mountDetached(",
        "wb-fence-ready",
        "wb-fence-reattach",
    ] {
        assert!(
            page.contains(pin),
            "wb-detached-fence.ts must keep the #346 pin {pin}"
        );
    }
    assert!(
        !html.contains("WBConsole.open(")
            && !entry.contains("WBConsole.open(")
            && !page.contains("WBConsole.open("),
        "the popup must expose no way to open a new console (#346)"
    );
    // The handshake's confidentiality control: a concrete targetOrigin, so a
    // page on any other origin never receives this fence's members. Pinned as
    // the ASSIGNMENT, not the bare noun — `window.location.origin` also
    // appears in the inbound guard below it, so rewriting `PEER` to `"*"`
    // would leave a noun pin green. Both scripts post to the opener, and
    // each has its own `PEER`.
    for (name, text) in [
        ("detached-fence-main.ts", entry),
        ("wb-detached-fence.ts", page),
    ] {
        assert!(
            text.contains("const PEER = window.location.origin;"),
            "{name}: the popup's PEER must resolve to a concrete origin (#346)"
        );
    }
    assert!(
        !page.contains(r#"postMessage({ type: "wb-fence-ready" }, "*")"#),
        "the popup must never broadcast its handshake to \"*\" (#346)"
    );
    // The opener's reply has the same control as the popup's PEER: a concrete
    // origin, never a broadcast.
    let opener = body("window.addEventListener(\"message\"");
    assert!(
        opener.contains("if (e.origin !== location.origin) return;") && !opener.contains("\"*\""),
        "the opener's handover must answer only its own origin (#346)"
    );
    // The tree-wide sweep in `shell_stores_only_the_view_in_the_browser`
    // scans .html too; keep this document out of the browser's stores.
    assert!(
        !html.contains("localStorage")
            && !html.contains("sessionStorage")
            && !entry.contains("localStorage")
            && !entry.contains("sessionStorage"),
        "the popup must store nothing in the browser (#346)"
    );
    // `.fence-tools` and `.fence` are transparent to pointer events, so a
    // control that does not opt back IN is drawn and unclickable — while
    // every source-text pin above stays green. Measured in #342.
    let css = served_css();
    let rule = |head: &str| -> String {
        let after = css
            .split_once(head)
            .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
            .1;
        after[..after.find('}').expect("the rule must close")].to_string()
    };
    for head in ["\n.fence-detach {", "\n.fence-detached {"] {
        assert!(
            rule(head).contains("pointer-events: auto"),
            "{head} must take pointer events, or the control is inert (#346)"
        );
    }
}

/// The detach survives an F5, and dies with the tab that opened it (#347).
/// Same split as `shell_detaches_a_fence`: the fold (`peerFold`) is driven by
/// `ui-tests/wb-console.test.mjs`, and the wiring and the popup document are
/// pinned here. Every pin is an EXPRESSION — #342 measured that a function's
/// own explanatory comment satisfies a bare-noun pin over deleted code.
#[test]
fn shell_survives_a_reload_with_its_detach() {
    // The console, its popup registry, its fences, its desk and its detach,
    // as one text: the registry write lives in the registry, the detach in
    // the fences, the restore in the desk, and the lifecycle channel in the
    // opener's side of the detach.
    let js = &[
        include_str!("../../assets/ui/wb-console.ts"),
        include_str!("../../assets/ui/wb-desk-popups.ts"),
        include_str!("../../assets/ui/wb-stage-fences.ts"),
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
        include_str!("../../assets/ui/wb-desk.ts"),
        include_str!("../../assets/ui/wb-desk-restore.ts"),
        include_str!("../../assets/ui/wb-desk-detach.ts"),
    ]
    .concat();
    let link = include_str!("../../assets/ui/wb-detach-link.ts");
    let html = super::DETACHED_FENCE_HTML;
    let entry = include_str!("../../assets/ui/detached-fence-main.ts");
    let page = include_str!("../../assets/ui/wb-detached-fence.ts");
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("wb-console.ts must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };

    // The shell reaches storage and channel only through the injected link.
    for pin in ["link.readRegistry()", "link.writeRegistry("] {
        assert!(
            js.contains(pin),
            "wb-console.ts must keep the #347 pin {pin}"
        );
    }
    // THE STORE SEAM, with its NEGATIVE CONTROL: no console family module may
    // name the session store, and the link module must be the one that does
    // — deleting the store wholesale has to be red, not green.
    for (name, src) in CONSOLE_FAMILY {
        assert!(
            !src.contains("sessionStorage"),
            "{name} must reach the registry only through the injected link (#347)"
        );
    }
    // The CALLS, not the bare nouns: this file's own prose names
    // `sessionStorage` three times, so a noun pin stays green over
    // no-op'd `readRecord`/`writeRecord` bodies — the exact defect the
    // doc comment above warns about.
    assert!(
        link.contains("sessionStorage.getItem(KEY)")
            && link.contains("sessionStorage.setItem(KEY,")
            && link.contains("new BroadcastChannel(CHANNEL)"),
        "wb-detach-link.ts IS the registry and the channel — deleting it is not how #347 stays green"
    );
    assert!(
        link.contains(r#"const KEY = "wb.detach.v1""#)
            && link.contains(r#"const CHANNEL = "wb.detach.v1""#),
        "wb-detach-link.ts must keep the one registry key and channel name (#347)"
    );
    // The boot-ordering INVARIANT: a fence detached before the reload must
    // never have its members put back on the plane, for ANY verdict —
    // `relaunch` would spawn a SECOND PTY against a console the popup drives.
    assert!(
        body("function restoreDesk(").contains("record && away.has(record.id)"),
        "restoreDesk must skip a detached fence's members (#347)"
    );
    // `reconcileDesk` emits `record: null` on the `adopt` verdict, and the
    // skip runs BEFORE the dispatch — an unguarded read threw into the
    // swallowing `.catch`, costing every fence and every glyph on any boot
    // carrying a live session no record claims.
    assert!(
        include_str!("../../assets/ui/wb-desk-folds.ts")
            .contains(r#"out.push({ record: null, session: s, action: "adopt", id: null })"#),
        "the adopt verdict's null record is what the guard above exists for (#347)"
    );
    // The registry carries the MEMBER IDS, not just the fence ids: a
    // detached fence may still be moved on the plane (§7a), after which a
    // geometry-derived membership answers "no members" and every one of
    // them comes back under a live popup.
    assert!(
        link.contains("readMembers()") && js.contains("link.readMembers()"),
        "the registry must carry each detached fence's member ids (#347)"
    );
    // The registry mirror and the store change in ONE place, so no return
    // path can leave them disagreeing.
    assert!(
        body("function commitDetached(")
            .contains("link.writeRegistry(detached, detachedMembers())"),
        "every detach transition must go through commitDetached (#347)"
    );
    assert!(
        !js.contains("detached = out.registry")
            && !console_family().contains("detached = out.registry"),
        "no caller may assign the registry behind commitDetached's back (#347)"
    );
    // THE F5 RULE: `pagehide` fires on a reload exactly as on a close, so it
    // must no longer close a popup. The `putSync` pin is the NEGATIVE
    // CONTROL that the listener itself was not simply deleted.
    assert!(
        !body("window.addEventListener(\"pagehide\"").contains(".handle.close()"),
        "pagehide must not close a detached popup — that is the F5 (#347)"
    );
    assert!(
        js.contains("deskSink.putSync("),
        "the pagehide flush must survive the popup-close deletion (#347)"
    );
    // THE POPUP'S FIFTH DENIED CAPABILITY, in its options.
    assert!(
        entry.contains("detachLink: WBDetachLink.none()"),
        "detached-fence-main.ts must keep the #347 pin detachLink: WBDetachLink.none()"
    );
    // Its half of the lifecycle, in its page script.
    for pin in [
        // The channel-only factory: this document must reach no store, not
        // even to read the copy `window.open` handed it.
        "WBDetachLink.channel()",
        "\"popup-here\"",
        "\"popup-gone\"",
        // The BEHAVIOUR, not the class name: `detached-lost` alone is
        // satisfied by the CSS rule in the page's own <style> block, so
        // deleting `lost()` outright would keep a bare-noun pin green.
        r#"'<p class="detached-lost">"#,
        "window.close()",
        "WBConsole.peerFold(",
    ] {
        assert!(
            page.contains(pin),
            "wb-detached-fence.ts must keep the #347 pin {pin}"
        );
    }
    // #346's confidentiality control is UNCHANGED: the channel carries only
    // lifecycle chatter, the members still ride the concrete-origin handshake.
    assert!(
        page.contains("const PEER = window.location.origin;"),
        "the initial handover must keep its concrete targetOrigin (#346, #347)"
    );
    assert!(
        !html.contains("localStorage")
            && !html.contains("sessionStorage")
            && !entry.contains("localStorage")
            && !entry.contains("sessionStorage")
            && !page.contains("localStorage")
            && !page.contains("sessionStorage"),
        "the popup must still store nothing in the browser (#346)"
    );
}

/// Silence is not death. A browser throttles a hidden tab's timers to one
/// tick per minute after about five minutes, so the six-second heartbeat
/// window closed a working popup and pulled its consoles home mid-work. Both
/// documents now demand a better witness before acting — a window handle,
/// or a probe that message delivery (which is not timer-throttled) still
/// answers. `wb_fence_347.py` scenario 5c drives it; this is CI's view.
#[test]
fn a_quiet_detach_peer_is_challenged_before_it_is_buried() {
    // Whitespace-free text: the code, not its layout. The opener's side of
    // the detach is `wb-desk-detach.ts`.
    let js = squeeze(include_str!("../../assets/ui/wb-desk-detach.ts"));
    assert!(
        js.contains("functionstillThere("),
        "the origin must ask whether a quiet popup is really gone"
    );
    assert!(
        js.contains("&&!stillThere("),
        "the re-attach must be gated on that answer, not on the fold alone"
    );
    assert!(
        js.contains(r#"m.type==="popup-ping""#) && js.contains(r#"type:"origin-here""#),
        "the origin must answer a probe from its MESSAGE handler, not a timer"
    );
    let popup = squeeze(include_str!("../../assets/ui/wb-detached-fence.ts"));
    assert!(
        popup.contains("constopenerGone=()=>opener()?.closed??true;"),
        "the popup's verdict is the opener HANDLE, which owes nothing to a timer"
    );
    assert!(
        popup.contains(r#"==="peer-lost"))silent();"#),
        "a lost peer must reach `silent`, which probes, and never `lost` directly"
    );
}
