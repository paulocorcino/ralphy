/* ---------------------------------------------------------------------------
   The FILES tree of the open project: an Alpine component on the one element
   after the project rows of the sidebar, shown for the open project.

   It owns the Wunderbaum tree and its `/ws/tree` socket, the level cache, the
   FILES search, the panel's states (loading, refused, stale, not live, the
   peer down), the tree's context menu, and the move and create gestures. A
   file opens through `openTab` of the Alpine store `tabs` and a note through
   its `openNote` (wb-tabs-store.ts). The move picker is its own
   component (wb-move-dialog.ts). The mount and the teardown of the tree are
   in wb-files-mount.ts, and the level cache is in wb-files-levels.ts.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4),
   and the shell reaches it only through `workbench:*` events, which `init()`
   hears. INVARIANT: `this` is Alpine's Proxy, so the tree is read and changed
   only through `rawTree()`.

   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { WBFileSearch } from "./wb-file-search.ts";
import { WBFleet } from "./wb-fleet.ts";
import { liveNode, reexpand, restoreView, snapshotLevel } from "./wb-files-reconcile.ts";
import { destroyTree, mountTree } from "./wb-files-mount.ts";
import { fetchTreeLevel, forgetValidatedBelow, levelShows, loadTreeLevel, pruneTreeCache, revalidateBelow, revalidateLevel } from "./wb-files-levels.ts";
import { classify, duplicateRel, fullPath, newEntryTitle, parentRel, treeIcon } from "./wb-file-paths.ts";
import { sendWindow } from "./wb-events.ts";
import { treeMenu } from "./wb-files-menu.ts";
import { applySearch, clearSearch, resetSearch, searchNow } from "./wb-files-search.ts";
import type { TreeSub } from "./wb-files-mount.ts";
import type { Hit } from "./wb-file-search.ts";
import type { Group } from "./wb-fleet.ts";

/** What a failed read hands back: a thrown error, or whatever a socket rejected with. */
type Failure = { message?: string } | null | undefined;

// A promise that settles when the reconcile of `rel` in flight finishes.
function waitForLevel(waiters: Map<string, (() => void)[]>, rel: string) {
  return new Promise<void>((resolve) => {
    if (!waiters.has(rel)) waiters.set(rel, []);
    waiters.get(rel)!.push(resolve);
  });
}

// Settle the callers waiting on `rel`. Drained by SPLICE: waiters arriving
// during a nested re-run land on THIS list and are drained here once.
function releaseWaiters(waiters: Map<string, (() => void)[]>, rel: string) {
  const list = waiters.get(rel);
  if (list?.length) for (const r of list.splice(0)) r();
}

