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

  // The row's two halves: the base name, and the directory that carries it.
  // `lastIndexOf` and not a split-and-join — `docs/deep/nested/readme.md` must
  // yield `readme.md`, not `deep/nested/readme.md`.
  function splitPath(path) {
    const cut = path.lastIndexOf("/");
    return cut < 0
      ? { name: path, dir: "" }
      : { name: path.slice(cut + 1), dir: path.slice(0, cut) };
  }

  function fold(reply) {
    // A non-ok reply can still carry a `changes` body (an error frame the daemon
    // built over a partial read); folding it would report a count nobody proved.
    const empty = { count: 0, entries: [], staged: [], unstaged: [] };
    if (!reply || reply.status !== "ok") return empty;
    const rows = reply.changes && reply.changes.changes;
    if (!Array.isArray(rows)) return empty;
    const entries = rows
      .filter((r) => r && typeof r.path === "string")
      .map((r) => {
        const status = r.status || "modified";
        const { mark, cls } = marker(status);
        const { name, dir } = splitPath(r.path);
        return {
          path: r.path,
          originalPath: r.original_path || null,
          status,
          mark,
          cls,
          title: r.original_path ? r.original_path + " → " + r.path : r.path,
          indexStatus: r.index_status || null,
          worktreeStatus: r.worktree_status || null,
          name,
          dir,
        };
      });
    // Every entry lands in at least one group: an older or malformed payload
    // carrying neither side field still renders under Changes rather than
    // vanishing from a list whose badge still counts it.
    return {
      count: entries.length,
      entries,
      staged: entries.filter((e) => e.indexStatus),
      unstaged: entries.filter((e) => e.worktreeStatus || !e.indexStatus),
    };
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

  // What a diff tab needs to resolve both of its sides from one changes entry.
  // A rename diffs the OLD path at HEAD against the NEW path in the working tree
  // — reading `entry.path` at HEAD would report the whole file as added. An added
  // or untracked path has no HEAD side; a deleted one has no working side.
  function diffTarget(entry, project) {
    return {
      id: "diff:" + project + ":" + entry.path,
      title: entry.path.split("/").pop() + " ↔ HEAD",
      headPath: entry.originalPath || entry.path,
      workingPath: entry.path,
      headAbsent: entry.status === "added" || entry.status === "untracked",
      workingAbsent: entry.status === "deleted",
    };
  }

  window.WBChanges = { fold, marker, shouldReload, diffTarget };
})(window);
