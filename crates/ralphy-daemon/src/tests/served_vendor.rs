//! The vendored terminal and editor libraries the daemon serves.

use super::*;

#[tokio::test]
async fn root_serves_vendored_xterm() {
    let resp = get_local("/vendor/xterm.js").await;
    assert_eq!(resp.status(), StatusCode::OK, "GET vendor/xterm.js → 200");
    assert_eq!(
        resp.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    let resp = get_local("/vendor/xterm.css").await;
    assert_eq!(resp.status(), StatusCode::OK, "GET vendor/xterm.css → 200");
    assert_eq!(
        resp.headers()[header::CONTENT_TYPE],
        "text/css; charset=utf-8"
    );

    let shell = body_string(get_local("/").await).await;
    assert!(
        shell.contains("vendor/xterm.js"),
        "the shell HTML must load the vendored xterm"
    );

    let terminal = body_string(get_local("/wb-console-terminal.js").await).await;
    assert!(
        terminal.contains("new Terminal("),
        "wb-console-terminal.js must construct a real xterm terminal"
    );
    assert!(
        terminal.contains("/ws/session"),
        "wb-console-terminal.js must open the session WebSocket"
    );
}

/// `showScrollbarWhileNeeded` (wb-console-terminal.ts) reaches the scrollbar
/// through xterm's private fields. An xterm that renames them would bring
/// back the bar that hides by itself, so every link of the path is pinned
/// against the vendored file, each with the class it is set in, and each must
/// be the only assignment of its field.
#[test]
fn the_console_scrollbar_path_matches_the_vendored_xterm() {
    let xterm = include_str!("../../assets/ui/vendor/xterm.js");
    for link in [
        "this._core=this._register(new r.CoreBrowserTerminal(e))",
        "this._viewport=this._register(this._instantiationService.createInstance(o.Viewport,",
        "this._scrollableElement=this._register(new h.SmoothScrollableElement(t,{vertical:1,",
        "void 0!==e.vertical&&(this._options.vertical=e.vertical)",
        "this._horizontalScrollbar.updateOptions(this._options),this._verticalScrollbar.updateOptions(this._options)",
        "this._visibilityController.setVisibility(e.vertical)",
        "e[e.Visible=3]=\"Visible\"",
    ] {
        assert!(
            xterm.contains(link),
            "the vendored xterm no longer has `{link}`"
        );
    }
    for field in ["this._viewport=", "this._scrollableElement="] {
        assert_eq!(
            xterm.matches(field).count(),
            1,
            "`{field}` must be set in one place, the one pinned above"
        );
    }
    // After `open`, which builds the viewport, and before the next addon.
    let terminal = include_str!("../../assets/ui/wb-console-terminal.ts");
    let after_open = terminal
        .split_once("term.open(body);")
        .expect("the console terminal must call `open`")
        .1;
    let call = after_open.find("showScrollbarWhileNeeded(term);");
    let next_addon = after_open.find("term.loadAddon(");
    assert!(
        matches!((call, next_addon), (Some(c), Some(a)) if c < a),
        "the scrollbar setting must run after `open` and before the next addon"
    );
}

/// The recipe is read with `include_str!` from OUTSIDE `assets/ui/`: it must
/// not be embedded (`include_dir!` would serve `node_modules/` to the
/// browser), and this is also the assertion that it exists.
#[test]
fn vendored_crepe_states_its_recipe() {
    let recipe = include_str!("../../vendor-build/crepe/package.json");
    let version = |name: &str| -> String {
        let after = recipe
            .split_once(&format!("\"{name}\": \""))
            .unwrap_or_else(|| panic!("the recipe must pin {name}"))
            .1;
        after[..after.find('"').expect("a closing quote")].to_string()
    };
    let build = include_str!("../../vendor-build/crepe/build.mjs");
    let features: Vec<&str> = build
        .split_once("const FEATURES = [")
        .expect("build.mjs must list the features")
        .1
        .split_once("].join")
        .expect("the feature list must close")
        .0
        .split('\'')
        .filter(|s| !s.trim().is_empty() && !s.contains(','))
        .collect();
    let header = format!(
        "/* crepe {} · esbuild {} · features: {} */",
        version("@milkdown/crepe"),
        version("esbuild"),
        features.join(",")
    );

    for artefact in ["vendor/crepe/crepe.js", "vendor/crepe/crepe.css"] {
        let src = UI
            .get_file(artefact)
            .and_then(|f| f.contents_utf8())
            .unwrap_or_else(|| panic!("{artefact} must be embedded as UTF-8"));
        assert_eq!(
            src.lines().next().unwrap_or_default(),
            header,
            "{artefact}'s header must name the recipe that built it — \
             rerun `node build.mjs` in vendor-build/crepe"
        );
    }
    // The mermaid node view is OURS and rides in the same bundle (ADR-0064
    // §15), so it is named in the header for the same reason the features
    // are — and both shells must carry the two globals it reads.
    assert!(
        features.contains(&"mermaid-view"),
        "the bundle carries the mermaid node view (ADR-0064 §15)"
    );
    // Also ours: "Add title" on an empty line (ADR-0064, 2026-09-27
    // amendment). Named in the header, and its class is in the artefact.
    assert!(
        features.contains(&"title-toggle"),
        "the bundle carries the empty line's title control (ADR-0064)"
    );
    assert!(
        UI.get_file("vendor/crepe/crepe.js")
            .and_then(|f| f.contents_utf8())
            .is_some_and(|src| src.contains("note-title-toggle")),
        "crepe.js must carry the title control it advertises"
    );
    // The header names it; this proves it is actually IN the artefact —
    // a class only our node view emits, so a stale rebuild reds here
    // rather than shipping a header that promises a view the bundle lost.
    assert!(
        UI.get_file("vendor/crepe/crepe.js")
            .and_then(|f| f.contents_utf8())
            .is_some_and(|src| src.contains("note-mermaid-figure")),
        "crepe.js must carry the mermaid node view it advertises"
    );
    // `htmlLabels: false` in BOTH renderers, and it is not a style: with
    // mermaid's default HTML labels every label is removed on the way in,
    // because DOMPurify 3.4 dropped `foreignObject` from its SVG
    // allowlist — the diagram arrives as unlabelled boxes (measured).
    for (what, src) in [
        (
            "vendor-build/crepe/entry.js",
            include_str!("../../vendor-build/crepe/entry.js"),
        ),
        (
            "wb-dbml-draw.ts",
            include_str!("../../assets/ui/wb-dbml-draw.ts"),
        ),
    ] {
        assert!(
            src.contains("htmlLabels: false"),
            "{what} must keep mermaid's labels as plain SVG text"
        );
    }
    for shell in ["index.html", "detached-fence.html"] {
        let html = UI
            .get_file(shell)
            .and_then(|f| f.contents_utf8())
            .expect("the shell must be embedded");
        for tag in ["vendor/mermaid.min.js", "vendor/dompurify.min.js"] {
            assert!(
                html.contains(tag),
                "{shell} must load {tag} — the note's mermaid fence reads it"
            );
        }
    }
    // The feature list is the lean bundle's whole argument: CodeMirror is a
    // SECOND editor engine beside Monaco (#308) and LaTeX drags KaTeX in.
    // Neither may return without a decision.
    for absent in ["code-mirror", "latex", "image-block", "top-bar", "ai"] {
        assert!(
            !features.contains(&absent),
            "{absent} is deliberately not in the lean bundle (ADR-0064 §6)"
        );
    }
    // The licence travels with the code.
    assert!(
        UI.get_file("vendor/crepe/LICENSE").is_some(),
        "the vendored bundle must carry its licence"
    );
}

/// The editor is served, and both shells that can hold a card load it.
#[tokio::test]
async fn root_serves_the_vendored_crepe() {
    let resp = get_local("/vendor/crepe/crepe.js").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    let resp = get_local("/vendor/crepe/crepe.css").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[header::CONTENT_TYPE],
        "text/css; charset=utf-8"
    );
    for shell in ["index.html", "detached-fence.html"] {
        let html = UI
            .get_file(shell)
            .and_then(|f| f.contents_utf8())
            .expect("the shell must be embedded");
        assert!(
            html.contains("vendor/crepe/crepe.js") && html.contains("vendor/crepe/crepe.css"),
            "{shell} must load the note editor (ADR-0064 §6)"
        );
    }
}
