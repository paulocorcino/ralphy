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
    // A grouped row is marked by ITS OWN side, not by the derived status: under
    // a "Changes" headline an `AM` path is a modification, and printing the
    // derived `A` there would assert something false about the worktree.
    const sided = (e, status) => {
      if (!status || status === e.status) return e;
      const { mark, cls } = marker(status);
      return { ...e, mark, cls };
    };
    // Every entry lands in at least one group: an older or malformed payload
    // carrying neither side field still renders under Changes rather than
    // vanishing from a list whose badge still counts it.
    return {
      count: entries.length,
      entries,
      staged: entries.filter((e) => e.indexStatus).map((e) => sided(e, e.indexStatus)),
      unstaged: entries
        .filter((e) => e.worktreeStatus || !e.indexStatus)
        .map((e) => sided(e, e.worktreeStatus)),
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

  // The sync row's model, folded from the daemon's `sync.status` Query reply
  // (`reply.sync.sync` — the CLI's `{sync:…}` nested under the verb's reply
  // field, exactly as `fold` reads `reply.changes.changes`).
  //
  // "No upstream" and "detached" are their own STATES with empty counts: an
  // absent upstream rendered as `↑0 ↓0` would assert the branch is in sync with
  // something it does not track. `now` is injected so the staleness label is
  // testable; it defaults to the wall clock.
  function foldSync(reply, now) {
    const unknown = {
      state: "unknown",
      branch: "",
      upstream: "",
      ahead: 0,
      behind: 0,
      lastFetch: null,
      counts: "",
      note: "sync unavailable",
      fetched: "",
    };
    if (!reply || reply.status !== "ok") return unknown;
    const body = reply.sync && reply.sync.sync;
    if (!body || !body.head || typeof body.head.kind !== "string") return unknown;

    const lastFetch = body.last_fetch || null;
    const base = {
      branch: "",
      upstream: "",
      ahead: 0,
      behind: 0,
      lastFetch,
      counts: "",
      note: "",
      fetched: staleness(lastFetch, now),
    };
    if (body.head.kind === "detached") {
      return { ...base, state: "detached", branch: body.head.sha || "", note: "detached HEAD" };
    }
    const branch = body.head.name || "";
    const t = body.tracking;
    if (!t || typeof t !== "object") {
      return { ...base, state: "no-upstream", branch, note: "no upstream" };
    }
    const ahead = Number(t.ahead) || 0;
    const behind = Number(t.behind) || 0;
    return {
      ...base,
      state: "tracking",
      branch,
      upstream: t.upstream || "",
      ahead,
      behind,
      counts: "↑" + ahead + " ↓" + behind,
    };
  }

  // How stale the counts are, as a locale-free RELATIVE string: a formatted date
  // would follow the browser locale and could carry no exact-string oracle.
  function staleness(lastFetch, now) {
    if (!lastFetch) return "never fetched";
    const then = Date.parse(lastFetch);
    if (isNaN(then)) return "fetch time unknown";
    const d = (typeof now === "number" ? now : Date.now()) - then;
    if (d < 60000) return "fetched just now";
    if (d < 3600000) return "fetched " + Math.floor(d / 60000) + "m ago";
    if (d < 86400000) return "fetched " + Math.floor(d / 3600000) + "h ago";
    return "fetched " + Math.floor(d / 86400000) + "d ago";
  }

  // The Projects-view change indicator for ONE slug (#317). Taking a single slug
  // — not the map — is what makes a cross-repo aggregate structurally impossible.
  // A slug nobody read renders nothing at all: an em dash there would claim a
  // failed read for a project that was never asked about, and a `0` would claim a
  // clean tree nobody looked at.
  function projectBadge(counts, errors, slug) {
    const count = counts && counts[slug];
    const error = errors && errors[slug];
    if (error) return { show: true, text: "—", zero: false, title: String(error) };
    // `text` is empty rather than absent: Alpine's `x-text` assigns whatever it
    // gets straight to `textContent`, so `undefined` here would put the literal
    // word "undefined" inside every unread row's (hidden) badge.
    if (typeof count !== "number") return { show: false, text: "", zero: false, title: "" };
    return { show: true, text: String(count), zero: count === 0, title: count + " changed" };
  }

  window.WBChanges = { fold, foldSync, marker, shouldReload, diffTarget, projectBadge };
})(window);
