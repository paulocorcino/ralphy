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
  // `checkout` (#407) pins the diff to the selected worktree the way a file tab
  // is pinned: it rides the id (so the primary's and the worktree's diff of one
  // path are two tabs) and both sides read from the pin, never the live
  // selection.
  function diffTarget(entry, project, checkout) {
    return {
      id: "diff:" + project + (checkout ? "@" + checkout : "") + ":" + entry.path,
      checkout: checkout || null,
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
      note: "Sync status unknown",
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
      return { ...base, state: "detached", branch: body.head.sha || "", note: "Detached HEAD" };
    }
    const branch = body.head.name || "";
    const t = body.tracking;
    if (!t || typeof t !== "object") {
      return { ...base, state: "no-upstream", branch, note: "No upstream" };
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
    if (!lastFetch) return "Never fetched";
    const then = Date.parse(lastFetch);
    if (isNaN(then)) return "Fetch time unknown";
    const d = (typeof now === "number" ? now : Date.now()) - then;
    if (d < 60000) return "Fetched a moment ago";
    if (d < 3600000) return "Fetched " + Math.floor(d / 60000) + "m ago";
    if (d < 86400000) return "Fetched " + Math.floor(d / 3600000) + "h ago";
    return "Fetched " + Math.floor(d / 86400000) + "d ago";
  }

  // The Projects-view change indicator for ONE slug (#317). Taking a single slug
  // — not the map — is what makes a cross-repo aggregate structurally impossible.
  // A badge only ever says there is something to look at, so it renders only
  // for a count that was read and is not zero. A slug nobody read, a clean tree
  // and a failed read all render nothing: a `0` would claim a clean tree nobody
  // looked at, and a failed read has no count to show. Its reason stays in the
  // Changes view's title (`changesReadError`).
  function projectBadge(counts, slug) {
    const count = counts && counts[slug];
    // `text` is empty rather than absent: Alpine's `x-text` assigns whatever it
    // gets straight to `textContent`, so `undefined` here would put the literal
    // word "undefined" inside every hidden badge.
    if (typeof count !== "number" || count === 0) return { show: false, text: "", title: "" };
    return { show: true, text: String(count), title: count + " changed" };
  }

  // A group action's path list (#318). The expansion is over entries the daemon
  // ALREADY sent, so a group click can name nothing the daemon did not first
  // hand over — no glob, no pathspec, no "everything" token ever leaves here.
  //
  // A rename's ORIGINAL path is included only when `withOriginal` is set, i.e.
  // for the UNSTAGE direction: `git restore --staged` needs it to undo the
  // deletion half. It must NOT travel on the stage direction — after `git mv a
  // b` the old path is in neither the index nor the worktree, so `git add a` is
  // fatal, and `git add` aborts the WHOLE invocation on one unmatched pathspec.
  // A single rename-then-edit entry would otherwise make the group's `+` stage
  // nothing at all (reproduced live; `ralphy_core::worktree::stage` refuses it
  // by value too, so this is the near half of a two-sided fix).
  //
  // De-duplicated because one path can sit in both groups (an `AM` entry) and
  // because a rename's old path can also be another entry's.
  function groupPaths(entries, withOriginal) {
    const out = [];
    const seen = new Set();
    for (const e of Array.isArray(entries) ? entries : []) {
      for (const p of [e && e.path, withOriginal ? e && e.originalPath : null]) {
        if (typeof p === "string" && p && !seen.has(p)) {
          seen.add(p);
          out.push(p);
        }
      }
    }
    return out;
  }

  // What the commit button says. It names the branch the commit will LAND on —
  // the operator is composing a commit in a sidebar, with no prompt to read it
  // off. A detached HEAD says so instead of naming a sha as if it were a branch,
  // and an unknown sync fold says the least it can rather than guessing.
  function commitTarget(sync) {
    if (!sync || typeof sync !== "object" || !sync.state || sync.state === "unknown") {
      return { label: "Commit", branch: "", detached: false };
    }
    if (sync.state === "detached") {
      return { label: "Commit (detached HEAD)", branch: sync.branch || "", detached: true };
    }
    const branch = sync.branch || "";
    return {
      label: branch ? "Commit to " + branch : "Commit",
      branch,
      detached: false,
    };
  }

  // Why the write controls are disabled, or `""` when they are not. Derived from
  // the repo's LIVE RUN list (`runs.list`, ADR-0047 §9): a run holding the
  // repo's lock is exactly what the CLI guard refuses under. The guard remains
  // the authority — this only makes the consequence visible before the click.
  //
  // `tail` names WHAT is closed, because the lock closes more than this panel:
  // the board's label editor is refused by the same guard (`mutate.rs`'s
  // `guard_run_lock(&ws, "label set", …)`) and needs the same sentence with a
  // different subject. One predicate, two subjects — never two predicates.
  function writeLockReason(runs, tail = "These controls work again when it finishes.") {
    if (!Array.isArray(runs) || runs.length === 0) return "";
    return `A run is active in this project. ${tail}`;
  }

  // The confirmation a discard must carry (#319). Two cases with different
  // RECOVERABILITY, so they are two different dialogs rather than one sentence
  // with a conditional clause: a tracked path's worktree edit is thrown away but
  // its staged blob and its commits remain, while an untracked file has never
  // been committed and nothing can bring it back.
  //
  // The name falls back to the whole path because git reports an untracked
  // DIRECTORY as one entry ending in `/` — `splitPath("newdir/").name` is `""`,
  // and a dialog asking to delete “” names nothing.
  function discardConfirm(entry) {
    const e = entry || {};
    const name = e.name || e.path || "";
    if (e.status === "untracked") {
      return {
        title: "Delete untracked file",
        message:
          "Delete “" +
          name +
          "”? It was never committed and cannot be recovered.",
        confirmLabel: "Delete permanently",
        danger: true,
        unrecoverable: true,
      };
    }
    return {
      title: "Discard changes",
      message:
        "Discard unstaged changes to “" +
        name +
        "”? Staged changes for this file are kept.",
      confirmLabel: "Discard changes",
      danger: true,
      unrecoverable: false,
    };
  }

  // What a group's discard removes, stated on the group head itself. The staged
  // group carries no discard control at all: `restore --worktree` does not touch
  // the index, so a control there would claim to throw away something it keeps.
  function groupDiscardNote(group) {
    if (group === "unstaged") return "Discard removes working-tree changes. Staged changes are kept.";
    if (group === "staged") return "Unstage first, then discard.";
    return "";
  }

  window.WBChanges = {
    fold,
    foldSync,
    marker,
    shouldReload,
    diffTarget,
    projectBadge,
    groupPaths,
    commitTarget,
    writeLockReason,
    discardConfirm,
    groupDiscardNote,
  };
})(window);
