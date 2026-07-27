// The PER-CLIENT view (issue #339, ADR-0051 §8): the viewport offset and the
// open file tabs, in this browser profile only.
//
// This is the ONE module in the workbench allowed to touch `localStorage`, and
// it holds ONE key. ADR-0050 §3 dropped the browser desk store — that rejection
// was of a second copy of the DESK (windows, and later fences), authoritative in
// no mode; the desk stays daemon-owned. The view is different state with a
// different lifetime: shared, one operator's panning would drag the other's
// view, which is the flapping defect translated to the canvas. So it lives here,
// per profile, and NOTHING about the layout may join it — `wb_desk_327.py`
// scenario 5 and `shell_stores_only_the_view_in_the_browser` assert exactly that.
//
// Two writers share the key (the console's offset, the shell's tabs), which is
// why `patch` is read-modify-write: either half writing the whole record would
// clobber the other's.
window.WBView = (function () {
  const KEY = "wb.view.v1";

  // A disabled store (private mode, a blocked third-party context, a full quota)
  // must degrade to "nothing stored", never throw into a caller's boot path —
  // both the landing and the tab restore run before anything else is on screen.
  function read() {
    try {
      const raw = localStorage.getItem(KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
      // A record from a future (or corrupt) version is not ours to interpret.
      if (parsed.v !== 1) return null;
      // NORMALISED, not merely returned: both callers run before anything is on
      // screen, and `for (const t of stored.tabs)` on a `tabs` that is a number
      // throws straight into the boot path — the failure mode this whole
      // try/catch exists to prevent, one level down.
      return {
        ...parsed,
        tabs: Array.isArray(parsed.tabs) ? parsed.tabs : [],
        off:
          parsed.off && typeof parsed.off === "object" && !Array.isArray(parsed.off)
            ? parsed.off
            : null,
      };
    } catch {
      return null;
    }
  }

  function patch(part) {
    try {
      const next = { ...(read() || {}), ...(part || {}), v: 1 };
      localStorage.setItem(KEY, JSON.stringify(next));
      return next;
    } catch {
      return null;
    }
  }

  return { KEY, read, patch };
})();
