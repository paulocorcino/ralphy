/* ---------------------------------------------------------------------------
   ralphy workbench — the desk WRITE seam

   `wb-console.js` owns the desk mirror and decides WHEN to persist; this module
   owns WHERE that write goes. Two implementations, one surface:

     daemon()  the shell's real write — `PUT /api/desk` (ADR-0050)
     none()    a sink that writes nowhere

   The seam exists because the same console module runs in a second document —
   the detached-fence popup — which renders a fence's consoles but must never
   author the desk: it holds a partial view of the plane, so a PUT from there
   would replace the operator's whole layout with a fragment of it. Injecting
   the sink is what makes the popup INCAPABLE of writing, rather than merely
   guarded at each call site by a `detached` test that a later edit can forget.

   Only the WRITE is injected. The GET stays in `wb-console.js` and DOES run in
   the popup (`reloadDesk` is called at module load in every document), so the
   suppression rests entirely on the sink here — not on an unlifted `deskLoaded`
   permit. Reading is harmless; replacing the desk from a partial view is not.
--------------------------------------------------------------------------- */
window.WBDeskSink = (function () {
  // The daemon-backed write. Both entry points take an already-serialised body,
  // because both callers snapshot the desk at SCHEDULE time — re-measuring at
  // fire time is what stored a zeroed pan in #339.
  function daemon() {
    // Chained on the previous flush so two mutations 250 ms apart cannot land
    // out of order over a LAN or a dev tunnel. INVARIANT: no drag, resize,
    // close or `persistWin` path may await or throw on this — a refused PUT
    // costs a stale position and the next mutation supersedes it (last write
    // wins, no ETag).
    let inFlight = Promise.resolve();
    return {
      put(body) {
        inFlight = inFlight
          .catch(() => {})
          .then(() =>
            fetch("/api/desk", {
              method: "PUT",
              headers: { "Content-Type": "application/json" },
              body,
            }).catch(() => {}),
          );
        return inFlight;
      },
      // The tab is going away: `keepalive` lets the request outlive the
      // document. Deliberately NOT chained — there is no next flush to order
      // against, and awaiting one would be awaiting past the document's life.
      putSync(body) {
        try {
          fetch("/api/desk", {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body,
            keepalive: true,
          }).catch(() => {});
        } catch {}
      },
    };
  }

  // Writes nowhere, and says so by returning the same shapes `daemon()` does —
  // a caller cannot tell them apart, which is the point.
  function none() {
    return {
      put() {
        return Promise.resolve();
      },
      putSync() {},
    };
  }

  return { daemon, none };
})();
