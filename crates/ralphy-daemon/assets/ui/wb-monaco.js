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
  // #1a1613 is what the browser acceptance asserts).
  const TOKENS = {
    logBg: "#1a1613",
    surface: "#241f1b",
    surfaceHi: "#322b25",
    border: "#3a332d",
    text: "#e8e2d9",
    textMuted: "#9b948a",
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
        "editor.lineHighlightBackground": "#1f1a17",
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
        { token: "comment", foreground: "9b948a", fontStyle: "italic" },
        { token: "keyword", foreground: "c98a7d" },
        { token: "type", foreground: "cba6c3" },
        { token: "type.identifier", foreground: "cba6c3" },
        { token: "number", foreground: "d8a76a" },
        { token: "string", foreground: "9fb98a" },
        { token: "variable", foreground: "9db7c9" },
        { token: "attribute.name", foreground: "e8d9a8" },
        { token: "attribute.value", foreground: "9fb98a" },
        { token: "tag", foreground: "c98a7d" },
        { token: "delimiter", foreground: "9b948a" },
        { token: "operator", foreground: "9b948a" },
        { token: "metatag", foreground: "9b948a" },
      ],
    });
  }

  // The four LSP contributions ship with `editor.main`, but their heavy workers
  // are deliberately NOT vendored — leaving a provider on means a lazily-loaded
  // mode chunk asking for a worker that 404s. Language *services* are out of
  // scope (PRD #297); Monarch highlighting needs none of this.
  const NO_PROVIDERS = {
    documentFormattingEdits: false,
    documentRangeFormattingEdits: false,
    completionItems: false,
    hovers: false,
    documentSymbols: false,
    tokens: false,
    colors: false,
    foldingRanges: false,
    diagnostics: false,
    selectionRanges: false,
  };

  function disableLanguageServices(monaco) {
    const langs = monaco.languages;
    langs.json?.jsonDefaults?.setModeConfiguration({ ...NO_PROVIDERS, documentFormattingEdits: false });
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
      automaticLayout: false, // WBViewer.setActive drives relayout explicitly
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

  window.WBMonaco = { ready, create, TOKENS };
})();
