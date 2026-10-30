/* ---------------------------------------------------------------------------
   The reconcile of one FILES tree level (`reconcileLevel` and
   `_reconcileOnce` of wb-files.ts): the guard that runs one pass per level at
   a time, then the steps of a pass: find the live node of the level, read
   what the operator sees before the teardown, open the folders again, and
   put the selection and the scroll offset back.

   Each step takes the files component as `files` and reads the tree only
   through `files.rawTree()`: `this` of the component is Alpine's Proxy, and a
   change made through the Proxy leaves rows at stale offsets.
   --------------------------------------------------------------------------- */
import { levelShows } from "./wb-files-levels.ts";

/** What a reconcile step reads from the files component. */
type ReconcileFiles = {
  rawTree(): WunderbaumTree | null;
  relPath(node: WunderbaumNode | null): string;
  isFolder(node: WunderbaumNode | null | undefined): boolean;
  revealRel(rel: string, opts?: { restore?: boolean }): Promise<unknown>;
  _revealSeq: number;
  _revealedRel: string | null;
};

/** What a whole reconcile pass reads, writes or calls on the files component. */
type PassFiles = ReconcileFiles &
  Parameters<typeof levelShows>[0] & {
    _reconciling: Set<string> | null;
    _reconcilePending: Set<string> | null;
    _reconcileWaiters: Map<string, (() => void)[]> | null;
    _reconcileOnce(node: WunderbaumNode, rel: string): Promise<void>;
    reconcileLevel(node: WunderbaumNode, rel: string): Promise<void>;
    findFolderByRel(rel: string): WunderbaumNode | null;
    fetchTreeLevel(rel: string): Promise<WunderbaumSource[]>;
    revalidateBelow(node: WunderbaumNode, rel: string): void;
    forgetValidatedBelow(rel: string): void;
  };

/** What the operator saw on a level before its teardown. */
type LevelView = { expandedRels: string[]; activeRel: string | null; scrollTop: number };

// The node of `rel` to tear down, or undefined when the reconcile stops. A
// reload of an ANCESTOR can land inside the fetch: its `removeChildren()`
// unregisters this node (`node.tree` null) and a teardown on the dead node
// throws inside Wunderbaum (MEASURED 2026-09-16, Safari). Re-resolve by rel:
// none is a level that no longer exists; one still loading is the ancestor's
// own re-expansion, and a teardown under an in-flight load doubles the
// children.
export function liveNode(files: ReconcileFiles, node: WunderbaumNode, rel: string) {
  if (node.tree) return node;
  const raw = files.rawTree();
  const live =
    rel === "" ? raw?.root : raw?.findFirst((n: WunderbaumNode) => files.relPath(n) === rel);
  if (!live || live.isLoading?.()) return undefined;
  return live;
}

// Read AFTER the fetch, right before the teardown: read before it, the
// snapshot missed every folder a FILES search opened meanwhile.
export function snapshotLevel(files: ReconcileFiles, node: WunderbaumNode): LevelView {
  const expandedRels: string[] = [];
  node.visit((n: WunderbaumNode) => {
    if (files.isFolder(n) && n.expanded) expandedRels.push(files.relPath(n));
  });
  const activeRel = files.relPath(files.rawTree()?.getActiveNode?.() || null) || null;
  // The teardown shrinks the list under the viewport, the browser clamps
  // `scrollTop` to the shorter list, and the re-activation in `restoreView`
  // scrolls to the selected row. Nothing else puts the offset back.
  const scrollTop = files.rawTree()?.element?.scrollTop ?? 0;
  return { expandedRels, activeRel, scrollTop };
}

// Shallow-first. Match by rel path (NOT findFolderByRel): a freshly
// reloaded folder has neither `folder` nor loaded `children` yet.
export async function reexpand(files: ReconcileFiles, expandedRels: string[]) {
  expandedRels.sort((a, b) => a.split("/").length - b.split("/").length);
  for (const r of expandedRels) {
    const f = files.rawTree()?.findFirst((n: WunderbaumNode) => files.relPath(n) === r);
    if (f && !f.expanded) await f.setExpanded(true);
  }
}

// `seq` is `_revealSeq` when the pass began: a reveal that landed since then
// keeps its row selected, and the snapshot is dropped.
async function restoreSelection(files: ReconcileFiles, seq: number, activeRel: string | null) {
  const target = (files._revealSeq || 0) > seq ? files._revealedRel : activeRel;
  if (target) await files.revealRel(target, { restore: true });
}

