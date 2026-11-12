/* ---------------------------------------------------------------------------
   Device facts (ADR-0074, CONTEXT.md → *Device facts*): what this page reads
   about its own browser, screen and machine, sent once per page load to
   `POST /api/device/facts` for the audit log.

   The page only collects. The daemon normalizes, and it never trusts these
   facts for access: a page can report anything.

   `collect(win)` reads only the `win` it is given, and every API that is
   missing or throws becomes `null`. `report(win)` sends the facts; before a
   login the route answers 401, and the `login` workbench action sends them
   again.

   `main.ts` calls `report(window)` once, on `index.html` only. The page that
   loads this module imports `wb-session-route.ts` for the tab's holder ID.
   --------------------------------------------------------------------------- */
import { WBSessionRoute } from "./wb-session-route.ts";

/** The page's own window, which `report` reads. */
type Page = Window & typeof globalThis;

/** The window members a browser, or a test's window, may lack. */
type MaybeMissing =
  | "document"
  | "matchMedia"
  | "CSS"
  | "getComputedStyle"
  | "speechSynthesis"
  | "navigator"
  | "screen";

/** The window `collect` reads: the page's own, or a test's. */
type Win = Omit<Page, MaybeMissing> & {
  document?: Document;
  matchMedia?: Page["matchMedia"];
  CSS?: Partial<Page["CSS"]>;
  getComputedStyle?: Page["getComputedStyle"];
  speechSynthesis?: Partial<SpeechSynthesis>;
  navigator?: NavigatorFacts;
  screen?: ScreenFacts;
};

/** The `navigator` members this module reads. A browser may lack any of them. */
type NavigatorFacts = Partial<Navigator> & {
  userAgentData?: {
    brands: { brand: string; version: string }[];
    mobile: boolean;
    platform: string;
    getHighEntropyValues(hints: string[]): Promise<{ [hint: string]: JsonValue }>;
  };
  getBattery?(): Promise<{ level: number; charging: boolean }>;
  deviceMemory?: number;
  /** iOS only. */
  standalone?: boolean;
  connection?: {
    type?: string;
    effectiveType?: string;
    rtt?: number;
    downlink?: number;
    saveData?: boolean;
  };
  keyboard?: { getLayoutMap?(): Promise<{ get(code: string): string | undefined }> };
};

/** The `screen` members this module reads. */
type ScreenFacts = Partial<Screen> & { isExtended?: boolean };

/** What `collect` returns: one field per fact. */
type Facts = { [fact: string]: JsonValue | undefined };

// `Number("12px")` is NaN: a padding needs `parseFloat`.
// oxlint-disable-next-line unicorn/prefer-number-coercion -- the value ends in "px"
const px = (v: string) => parseFloat(v) || 0;

