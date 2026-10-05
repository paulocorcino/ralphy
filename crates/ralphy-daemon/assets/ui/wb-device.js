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

   Load order: after `wb-session-route.js` (the tab's holder ID), before
   `app.js`, on `index.html` only.
   --------------------------------------------------------------------------- */
window.WBDevice = (function () {
  async function attempt(fn) {
    try {
      const v = await fn();
      return v === undefined ? null : v;
    } catch {
      return null;
    }
  }

  function media(win, query) {
    try {
      return win.matchMedia ? win.matchMedia(query).matches : null;
    } catch {
      return null;
    }
  }

  function firstMatch(win, feature, values) {
    for (const v of values) {
      if (media(win, `(${feature}: ${v})`)) return v;
    }
    return null;
  }

  function cssSupports(win, prop, value) {
    try {
      return win.CSS && win.CSS.supports ? win.CSS.supports(prop, value) : null;
    } catch {
      return null;
    }
  }

  // One context, read, then lost at once. The consoles hold at most 12
  // WebGL contexts (wb-console.js GPU_BUDGET) and Chrome drops the oldest
  // past 16, so one more for a moment takes no console's place.
  function gpu(win) {
    const doc = win.document;
    if (!doc) return null;
    const canvas = doc.createElement("canvas");
    const gl = canvas.getContext("webgl") || canvas.getContext("experimental-webgl");
    if (!gl) return null;
    const out = { vendor: gl.getParameter(gl.VENDOR), renderer: gl.getParameter(gl.RENDERER) };
    const dbg = gl.getExtension("WEBGL_debug_renderer_info");
    if (dbg) {
      out.unmasked_vendor = gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL);
      out.unmasked_renderer = gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
    }
    const lose = gl.getExtension("WEBGL_lose_context");
    if (lose) lose.loseContext();
    return out;
  }

  function safeArea(win) {
    const doc = win.document;
    if (!doc || !doc.body || !win.getComputedStyle) return null;
    const el = doc.createElement("div");
    el.style.cssText =
      "position:fixed;visibility:hidden;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left)";
    doc.body.appendChild(el);
    const cs = win.getComputedStyle(el);
    const out = {
      top: parseFloat(cs.paddingTop) || 0,
      right: parseFloat(cs.paddingRight) || 0,
      bottom: parseFloat(cs.paddingBottom) || 0,
      left: parseFloat(cs.paddingLeft) || 0,
    };
    el.remove();
    return out;
  }

  // The count and the first few voices: the whole list ran to 324 entries
  // on Windows, past the route's size cap.
  const VOICE_SAMPLE = 5;
  function voices(win) {
    const s = win.speechSynthesis;
    if (!s || !s.getVoices) return null;
    const read = () => {
      const all = s.getVoices().map((v) => `${v.name}|${v.lang}|${v.localService ? 1 : 0}`);
      return { count: all.length, sample: all.slice(0, VOICE_SAMPLE) };
    };
    const now = read();
    if (now.count || !s.addEventListener) return Promise.resolve(now);
    // Chromium fills the list a moment after the first call.
    return new Promise((resolve) => {
      const done = () => resolve(read());
      s.addEventListener("voiceschanged", done, { once: true });
      setTimeout(done, 800);
    });
  }

  function battery(nav) {
    if (!nav.getBattery) return null;
    return nav.getBattery().then((b) => ({ level: b.level, charging: b.charging }));
  }

  function keyboard(nav) {
    if (!nav.keyboard || !nav.keyboard.getLayoutMap) return null;
    return nav.keyboard.getLayoutMap().then((m) => ({
      KeyQ: m.get("KeyQ"),
      Semicolon: m.get("Semicolon"),
      Backslash: m.get("Backslash"),
      IntlRo: m.get("IntlRo"),
    }));
  }

  async function collect(win) {
    const nav = win.navigator || {};
    const scr = win.screen || {};
    const uad = nav.userAgentData;
    const intl = await attempt(() => win.Intl.DateTimeFormat().resolvedOptions());
    return {
      ua: await attempt(() => nav.userAgent),
      ua_data: await attempt(() =>
        uad
          ? uad
              .getHighEntropyValues(["platformVersion", "model", "architecture", "bitness", "fullVersionList", "formFactors"])
              .then((h) => ({ ...h, brands: uad.brands, mobile: uad.mobile, platform: uad.platform }))
          : null,
      ),
      platform: await attempt(() => nav.platform),
      vendor: await attempt(() => nav.vendor),
      languages: await attempt(() => Array.from(nav.languages || [])),
      intl: intl && {
        time_zone: intl.timeZone,
        locale: intl.locale,
        calendar: intl.calendar,
        numbering_system: intl.numberingSystem,
        hour_cycle: await attempt(
          () => new win.Intl.DateTimeFormat(undefined, { hour: "numeric" }).resolvedOptions().hourCycle,
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
        is_extended: scr.isExtended === undefined ? null : scr.isExtended,
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
        display_mode: firstMatch(win, "display-mode", ["standalone", "fullscreen", "minimal-ui", "browser"]),
      },
      engine_signals: {
        webkit_touch_callout: cssSupports(win, "-webkit-touch-callout", "none"),
        gesture_event: "GestureEvent" in win,
        safari_object: "safari" in win,
        ios_standalone: nav.standalone === undefined ? null : nav.standalone,
        moz_appearance: cssSupports(win, "-moz-appearance", "none"),
        chrome_object: "chrome" in win,
      },
      gpu: await attempt(() => gpu(win)),
      voices: await attempt(() => voices(win)),
      automation: await attempt(() => nav.webdriver),
      plugins: await attempt(() => (nav.plugins ? nav.plugins.length : null)),
      pdf_viewer: await attempt(() => nav.pdfViewerEnabled),
      cookies: await attempt(() => nav.cookieEnabled),
      do_not_track: await attempt(() => nav.doNotTrack),
      storage: await attempt(() =>
        nav.storage && nav.storage.estimate
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

  // Send once per page load. A 401 (no login yet) leaves it unsent, and the
  // `login` action sends it again.
  function report(win) {
    let sent = false;
    async function send() {
      if (sent) return;
      const facts = await collect(win);
      facts.holder = (await attempt(() => win.WBSessionRoute.tabHolder())) ?? null;
      const res = await win.fetch("/api/device/facts", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(facts),
      });
      if (res.ok) sent = true;
    }
    const quiet = () => send().catch(() => {});
    win.document.addEventListener("workbench:action", (e) => {
      if (e.detail && e.detail.action === "login") quiet();
    });
    quiet();
  }

  if (typeof window !== "undefined" && window.document && window.fetch) report(window);

  return { collect, report };
})();
