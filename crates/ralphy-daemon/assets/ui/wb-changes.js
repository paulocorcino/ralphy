// The Changes section's model, folded from the daemon's `changes.list` Query
// reply (`reply.changes.changes` — the CLI's `{changes:[…]}` nested under the
// verb's reply field). Pure: no DOM, no fetch — unit-tested in
// ui-tests/wb-changes.test.mjs.
(function (window) {
  "use strict";

  // `entries` rides along unused by this slice's count-only render: the fold is
  // one JSON-to-list-model step, and splitting it would mean rewriting it when
  // the list lands.
  function fold(reply) {
    // A non-ok reply can still carry a `changes` body (an error frame the daemon
    // built over a partial read); folding it would report a count nobody proved.
    if (!reply || reply.status !== "ok") return { count: 0, entries: [] };
    const rows = reply.changes && reply.changes.changes;
    if (!Array.isArray(rows)) return { count: 0, entries: [] };
    const entries = rows
      .filter((r) => r && typeof r.path === "string")
      .map((r) => ({
        path: r.path,
        originalPath: r.original_path || null,
        status: r.status || "modified",
      }));
    return { count: entries.length, entries };
  }

  window.WBChanges = { fold };
})(window);
