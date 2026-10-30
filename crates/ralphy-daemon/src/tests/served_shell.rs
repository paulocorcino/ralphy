//! The page the daemon serves: its headers, its text, its favicon and its fixed tabs.

use super::*;

/// The security headers ride EVERY response (audit F3): the shell, an
/// asset, an API answer and a refusal alike, from one layer over the
/// router. The CSP's `script-src` admits no injected tag: no shell carries
/// an inline script, so it names no hash and no `'unsafe-inline'`.
#[tokio::test]
async fn every_response_carries_the_security_headers() {
    for path in ["/", "/app.js", "/api/session", "/api/nope"] {
        let resp = get_local(path).await;
        let h = resp.headers();
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY", "{path}");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{path}");
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer", "{path}");
        let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'"), "{path}: {csp}");
        assert!(
            csp.contains("script-src 'self' 'unsafe-eval';"),
            "{path}: {csp}"
        );
        assert!(
            csp.contains("img-src 'self' data: blob:;"),
            "remote images are opt-in, off by default: {path}: {csp}"
        );
        // A socket goes only to the daemon's own origin (ADR-0072 D10).
        assert!(
            csp.contains("connect-src 'self';") && !csp.contains("ws:"),
            "{path}: {csp}"
        );
        // The client hints the audit log records (ADR-0074).
        let accept_ch = h["accept-ch"].to_str().unwrap();
        assert!(
            accept_ch.contains("Sec-CH-UA-Model")
                && accept_ch.contains("Sec-CH-UA-Platform-Version"),
            "{path}: {accept_ch}"
        );
    }
    // No served shell carries an inline script (ADR-0075: every page script
    // is a module), so the header's hash list is empty.
    for path in ["/", "/popup", "/fence"] {
        let shell = body_string(get_local(path).await).await;
        assert_eq!(
            routes::inline_script_bodies(&shell),
            Vec::<&str>::new(),
            "{path} carries an inline script"
        );
    }
    let csp = routes::content_security_policy(false).to_str().unwrap();
    assert!(!csp.contains("'sha256-"), "{csp}");
}