export const WBDevice = (function () {
  async function attempt<T>(fn: () => T | Promise<T>): Promise<T | null> {
    try {
      const v: T | undefined = await fn();
      return v ?? null;
    } catch {
      return null;
    }
  }

  function media(win: Win, query: string) {
    try {
      return win.matchMedia ? win.matchMedia(query).matches : null;
    } catch {
      return null;
    }
  }

  function firstMatch(win: Win, feature: string, values: string[]) {
    for (const v of values) {
      if (media(win, `(${feature}: ${v})`) === true) return v;
    }
    return null;
  }

  function cssSupports(win: Win, prop: string, value: string) {
    try {
      return win.CSS?.supports ? win.CSS.supports(prop, value) : null;
    } catch {
      return null;
    }
  }

  // `getParameter` is typed `any`; the four names asked here are strings.
  function glString(gl: WebGLRenderingContext, name: number): string {
    const v: unknown = gl.getParameter(name);
    return typeof v === "string" ? v : "";
  }

  // One context, read, then lost at once. The consoles hold at most 12
  // WebGL contexts (wb-console.ts GPU_BUDGET) and Chrome drops the oldest
  // past 16, so one more for a moment takes no console's place.
  function legacyGl(canvas: HTMLCanvasElement) {
    const old = canvas.getContext("experimental-webgl");
    return old instanceof WebGLRenderingContext ? old : null;
  }

  function gpu(win: Win) {
    const doc = win.document;
    if (!doc) return null;
    const canvas = doc.createElement("canvas");
    const gl = canvas.getContext("webgl") ?? legacyGl(canvas);
    if (!gl) return null;
    const out: Record<string, string> = {
      vendor: glString(gl, gl.VENDOR),
      renderer: glString(gl, gl.RENDERER),
    };
    const dbg = gl.getExtension("WEBGL_debug_renderer_info");
    if (dbg) {
      out["unmasked_vendor"] = glString(gl, dbg.UNMASKED_VENDOR_WEBGL);
      out["unmasked_renderer"] = glString(gl, dbg.UNMASKED_RENDERER_WEBGL);
    }
    const lose = gl.getExtension("WEBGL_lose_context");
    if (lose) lose.loseContext();
    return out;
  }

  function safeArea(win: Win) {
    const doc = win.document;
    if (!doc || !win.getComputedStyle) return null;
    // oxlint-disable-next-line typescript/no-unnecessary-condition -- null in a page not parsed yet
    if (doc.body === null) return null;
    const el = doc.createElement("div");
    el.style.cssText =
      "position:fixed;visibility:hidden;" +
      "padding:env(safe-area-inset-top) env(safe-area-inset-right) " +
      "env(safe-area-inset-bottom) env(safe-area-inset-left)";
    doc.body.append(el);
    const cs = win.getComputedStyle(el);
    const out = {
      top: px(cs.paddingTop),
      right: px(cs.paddingRight),
      bottom: px(cs.paddingBottom),
      left: px(cs.paddingLeft),
    };
    el.remove();
    return out;
  }

  // The count and the first few voices: the whole list ran to 324 entries
  // on Windows, past the route's size cap.
  const VOICE_SAMPLE = 5;
  function voices(win: Win) {
    const s = win.speechSynthesis;
    const getVoices = s?.getVoices?.bind(s);
    if (!s || !getVoices) return null;
    const listen = s.addEventListener?.bind(s);
    const read = () => {
      const all = getVoices().map((v) => `${v.name}|${v.lang}|${v.localService ? 1 : 0}`);
      return { count: all.length, sample: all.slice(0, VOICE_SAMPLE) };
    };
    const now = read();
    if (now.count || !listen) return Promise.resolve(now);
    // Chromium fills the list a moment after the first call.
    return new Promise<ReturnType<typeof read>>((resolve) => {
      const done = () => {
        resolve(read());
      };
      listen("voiceschanged", done, { once: true });
      setTimeout(done, 800);
    });
  }

  function battery(nav: NavigatorFacts) {
    if (!nav.getBattery) return null;
    return nav.getBattery().then((b) => ({ level: b.level, charging: b.charging }));
  }

  function keyboard(nav: NavigatorFacts) {
    if (!nav.keyboard || !nav.keyboard.getLayoutMap) return null;
    return nav.keyboard.getLayoutMap().then((m) => ({
      KeyQ: m.get("KeyQ"),
      Semicolon: m.get("Semicolon"),
      Backslash: m.get("Backslash"),
      IntlRo: m.get("IntlRo"),
    }));
  }

  async function collect(win: Win): Promise<Facts> {
    const nav: NavigatorFacts = win.navigator ?? {};
    const scr: ScreenFacts = win.screen ?? {};
    const uad = nav.userAgentData;
    const intl = await attempt(() => win.Intl.DateTimeFormat().resolvedOptions());
    return {
      ua: await attempt(() => nav.userAgent),
      ua_data: await attempt(() =>
        uad
          ? uad
              .getHighEntropyValues([
                "platformVersion",
                "model",
                "architecture",
                "bitness",
                "fullVersionList",
                "formFactors",
              ])
              .then((h) => ({
                ...h,
                brands: uad.brands,
                mobile: uad.mobile,
                platform: uad.platform,
              }))
          : null,
      ),
      platform: await attempt(() => nav.platform),
      vendor: await attempt(() => nav.vendor),
      languages: await attempt(() => Array.from(nav.languages ?? [])),
      intl: intl && {
        time_zone: intl.timeZone,
        locale: intl.locale,
        calendar: intl.calendar,
        numbering_system: intl.numberingSystem,
        hour_cycle: await attempt(
          () =>
            new win.Intl.DateTimeFormat(undefined, { hour: "numeric" }).resolvedOptions().hourCycle,
        ),
      },
      tz_offset_min: await attempt(() => -new win.Date().getTimezoneOffset()),
      screen: await attempt(() => ({
        width: scr.width,
        height: scr.height,
        avail_width: scr.availWidth,
        avail_height: scr.availHeight,
        color_depth: scr.colorDepth,
        orientation: scr.orientation ? scr.orientation.type : null,
        is_extended: scr.isExtended ?? null,
      })),
      dpr: await attempt(() => win.devicePixelRatio),
      window: await attempt(() => ({
        inner: [win.innerWidth, win.innerHeight],
        outer: [win.outerWidth, win.outerHeight],
        visual_scale: win.visualViewport ? win.visualViewport.scale : null,
      })),
      safe_area: await attempt(() => safeArea(win)),
      touch_points: await attempt(() => nav.maxTouchPoints),
      cores: await attempt(() => nav.hardwareConcurrency),
      memory_gb: await attempt(() => nav.deviceMemory),
      media: {
        pointer: firstMatch(win, "pointer", ["fine", "coarse", "none"]),
        any_pointer: firstMatch(win, "any-pointer", ["fine", "coarse", "none"]),
        hover: firstMatch(win, "hover", ["hover", "none"]),
        color_scheme: firstMatch(win, "prefers-color-scheme", ["dark", "light"]),
        reduced_motion: firstMatch(win, "prefers-reduced-motion", ["reduce", "no-preference"]),
        contrast: firstMatch(win, "prefers-contrast", ["more", "less", "custom", "no-preference"]),
        forced_colors: firstMatch(win, "forced-colors", ["active", "none"]),
        inverted_colors: firstMatch(win, "inverted-colors", ["inverted", "none"]),
        color_gamut: firstMatch(win, "color-gamut", ["rec2020", "p3", "srgb"]),
        dynamic_range: firstMatch(win, "dynamic-range", ["high", "standard"]),
        display_mode: firstMatch(win, "display-mode", [
          "standalone",
          "fullscreen",
          "minimal-ui",
          "browser",
        ]),
      },
      engine_signals: {
        webkit_touch_callout: cssSupports(win, "-webkit-touch-callout", "none"),
        gesture_event: "GestureEvent" in win,
        safari_object: "safari" in win,
        ios_standalone: nav.standalone ?? null,
        moz_appearance: cssSupports(win, "-moz-appearance", "none"),
        chrome_object: "chrome" in win,
      },
      gpu: await attempt(() => gpu(win)),
      voices: await attempt(() => voices(win)),
      automation: await attempt(() => nav.webdriver),
      // browsers keep `PluginArray.length` for pages that read it
      // oxlint-disable-next-line typescript/no-deprecated -- the report has always sent this count
      plugins: await attempt(() => (nav.plugins ? nav.plugins.length : null)),
      pdf_viewer: await attempt(() => nav.pdfViewerEnabled),
      cookies: await attempt(() => nav.cookieEnabled),
      do_not_track: await attempt(() => nav.doNotTrack),
      storage: await attempt(() =>
        nav.storage
          ? nav.storage.estimate().then((e) => ({ quota: e.quota, usage: e.usage }))
          : null,
      ),
      connection: await attempt(() => {
        const c = nav.connection;
        return c
          ? {
              type: c.type ?? null,
              effective_type: c.effectiveType ?? null,
              rtt: c.rtt ?? null,
              downlink: c.downlink ?? null,
              save_data: c.saveData ?? null,
            }
          : null;
      }),
      battery: await attempt(() => battery(nav)),
      keyboard: await attempt(() => keyboard(nav)),
    };
  }

  // A 409 means the daemon has not given this browser its device yet: the
  // page's first `/api/session` is still on its way (ADR-0074 D5).
  const RETRY_MS = 1000;
  const RETRIES = 5;

  // Send once per page load. A 401 (no login yet) leaves it unsent, and the
  // `login` action sends it again. A 409 sends again a little later.
  function report(win: Page) {
    let sent = false;
    async function send(left: number) {
      if (sent) return;
      const facts = await collect(win);
      facts["holder"] = (await attempt(() => WBSessionRoute.tabHolder())) ?? null;
      const res = await win.fetch("/api/device/facts", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(facts),
      });
      if (res.ok) sent = true;
      else if (res.status === 409 && left > 0) win.setTimeout(() => quiet(left - 1), RETRY_MS);
    }
    const quiet = (left = RETRIES) => send(left).catch(() => {});
    win.document.addEventListener("workbench:action", (e: DocumentEventMap["workbench:action"]) => {
      if (e.detail.action === "login") void quiet();
    });
    void quiet();
  }

  return { collect, report };
})();
