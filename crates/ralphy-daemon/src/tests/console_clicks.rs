//! The clicks that ask first, the relaunch opt-in and the settings keys the CLI accepts.

use super::*;

/// The three clicks that ask first. Tiling a fence moves every console in
/// it, removing a fence takes the region out from under them, and a
/// console's × ends a live session — all one pixel from something harmless
/// on the same title bar. The dialog is built in `wb-console.ts` rather
/// than borrowed from the shell's Alpine one, because the module also runs
/// in the detached-fence popup, which has neither; `window.confirm` is
/// pinned OUT because an automated browser dismisses it by default, which
/// would turn every guarded click into a silently cancelled one.
#[test]
fn the_destructive_console_clicks_confirm_first() {
    // The whole console family and the door for operator messages, as one
    // text: the console's ×, the fence's tile and remove buttons, the remove
    // verb and the two restarts live in the family, and the dialog in the
    // door. The exact counts below hold over every family module.
    let js = &(console_family() + include_str!("../../assets/ui/wb-messages.ts"));
    assert!(
        js.contains("function askConfirm({"),
        "wb-messages.ts must own a confirmation dialog of its own"
    );
    assert!(
        !js.contains("window.confirm("),
        "the native confirm is not the seam — an automated browser dismisses it"
    );
    // Each call site acts on the answer: the statement right after the
    // `askConfirm({ … });` call is the one that reads `ok`. A site that
    // ignored the answer would still show the dialog.
    for (title, sites, then) in [
        (
            r#"title: "Tile this fence?""#,
            1,
            "if (ok) arrangeFence(f.id);",
        ),
        (
            r#"title: "Remove this fence?""#,
            1,
            "if (ok) removeFence(f.id);",
        ),
        ("title: `Restart in ${", 1, "if (!ok) return;"),
        (r#"title: "Restart session?""#, 1, "if (!ok) return;"),
        // The live console's ×, and the dormant one's.
        (r#"title: "Close this console?""#, 2, "if (!ok) return;"),
    ] {
        let found: Vec<&str> = js
            .match_indices(title)
            .map(|(at, _)| {
                assert!(
                    js[..at].trim_end().ends_with("askConfirm({"),
                    "{title} must be the first key of an askConfirm call"
                );
                let rest = &js[at..];
                let close = rest.find("});").expect("the askConfirm call must close");
                rest[close + 3..]
                    .trim_start()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
            })
            .collect();
        assert_eq!(
            found,
            vec![then; sites],
            "each {title} dialog must be followed by {then}"
        );
    }
    // The EXPORTED verbs stay unguarded: a caller that names `arrangeFence`
    // has already decided, and the dialog belongs to the accidental click.
    let verb = js
        .split_once("\n  function removeFence(id: string) {")
        .expect("wb-console.ts must keep removeFence")
        .1;
    assert!(
        !verb[..verb.find("\n  }").expect("removeFence must close")].contains("askConfirm"),
        "the verb must not ask — only the button does"
    );
    // The dialog wears the shell's own modal classes, so the popup (which
    // loads the same stylesheet and no Alpine) shows the same dialog.
    assert!(
        js.contains(r#"scrim.className = "modal-scrim wb-confirm""#),
        "the dialog must reuse the shared modal chrome"
    );
}

/// The standing authorization to relaunch agent consoles on load. It is a
/// spending decision — one vendor CLI per saved agent console, on every page
/// load — so what this pins is that it stays OFF unless the operator turned
/// it on, and that it stays in the BROWSER: daemon-wide, every tab pointed
/// at the same desk would relaunch the same consoles and spend the quota
/// once per tab. Playwright proves the fold (`wb_desk_303.py` scenario 2);
/// this is the gate CI can see.
#[test]
fn relaunching_agent_consoles_on_load_is_opt_in() {
    // Every pin reads whitespace-free text: the code, not its layout. The
    // restore is in the console's desk restore.
    let js = squeeze(include_str!("../../assets/ui/wb-desk-restore.ts"));
    let folds = squeeze(include_str!("../../assets/ui/wb-desk-folds.ts"));
    assert!(
        folds.contains(r#"record.kind==="console"||relaunchAgents?"relaunch":"placeholder""#),
        "the restore fold must relaunch an agent console ONLY under the opt-in"
    );
    // The DEFAULT is the whole guard: a caller that omits the option — the
    // fold's own tests, a later call site — must get the parked placeholder.
    assert!(
        folds.contains("relaunchAgents=false,}"),
        "an omitted `relaunchAgents` must default to false, never to launching"
    );
    // The relaunch verdict must ask for the record's OWN kind. `{ console:
    // true }` is the shell request, and reaching an agent record with it —
    // which the opt-in made possible — opened a plain shell in the agent's
    // box (measured in `wb_desk_303.py` scenario 10 before this line).
    // Since #411 the request is `relaunchRequest`'s, which also carries
    // the worktree the record was in; the fold is pinned by
    // `ui-tests/wb-console-session.test.mjs`, and what stays here is that the
    // restore path goes through it and nothing else.
    let session = squeeze(include_str!("../../assets/ui/wb-console-session.ts"));
    assert!(
        session.contains(
            r#"if(record.kind!=="agent")return{console:true,repo,command:consoleCommand(record.agent)};"#
        ),
        "a relaunched agent console must be requested by its vendor, not as a shell"
    );
    assert!(
        js.contains("spawnOrMissing(relaunchRequest(record),record.agent,record.repo,record)"),
        "the restore fold's relaunch must go through relaunchRequest"
    );
    // The popup holds a fragment of the plane and authors no session; its
    // injected `viewStore` reads nothing, and `canLaunch` refuses besides.
    assert!(
        js.contains("OPTS.canLaunch!==false&&viewStore?.read()?.relaunch===true"),
        "the opt-in must be read through the injected view store, and never in the popup"
    );

    // The knob, and the store it writes to. `config.set` would put a
    // per-browser choice in a repo's settings.json for every client to obey.
    let settings = squeeze(include_str!("../../assets/ui/wb-settings.ts"));
    for pin in [
        r#"scope:"client""#,
        r#"key:"consoles.relaunch_on_load""#,
        r#"type:"toggle""#,
    ] {
        assert!(
            settings.contains(pin),
            "wb-settings.ts must keep the pin {pin}"
        );
    }
    let app = squeeze(include_str!("../../assets/ui/wb-settings-dialog.ts"));
    assert!(
        app.contains("window.WBView.patch({relaunch:value===true})"),
        "wb-settings-dialog.ts must persist the toggle through the view store"
    );
    assert!(
        app.contains("if(this.CLIENT_KEYS.has(key)){"),
        "a client-scoped key must return before the config.set path"
    );
    let html = squeeze(super::INDEX_HTML);
    assert!(
        html.contains(r#"it.type==='toggle'"#),
        "index.html must render the toggle control"
    );
}

/// A project-scoped key in the settings schema is an INTENT ON THE CLI: the
/// panel persists it with `config.set`, the daemon relays that to
/// `ralphy config set` verbatim (ADR-0036 — the daemon shape-checks the key
/// and keeps no value allowlist of its own), and the CLI refuses anything
/// outside `SUPPORTED_KEYS`. So a key the UI offers and the CLI does not
/// know is a control that cannot be operated: it renders, it accepts a
/// click, and the save comes back refused.
///
/// Nothing else notices. The daemon may not depend on `ralphy-cli` (the
/// arrow points inward), so the two lists are joined the only way that seam
/// allows — by reading the CLI's source as text, the precedent
/// `session.rs` sets for the adapters' settings schemas.
///
/// This gate found three such controls when it was written: a Schedule
/// section backed by the `ralphy schedule` SUBCOMMAND rather than by any
/// persisted key, and an "Eligible labels" field backed by nothing at all —
/// no `SUPPORTED_KEYS` entry, no field in `ralphy-core`'s `Settings`. All
/// three are gone from the schema; this is what keeps the fourth out.
#[test]
fn every_settable_key_the_panel_offers_is_a_key_the_cli_accepts() {
    let schema = include_str!("../../assets/ui/wb-settings.ts");
    let cli = include_str!("../../../ralphy-cli/src/config.rs");
    let supported = cli
        .split_once("const SUPPORTED_KEYS: &[&str] = &[")
        .expect("the CLI renamed the key registry the panel is written against")
        .1
        .split_once("];")
        .expect("unterminated SUPPORTED_KEYS")
        .0;

    // A section's `scope` precedes its `items`, so one linear pass over the
    // two literals attributes every key to the section it was declared in,
    // and each item's own text carries its `readonly` flag.
    let mut checked = 0;
    for (i, _) in schema.match_indices("scope: \"") {
        let rest = &schema[i + "scope: \"".len()..];
        let scope = &rest[..rest.find('"').expect("unterminated scope")];
        // Everything up to the NEXT section's scope belongs to this one.
        let section = match rest.find("scope: \"") {
            Some(end) => &rest[..end],
            None => rest,
        };
        if scope != "project" {
            continue;
        }
        for (j, _) in section.match_indices("key: \"") {
            let tail = &section[j + "key: \"".len()..];
            let key = &tail[..tail.find('"').expect("unterminated key")];
            // The item runs to the next key, or to the end of the section.
            let item = match tail.find("key: \"") {
                Some(end) => &tail[..end],
                None => tail,
            };
            assert!(
                supported.contains(&format!("\"{key}\"")),
                "the panel offers {key} but `ralphy config set` refuses it — \
                 a control that renders, takes an edit and answers 'refused'. \
                 Add the key to SUPPORTED_KEYS, or take the item out of the schema."
            );
            // The other half: a key the CLI knows but the DAEMON denies is
            // just as inert from a browser, and the schema is where that has
            // to be admitted.
            assert_eq!(
                dispatch::LOCAL_ONLY_KEYS.contains(&key),
                item.contains("readonly: true"),
                "{key}: a key denied at the daemon boundary must be declared \
                 `readonly: true`, and only such a key may be"
            );
            checked += 1;
        }
    }
    // The daemon-scope sections carry keys the CLI never sees (`daemon.*`,
    // `telegram.*`), so they are not cross-checked above — but a LOCAL-ONLY
    // key offered there (`events.token`, audit F12) must still be declared
    // `readonly`, wherever it sits.
    for key in dispatch::LOCAL_ONLY_KEYS {
        let Some(at) = schema.find(&format!("key: \"{key}\"")) else {
            continue;
        };
        let tail = &schema[at..];
        let item = match tail[1..].find("key: \"") {
            Some(end) => &tail[..end],
            None => tail,
        };
        assert!(
            item.contains("readonly: true"),
            "{key} is denied at the daemon boundary but the panel offers it editable"
        );
    }
    // The declaration is worth nothing if the markup ignores it: an
    // `it.readonly` the input never reads is a field that still takes an
    // edit and still comes back refused.
    // One binding per control a readonly key can use: text, password, toggle.
    assert_eq!(
        super::INDEX_HTML
            .matches(r#":disabled="it.readonly === true""#)
            .count(),
        3,
        "index.html must disable the text, password and toggle control a readonly item declares"
    );
    // Non-vacuous: a scan that stopped recognizing the schema's shape would
    // otherwise pass by checking nothing at all.
    assert!(
        checked > 10,
        "only {checked} project keys were cross-checked — the scan stopped seeing the schema"
    );
}