/// The one inline form a hash cannot cover: an `on*=` event-handler
/// attribute (it would need `'unsafe-hashes'`). None of the shells has one —
/// Alpine's `@click` is the idiom — and this keeps it that way.
#[tokio::test]
async fn no_shell_carries_an_inline_event_handler() {
    let re = regex::Regex::new(r#"\son[a-z]+="#).unwrap();
    for path in ["/", "/popup", "/fence"] {
        let body = body_string(get_local(path).await).await;
        for line in body.lines() {
            let live = line.split("<!--").next().unwrap_or(line);
            assert!(!re.is_match(live), "{path}: inline handler in {line:?}");
        }
    }
}

/// The workbench routes a `.note` to its CARD and never to a tab
/// (ADR-0064 §11), and mirrors the daemon's one denylist carve-out.
///
/// Both halves are load-bearing and neither is visible to a substring pin
/// of the other: a `.note` opened as a tab would be a SECOND editor over a
/// file a card already holds — the exact thing §11 exists to prevent — and
/// a UI that still believed `.ralphy/notes/` was protected would hide the
/// gestures the daemon now accepts there.
#[test]
fn the_explorer_opens_a_note_as_a_card() {
    let paths = include_str!("../../assets/ui/wb-file-paths.ts");
    assert!(
        paths.contains(r#"if (ext === "note") return "note";"#),
        "classify must name a `.note` (ADR-0064 §11)"
    );
    let files = include_str!("../../assets/ui/wb-files.ts");
    assert!(
        files.contains("this.$store.tabs.openNote(path)"),
        "wb-files.ts must keep the ADR-0064 pin openNote(path)"
    );
    let app = shell_family();
    assert!(
        app.contains("WBNotes.openFromExplorer("),
        "the shell must keep the ADR-0064 pin WBNotes.openFromExplorer("
    );
    // The UI's mirror of the denylist carve-out (`isNoteInNotesDir`) is
    // driven through the context menu by `ui-tests/wb-files.test.mjs`.
    // The card's own file actions go through the GENERIC byte-ops — there
    // is no `note.rename`/`note.delete`, and adding one would re-derive the
    // confinement the carve-out already gives.
    let notes = include_str!("../../assets/ui/wb-notes.ts");
    let file = include_str!("../../assets/ui/wb-notes-file.ts");
    for pin in [r#"write("file.rename""#, r#"write("file.delete""#] {
        assert!(
            file.contains(pin),
            "wb-notes-file.ts must keep the pin {pin}"
        );
    }
    // The native dialogs are pinned OUT for the reason `wb-messages.ts`
    // records: an automated browser dismisses them by default, which turns
    // a guarded click into a silently cancelled one.
    let stage = include_str!("../../assets/ui/wb-notes-stage.ts");
    let chrome = include_str!("../../assets/ui/wb-notes-chrome.ts");
    for js in [notes, stage, file, chrome] {
        assert!(
            !js.contains("window.confirm(") && !js.contains("window.prompt("),
            "a note's file actions must use the workbench's own dialog and field"
        );
    }
    assert!(
        file.contains("messages?.askConfirm({"),
        "deleting a note's FILE must ask first (ADR-0064 §11)"
    );
    // THE LOCK PINS THE CARD AND NOTHING ELSE (ADR-0064, amendment
    // 2026-09-22): §8 gave it a second job — put the editor in read-only —
    // and the two are different questions. A pinned note is still typed
    // into. This is the NEGATIVE CONTROL for that clause: the gesture
    // guards in `makeDraggable`/`startResize` are the whole of the lock,
    // and a `setReadonly` wired back to it would silently take the note
    // away again.
    assert!(
        !notes.contains("setReadonly(!!locked)"),
        "the lock pins the card; it must not put the editor in read-only \
         (ADR-0064 amendment)"
    );
    assert!(
        notes.contains("locked: () => !!el._noteLocked"),
        "the lock must reach the drag and the resize, which are what it IS"
    );
}

/// The vendored Crepe bundle states where it came from, and the recipe that
/// built it is in the repository (ADR-0064 §6, ADR-0057).
///
/// Crepe is the workbench's first vendored asset that is BUILT rather than
/// copied, so "which upstream, at which version, with which features" is
/// not recoverable by diffing a tarball — the header line is the record,
/// and this test is what keeps it honest. The three constants are read from
/// the recipe itself, so a bump that edits `package.json` without rerunning
/// `node build.mjs` reds here rather than shipping a bundle whose header
/// lies about it.
///
/// A note's HAND and SIZE are a closed set on both sides of the seam
/// (ADR-0064 §8, amendment 2026-09-22). The shell writes the name into the
/// file's front matter and onto a `data-*`; the stylesheet is what turns it
/// into a face. A name in one list and not the other is the silent failure
/// this closes — the card would carry `data-font="hand"`, no rule would
/// match, and it would paint in the default with the palette still showing
/// the chip as chosen.
///
/// `sans` and `m` are deliberately absent from the CSS: they are the
/// declared defaults on `.note-card` itself, so a rule for them would be a
/// second place to change one value.
#[test]
fn a_notes_hand_and_size_are_a_closed_set_on_both_sides() {
    let palette = include_str!("../../assets/ui/wb-notes-palette.ts");
    let folds = include_str!("../../assets/ui/wb-notes-folds.ts");
    let css = include_str!("../../assets/ui/styles/13-notes.css");
    assert!(
        folds.contains(r#"const FONTS = ["sans", "serif", "mono"]"#)
            && folds.contains(r#"const SIZES = ["xs", "s", "m", "l", "xl"]"#),
        "the hand and the size must be closed sets, not a free font-family string"
    );
    for name in ["serif", "mono"] {
        assert!(
            css.contains(&format!(r#".note-card[data-font="{name}"]"#))
                && css.contains(&format!(r#".note-swatch-font[data-name="{name}"]"#)),
            "the font {name} must have a face on the card AND on its chip"
        );
    }
    for name in ["xs", "s", "l", "xl"] {
        assert!(
            css.contains(&format!(r#".note-card[data-size="{name}"]"#)),
            "the size {name} must have a rule, or the chip sets nothing"
        );
    }
    // `hidden` MUST BE DRAWN, not merely set. `.note-menu-item` carries
    // `all: unset`, which is an author `display: inline` and therefore
    // beats the UA's `[hidden] { display: none }` — SEEN in a browser
    // 2026-09-22 with the missing card's `Use another file…` sitting in a
    // healthy note's menu. The popovers learned this twice already; this
    // is the assertion for the items, where two verbs now depend on it.
    assert!(
        css.contains(".note-menu-item[hidden]"),
        "an item this shell hides must be hidden in paint too (`all: unset` resets display)"
    );
    // The EDITOR is what the operator is actually looking at: the card's
    // tokens have to reach Crepe, or only the card's own padding changes.
    assert!(
        css.contains("--crepe-base-font-size: var(--note-size);")
            && css.contains("--crepe-font-default: var(--note-font);"),
        "the card's hand and size must reach the editor, not stop at the body"
    );
    // A MERMAID FENCE IS DRAWN ON THE CARD, not in a window cut out of it
    // (asked 2026-09-22). Mermaid derives its whole palette from a few
    // seeds by lightening, darkening and inverting them and never looks at
    // the page, so a built-in theme lands its own canvas on whatever tone
    // the note is wearing. The seeds come from the card instead, and the
    // host paints nothing behind the drawing.
    let crepe = include_str!("../../vendor-build/crepe/entry.js");
    assert!(
        !crepe.contains("theme: 'dark'") && crepe.contains("theme: 'base'"),
        "only `base` takes replacement theme variables; the built-ins fight the card"
    );
    assert!(
        crepe.contains("themeDirective(host) + source"),
        "the palette must travel WITH the diagram — `initialize` is global and two \
         cards render at once"
    );
    assert!(
        css.contains("background: transparent;"),
        "the mermaid host must paint nothing behind the drawing"
    );
    // The colours are inline fills in the SVG, so the cascade cannot reach
    // them: a restyle has to redraw or the diagram keeps the old tone.
    assert!(
        palette.contains("el._noteEditor?.redrawDiagrams?.()") && crepe.contains("redrawDiagrams:"),
        "restyling a card must repaint the diagrams in it (ADR-0064 §15 amendment)"
    );
    // Written to the FILE, so a note keeps its face when the card is
    // closed and opened again — the look is the note's, not the desk's.
    assert!(
        folds.contains("lines.push(`font: ${font}`)")
            && folds.contains("lines.push(`size: ${size}`)"),
        "the hand and the size belong in the note's front matter (ADR-0064 §8)"
    );
}

/// The terminal wears the workbench's palette, and wears it in lockstep.
///
/// xterm.js reads no CSS variable — WebGL paints the glyphs — so the theme is
/// literal hex mirroring `:root`. That mirror is the only thing a drift can
/// break, and it breaks silently: a console a different shade from the pane
/// around it. The ANSI slots stay xterm's own, so this pins the base colours
/// only.
#[test]
fn the_console_terminal_is_themed_in_lockstep_with_the_stylesheet() {
    let js = include_str!("../../assets/ui/wb-console-terminal.ts");
    let css = served_css();
    assert!(
        js.contains("new Terminal({ convertEol: false, theme: TERMINAL_THEME })"),
        "wb-console-terminal.ts must hand xterm a theme — an unthemed Terminal is xterm's black default"
    );
    assert!(
        js.contains("background: \"#000000\""),
        "the terminal surface is pure black — the same exception wb-monaco.ts makes for code"
    );
    for (token, hex) in [
        ("--text", "#d4ccc0"),
        ("--console-text", "#e8d9a8"),
        ("--surface-hi", "#423a31"),
    ] {
        assert!(
            js.contains(hex),
            "TERMINAL_THEME must still carry {token}'s value {hex}"
        );
        assert!(
            css.contains(&format!("{token}: {hex};")),
            "styles.css must still define {token} as {hex} — wb-console-terminal.ts mirrors it"
        );
    }
}

/// The console's clipboard contract, pinned where deleting it would regress
/// in SILENCE.
///
/// Every assertion here guards a property no rendering test can see: that an
/// agent's OSC 52 is honoured at all, that it is honoured only ONCE and only
/// in the window that owns the clipboard, and that nothing in this file ever
/// reads the clipboard back. The feature itself is visible the moment you
/// copy; these are the invariants that are not.
#[test]
fn the_console_clipboard_is_write_only_and_refused_on_replay() {
    // The whole console family, as one text: the exact counts below hold
    // over every family module, so no clipboard read may enter any of them.
    let js = console_family();
    for pin in [
        // The gap that made an agent announce a copy it never made.
        "registerOscHandler(52",
        // The read form is refused, never answered.
        "payload === \"?\"",
        // The replay gate, and the write callback that is the ONLY correct
        // way to clear it — reverting to a bare `term.write(event.bytes)`
        // reintroduces the clipboard clobber with every other pin still green.
        "replaying = connOpts.id != null",
        // Whose clipboard it is: not every attached window's.
        "if (replaying || watching) return true;",
        // Ctrl+Insert, and NOT Ctrl+Shift+C (the DevTools accelerator).
        "e.key !== \"Insert\"",
    ] {
        assert!(
            js.contains(pin),
            "the console must keep the console-clipboard pin {pin}"
        );
    }
    // The write callback that clears the replay gate once the replayed bytes
    // are written. Whitespace is dropped so a reformat keeps it green; the
    // declaration `let replaying = false;` does not match it.
    let flat: String = js.split_whitespace().collect();
    assert!(
        flat.contains("term.write(event.bytes,replaying?()=>{replaying=false;}:undefined"),
        "the terminal must clear the replay gate in the replay's write callback"
    );
    // The read property. The clipboard is read only inside an operator's
    // gesture — the key bar's paste key and the right button under a TUI —
    // and never on a path an agent can trigger: the OSC 52 read form stays
    // refused above, and no other call site may appear. `readClipboard` wraps the API so the raw
    // `read`/`readText` have one occurrence each to count, plus the
    // feature-detect in `pasteOffered` (a `typeof`, not a call).
    for call in ["clip.read()", "clip.readText()", "navigator.clipboard.read"] {
        let want = usize::from(call.starts_with("clip."));
        assert_eq!(
            js.matches(call).count(),
            want,
            "the console reads the clipboard in readClipboard() only ({call})"
        );
    }
    assert_eq!(
        js.matches("readClipboard()").count(),
        3,
        "readClipboard() has one definition and two callers (the paste key, the right button)"
    );
    assert!(
        js.contains("name === \"paste\" ? readClipboard() : null"),
        "one clipboard read is the paste key's tap"
    );
    assert!(
        js.split_once("(rightTaken === \"paste\") {")
            .is_some_and(|(_, rest)| rest.trim_start().starts_with("readClipboard()")),
        "the other clipboard read is the right button's press"
    );
    // The OSC handler must not RETURN the clipboard promise: xterm's
    // `OscHandler.end` awaits a returned Promise and pauses the parser until
    // it settles, so a rejected write would stall the terminal.
    let handler = js
        .split_once("registerOscHandler(52")
        .expect("the OSC 52 handler")
        .1
        .split_once("\n  });")
        .expect("the end of the OSC 52 handler")
        .0;
    assert!(
        !handler.contains("return writeClipboard") && !handler.contains("return navigator"),
        "the OSC 52 handler must not return a promise — it stalls xterm's parser"
    );
    assert!(
        handler.contains("scrubClipboard"),
        "an agent's clipboard text is scrubbed before it reaches the operator"
    );
}

#[tokio::test]
async fn favicon_is_served_for_both_pages_and_the_bare_request() {
    // The browser asks for /favicon.ico on its own, whatever the markup says,
    // so the .ico must exist even though the SVG is what a modern browser
    // picks. A 404 here is the console error this replaced.
    let resp = get_local("/favicon.ico").await;
    assert_eq!(resp.status(), StatusCode::OK, "GET /favicon.ico → 200");
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/x-icon"
    );
    let resp = get_local("/favicon.svg").await;
    assert_eq!(resp.status(), StatusCode::OK, "GET /favicon.svg → 200");
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/svg+xml"
    );
    for page in ["/", "/popup", "/fence"] {
        let html = body_string(get_local(page).await).await;
        assert!(
            html.contains("favicon.svg") && html.contains("favicon.ico"),
            "{page} must link both favicon forms"
        );
    }
}

#[tokio::test]
async fn served_ui_copy_has_no_mock_or_false_claims() {
    for path in swept_ui_assets() {
        let path = path.as_str();
        let resp = get_local(path).await;
        assert_eq!(resp.status(), StatusCode::OK, "GET {path} → 200");
        let lc = body_string(resp).await.to_ascii_lowercase();
        assert!(!lc.contains("mock"), "{path} still contains \"mock\"");
        assert!(
            !lc.contains("nothing is written to disk"),
            "{path} still claims \"nothing is written to disk\""
        );
        assert!(
            !lc.contains("no secrets are stored"),
            "{path} still claims \"no secrets are stored\""
        );
        assert!(
            !lc.contains("any 6-digit code"),
            "{path} still claims \"any 6-digit code\""
        );
    }
}

/// The daemon ships and serves no fabricated data.
///
/// The seed of the old `file://` demo was compiled into every binary by
/// `include_dir!` and handed to any caller of `GET /`, which is
/// unauthenticated on purpose (the shell draws its own login gate). So an
/// unauthenticated stranger was served four fabricated `plan.md` documents
/// (#300). The demo is archived under the tag `workbench-demo-archive`
/// (ADR-0075 D8), and this pin keeps fabricated data out of the served tree.
#[tokio::test]
async fn the_served_ui_carries_no_seed() {
    // Identifiers, not prose: a comment may legitimately DISCUSS the seed
    // (several now do, explaining where it went), and a sweep that cannot
    // tell a mention from the data would make documenting the move fail.
    // The ASSIGNMENT, not the name: what must not come back is a served file
    // that DEFINES the data.
    const SEED: &[&str] = &[
        "window.WB_RUNS = {",
        "window.WB_KANBAN = {",
        "window.WB_SEED_PROJECTS =",
        "window.WB_SEED_ROSTER =",
        "id=\"seed-plan-",
        "function fakeContent(",
        "function fakeMarkdown(",
    ];
    for path in swept_ui_assets() {
        let body = body_string(get_local(&path).await).await;
        for pin in SEED {
            assert!(!body.contains(pin), "{path} still carries the seed {pin}");
        }
    }
    // The other half of the claim: the daemon has no bytes to serve under
    // some other name either. Stated over the TREE rather than as five
    // guessed 404s — "no embedded path is a seed file" is strictly stronger
    // than "these five particular names 404", and it cannot be defeated by
    // naming the sixth one something else.
    for path in embedded_ui_paths() {
        assert!(
            !path.contains("wb-seed-"),
            "{path} is embedded, and the served tree carries no seed (#300)"
        );
    }
    assert_eq!(
        get_local("/../ui-demo/wb-seed-runs.js").await.status(),
        StatusCode::NOT_FOUND,
        "the sibling seed directory must not be reachable by traversal"
    );
}

#[tokio::test]
async fn consoles_tab_is_fixed_and_named() {
    // The canvas tabs are the Alpine store `tabs` (#621).
    let body = body_string(get_local("/wb-tabs-store.js").await).await;
    assert!(
        !body.contains(r#"title: "Agents""#),
        "wb-tabs-store.ts must not carry the old tab title \"Agents\""
    );
    // The object literal that sets `id: "consoles"`, however it is wrapped.
    let code = squeeze(&body);
    let at = code
        .find(r#"id:"consoles""#)
        .unwrap_or_else(|| panic!("no object in wb-tabs-store.ts sets id: \"consoles\""));
    let open = code[..at]
        .rfind('{')
        .expect("the id sits in an object literal");
    let close = at + code[at..].find('}').expect("the object literal closes");
    let tab = &code[open..close];
    assert!(
        tab.contains(r#"title:"Consoles""#),
        "the object setting id: \"consoles\" must also set title: \"Consoles\"; got: {tab}"
    );
    assert!(
        tab.contains("closable:false"),
        "the object setting id: \"consoles\" must also set closable: false; got: {tab}"
    );
}
