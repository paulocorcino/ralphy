/* ---------------------------------------------------------------------------
   The steps of one reconcile of a FILES tree level (`_reconcileOnce` in
   wb-files.ts): find the live node of the level, read what the operator sees
   before the teardown, open the folders again, and put the selection and the
   scroll offset back.

   Each step takes the files component as `files` and reads the tree only
   through `files.rawTree()`: `this` of the component is Alpine's Proxy, and a
   change made through the Proxy leaves rows at stale offsets.
   --------------------------------------------------------------------------- */

/** What a reconcile step reads from the files component. */
type ReconcileFiles = {
  rawTree(): WunderbaumTree | null;
  relPath(node: WunderbaumNode | null): string;
  isFolder(node: WunderbaumNode | null | undefined): boolean;
  revealRel(rel: string, opts?: { restore?: boolean }): Promise<unknown>;
  _revealSeq: number;
  _revealedRel: string | null;
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
  const live = rel === "" ? raw?.root : raw?.findFirst((n: WunderbaumNode) => files.relPath(n) === rel);
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
  // `scrollTop` to the shorter list, and the re-activation below scrolls
  // to the selected row. Nothing else puts the offset back.
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
