/* ---------------------------------------------------------------------------
   The steps of the FILES search that touch the tree (`fileSearchNow`,
   `applyFileSearch`, `clearFileSearch` and `resetFileSearch` of wb-files.ts):
   the read of the daemon, the narrowing of the tree to the hits, and the
   fold back to what the operator had open.

   Each step takes the files component as `files`. The tree is read and
   changed only through `files.rawTree()`: `this` of the component is Alpine's
   Proxy, and a change made through the Proxy leaves rows at stale offsets.
   The decisions (the floor, the verb, the hit map) are `WBFileSearch`'s.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBFileSearch } from "./wb-file-search.ts";
import type { Hit } from "./wb-file-search.ts";

/** What a search step reads, writes or calls on the files component. */
type SearchFiles = {
  fileSearch: {
    mode: string;
    query: string;
    seq: number;
    hits: Hit[];
    truncated: boolean;
    note: string;
    expandedBefore: string[] | null;
  };
  _fileSearchTimer: ReturnType<typeof setTimeout> | null;
  _fileHits: Map<string, number | true> | null;
  _restoringExpansion: boolean;
  $store: import("./wb-alpine.ts").AlpineMagics["$store"];
  rawTree(): WunderbaumTree | null;
  relPath(node: WunderbaumNode | null): string;
  isFolder(node: WunderbaumNode | null | undefined): boolean;
  expandedRels(): string[];
  treeMem(): void;
  useDaemonTree(): boolean;
  applyFileSearch(hits: Hit[], truncated: boolean, seq: number): Promise<void>;
  clearFileSearch(): Promise<void>;
};

// One Observe read, dated by `seq`: a reply that is not the newest, or
// arrives after a project switch, is dropped.
export function searchNow(files: SearchFiles): Promise<void> {
  clearTimeout(files._fileSearchTimer);
  const query = String(files.fileSearch.query ?? "").trim();
  if (!WBFileSearch.worthSearching(query)) {
    return files.clearFileSearch();
  }
  const seq = ++files.fileSearch.seq;
  if (!files.useDaemonTree()) {
    files.fileSearch.note = "search needs a daemon";
    return Promise.resolve();
  }
  const slug = files.$store.projects.openSlug;
  const verb = WBFileSearch.verbFor(files.fileSearch.mode);
  files.fileSearch.note = "searching…";
  // Find and grep walk the SELECTED tree (#406).
  const payload = window.WBDaemon.withCheckout({ repo: slug, query }, files.$store.checkouts.checkoutOf(slug));
  return window.WBDaemon.observe(verb, payload)
    .then((reply) => {
      if (seq !== files.fileSearch.seq || slug !== files.$store.projects.openSlug) return;
      if (WBFail.isError(reply) || !Array.isArray(reply?.hits)) {
        files.fileSearch.note = WBFail.failed(reply, "Could not search: the daemon gave no reason.");
        return;
      }
      return files.applyFileSearch(reply.hits, !!reply.truncated, seq);
    })
    .catch((err) => {
      if (seq !== files.fileSearch.seq) return;
      files.fileSearch.note = WBFail.failed({ message: err?.message }, "Could not search: the daemon did not answer.");
    });
}

// Narrow the tree to `hits`: snapshot the expansion once per search
// session, load every ancestor level (shallow-first), then filter. The
// expands run under `_restoringExpansion`: the remembered expansion must
// not learn them.
export async function applySearch(files: SearchFiles, hits: Hit[], truncated: boolean, seq: number) {
  files.treeMem();
  files.fileSearch.hits = hits;
  files.fileSearch.truncated = truncated;
  files.fileSearch.note = WBFileSearch.note({ hits, truncated });
  const tree = files.rawTree();
  if (!tree) return;
  if (files.fileSearch.expandedBefore === null) files.fileSearch.expandedBefore = files.expandedRels();
  // ONE paint, at the end: every lazy level repaints on its own, and one
  // landing between the old marks cleared and the new ones set left a
  // blank tree that nothing repainted (MEASURED 2026-09-15).
  files._restoringExpansion = true;
  tree.enableUpdate(false);
  try {
    for (const dir of WBFileSearch.dirsToLoad(hits)) {
      // A newer search, or a torn-down tree, owns the screen now.
      if (seq !== files.fileSearch.seq || tree !== files.rawTree()) return;
      const f = tree.findFirst((n: WunderbaumNode) => files.relPath(n) === dir);
      if (f && files.isFolder(f) && !f.expanded) await f.setExpanded(true);
    }
    if (seq !== files.fileSearch.seq || tree !== files.rawTree()) return;
    files._fileHits = WBFileSearch.hitMap(hits);
    // `autoExpand: false`: the extension would also open every MATCHED
    // folder, a burst of lazy loads.
    tree.filterNodes((n: WunderbaumNode) => files._fileHits!.has(files.relPath(n)), {
      mode: "hide",
      autoExpand: false,
      matchBranch: false,
      noData: false,
    });
  } finally {
    files._restoringExpansion = false;
    // Set BEFORE the paint: the row window is computed from `scrollTop`,
    // and a leftover offset painted two rows at the bottom of a 52-row tree.
    if (tree === files.rawTree()) {
      tree.element.scrollTop = 0;
      // Re-enabling paints immediately and in full.
      tree.enableUpdate(true);
    }
  }
}

// Take the filter off and fold back what the search opened, deepest first.
export async function clearSearch(files: SearchFiles) {
  files.treeMem();
  files._fileHits = new Map();
  files.fileSearch.hits = [];
  files.fileSearch.truncated = false;
  files.fileSearch.note = "";
  const before = files.fileSearch.expandedBefore;
  files.fileSearch.expandedBefore = null;
  const tree = files.rawTree();
  if (!tree) return;
  // Same one-paint discipline as `applySearch`.
  files._restoringExpansion = true;
  tree.enableUpdate(false);
  try {
    if (tree.isFilterActive?.()) tree.clearFilter();
    const fold = WBFileSearch.toCollapse(before, files.expandedRels());
    for (const rel of fold) {
      if (tree !== files.rawTree()) return;
      const f = tree.findFirst((n: WunderbaumNode) => files.relPath(n) === rel);
      if (f && f.expanded) await f.setExpanded(false);
    }
  } finally {
    files._restoringExpansion = false;
    if (tree === files.rawTree()) {
      tree.element.scrollTop = 0;
      tree.enableUpdate(true);
    }
  }
}

// The search's memory of a tree that no longer exists (see `destroyTree`).
export function resetSearch(files: SearchFiles) {
  clearTimeout(files._fileSearchTimer);
  files.fileSearch.seq++;
  files.fileSearch.query = "";
  files.fileSearch.hits = [];
  files.fileSearch.truncated = false;
  files.fileSearch.note = "";
  files.fileSearch.expandedBefore = null;
  files._fileHits = new Map();
}
