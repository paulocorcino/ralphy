// The Changes section's model, folded from the daemon's `changes.list` Query
// reply (`reply.changes.changes` — the CLI's `{changes:[…]}` nested under the
// verb's reply field). Pure: no DOM, no fetch — unit-tested in
// ui-tests/wb-changes.test.mjs.
(function (window) {
  "use strict";

  // Closed marker vocabulary for the producer's six-value status enum
  // (`ralphy-core/src/changes.rs` `ChangeStatus`, `#[serde(rename_all =
  // "snake_case")]`) — anything outside it renders as `?`/`st-unknown` so a
  // future seventh variant stays legible instead of blank.
  const MARKS = {
    modified: "M",
    added: "A",
    deleted: "D",
    renamed: "R",
    untracked: "U",
    conflicted: "!",
  };

  function marker(status) {
    const mark = MARKS[status];
    return mark ? { mark, cls: "st-" + status } : { mark: "?", cls: "st-unknown" };
  }

  function fold(reply) {
    // A non-ok reply can still carry a `changes` body (an error frame the daemon
    // built over a partial read); folding it would report a count nobody proved.
    if (!reply || reply.status !== "ok") return { count: 0, entries: [] };
    const rows = reply.changes && reply.changes.changes;
    if (!Array.isArray(rows)) return { count: 0, entries: [] };
    const entries = rows
      .filter((r) => r && typeof r.path === "string")
      .map((r) => {
        const status = r.status || "modified";
        const { mark, cls } = marker(status);
        return {
          path: r.path,
          originalPath: r.original_path || null,
          status,
          mark,
          cls,
          title: r.original_path ? r.original_path + " → " + r.path : r.path,
        };
      });
    return { count: entries.length, entries };
  }

  // The run-completion nudge filter (#310, ADR-0036 amendment): the daemon pushes
  // `changes.dirty` to EVERY `/ws/tree` connection with no subscription verb, so
  // the repo match happens HERE — a nudge for a repo this browser does not have
  // open must change nothing on screen.
  function shouldReload(frame, openSlug) {
    return !!(
      frame &&
      frame.verb === "changes.dirty" &&
      frame.payload &&
      frame.payload.repo &&
      openSlug &&
      frame.payload.repo === openSlug
    );
  }

  window.WBChanges = { fold, marker, shouldReload };
})(window);
