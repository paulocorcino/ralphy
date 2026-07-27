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
      return parsed;
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
