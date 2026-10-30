//! The records a console and a note card write: stacking, desk changes, the console name and the session identity.

use super::*;

/// A note card is a surface on the WINDOW tier and wears the plane's own
/// chrome (ADR-0064 §8, amendment 2026-09-22). Both halves were found by
/// the operator on the first plane that had a note on it, and neither is
/// visible to a fold test: a card without a `z-index` is drawn UNDER every
/// console, so a click on its body lands on a terminal's canvas and the
/// keystrokes go to the shell — a note that cannot be typed into.
#[test]
fn a_note_card_is_stacked_and_wears_the_console_chrome() {
    // Normalized: the pin below spans line ends, and a Windows checkout
    // (CI's included) embeds the asset with CRLF.
    let console = include_str!("../../assets/ui/wb-console-api.ts").replace("\r\n", "\n");
    let stack = include_str!("../../assets/ui/wb-stage-stack.ts");
    // The card is built by two modules: `wb-notes.ts` and its chrome.
    let card = [
        include_str!("../../assets/ui/wb-notes.ts"),
        include_str!("../../assets/ui/wb-notes-chrome.ts"),
    ]
    .concat();
    let notes = card.as_str();
    let veil = include_str!("../../assets/ui/wb-notes-veil.ts");
    let menus = include_str!("../../assets/ui/wb-notes-menus.ts");
    // The seam: a place in the tier WITHOUT focus, because a restore
    // focuses nothing and `focusWin` is the only other way to get one.
    assert!(
        stack.contains("function stackWin(")
            && console.contains("\n    stackWin: stack.stackWin,\n"),
        "wb-stage-stack.ts must declare stackWin and wb-console-api.ts must export it, \
         the tier a restored surface enters by"
    );
    assert!(
        notes.contains("stack?.stackWin(el)"),
        "a card must enter the window tier when it is built (ADR-0064 §8 amendment)"
    );
    // One titlebar vocabulary on the plane: the card's controls are the
    // console's icons, and the console's own two lock glyphs verbatim.
    for pin in [
        r#"<i class="bi bi-grip-vertical"></i>"#,
        r#"<i class="bi bi-palette"></i>"#,
        r#"<i class="bi bi-gear"></i>"#,
        r#"<i class="bi bi-x-lg"></i>"#,
        r#"'<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>'"#,
    ] {
        assert!(
            notes.contains(pin),
            "the card's chrome must be the console's icon {pin}"
        );
    }
    // NEGATIVE CONTROL: the text glyphs this replaced, as the ASSIGNMENT
    // that drew them — the characters themselves still appear in prose
    // naming the controls, and a card drawing its own is the defect, not
    // the word for it.
    for glyph in [
        '\u{28ff}',
        '\u{25d1}',
        '\u{22ef}',
        '\u{1f512}',
        '\u{1f513}',
        '\u{d7}',
    ] {
        let drawn = format!("textContent = \"{glyph}\"");
        assert!(
            !notes.contains(&drawn),
            "the card must not draw the text glyph {glyph:?} for a control"
        );
    }
    // THE GEAR IS THE FOOTER'S, beside the path it acts on (ADR-0064,
    // amendment 2026-09-22). Nothing in its menu is about the card, and in
    // the head it put `Delete file…` two pixels from the close button. The
    // head's cluster is pinned WITHOUT it, which is the half that would
    // rot first — a control put back there would read as one of the
    // card's own.
    assert!(
        notes.contains("foot.append(more, path, dir, rename, state)"),
        "the file gear belongs in the footer, beside the path it acts on"
    );
    assert!(
        notes.contains("tools.append(tone, index, veil, lock, close, back)"),
        "the head's cluster is the CARD's controls; the file's is not among them"
    );
    // `Hide this note` is gone (the operator's call, 2026-09-22): the eye
    // hides any card, so the entry was a second door to one place. The
    // UNMARK is not a duplicate of anything — the eye can mark and reveal
    // but never unmark — so it stays, shown only in the state it undoes.
    assert!(
        [notes, veil]
            .iter()
            .map(|f| f.matches(r#""Hide this note""#).count())
            .sum::<usize>()
            == 1
            && veil.contains(r#"btn.title = !marked ? "Hide this note""#),
        "the eye is how a note is hidden; the menu must not offer it twice"
    );
    assert!(
        notes.contains(r#"mark.textContent = "Stop hiding this note""#)
            && menus.contains("mark.hidden = !veiledOf(el._noteMarkdown)"),
        "the one door OUT of the veil must stay, and show only on a veiled note"
    );
    // A veiled card holds only its header, so unmarking straight from it
    // would write that header over the note. The file is read back first.
    assert!(
        veil.contains("if (!marked && veiledNow(el))"),
        "unmarking a veiled card must re-read the file, not write its bare header"
    );
    // The title is `flex: 1` and therefore most of the HEAD, which is the
    // drag handle. Opening the rename on the way DOWN (and stopping the
    // press so it cannot arm a drag) left the card movable only by its
    // grip — measured. The decision is taken on the way up, against the
    // same 3 px the gesture calls a drag, and nothing is stopped.
    assert!(
        !notes.contains("ev.stopPropagation();\n      beginTitle(el)"),
        "the title must not swallow the head's press — the head is the drag handle"
    );
    assert!(
        notes.contains(r#"title.addEventListener("pointerup""#)
            && notes.contains("Math.abs(ev.clientX - from.x) > 3"),
        "the rename must open on a press that did not move (ADR-0064 §8 amendment)"
    );
    // The look's closed sets and the omitted defaults are driven by
    // `ui-tests/wb-notes-folds.test.mjs` (`withStyle`).
}

/// A page writes the desk only as desk changes, each with the fields its act
/// changed (ADR-0050 amendment 2026-10-04, changes, not the desk). The old
/// writer built a whole record from the screen with a fresh clock, and seven of
/// its eleven callers were not about the rect. These pins keep the shape out:
/// no whole-record writer, no whole-desk body, no client clock in a record, and
/// one place that builds the body.
#[test]
fn the_shell_writes_the_desk_only_as_changes() {
    let console = include_str!("../../assets/ui/wb-console.ts");
    let desk = include_str!("../../assets/ui/wb-desk.ts");
    let records = include_str!("../../assets/ui/wb-desk-records.ts");
    let notes = include_str!("../../assets/ui/wb-notes.ts");
    let folds = include_str!("../../assets/ui/wb-notes-folds.ts");
    let sync = include_str!("../../assets/ui/wb-desk-sync.ts");
    // The whole console family, and the other modules that write the desk.
    let others = [
        ("wb-notes.ts", notes),
        ("wb-notes-folds.ts", folds),
        (
            "wb-notes-chrome.ts",
            include_str!("../../assets/ui/wb-notes-chrome.ts"),
        ),
        (
            "wb-notes-stage.ts",
            include_str!("../../assets/ui/wb-notes-stage.ts"),
        ),
        (
            "wb-notes-editor.ts",
            include_str!("../../assets/ui/wb-notes-editor.ts"),
        ),
        (
            "wb-notes-on-top.ts",
            include_str!("../../assets/ui/wb-notes-on-top.ts"),
        ),
        (
            "wb-notes-veil.ts",
            include_str!("../../assets/ui/wb-notes-veil.ts"),
        ),
        (
            "wb-notes-map.ts",
            include_str!("../../assets/ui/wb-notes-map.ts"),
        ),
        (
            "wb-notes-dormancy.ts",
            include_str!("../../assets/ui/wb-notes-dormancy.ts"),
        ),
        (
            "wb-notes-file.ts",
            include_str!("../../assets/ui/wb-notes-file.ts"),
        ),
        (
            "wb-notes-open.ts",
            include_str!("../../assets/ui/wb-notes-open.ts"),
        ),
        (
            "wb-notes-menus.ts",
            include_str!("../../assets/ui/wb-notes-menus.ts"),
        ),
        (
            "wb-notes-palette.ts",
            include_str!("../../assets/ui/wb-notes-palette.ts"),
        ),
        (
            "wb-messages.ts",
            include_str!("../../assets/ui/wb-messages.ts"),
        ),
    ];
    for (name, js) in [CONSOLE_FAMILY, &others].concat() {
        for banned in [
            "persistWin(",
            "deskBody(",
            "mergeDesk(",
            "removed: {",
            "ts: Date.now() }",
        ] {
            assert!(
                !js.contains(banned),
                "{name} must not contain `{banned}`: a desk write carries only the changed fields"
            );
        }
    }
    // Every window write names its fields, and the body is built in one place.
    let squeezed = squeeze(records);
    assert!(
        squeezed.contains(r#"emitDesk({op:"set",type:"window",id:win._deskId,fields});"#),
        "setWin must send the fields it was handed, and nothing else"
    );
    // The family holds the sync module itself, which builds the body.
    let outside_sync: String = CONSOLE_FAMILY
        .iter()
        .filter(|(name, _)| *name != "wb-desk-sync.ts")
        .map(|(_, text)| *text)
        .collect();
    assert!(
        !outside_sync.contains("changes:")
            && !console.contains("changes:")
            && !desk.contains("changes:")
            && !records.contains("changes:")
            && sync.contains("changes: pending.slice(0, inflight.count)"),
        "the upload body is built by wb-desk-sync.ts alone"
    );
}

/// ADR-0066 Consequences: the shell builds a record field by field in more than
/// one place, and a copy that misses `consoleName` drops the name — on a
/// create (`recordOf`), on a restart or a worktree switch (`deskOf`, the carry
/// of `relaunchIn`), or at birth (`buildChrome`, through the window inventory).
#[test]
fn console_name_rides_every_record_copy() {
    // The whole console family, as one text: the record copies and the birth
    // live in its modules.
    let js = &console_family();
    let body = |name: &str| -> String {
        let after = js
            .split_once(name)
            .unwrap_or_else(|| panic!("the console must keep {name}"))
            .1;
        after[..after.find("\n  }").expect("the function must close")].to_string()
    };
    for copy in [
        "function recordOf(",
        "function deskOf(",
        "function buildChrome(",
    ] {
        assert!(
            body(copy).contains("consoleName"),
            "{copy} must carry the console name (#479)"
        );
    }
    assert!(
        include_str!("../../assets/ui/wb-window-state.ts").contains("_deskConsoleName:"),
        "the window inventory must declare the console name (#479)"
    );
}

/// Every agent launch path — a new console, a restart, a worktree restart, a
/// relaunch after a daemon restart — reaches `spawnWindow`, so that is where
/// the console name joins the launch. Neither the Playwright suite nor the
/// node table covers the wiring in CI, so a deletion fails HERE (#480).
#[test]
fn spawn_window_sends_the_console_name_on_a_new_agent_launch() {
    let js = console_family();
    let after = js
        .split_once("function spawnWindow(")
        .expect("the console family must keep spawnWindow")
        .1;
    let body = squeeze(&after[..after.find("\n  }").expect("the function must close")]);
    // The guard of the statement that adds the name, whatever its layout
    // and whatever the order of its two conditions.
    let set = body
        .find("termOpts={...termOpts,name:win._deskConsoleName}")
        .expect("spawnWindow must send the console name (#480)");
    let guard = &body[body[..set]
        .rfind("if(")
        .expect("the name is sent under a guard")..set];
    for condition in ["termOpts.id==null", "!termOpts.console"] {
        assert!(
            guard.contains(condition),
            "spawnWindow must send the console name with a new agent launch only (#480): {guard}"
        );
    }
}

/// Peer session ownership must survive every browser reconnect and close
/// path; exact repo equality keeps a local slug from lighting a peer row.
/// The route's folds are driven by `ui-tests/wb-session-route.test.mjs`, and
/// the session name's tooltip by `sessionPresentation` in
/// `wb-console.test.mjs`; this test holds the call sites that use them.
#[test]
fn workbench_session_assets_preserve_composite_repo_identity() {
    // The whole console family: the terminal reconnects and reads the
    // announcement, the console closes.
    let console = console_family();
    // The close is pinned in the module that closes a console: the worktree
    // switcher also calls `closeUrl`, so the family text alone would hide a
    // lost close.
    assert!(
        include_str!("../../assets/ui/wb-console-spawn.ts")
            .contains("fetch(WBSessionRoute.closeUrl(id, win._deskRepo)"),
        "the console close must keep its closeUrl call in wb-console-spawn.ts"
    );
    for pin in [
        r#"connect({ id: currentSessionId, repo: currentRepo"#,
        r#"c.verb === "session-open""#,
        "WBSessionRoute.url(",
        "WBSessionRoute.announcement(",
    ] {
        assert!(console.contains(pin), "the console must keep {pin}");
    }
    assert!(shell_family().contains("WBSessionRoute.matchesRepo("));
}
