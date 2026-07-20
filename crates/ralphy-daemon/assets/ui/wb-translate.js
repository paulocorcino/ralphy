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

  // shown when detection fails — never a translate request with a guessed source
  NO_SOURCE_MSG: "couldn't detect the source language — translation skipped",

  supported() {
    if (this._supported === null) {
      this._supported = typeof self !== "undefined" && "Translator" in self;
    }
    return this._supported;
  },

  // Pure: the live download-progress label. `fraction` is 0..1 from the monitor.
  progressText(fraction) {
    return `downloading model — ${Math.round((Number(fraction) || 0) * 100)}%`;
  },

  // Pure: turn a raw Translator/create failure into something the operator can
  // act on. The empty/generic Chromium message ("Other generic failures
  // occurred.") carries no cause, so we map it to the likely cause + action;
  // already-actionable messages (our own "no <pair> model …") pass through.
  explainError(rawMsg, source, target) {
    const raw = rawMsg || "";
    const pair = `${source || "?"}→${target || "?"}`;
    if (raw === "" || /generic failure/i.test(raw)) {
      return `couldn't download the ${pair} model — check your connection and free disk space`;
    }
    return raw;
  },

  // Pure: availability × detected-source → the action to take. Keeps the async
  // translate() a thin driver over a testable decision.
  decide(source, target, availability) {
    if (!source) return { action: "fail", message: this.NO_SOURCE_MSG };
    if (source === target) return { action: "same" };
    if (availability === "unavailable") {
      return { action: "fail", message: `no ${source}→${target} model available on this device` };
    }
    if (["downloadable", "downloading"].includes(availability)) {
      return { action: "download", source, target };
    }
    return { action: "translate", source, target };
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
  // `onProgress(label, fraction)` fires while a language-pair model downloads.
  // Rejects (with an actionable message) when detection or the model fails.
  async translate(text, target, onProgress) {
    if (!this.supported()) throw new Error("Translator API unavailable (needs Chrome/Edge 138+)");
    const source = await this.detect(text); // no guessed fallback — a null source fails cleanly
    const availability =
      source && source !== target
        ? await Translator.availability({ sourceLanguage: source, targetLanguage: target })
        : "available";
    const plan = this.decide(source, target, availability);
    if (plan.action === "fail") throw new Error(plan.message);
    if (plan.action === "same") return { text, source, target, same: true };
    let tr;
    try {
      tr = await Translator.create({
        sourceLanguage: source,
        targetLanguage: target,
        // a model may download on first use; progress arrives on the monitor
        monitor: (m) => {
          m.addEventListener("downloadprogress", (e) => {
            const loaded = e?.loaded ?? 0;
            if (typeof onProgress === "function") onProgress(this.progressText(loaded), loaded);
          });
        },
      });
      const out = await tr.translate(text);
      return { text: out, source, target, same: false };
    } catch (e) {
      throw new Error(this.explainError(e?.message, source, target));
    } finally {
      // never leak the created translator — runs on every exit path once created
      tr?.destroy?.();
    }
  },
};
