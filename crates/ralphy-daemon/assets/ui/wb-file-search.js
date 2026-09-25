/* ---------------------------------------------------------------------------
   The FILES search (ADR-0036 amendment 2026-09-15) — the pure half. Which verb
   a mode names, whether a query is worth a walk, which directories a set of
   hits needs loaded before the tree can narrow to them, what the gutter says
   about a reply, and which folders to fold back when the search is cleared.

   No `this`, no tree, no socket: `app.js` owns the Wunderbaum instance and the
   `WBDaemon.observe` call and delegates every decision here, so the decisions
   are testable without a browser (ADR-0057 D4). The tree is what is filtered;
   there is no results panel, so nothing here builds one.
   --------------------------------------------------------------------------- */
window.WBFileSearch = (function () {
  // The shortest query the daemon walks for (`tree::search::MIN_QUERY_CHARS`).
  const MIN_CHARS = 2;
  // A keystroke is not a search; the pause after one is. Long enough that a
  // word typed at speed is one walk, not four: every intermediate query is a
  // daemon walk (five seconds on a peer at worst) plus the ancestor loads of
  // up to 200 hits, and a search cancelled by the next keystroke still ran.
  // Enter searches at once.
  const DEBOUNCE_MS = 800;
  // The daemon's hit cap; the note names it so "200" is not a mystery.
  const MAX_HITS = 200;

  // The verb behind each half of the Name | Content toggle. Anything else is
  // "name": the toggle has two states and a stale one must not send nothing.
  function verbFor(mode) {
    return mode === "content" ? "tree.grep" : "tree.find";
  }

  // Trimmed and at least MIN_CHARS: below that the daemon answers empty anyway,
  // so the client does not spend a socket to learn it.
  function worthSearching(query) {
    return String(query ?? "").trim().length >= MIN_CHARS;
  }

  // The distinct directories the hits live in — every ancestor, not just the
  // parent — shallow-first, so each can be loaded before its child is looked
  // for. A hit at the root has no ancestor and contributes nothing.
  function dirsToLoad(hits) {
    const dirs = new Set();
    for (const h of hits || []) {
      const parts = String(h.path || "").split("/");
      for (let i = 1; i < parts.length; i++) dirs.add(parts.slice(0, i).join("/"));
    }
    return [...dirs].sort((a, b) => a.split("/").length - b.split("/").length || (a < b ? -1 : 1));
  }

  // The gutter line for a settled search. Empty when the tree says it all.
  function note({ hits, truncated }) {
    if (!hits.length) return "No matches";
    if (truncated) return `First ${MAX_HITS} matches. Narrow the search to see more.`;
    return "";
  }

  // Folders expanded NOW that were not expanded BEFORE the search, deepest
  // first — collapsing a child before its parent never fights the parent's
  // own collapse. `before === null` means no search ever expanded anything.
  function toCollapse(before, now) {
    if (!before) return [];
    const had = new Set(before);
    return (now || [])
      .filter((rel) => !had.has(rel))
      .sort((a, b) => b.split("/").length - a.split("/").length || (a < b ? -1 : 1));
  }

  // The per-path lookup the filter predicate reads: `count` for a content hit,
  // `true` for a name hit (which has no count to show).
  function hitMap(hits) {
    const m = new Map();
    for (const h of hits || []) m.set(h.path, typeof h.count === "number" ? h.count : true);
    return m;
  }

  return { MIN_CHARS, DEBOUNCE_MS, MAX_HITS, verbFor, worthSearching, dirsToLoad, note, toCollapse, hitMap };
})();
