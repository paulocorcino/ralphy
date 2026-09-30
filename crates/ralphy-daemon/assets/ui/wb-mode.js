/* ---------------------------------------------------------------------------
   Operation mode — the single "seed or honest error?" predicate (#202).

   The workbench ships as ONE bundle that runs in two worlds:
   the daemon-backed app (served over http/https by ralphy-daemon) and a static
   `file://` demo (double-click index.html, no backend). Every decision that used
   to be a scattered `location.protocol !== "file:"` check or a silent `catch {}`
   falling into seed/`fakeContent` now routes through this predicate, so synthetic
   data is reachable ONLY in demo — a daemon-mode transport failure surfaces the
   error instead of masking it with seed data.

   Pure walkthrough (protocol → mode / seedAllowed), no JS harness needed:
     file:            → demo   / seedAllowed=true
     http:  / https:  → daemon / seedAllowed=false
--------------------------------------------------------------------------- */
(function () {
  function modeFor(protocol) {
    return protocol === "file:" ? "demo" : "daemon";
  }
  function isDemo(protocol = location.protocol) {
    return modeFor(protocol) === "demo";
  }
  function isDaemon(protocol = location.protocol) {
    return modeFor(protocol) === "daemon";
  }
  // Seeds are honest only in the static demo; in daemon mode a failure
  // must show as a failure, never as seed data.
  function seedAllowed(protocol = location.protocol) {
    return isDemo(protocol);
  }
  // The URL of a torn-off window. The daemon serves each page at a route and
  // refuses its file name (`Shell` in the daemon's `assets.rs`); a `file://`
  // demo has no router, so it opens the file itself.
  const PAGES = {
    popup: { route: "popup", file: "detached.html" },
    fence: { route: "fence", file: "detached-fence.html" },
  };
  function pageUrl(page, protocol = location.protocol) {
    return isDemo(protocol) ? PAGES[page].file : PAGES[page].route;
  }
  window.WBMode = { modeFor, isDemo, isDaemon, seedAllowed, pageUrl };
})();
