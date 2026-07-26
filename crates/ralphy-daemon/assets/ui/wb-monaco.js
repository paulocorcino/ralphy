/* ---------------------------------------------------------------------------
   ralphy workbench shell — the Monaco boot seam (#308)

   Monaco is the workbench's ONE editor engine. It boots through its own AMD
   loader, so creating an editor is asynchronous: every caller goes through
   `WBMonaco.ready()` (one memoised boot for the whole page) and only then
   `WBMonaco.create()`.

   Load order is load-bearing: `vendor/monaco/vs/loader.js` installs a global
   `define` with `define.amd`, which every UMD vendor on the page (marked,
   DOMPurify, mermaid, Wunderbaum, xterm + addons) would latch onto instead of
   exporting its global. The loader tag therefore comes AFTER all of them.
--------------------------------------------------------------------------- */
(function () {
  // ADR-0035's warm-dark palette, as literal hex — Monaco's theme API takes no
  // CSS variables, so these mirror :root in styles.css (the ground `--log-bg`
  // #2a2521 is what the browser acceptance asserts). Keep them in lockstep with
  // the stylesheet: a drift here shows up as an editor that is a different
  // shade from the pane it sits in.
  const TOKENS = {
    logBg: "#2a2521",
    surface: "#342d27",
    surfaceHi: "#423a31",
    border: "#4c4239",
    text: "#d4ccc0",
    textMuted: "#a49c91",
    consoleText: "#e8d9a8",
  };

  function defineTheme(monaco) {
    monaco.editor.defineTheme("wb", {
      base: "vs-dark",
      inherit: true,
      colors: {
        "editor.background": TOKENS.logBg,
        "editor.foreground": TOKENS.text,
        "editorGutter.background": TOKENS.logBg,
        "editorLineNumber.foreground": TOKENS.textMuted,
        "editorCursor.foreground": TOKENS.text,
        "editor.selectionBackground": TOKENS.surfaceHi,
        "editor.lineHighlightBackground": "#332d28",
        "editorWidget.background": TOKENS.surface,
        "editorWidget.border": TOKENS.border,
        "input.background": TOKENS.logBg,
        "input.foreground": TOKENS.text,
        "input.border": TOKENS.border,
        "editor.findMatchBackground": "#4a3f22",
        "editor.findMatchHighlightBackground": "#3a3016",
        "editorBracketMatch.background": "#2b2410",
        "editorBracketMatch.border": "#5a4a1e",
      },
      rules: [
        { token: "comment", foreground: "a49c91", fontStyle: "italic" },
        { token: "keyword", foreground: "c98a7d" },
        { token: "type", foreground: "cba6c3" },
        { token: "type.identifier", foreground: "cba6c3" },
        { token: "number", foreground: "d8a76a" },
        { token: "string", foreground: "9fb98a" },
        { token: "variable", foreground: "9db7c9" },
        { token: "attribute.name", foreground: "e8d9a8" },
        { token: "attribute.value", foreground: "9fb98a" },
        { token: "tag", foreground: "c98a7d" },
        { token: "delimiter", foreground: "a49c91" },
        { token: "operator", foreground: "a49c91" },
        { token: "metatag", foreground: "a49c91" },
      ],
    });
  }

  // The four LSP contributions ship with `editor.main`, but their heavy workers
  // are deliberately NOT vendored — leaving a provider on means a lazily-loaded
  // mode chunk asking for a worker that 404s. Language *services* are out of
  // scope (PRD #297); Monarch highlighting needs none of this.
  // NOTE `tokens` is deliberately absent. It is the ONLY provider here that is
  // not a worker client: `jsonMode` gates `setTokensProvider` on it, and JSON is
  // the one language with no `basic-languages` Monarch grammar to fall back on,
  // so switching it off would leave every .json file unstyled for nothing.
  const NO_PROVIDERS = {
    documentFormattingEdits: false,
    documentRangeFormattingEdits: false,
    completionItems: false,
    hovers: false,
    documentSymbols: false,
    colors: false,
    foldingRanges: false,
    diagnostics: false,
    selectionRanges: false,
  };

  function disableLanguageServices(monaco) {
    const langs = monaco.languages;
    langs.json?.jsonDefaults?.setModeConfiguration(NO_PROVIDERS);
    langs.json?.jsonDefaults?.setDiagnosticsOptions({ validate: false, schemaValidation: "ignore" });
    langs.css?.cssDefaults?.setModeConfiguration(NO_PROVIDERS);
    langs.css?.cssDefaults?.setOptions({ validate: false });
    langs.html?.htmlDefaults?.setModeConfiguration(NO_PROVIDERS);
    for (const d of [langs.typescript?.typescriptDefaults, langs.typescript?.javascriptDefaults]) {
      d?.setModeConfiguration(NO_PROVIDERS);
      d?.setDiagnosticsOptions({ noSemanticValidation: true, noSyntaxValidation: true, noSuggestionDiagnostics: true });
    }
  }

  let booting = null;

  // One boot for the whole page. Resolves with the global `monaco`.
  function ready() {
    if (booting) return booting;
    booting = new Promise((resolve, reject) => {
      if (typeof window.require !== "function" || !window.require.config) {
        reject(new Error("monaco AMD loader is not on the page"));
        return;
      }
      window.require.config({ paths: { vs: "vendor/monaco/vs" } });
      window.require(
        ["vs/editor/editor.main"],
        () => {
          const monaco = window.monaco;
          defineTheme(monaco);
          // `.toml` is the one extension the workbench needs that Monaco does
          // not resolve natively; `ini`'s grammar covers [section] / key = value.
          monaco.languages.register({ id: "ini", extensions: [".toml"] });
          disableLanguageServices(monaco);
          resolve(monaco);
        },
        reject,
      );
    });
    return booting;
  }

  // A model URI must be unique per open pane: `uid` is the viewer's monotonic
  // sequence, so reopening a closed tab (or the same path in the markdown raw
  // editor) cannot hit Monaco's "two models with the same URI" throw.
  function create(container, { value, path, uid, project, wordWrap }) {
    const monaco = window.monaco;
    const uri = monaco.Uri.file("/" + uid + "/" + project + "/" + path);
    return monaco.editor.create(container, {
      model: monaco.editor.createModel(value, undefined, uri),
      theme: "wb",
      // Monaco sizes itself from inline dimensions, so it does NOT reflow from
      // CSS the way CodeMirror 5 did: without this its own ResizeObserver, a
      // sidebar collapse or a window resize leaves the pane clipped and the
      // mouse-to-text mapping offset until the next tab switch — and the
      // detached popup, which has no tab bar, would never relayout at all.
      automaticLayout: true,
      readOnly: false,
      minimap: { enabled: false },
      wordBasedSuggestions: "off",
      quickSuggestions: false,
      wordWrap: wordWrap || "off",
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      fontSize: 13,
      lineNumbers: "on",
      renderLineHighlight: "line",
      matchBrackets: "always",
      scrollBeyondLastLine: false,
    });
  }

  // A side-by-side diff pane: HEAD on the left, the working tree on the right.
  // Monaco computes the diff itself from the two texts, so nothing on the Rust
  // side produces a patch. The two model URIs must DIFFER (Monaco throws on a
  // duplicate), hence the `head`/`work` segment — and `uid` still separates a
  // reopened tab from the closed one whose models are being torn down.
  function createDiff(container, { original, modified, path, uid, project }) {
    const monaco = window.monaco;
    const at = (side) => monaco.Uri.file("/" + uid + "/" + side + "/" + project + "/" + path);
    const ed = monaco.editor.createDiffEditor(container, {
      theme: "wb",
      // See create(): without Monaco's own ResizeObserver the panes stay clipped
      // through a sidebar collapse or a window resize.
      automaticLayout: true,
      readOnly: true,
      originalEditable: false,
      renderSideBySide: true,
      // `renderSideBySide: true` alone does NOT hold: Monaco's own default
      // `useInlineViewWhenSpaceIsLimited` silently swaps to the inline view under
      // `renderSideBySideInlineBreakpoint` (900px), so a narrow window would quietly
      // stop being the two-sided review this surface promises.
      useInlineViewWhenSpaceIsLimited: false,
      // The margin revert arrow WRITES to the modified side — read-only already
      // suppresses it, but this surface forbids the control outright.
      renderMarginRevert: false,
      // Unchanged regions collapse to a click-to-expand ruler: reviewing what the
      // agent wrote must not mean scrolling past what it left alone.
      hideUnchangedRegions: { enabled: true },
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      fontSize: 13,
      lineNumbers: "on",
    });
    // Built into locals, not inline in the setModel argument: a throw from the
    // SECOND createModel would otherwise leave the editor and the first model
    // unreachable (the caller assigns `rec.ed` only after this returns, so
    // disposeEditor could never see them).
    let head;
    let work;
    try {
      head = monaco.editor.createModel(original, undefined, at("head"));
      work = monaco.editor.createModel(modified, undefined, at("work"));
    } catch (err) {
      head?.dispose();
      work?.dispose();
      ed.dispose();
      throw err;
    }
    ed.setModel({ original: head, modified: work });
    return ed;
  }

  window.WBMonaco = { ready, create, createDiff, TOKENS };
})();
