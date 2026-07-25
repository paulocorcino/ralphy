// The Changes section's model, folded from the daemon's `changes.list` Query
// reply (`reply.changes.changes` — the CLI's `{changes:[…]}` nested under the
// verb's reply field). Pure: no DOM, no fetch — unit-tested in
// ui-tests/wb-changes.test.mjs.
(function (window) {
  "use strict";

  const EMPTY = { count: 0, entries: [] };

  // `entries` rides along unused by this slice's count-only render: the fold is
  // one JSON-to-list-model step, and splitting it would mean rewriting it when
  // the list lands.
  function fold(reply) {
    const rows = reply && reply.changes && reply.changes.changes;
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

  window.WBChanges = { fold, EMPTY };
})(window);
