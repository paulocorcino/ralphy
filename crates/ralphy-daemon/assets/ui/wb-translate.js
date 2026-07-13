/* ---------------------------------------------------------------------------
   ralphy workbench shell — shared on-device translation

   One implementation over the browser's built-in AI: the Translator API plus
   LanguageDetector (Chrome/Edge 138+). Free, on-device, no network, no key; the
   model for a language pair downloads once on first use. Degrades gracefully
   where the API is absent (callers disable their toggle).

   Used by the Runs plan blocks (app.js) and the Markdown viewer (wb-viewer.js).
--------------------------------------------------------------------------- */
window.WBTranslate = {
  _supported: null,
  // the languages offered in the pickers (label = the compact code shown)
  LANGS: [
    { code: "pt", label: "PT" },
    { code: "en", label: "EN" },
    { code: "es", label: "ES" },
    { code: "fr", label: "FR" },
    { code: "de", label: "DE" },
  ],

  supported() {
    if (this._supported === null) {
      this._supported = typeof self !== "undefined" && "Translator" in self;
    }
    return this._supported;
  },
  // the browser UI language, the natural default target ("translate to mine")
  browserLang() {
    return (navigator.language || "en").split("-")[0].toLowerCase();
  },

  // Best-effort source-language detection; null when the detector is absent.
  async detect(text) {
    if (typeof self === "undefined" || !("LanguageDetector" in self)) return null;
    try {
      const d = await LanguageDetector.create();
      const r = await d.detect(text);
      d.destroy?.();
      return r?.[0]?.detectedLanguage || null;
    } catch {
      return null;
    }
  },

  // Translate `text` into `target`. Resolves to { text, source, target, same }.
  // `same` is true when the detected source already is the target — a clean
  // no-op, surfaced so the UI can say "already in X" instead of looking broken.
  // Rejects when the API or the specific language-pair model is unavailable.
  async translate(text, target) {
    if (!this.supported()) throw new Error("Translator API unavailable (needs Chrome/Edge 138+)");
    const source = (await this.detect(text)) || "en";
    if (source === target) return { text, source, target, same: true };
    const avail = await Translator.availability({ sourceLanguage: source, targetLanguage: target });
    if (avail === "unavailable") throw new Error(`no ${source} → ${target} model`);
    const tr = await Translator.create({
      sourceLanguage: source,
      targetLanguage: target,
      // a model may download on first use; progress arrives on the monitor
      monitor(m) {
        m.addEventListener("downloadprogress", () => {});
      },
    });
    const out = await tr.translate(text);
    tr.destroy?.();
    return { text: out, source, target, same: false };
  },
};