export function wbFiles() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["checkoutChipClick"], {
    // --- accordion --------------------------------------------------------

    // Wunderbaum copies source keys it does not define into `node.data`, so
    // `folder:true` lands at `node.data.folder` and `node.folder` is forever
    // `undefined`; `node.children` is `null` on a lazy or empty folder. Either
    // alone made EVERY collapsed folder answer "file".
    isFolder(node: WunderbaumNode | null | undefined) {
      if (!node) return false;
      return !!(node.data?.folder || node.lazy || Array.isArray(node.children));
    },

    // --- file-type icons (Devicon font; folders use Wunderbaum defaults) ---
    fileIcon(title: string) {
      return treeIcon(title);
    },

    // --- Wunderbaum mount / teardown --------------------------------------
    // On `workbench:project-changed`: after the paint, the tree of the open
    // project is mounted.
    filesFollowProject() {
      this.$nextTick(() => {
        this.destroyTree();
        if (this.$store.projects.openSlug) this.mountTree();
      });
    },
    // The tree, its `/ws/tree` socket and its context menu (wb-files-mount.ts).
    mountTree() {
      mountTree(this);
    },

    // False only when the daemon client script is not loaded (a unit test).
    useDaemonTree() {
      return !!window.WBDaemon?.observe;
    },

    // One directory level, cache-first (wb-files-levels.ts).
    loadTreeLevel(rel: string): Promise<WunderbaumSource[]> {
      return loadTreeLevel(this, rel);
    },

    // Lazily create the three tree-memory collections (see the note there).
    treeMem() {
      this._treeCache ||= new Map();
      this._treeValidated ||= new Set();
      this._treeExpanded ||= new Map();
      // path → count (content) or `true` (name): what the filter predicate reads.
      this._fileHits ||= new Map();
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
      return `${this.$store.projects.openSlug}\n${this.$store.checkouts.checkoutOf(this.$store.projects.openSlug) || ""}\n${rel}`;
    },

    // Re-read a level painted from cache (wb-files-levels.ts).
    revalidateLevel(rel: string) {
      return revalidateLevel(this, rel);
    },

    // The tree shows a listing it could not confirm: record the reason, leave
    // every row alone. Cleared by `treeFresh`.
    treeWentStale(err: Failure) {
      if (!this.useDaemonTree()) return;
      this.treeStale = `${WBFail.failed({ message: err?.message }, "Could not refresh the file list: the daemon gave no reason.")} The list shown is the last one read.`;
    },

    // A read landed: whatever the tree is showing is confirmed again.
    treeFresh() {
      this.treeStale = "";
    },

    // The fleet group of the open project's peer while the fleet calls that
    // peer down, else null. FILES takes this fact from the fleet, its owner.
    openPeerDown() {
      const daemon = WBFleet.refDaemon(this.$store.projects.openSlug || "");
      if (!daemon) return null;
      const group = this.$store.projects.fleetGroups().find((g) => g.daemon === daemon);
      return group && !WBFleet.available(group) ? group : null;
    },
    peerDownText(g: Group) {
      return `${WBFleet.peerName(g)} is not connected. The list shown is the last one read.`;
    },
    peerDownAction(g: Group) {
      return WBFleet.wakeable(g) ? "Wake" : "Try again";
    },
    peerDownAct() {
      const g = this.openPeerDown();
      if (!g) return;
      if (WBFleet.wakeable(g)) this.$store.projects.wakePeer(g.daemon);
      else this.$store.projects.readFleetNow();
    },

    // After each good fleet read: the read that calls the open project's peer
    // back reads again the levels the tree shows.
    filesFollowFleet() {
      const down = !!this.openPeerDown();
      if (!down && this._filesPeerDown && this._filesPeerDown === this.$store.projects.openSlug && this._tree) {
        this.treeFresh();
        this.revalidateLevel("");
        this.rawTree()!.root.visit((n: WunderbaumNode) => {
          if (this.isFolder(n) && n.expanded) this.revalidateLevel(this.relPath(n));
        });
      }
      this._filesPeerDown = down ? this.$store.projects.openSlug : null;
    },
    _filesPeerDown: null as string | null,

    // The daemon says it could not watch a dir of this tree (`reason`), or
    // `null` when a new socket holds every dir again.
    treeWatchFailed(reason: string | null) {
      this.treeNotLive = reason
        ? `The file list no longer updates by itself: ${reason}. Reopen the project to try again.`
        : "";
    },

    _treeSub: null as TreeSub | null, // the live `/ws/tree` subscription for the open project, if any
    // Tree memory, `null` until `treeMem` creates it:
    //   _treeCache     directory levels already shown, keyed `repo\nrel`.
    //                  Survives closing a project. Memory only.
    //   _treeValidated cached levels re-read against the disk during THIS open.
    //                  Cleared on every mount.
    //   _treeExpanded  folders expanded when a project was last closed, by repo.
    //   _fileHits      what the FILES search filter reads (`treeMem`).
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
      if (this.fileSearch.open) this.closeFileSearch();
      else this.openFileSearch();
    },

    openFileSearch() {
      if (!this.$store.projects.openSlug) return;
      this.fileSearch.open = true;
      this.$nextTick(() => this.$refs.fileSearch?.focus?.());
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
      this.$refs.fileSearch?.focus?.();
      return this.clearFileSearch();
    },

    setFileSearchMode(mode: string) {
      if (this.fileSearch.mode === mode) return Promise.resolve();
      this.fileSearch.mode = mode;
      this.$refs.fileSearch?.focus?.();
      return this.fileSearchNow();
    },

    _fileSearchTimer: null as ReturnType<typeof setTimeout> | null,
    // A keystroke arms the debounce; a query under the floor clears instead.
    fileSearchTyped() {
      clearTimeout(this._fileSearchTimer);
      if (!WBFileSearch.worthSearching(this.fileSearch.query)) {
        this.fileSearch.seq++;
        this.clearFileSearch();
        return;
      }
      this._fileSearchTimer = setTimeout(() => this.fileSearchNow(), WBFileSearch.DEBOUNCE_MS);
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
      return t && window.Alpine?.raw ? window.Alpine.raw(t) : t;
    },

    // The rels of every expanded folder — the tree's current shape.
    expandedRels() {
      const rels: string[] = [];
      this.rawTree()?.root?.visit((n: WunderbaumNode) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      return rels;
    },

    // Narrow the tree to `hits` (wb-files-search.ts).
    async applyFileSearch(hits: Hit[], truncated: boolean, seq: number) {
      return applySearch(this, hits, truncated, seq);
    },

    // Take the filter off and fold back what the search opened.
    async clearFileSearch() {
      return clearSearch(this);
    },

    // The search's memory of a tree that no longer exists (see `destroyTree`).
    resetFileSearch() {
      return resetSearch(this);
    },

    // A `head.dirty` push: the open checkout's HEAD moved (a switch, a commit).
    // Git re-reads what the branch drives on `workbench:head-moved`. The gitdir
    // and its `logs/` push together for one move, so a short trailing timer
    // makes them one read.
    onHeadMoved() {
      clearTimeout(this._headTimer);
      this._headTimer = setTimeout(() => {
        const ref = this.$store.projects.openSlug;
        if (!ref) return;
        sendWindow(window, "workbench:head-moved", { ref });
      }, this.HEAD_SETTLE_MS);
    },
    HEAD_SETTLE_MS: 250,
    _headTimer: null as ReturnType<typeof setTimeout> | null,

    // A `tree.dirty` nudge for `rel`: refetch that level IF it is on screen. A
    // nudge for a collapsed/absent dir is DROPPED (ADR-0036 §4).
    onTreeDirty(rel: string) {
      const tree = this.rawTree();
      if (!tree) return;
      const node = rel === "" ? tree.root : this.findFolderByRel(rel);
      if (!node) return; // not in the tree → invisible, drop
      if (rel !== "" && !node.expanded) return; // collapsed → invisible, drop
      // Reconcile in place, then freshen the open tabs in this directory. A
      // reconcile failure leaves every row in place (`_reconcileOnce` resolves
      // the listing BEFORE touching the tree) and must still refresh viewers.
      return this.reconcileLevel(node, rel)
        .catch((err) => this.treeWentStale(err))
        .then(() => this.refreshOpenViewers(rel));
    },

    // Re-list one level WITHOUT duplicating nodes, preserving descendant
    // expansion and the active selection. `node.load` appends, so
    // `removeChildren()` first, then re-expand and re-activate by captured rel.
    // The reconcile passes in flight, those to run again, and the callers
    // waiting on each, by rel: `null` until the first pass.
    _reconciling: null as Set<string> | null,
    _reconcilePending: null as Set<string> | null,
    _reconcileWaiters: null as Map<string, (() => void)[]> | null,
    async reconcileLevel(node: WunderbaumNode, rel: string): Promise<void> {
      // Reentrancy guard: overlapping removeChildren()+load() passes double
      // the children. A pass in flight for `rel` re-runs once when it finishes.
      this._reconciling ||= new Set();
      this._reconcilePending ||= new Set();
      this._reconcileWaiters ||= new Map();
      // A COALESCED caller still gets a promise that settles when the level
      // does; otherwise a reveal awaiting it runs against children the pending
      // pass is about to tear down.
      if (this._reconciling.has(rel)) {
        this._reconcilePending.add(rel);
        return waitForLevel(this._reconcileWaiters, rel);
      }
      this._reconciling.add(rel);
      // The drain is in an OUTER `finally`: `WBDaemon.observe` REJECTS on a
      // socket drop, and every coalesced awaiter would otherwise hang forever.
      try {
        try {
          await this._reconcileOnce(node, rel);
        } finally {
          this._reconciling.delete(rel);
        }
        if (this._reconcilePending.delete(rel)) {
          const again = rel === "" ? this.rawTree()?.root : this.findFolderByRel(rel);
          if (again) await this.reconcileLevel(again, rel);
        }
      } finally {
        // A pass that threw never consumed its pending flag.
        this._reconcilePending.delete(rel);
        releaseWaiters(this._reconcileWaiters, rel);
      }
    },

    async _reconcileOnce(nodeAtCall: WunderbaumNode, rel: string) {
      // The selection restored is a SNAPSHOT; a reveal landing mid-pass must
      // not be undone by it. `_revealSeq` dates the snapshot.
      const seq = this._revealSeq || 0;

      // Resolve the fresh level BEFORE touching the tree: `node.load` given a
      // PROMISE appends (Wunderbaum quirk); a resolved ARRAY after
      // `removeChildren()` replaces cleanly. FRESH, never the cache.
      const source = await this.fetchTreeLevel(rel);
      const node = liveNode(this, nodeAtCall, rel);
      if (!node) return;
      const hasGitignore = source.some((n) => !n.folder && n.title === ".gitignore");
      // A write to a file already listed nudges its directory too. When the
      // rows on screen already match the fresh listing, the teardown would
      // change nothing but the operator's scroll position.
      if (levelShows(this, node, source)) {
        if (hasGitignore) this.revalidateBelow(node, rel);
        return;
      }
      const view = snapshotLevel(this, node);
      node.removeChildren();
      await node.load(source);
      // `load` leaves the reloaded node collapsed, and the NEXT nudge would
      // hit the `!expanded` drop guard.
      if (rel !== "" && !node.expanded) await node.setExpanded(true);
      // A level with a `.gitignore` may have changed the ignore marks of every
      // level below it, but the re-expansion below paints those from the
      // cache. Forgetting that they were validated makes `loadTreeLevel`
      // re-read each one in the background.
      if (hasGitignore) this.forgetValidatedBelow(rel);
      await reexpand(this, view.expandedRels);
      await restoreView(this, seq, view);
    },

    // Re-read the expanded levels below an unchanged `rel` (wb-files-levels.ts).
    revalidateBelow(node: WunderbaumNode, rel: string) {
      revalidateBelow(this, node, rel);
    },

    // After a directory nudge, re-read any open tab whose file lives in `rel`
    // and push the bytes to its viewer. A failure keeps the tab's bytes.
    refreshOpenViewers(rel: string): Promise<void | void[]> {
      if (!this.useDaemonTree()) return Promise.resolve();
      const dirOf = (p: string | undefined) => {
        if (typeof p !== "string") return null;
        const i = p.lastIndexOf("/");
        return i < 0 ? "" : p.slice(0, i);
      };
      const reads: Promise<void>[] = [];
      for (const t of this.$store.tabs.tabs) {
        if (t.project !== this.$store.projects.openSlug || dirOf(t.path) !== rel) continue;
        // A tab pinned to another checkout (#406) is not this nudge's.
        if ((t.checkout ?? null) !== (this._treeCheckout ?? null)) continue;
        // An image tab re-reads through its OWN verb (ADR-0049 §1). A text
        // tab re-reads with ITS encoding as the hint: the encoding is sticky
        // once opened, so a nudge never silently re-detects it.
        const payload = WBDaemon.withCheckout({ repo: t.project, path: t.path }, t.checkout);
        const enc = WBViewer.encodingOf?.(t.id)?.encoding;
        if (enc) payload.encoding = enc;
        const fresh: Promise<string | null | undefined> =
          t.kind === "image"
            ? WBDaemon.readImage(t.project, t.path!, undefined, t.checkout)
            : WBDaemon.observe("file.read", payload).then((reply) =>
                reply?.status === "ok" ? reply.content : null,
              );
        reads.push(
          fresh
            .then((content) => {
              if (content != null) WBViewer.externalChange(t.id, content);
            })
            .catch(() => {}),
        );
      }
      // The settled batch, so a caller can await a fully-refreshed set.
      return Promise.all(reads);
    },

    // Bring `rel` on screen and select it: expand every ancestor
    // shallow-first, then activate. The one reveal primitive. Matches by rel
    // path, NOT findFolderByRel (a freshly loaded folder carries neither
    // `folder` nor children yet). `opts.restore` is the reconcile path's own
    // re-activation: it must NOT bump `_revealSeq`, or a stale pass would date
    // its restore as newer than the reveal it undoes.
    _revealSeq: 0,
    _revealedRel: null as string | null,
    async revealRel(rel: string, opts: { restore?: boolean } = {}) {
      const tree = this.rawTree();
      if (!tree || typeof rel !== "string" || rel === "") return null;
      if (!opts.restore) {
        this._revealSeq = (this._revealSeq || 0) + 1;
        this._revealedRel = rel;
      }
      const parts = rel.split("/");
      // Ancestors only: a revealed FILE has nothing to expand.
      for (let i = 1; i < parts.length; i++) {
        const prefix = parts.slice(0, i).join("/");
        const f = tree.findFirst((n: WunderbaumNode) => this.relPath(n) === prefix);
        if (!f) return null; // an unmounted ancestor: nothing to reveal
        if (!f.expanded) await f.setExpanded(true);
      }
      const node = tree.findFirst((n: WunderbaumNode) => this.relPath(n) === rel);
      if (!node) return null;
      node.setActive();
      return node;
    },

    // The folder node whose rel path is `rel`, or `null` if none is mounted.
    findFolderByRel(rel: string) {
      return this.rawTree()?.findFirst((n: WunderbaumNode) => this.isFolder(n) && this.relPath(n) === rel) || null;
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

    openFile(node: WunderbaumNode) {
      const path = this.relPath(node);
      const ftype = classify(node.title);
      window.WB.emit("open", { ...this.nodeDetail(node), ftype });
      // A note opens as a CARD, not as a tab (ADR-0064 §11): on the stage if
      // it is not there yet, and by a jump if it is.
      if (ftype === "note") {
        this.$store.tabs.openNote(path);
        return;
      }
      // No tree is mounted without an open project.
      const slug = this.$store.projects.openSlug;
      if (!slug) return;
      if (ftype === "binary") {
        // Flash it too: a click that silently does nothing reads as a broken
        // tree.
        window.WB.emit("open-refused", { project: slug, path, reason: "binary" });
        this.$store.modals.flash?.("Cannot open binary files.");
        return;
      }
      // Out of a CONTENT search: the tab lands on the first occurrence
      // (ADR-0036 amendment 2026-09-15).
      const find = this.fileSearchFindTerm();
      this.$store.tabs.openTab({ project: slug, path, title: node.title, ftype, find });
    },

    // The term to land on: the live query, only while the CONTENT filter is on.
    fileSearchFindTerm() {
      const fs = this.fileSearch;
      if (!fs.open || fs.mode !== "content" || !fs.hits.length) return null;
      const q = String(fs.query ?? "").trim();
      return q || null;
    },

    // On `workbench:checkout-changed`: a tree built for another checkout is
    // remounted (cache key, watch and rows are per checkout).
    filesFollowCheckout() {
      const ref = this.$store.projects.openSlug;
      if (ref && this._treeCheckout !== this.$store.checkouts.checkoutOf(ref)) {
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

    // --- the backend seam -------------------------------------------------
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

    // `full` joins the project's absolute `root` onto the rel path in the
    // ROOT's own separator, so it pastes into a native shell; the rel path
    // when no root is known. `navigator.clipboard` is undefined on an
    // insecure non-loopback origin, so the call is optional-chained.
    copyPath(node: WunderbaumNode, full = false) {
      const rel = this.relPath(node);
      const root = full ? this.$store.projects.projects.find((p) => p.slug === this.$store.projects.openSlug)?.root : "";
      const path = root ? fullPath(root, rel) : rel;
      navigator.clipboard?.writeText(path).catch(() => {});
      window.WB.emit("copy-path", { ...this.nodeDetail(node), path });
    },

    // Duplicate a file beside itself, NO prompt. `file.copy` refuses an
    // existing dst, so the free name is found HERE (`duplicateRel`).
    async duplicateNode(node: WunderbaumNode) {
      const rel = this.relPath(node);
      if (!rel) return;
      const parent = parentRel(rel);

      // The tree's gestures speak the tree's checkout (#406); a refusal beats
      // a primary-aimed copy that silently misfires.
      const checkout = this.$store.checkouts.checkoutOf(this.$store.projects.openSlug);
      const listing = await WBDaemon.observe(
        "tree.list",
        WBDaemon.withCheckout({ repo: this.$store.projects.openSlug, path: parent }, checkout),
      ).catch(() => null);
      // A refused listing must NOT degrade to an empty `taken` set.
      if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
        this.$store.modals.flash?.("couldn't list the folder");
        return;
      }
      const to = duplicateRel(rel, new Set(listing.entries.map((e) => e.name)));

      const reply = await WBDaemon.write(
        "file.copy",
        WBDaemon.withCheckout({ repo: this.$store.projects.openSlug, path: rel, to }, checkout),
      ).catch(() => null);
      if (!reply || WBFail.isError(reply)) {
        this.$store.modals.flash?.(
          reply
            ? WBFail.failed(reply, "Could not duplicate the file: the daemon gave no reason.")
            : "Could not duplicate the file: the daemon did not answer.",
        );
        return;
      }
      await this.onTreeDirty(parent);
      await this.revealRel(to);
    },

    // The destination is PICKED, never typed: the move dialog
    // (wb-move-dialog.ts) browses real directories through `tree.list` and
    // answers with `workbench:move-confirmed`.
    moveNode(node: WunderbaumNode) {
      const rel = this.relPath(node);
      if (!rel) return;
      sendWindow(window, "workbench:move-open", { from: rel });
    },

    // Through `WBDaemon.write`, not the fire-and-forget `window.WB.emit("rename")`:
    // the reveal, the flash and the tab re-path need the reply. INVARIANT: no
    // tab is re-pathed and no reveal happens on a refusal.
    async performMove(from: string, to: string) {
      const reply = await WBDaemon.write(
        "file.rename",
        WBDaemon.withCheckout(
          { repo: this.$store.projects.openSlug, path: from, to },
          this.$store.checkouts.checkoutOf(this.$store.projects.openSlug),
        ),
      ).catch(() => null);
      if (!reply) {
        this.$store.modals.flash?.("Could not move: the daemon did not answer.");
        return;
      }
      if (WBFail.isError(reply)) {
        this.$store.modals.flash?.(WBFail.failed(reply, "Could not move: the daemon gave no reason."));
        return;
      }
      await this.onTreeDirty(parentRel(from));
      await this.onTreeDirty(parentRel(to));
      this.$store.tabs.repathTabs(from, to);
      // LAST, so its `setActive()` is the final write.
      await this.revealRel(to);
    },

    // A `create` intent carries the DIRECTORY, already resolved (`createDir`).
    emitCreate(node: WunderbaumNode | null, kind: "file" | "folder") {
      window.WB.emit("create", { project: this.$store.projects.openSlug, path: this.createDir(node), kind });
    },

    // The directory a create addressed at `node` lands in: the folder itself,
    // the folder CONTAINING a file, or the repo root ("") for no node at all.
    createDir(node: WunderbaumNode | null) {
      const rel = node ? this.relPath(node) : "";
      return !node || this.isFolder(node) ? rel : parentRel(rel);
    },

    // The Files-header buttons create relative to the tree's active node.
    createHere(kind: "file" | "folder") {
      this.emitCreate(this.rawTree()?.getActiveNode() || null, kind);
    },

    // The header buttons' tooltip: the directory a create lands in.
    createTitle(kind: string) {
      return newEntryTitle(kind, this.createDir(this.rawTree()?.getActiveNode() || null));
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

    // The listeners, added once when Alpine builds the files.
    init() {
      window.addEventListener("workbench:project-changed", () => this.filesFollowProject());
      window.addEventListener("workbench:fleet-read", () => this.filesFollowFleet());
      window.addEventListener("workbench:peer-woken", (e) => this.filesFollowWake(e.detail.daemon));
      window.addEventListener("workbench:checkout-changed", () => this.filesFollowCheckout());
      // `resumeSockets`: the shell's heartbeat verdict, for this socket too.
      window.addEventListener("workbench:sockets-resume", (e) => this._treeSub?.resume?.(e.detail.stale));
      // The tab became visible, or a login: the socket sends what it missed.
      window.addEventListener("workbench:panels-reread", () => this._treeSub?.replay?.());
      // `/` and Ctrl/Cmd+Shift+F (`wire()` in app.ts).
      window.addEventListener("workbench:file-search-open", () => this.openFileSearch());
      window.addEventListener("workbench:move-confirmed", (e) => this.performMove(e.detail.from, e.detail.to));
      // A create or a delete of the write seam (`wire()` in app.ts): re-list the
      // level, THEN reveal, so `setActive()` is the last write. `revealRel`
      // expands the ancestors: a nudge for a COLLAPSED dir is dropped, so a
      // nudge alone would leave a new entry invisible.
      window.addEventListener("workbench:tree-dirty", async (e) => {
        await this.onTreeDirty(e.detail.rel);
        if (e.detail.reveal) await this.revealRel(e.detail.reveal);
      });
    },
  });
}