// The selection, the search marks and the scroll offset after a re-list.
export async function restoreView(files: ReconcileFiles, seq: number, view: LevelView) {
  await restoreSelection(files, seq, view.activeRel);
  // A search is on: the reloaded level has no match marks yet.
  const raw = files.rawTree();
  if (raw?.isFilterActive?.()) raw.updateFilter();
  // A reveal that landed mid-pass owns the scroll position.
  if (raw?.element && (files._revealSeq || 0) === seq) {
    // The list must have its full height first, or the offset is clamped.
    raw.updatePendingModifications?.();
    raw.element.scrollTop = view.scrollTop;
  }
}

// A promise that settles when the reconcile of `rel` in flight finishes.
function waitForLevel(waiters: Map<string, (() => void)[]>, rel: string) {
  return new Promise<void>((resolve) => {
    const list = waiters.get(rel) ?? [];
    list.push(resolve);
    waiters.set(rel, list);
  });
}

// Settle the callers waiting on `rel`. Drained by SPLICE: waiters arriving
// during a nested re-run land on THIS list and are drained here once.
function releaseWaiters(waiters: Map<string, (() => void)[]>, rel: string) {
  const list = waiters.get(rel);
  if (list?.length) for (const r of list.splice(0)) r();
}

// The reentrancy guard of a level's reconcile (`reconcileLevel` of wb-files.ts).
export async function reconcileLevel(
  files: PassFiles,
  node: WunderbaumNode,
  rel: string,
): Promise<void> {
  // Reentrancy guard: overlapping removeChildren()+load() passes double
  // the children. A pass in flight for `rel` re-runs once when it finishes.
  files._reconciling ||= new Set();
  files._reconcilePending ||= new Set();
  files._reconcileWaiters ||= new Map();
  // A COALESCED caller still gets a promise that settles when the level
  // does; otherwise a reveal awaiting it runs against children the pending
  // pass is about to tear down.
  if (files._reconciling.has(rel)) {
    files._reconcilePending.add(rel);
    return waitForLevel(files._reconcileWaiters, rel);
  }
  files._reconciling.add(rel);
  // The drain is in an OUTER `finally`: `WBDaemon.observe` REJECTS on a
  // socket drop, and every coalesced awaiter would otherwise hang forever.
  try {
    try {
      await files._reconcileOnce(node, rel);
    } finally {
      files._reconciling.delete(rel);
    }
    if (files._reconcilePending.delete(rel)) {
      const again = rel === "" ? files.rawTree()?.root : files.findFolderByRel(rel);
      if (again) await files.reconcileLevel(again, rel);
    }
  } finally {
    // A pass that threw never consumed its pending flag.
    files._reconcilePending.delete(rel);
    releaseWaiters(files._reconcileWaiters, rel);
  }
}

// One pass of a level's reconcile (`_reconcileOnce` of wb-files.ts).
export async function reconcileOnce(files: PassFiles, nodeAtCall: WunderbaumNode, rel: string) {
  // The selection restored is a SNAPSHOT; a reveal landing mid-pass must
  // not be undone by it. `_revealSeq` dates the snapshot.
  const seq = files._revealSeq || 0;

  // Resolve the fresh level BEFORE touching the tree: `node.load` given a
  // PROMISE appends (Wunderbaum quirk); a resolved ARRAY after
  // `removeChildren()` replaces cleanly. FRESH, never the cache.
  const source = await files.fetchTreeLevel(rel);
  const node = liveNode(files, nodeAtCall, rel);
  if (!node) return;
  const hasGitignore = source.some((n) => !n.folder && n.title === ".gitignore");
  // A write to a file already listed nudges its directory too. When the
  // rows on screen already match the fresh listing, the teardown would
  // change nothing but the operator's scroll position.
  if (levelShows(files, node, source)) {
    if (hasGitignore) files.revalidateBelow(node, rel);
    return;
  }
  const view = snapshotLevel(files, node);
  node.removeChildren();
  await node.load(source);
  // `load` leaves the reloaded node collapsed, and the NEXT nudge would
  // hit the `!expanded` drop guard.
  if (rel !== "" && !node.expanded) await node.setExpanded(true);
  // A level with a `.gitignore` may have changed the ignore marks of every
  // level below it, but the re-expansion below paints those from the
  // cache. Forgetting that they were validated makes `loadTreeLevel`
  // re-read each one in the background.
  if (hasGitignore) files.forgetValidatedBelow(rel);
  await reexpand(files, view.expandedRels);
  await restoreView(files, seq, view);
}
