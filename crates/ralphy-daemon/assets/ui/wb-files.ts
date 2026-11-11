/* ---------------------------------------------------------------------------
   The FILES tree of the open project: an Alpine component on the one element
   after the project rows of the sidebar, shown for the open project.

   It owns the Wunderbaum tree and its `tree` channel of the page socket, the
   level cache, the FILES search, the panel's states (loading, refused, stale,
   not live, the peer down), the tree's context menu, and the move and create
   gestures. A file opens through `openTab` of the Alpine store `tabs` and a
   note through its `openNote` (wb-tabs-store.ts). The move picker is its own
   component (wb-move-dialog.ts). The steps that read the component as
   `files` are in the `wb-files-*.ts` modules.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4),
   and the shell reaches it only through `workbench:*` events, which `init()`
   hears. INVARIANT: `this` is Alpine's Proxy, so the tree is read and changed
   only through `rawTree()`.

   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { WBFleet } from "./wb-fleet.ts";
import { reconcileLevel, reconcileOnce } from "./wb-files-reconcile.ts";
import { destroyTree, mountTree } from "./wb-files-mount.ts";
import { headMoved, openViewersRefresh, treeDirty } from "./wb-files-nudges.ts";
import { followFleet, peerDownGroup, wakePeerDown } from "./wb-files-peer.ts";
import { reveal } from "./wb-files-reveal.ts";
import { moveEntry } from "./wb-files-move.ts";
import { copyEntryPath, openEntry, searchFindTerm } from "./wb-files-open.ts";
import {
  fetchTreeLevel,
  forgetValidatedBelow,
  loadTreeLevel,
  pruneTreeCache,
  revalidateBelow,
  revalidateLevel,
} from "./wb-files-levels.ts";
import { treeMemory } from "./wb-files-memory.ts";
import { newEntryTitle, parentRel, treeIcon } from "./wb-file-paths.ts";
import { sendWindow } from "./wb-events.ts";
import { treeMenu } from "./wb-files-menu.ts";
import { duplicateNode } from "./wb-files-duplicate.ts";
import { applySearch, clearSearch, resetSearch, searchNow } from "./wb-files-search.ts";
import { typedSearch } from "./wb-files-typing.ts";
import { listenFiles, logFailure } from "./wb-files-events.ts";
import type { TreeSub } from "./wb-files-mount.ts";
import type { TreeMemory } from "./wb-files-levels.ts";
import type { Hit } from "./wb-file-search.ts";
import type { Group } from "./wb-fleet.ts";

export function wbFiles() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["checkoutChipClick"], {
    // Wunderbaum copies source keys it does not define into `node.data`, so
    // `folder:true` lands at `node.data.folder` and `node.folder` is forever
    // `undefined`; `node.children` is `null` on a lazy or empty folder. Either
    // alone made EVERY collapsed folder answer "file".
    isFolder(node: WunderbaumNode | null | undefined) {
      if (!node) return false;
      return node.data.folder === true || node.lazy || Array.isArray(node.children);
    },

    // The Devicon class of a file; folders use Wunderbaum defaults.
    fileIcon(title: string) {
      return treeIcon(title);
    },

    // On `workbench:project-changed`: after the paint, the tree of the open
    // project is mounted.
    filesFollowProject() {
      void this.$nextTick(() => {
        this.destroyTree();
        if ((this.$store.projects.openSlug ?? "") !== "") this.mountTree();
      });
    },
    // The tree, its `tree` channel and its context menu (wb-files-mount.ts).
    mountTree() {
      mountTree(this);
    },

    // False only when the daemon client script is not loaded (a unit test).
    useDaemonTree() {
      return Boolean(window.WBDaemon.observe);
    },

    // One directory level, cache-first (wb-files-levels.ts).
    loadTreeLevel(rel: string): Promise<WunderbaumSource[]> {
      return loadTreeLevel(this, rel);
    },

    // Lazily create the three tree-memory collections (see the note there).
    treeMem(): TreeMemory {
      return treeMemory(this);
    },

    // One level, read fresh (wb-files-levels.ts).
    fetchTreeLevel(rel: string): Promise<WunderbaumSource[]> {
      return fetchTreeLevel(this, rel);
    },

    // The remembered levels that a fresh listing of `rel` contradicts go
    // (wb-files-levels.ts).
    pruneTreeCache(rel: string, entries: TreeEntry[]) {
      pruneTreeCache(this, rel, entries);
    },

    // The cached levels below `rel` are read again on their next load
    // (wb-files-levels.ts).
    forgetValidatedBelow(rel: string) {
      forgetValidatedBelow(this, rel);
    },

    // Cache key, scoped by REPO and by CHECKOUT (#406).
    treeKey(rel: string) {
      const slug = this.$store.projects.openSlug;
      return `${slug}\n${this.$store.checkouts.checkoutOf(slug) ?? ""}\n${rel}`;
    },

    // Re-read a level painted from cache (wb-files-levels.ts).
    revalidateLevel(rel: string) {
      return revalidateLevel(this, rel);
    },

    // The tree shows a listing it could not confirm: record the reason, leave
    // every row alone. Cleared by `treeFresh`.
    treeWentStale(err: unknown) {
      if (!this.useDaemonTree()) return;
      const why = WBFail.failed(
        { message: err instanceof Error ? err.message : undefined },
        "Could not refresh the file list: the daemon gave no reason.",
      );
      this.treeStale = `${why} The list shown is the last one read.`;
    },

    // A read landed: whatever the tree is showing is confirmed again.
    treeFresh() {
      this.treeStale = "";
    },

    // The fleet group of the open project's peer while the fleet calls that
    // peer down, else null (wb-files-peer.ts).
    openPeerDown() {
      return peerDownGroup(this);
    },
    peerDownText(g: Group) {
      return `${WBFleet.peerName(g)} is not connected. The list shown is the last one read.`;
    },
    peerDownAction(g: Group) {
      return WBFleet.wakeable(g) ? "Wake" : "Try again";
    },
    peerDownAct() {
      wakePeerDown(this);
    },

    // After each good fleet read: the read that calls the open project's peer
    // back reads again the levels the tree shows (wb-files-peer.ts).
    filesFollowFleet() {
      followFleet(this);
    },
    _filesPeerDown: null as string | null,

    // The daemon says it could not watch a dir of this tree (`reason`), or
    // `null` when a new socket holds every dir again.
    treeWatchFailed(reason: string | null) {
      this.treeNotLive =
        (reason ?? "") === ""
          ? ""
          : `The file list no longer updates by itself: ${reason}. ` +
            "Reopen the project to try again.";
    },

    _treeSub: null as TreeSub | null, // the `tree` channel of the open project, if any
    // Tree memory, `null` until `treeMem` creates it:
    //   _treeCache     levels already shown, keyed `repo\nrel`; memory only.
    //   _treeValidated cached levels re-read during THIS open; cleared on mount.
    //   _treeExpanded  folders expanded when a project was last closed, by repo.
    //   _fileHits      what the FILES search filter reads.
    _treeCache: null as Map<string, TreeEntry[]> | null,
    _treeValidated: null as Set<string> | null,
    _treeExpanded: null as Map<string, string[]> | null,
    _fileHits: null as Map<string, number | true> | null,
    // The checkout this tree was built for (#406): `mountTree` sets it.
    _treeCheckout: null as string | null,
    // The mount generation (`mountTree`).
    _treeGen: 0,
    // Set while a restore or a search expands folders itself.
    _restoringExpansion: false,
    // The FILES panel's states: "no files", "still looking" and "refused" must
    // not be one blank.
    treeLoading: false,
    treeError: "",
    // The tree shows a listing the daemon could not confirm (a refusal, a
    // dropped socket). Distinct from `treeError`: rows are on screen, but stale.
    treeStale: "",
    // The daemon could not keep watching this tree: the rows are right now,
    // but a change on disk will not show until the project is opened again.
    treeNotLive: "",
    _tree: null as WunderbaumTree | null, // the live Wunderbaum instance, if any

    // --- the FILES search (ADR-0036 amendment 2026-09-15) -----------------
    // The daemon answers with the hits' rel paths, every hit's ancestors are
    // loaded, and Wunderbaum's filter hides every other row. The decisions
    // are `WBFileSearch`'s.
    toggleFileSearch() {
      if (this.fileSearch.open) void this.closeFileSearch();
      else this.openFileSearch();
    },

    openFileSearch() {
      if ((this.$store.projects.openSlug ?? "") === "") return;
      this.fileSearch.open = true;
      void this.$nextTick(() => this.$refs["fileSearch"]?.focus());
    },

    // Escape, or the lupe again: field, query and filter all go.
    closeFileSearch() {
      this.fileSearch.open = false;
      this.fileSearch.query = "";
      this.fileSearch.seq++;
      return this.clearFileSearch();
    },

    // The clear button: the field stays open and focused.
    clearFileSearchQuery() {
      this.fileSearch.query = "";
      this.fileSearch.seq++;
      this.$refs["fileSearch"]?.focus();
      return this.clearFileSearch();
    },

    setFileSearchMode(mode: string) {
      if (this.fileSearch.mode === mode) return Promise.resolve();
      this.fileSearch.mode = mode;
      this.$refs["fileSearch"]?.focus();
      return this.fileSearchNow();
    },

    _fileSearchTimer: null as ReturnType<typeof setTimeout> | null,
    // A keystroke arms the debounce; a query under the floor clears instead.
    fileSearchTyped() {
      typedSearch(this);
    },

    // One Observe read, dated by `seq` (wb-files-search.ts).
    fileSearchNow() {
      return searchNow(this);
    },

    // The Wunderbaum instance WITHOUT Alpine's Proxy. Every node reached
    // through `this._tree` is a Proxy while the tree's own handlers hold raw
    // objects, and the row painter compares by identity: a mixed paint leaves
    // rows at stale offsets (MEASURED 2026-09-15/16). INVARIANT: everything
    // that reads or mutates the tree goes through this; `this._tree` is only
    // assigned, null-checked and destroyed.
    rawTree() {
      const t = this._tree;
      return t && typeof window.Alpine.raw === "function" ? window.Alpine.raw(t) : t;
    },

    // The rels of every expanded folder — the tree's current shape.
    expandedRels() {
      const rels: string[] = [];
      this.rawTree()?.root.visit((n: WunderbaumNode) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      return rels;
    },

    // Narrow the tree to `hits` (wb-files-search.ts).
    applyFileSearch(hits: Hit[], truncated: boolean, seq: number) {
      return applySearch(this, hits, truncated, seq);
    },

    // Take the filter off and fold back what the search opened.
    clearFileSearch() {
      return clearSearch(this).catch(logFailure("the file search did not clear"));
    },

    // The search's memory of a tree that no longer exists (see `destroyTree`).
    resetFileSearch() {
      resetSearch(this);
    },

    // A `head.dirty` push: the open checkout's HEAD moved (wb-files-nudges.ts).
    onHeadMoved() {
      headMoved(this);
    },
    HEAD_SETTLE_MS: 250,
    _headTimer: null as ReturnType<typeof setTimeout> | null,

    // A `tree.dirty` nudge for `rel`: refetch that level IF it is on screen
    // (wb-files-nudges.ts).
    onTreeDirty(rel: string) {
      return treeDirty(this, rel);
    },

    // Re-list one level WITHOUT duplicating nodes, preserving descendant
    // expansion and the active selection. `node.load` appends, so
    // `removeChildren()` first, then re-expand and re-activate by captured rel.
    // The reconcile passes in flight, those to run again, and the callers
    // waiting on each, by rel: `null` until the first pass.
    _reconciling: null as Set<string> | null,
    _reconcilePending: null as Set<string> | null,
    _reconcileWaiters: null as Map<string, (() => void)[]> | null,
    reconcileLevel(node: WunderbaumNode, rel: string): Promise<void> {
      return reconcileLevel(this, node, rel);
    },

    _reconcileOnce(nodeAtCall: WunderbaumNode, rel: string) {
      return reconcileOnce(this, nodeAtCall, rel);
    },

    // Re-read the expanded levels below an unchanged `rel` (wb-files-levels.ts).
    revalidateBelow(node: WunderbaumNode, rel: string) {
      revalidateBelow(this, node, rel);
    },

    // After a directory nudge, re-read any open tab whose file lives in `rel`
    // (wb-files-nudges.ts).
    refreshOpenViewers(rel: string): Promise<void | void[]> {
      return openViewersRefresh(this, rel);
    },

    // Bring `rel` on screen and select it: expand every ancestor
    // shallow-first, then activate. The one reveal primitive. Matches by rel
    // path, NOT findFolderByRel (a freshly loaded folder carries neither
    // `folder` nor children yet). `opts.restore` is the reconcile path's own
    // re-activation: it must NOT bump `_revealSeq`, or a stale pass would date
    // its restore as newer than the reveal it undoes.
    _revealSeq: 0,
    _revealedRel: null as string | null,
    revealRel(rel: string, opts: { restore?: boolean } = {}) {
      return reveal(this, rel, opts);
    },

    // The folder node whose rel path is `rel`, or `null` if none is mounted.
    findFolderByRel(rel: string) {
      return (
        this.rawTree()?.findFirst(
          (n: WunderbaumNode) => this.isFolder(n) && this.relPath(n) === rel,
        ) ?? null
      );
    },

    // The tree, its socket and every state that describes it go (wb-files-mount.ts).
    destroyTree() {
      destroyTree(this);
    },

    // The FILES search (ADR-0036 amendment 2026-09-15). `seq` dates each
    // request so a slow reply never paints over a newer one; `expandedBefore`
    // is the expansion snapshot `clearFileSearch` folds the tree back to
    // (`null` = nothing expanded yet).
    fileSearch: {
      open: false,
      mode: "name",
      query: "",
      seq: 0,
      hits: [] as Hit[],
      truncated: false,
      note: "",
      expandedBefore: null as string[] | null,
    },

    // A note opens as a card, any other file as a tab (wb-files-open.ts).
    openFile(node: WunderbaumNode) {
      openEntry(this, node);
    },

    // The term to land on: the live query, only while the CONTENT filter is on.
    fileSearchFindTerm() {
      return searchFindTerm(this);
    },

    // On `workbench:checkout-changed`: a tree built for another checkout is
    // remounted (cache key, watch and rows are per checkout).
    filesFollowCheckout() {
      const ref = this.$store.projects.openSlug ?? "";
      if (ref !== "" && this._treeCheckout !== this.$store.checkouts.checkoutOf(ref)) {
        this.destroyTree();
        this.mountTree();
      }
    },
    // On `workbench:peer-woken`: a row opened against the sleeping peer has an
    // empty tree: remount.
    filesFollowWake(daemon: string) {
      if (WBFleet.refDaemon(this.$store.projects.openSlug) === daemon) {
        this.destroyTree();
        this.mountTree();
      }
    },

    // The rows come from `treeMenu` (wb-files-menu.ts).
    showMenu(x: number, y: number, node: WunderbaumNode | null) {
      this.$store.tabs.renderMenu(x, y, treeMenu(this, node));
    },

    // Build the repo-relative path by walking parent titles.
    relPath(node: WunderbaumNode | null) {
      const parts: string[] = [];
      let n = node;
      while (n && n.title && n.parent) {
        parts.unshift(n.title);
        n = n.parent;
      }
      return parts.join("/");
    },

    // The rel path, or with `full` the absolute path (wb-files-open.ts).
    copyPath(node: WunderbaumNode, full = false) {
      copyEntryPath(this, node, full);
    },

    // Duplicate a file beside itself, NO prompt (wb-files-duplicate.ts).
    duplicateNode(node: WunderbaumNode) {
      return duplicateNode(this, node);
    },

    // The destination is PICKED, never typed: the move dialog
    // (wb-move-dialog.ts) browses real directories through `tree.list` and
    // answers with `workbench:move-confirmed`.
    moveNode(node: WunderbaumNode) {
      const rel = this.relPath(node);
      if (!rel) return;
      sendWindow(window, "workbench:move-open", { from: rel });
    },

    // The rename goes through the write seam (wb-files-move.ts).
    performMove(from: string, to: string) {
      return moveEntry(this, from, to);
    },

    // A `create` intent carries the DIRECTORY, already resolved (`createDir`).
    emitCreate(node: WunderbaumNode | null, kind: "file" | "folder") {
      window.WB.emit("create", {
        project: this.$store.projects.openSlug,
        path: this.createDir(node),
        kind,
      });
    },

    // The directory a create addressed at `node` lands in: the folder itself,
    // the folder CONTAINING a file, or the repo root ("") for no node at all.
    createDir(node: WunderbaumNode | null) {
      const rel = node ? this.relPath(node) : "";
      return !node || this.isFolder(node) ? rel : parentRel(rel);
    },

    // The Files-header buttons create relative to the tree's active node.
    createHere(kind: "file" | "folder") {
      this.emitCreate(this.rawTree()?.getActiveNode() ?? null, kind);
    },

    // The header buttons' tooltip: the directory a create lands in.
    createTitle(kind: string) {
      return newEntryTitle(kind, this.createDir(this.rawTree()?.getActiveNode() ?? null));
    },

    // The fields every gesture on one node sends.
    nodeDetail(node: WunderbaumNode): WorkbenchNodeDetail {
      return {
        project: this.$store.projects.openSlug,
        path: this.relPath(node),
        title: node.title,
        isFolder: this.isFolder(node),
      };
    },

    // The listeners, added once when Alpine builds the files (wb-files-events.ts).
    init() {
      listenFiles(this);
    },
  });
}
