/* ---------------------------------------------------------------------------
   Operation mode — the single "seed or honest error?" predicate (#202).

   The workbench ships as ONE bundle that runs in two worlds:
   the daemon-backed app (served over http/https by ralphy-daemon) and a static
   `file://` demo (double-click index.html, no backend). Every decision that used
   to be a scattered `location.protocol !== "file:"` check or a silent `catch {}`
   falling into seed/`fakeContent` now routes through this predicate, so synthetic
   data is reachable ONLY in demo — a daemon-mode transport failure surfaces the
   error instead of masking it with a mock.

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
  // Seeds/mocks are honest only in the static demo; in daemon mode a failure
  // must show as a failure, never as seed data.
  function seedAllowed(protocol = location.protocol) {
    return isDemo(protocol);
  }
  window.WBMode = { modeFor, isDemo, isDaemon, seedAllowed };
})();
