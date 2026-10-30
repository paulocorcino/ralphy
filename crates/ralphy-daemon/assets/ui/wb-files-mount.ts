/* ---------------------------------------------------------------------------
   The mount and the teardown of the FILES tree (`mountTree` and `destroyTree`
   of wb-files.ts): the Wunderbaum options, the `/ws/tree` subscription, the
   context-menu listener of the host, and the memory of the expanded folders
   that a mount puts back.

   Each step takes the files component as `files`. The tree is read and
   changed only through `files.rawTree()`: `this` of the component is Alpine's
   Proxy, and a change made through the Proxy leaves rows at stale offsets.
   --------------------------------------------------------------------------- */
import { WBFleet } from "./wb-fleet.ts";
import { WBReadRetry } from "./wb-read-retry.ts";
import { parentRel } from "./wb-file-paths.ts";
import { propertiesKey } from "./wb-files-properties.ts";
import { guardEmptySpace, treeDrop } from "./wb-files-upload.ts";
import type { AlpineMagics } from "./wb-alpine.ts";
import type { WBDaemonApi } from "./wb-daemon.ts";
import type { TreeMemory } from "./wb-files-levels.ts";

/** The live `/ws/tree` subscription of the open project. */
export type TreeSub = ReturnType<WBDaemonApi["subscribeTree"]>;

/** What a mount or a teardown reads, writes or calls on the files component. */
type MountFiles = {
  $store: AlpineMagics["$store"];
  _tree: WunderbaumTree | null;
  _treeSub: TreeSub | null;
  _treeCheckout: string | null;
  _treeGen: number;
  _treeCache: Map<string, TreeEntry[]> | null;
  _treeValidated: Set<string> | null;
  _treeExpanded: Map<string, string[]> | null;
  _fileHits: Map<string, number | true> | null;
  _restoringExpansion: boolean;
  treeLoading: boolean;
  treeError: string;
  treeStale: string;
  treeNotLive: string;
  rawTree(): WunderbaumTree | null;
  relPath(node: WunderbaumNode | null): string;
  isFolder(node: WunderbaumNode | null | undefined): boolean;
  nodeDetail(node: WunderbaumNode): WorkbenchNodeDetail;
  treeMem(): TreeMemory;
  treeKey(rel: string): string;
  useDaemonTree(): boolean;
  loadTreeLevel(rel: string): Promise<WunderbaumSource[]>;
  treeWentStale(err: unknown): void;
  openPeerDown(): unknown;
  openFile(node: WunderbaumNode): void;
  onTreeDirty(rel: string): unknown;
  onHeadMoved(): void;
  treeWatchFailed(reason: string | null): void;
  showMenu(x: number, y: number, node: WunderbaumNode | null): void;
  resetFileSearch(): void;
} & Parameters<typeof treeDrop>[0];

// The tree hosts that already carry the context-menu listener (`mountTree`).
// A raw DOM element, kept outside the component: Alpine's Proxy would wrap it.
const menuHosts = new WeakSet<Element>();

export function mountTree(files: MountFiles) {
  const host = document.querySelector<HTMLElement>(".files-pane .wb-host");
  const project = files.$store.projects.projects.find(
    (p) => files.$store.projects.repoRef(p) === files.$store.projects.openSlug,
  );
  if (!host || !project) return;
  const mem = files.treeMem();
  // Freshness is per-open: the watch that kept a level honest died with
  // the last close.
  mem.validated.clear();
  // The spinner is armed only when the root has to come off the daemon; a
  // re-open paints from memory in the same frame.
  files.treeError = "";
  files.treeStale = "";
  files.treeNotLive = "";
  // The checkout this tree is built for (#406): cache key, every level
  // read and the watch carry it.
  files._treeCheckout = files.$store.checkouts.checkoutOf(files.$store.projects.openSlug);
  // A mount generation: a root read failing AFTER this tree was replaced
  // must not paint its error onto the fresh mount.
  const gen = (files._treeGen = (files._treeGen || 0) + 1);
  files.treeLoading = files.useDaemonTree() && !mem.cache.has(files.treeKey(""));

  files._tree = new mar10.Wunderbaum({
    element: host,
    header: false,
    // The FILES search narrows the tree through this (`applyFileSearch`).
    // `autoApply` lets `updateFilter()` re-run after a level (re)loads;
    // without it a reloaded level vanishes.
    filter: { autoApply: true, mode: "hide" },
    // The root level from `tree.list`, folders `lazy`. A failed root read
    // says so and renders nothing.
    source: files.useDaemonTree()
      ? files.loadTreeLevel("").catch(() => {
          if (gen === files._treeGen) files.treeError = "Could not read the files of this project.";
          return [];
        })
      : [],
    // A failed level returns `false`: Wunderbaum then leaves the folder
    // unloaded, so the next expand reads it again, and draws no row for
    // the failure. A rethrow drew an "Error (…)" row inside the folder.
    // The footer says why; the folder closes, because it shows nothing.
    lazyLoad: (e: WunderbaumEvent) =>
      WBReadRetry.patiently(() => files.loadTreeLevel(files.relPath(e.node))).catch(
        (err: unknown) => {
          files.treeWentStale(err);
          if (WBFleet.refDaemon(files.$store.projects.openSlug || ""))
            void files.$store.projects.readFleetNow();
          setTimeout(() => e.node.setExpanded(false));
          return false;
        },
      ),
    // While the fleet calls the open project's peer down, a folder with
    // no level read before does not open: its read would fail. A level
    // read before opens from memory. A restore of the expanded folders
    // stops here too.
    beforeExpand: (e: WunderbaumExpandEvent) => {
      if (!e.flag || !files.openPeerDown() || e.node.children) return undefined;
      return mem.cache.has(files.treeKey(files.relPath(e.node))) ? undefined : false;
    },
    // A level (re)loaded while a search is on carries no match marks, and
    // in `hide` mode an unmarked row is not painted.
    load: (e: WunderbaumEvent) => {
      if (e.tree.isFilterActive?.()) e.tree.updateFilter();
    },
    // The content-search badge: hit count beside the title; nothing once
    // the filter is gone (the map is empty by then).
    render: (e: WunderbaumRenderEvent) => {
      const count = files._fileHits?.get(files.relPath(e.node));
      const old = e.nodeElem.querySelector(".wb-hits");
      if (typeof count !== "number") {
        old?.remove();
        return;
      }
      const badge = old || document.createElement("span");
      badge.className = "wb-hits";
      badge.textContent = String(count);
      if (!old) e.nodeElem.querySelector(".wb-title")?.after(badge);
    },
    // The root level has settled: put the expanded folders back.
    init: (e: WunderbaumInitEvent) => {
      if (gen !== files._treeGen) return;
      files.treeLoading = false;
      if (e.error) files.treeError = "Could not read the files of this project.";
      else void restoreExpansion(files);
    },
    edit: {
      trigger: ["F2", "macEnter"],
      apply: (e: WunderbaumEditEvent) => {
        // The shared listener takes full rel paths.
        const parent = parentRel(files.relPath(e.node));
        window.WB.emit("rename", {
          ...files.nodeDetail(e.node),
          from: parent ? `${parent}/${e.oldValue}` : e.oldValue,
          to: parent ? `${parent}/${e.newValue}` : e.newValue,
        });
        return true; // let the tree reflect it optimistically
      },
    },
    // Live watch-set (#196): the daemon watches only the expanded set.
    expand: (e: WunderbaumExpandEvent) => {
      if (!files.isFolder(e.node)) return;
      const rel = files.relPath(e.node);
      if (e.flag) files._treeSub?.watch(rel);
      else files._treeSub?.unwatch(rel);
      rememberExpansion(files);
    },
    // Double-click / Enter on a leaf = "open this file".
    dblclick: (e: WunderbaumEvent) => {
      if (!files.isFolder(e.node)) files.openFile(e.node);
      return false;
    },
    keydown: (e: WunderbaumKeyEvent) => propertiesKey(files, e),
    // Files dropped from the computer open the upload dialog.
    dnd: treeDrop(files),
  });

  // One `/ws/tree` subscription per open project; the root is always
  // watched. A `tree.dirty` push refetches only the affected subtree; a
  // `head.dirty` push re-reads the branch.
  if (files.useDaemonTree() && window.WBDaemon?.subscribeTree) {
    files._treeSub = window.WBDaemon.subscribeTree(
      files.$store.projects.repoRef(project),
      (rel: string) => {
        if (!files.$store.connection.tabHidden()) files.onTreeDirty(rel);
      },
      files._treeCheckout,
      () => {
        if (!files.$store.connection.tabHidden()) files.onHeadMoved();
      },
      (reason: string | null) => files.treeWatchFailed(reason),
    );
    files._treeSub.watch("");
  }

  // Right-click → our own context menu. Empty space below the rows is the
  // repo root: the only gesture that can create a top-level entry.
  // INVARIANT: one listener per host. The host outlives every mount
  // (`destroyTree` only empties it), so a second add would run the menu
  // once more per mount.
  if (!menuHosts.has(host)) {
    menuHosts.add(host);
    host.addEventListener("contextmenu", (ev: MouseEvent) => {
      const node = mar10.Wunderbaum.getNode(ev);
      ev.preventDefault();
      node?.setActive();
      files.showMenu(ev.clientX, ev.clientY, node || null);
    });
    guardEmptySpace(host, files);
  }
}

// Snapshot which folders are open, on every expand/collapse rather than
// at close: a sidebar refresh or a peer going away is not a close we see.
function rememberExpansion(files: MountFiles) {
  // A restore expands nodes itself; recording a half-restored tree would
  // truncate the list being replayed.
  if (files._restoringExpansion || !files._tree || !files.$store.projects.openSlug) return;
  const { expanded } = files.treeMem();
  const rels: string[] = [];
  files.rawTree()?.root.visit((n: WunderbaumNode) => {
    if (files.isFolder(n) && n.expanded) rels.push(files.relPath(n));
  });
  expanded.set(files.$store.projects.openSlug, rels);
}

// Re-expand the folders this project was left with, shallow-first (a
// child cannot be found before its parent loads). Each level comes from
// `_treeCache`; every expand still re-registers the daemon watch.
async function restoreExpansion(files: MountFiles) {
  const slug = files.$store.projects.openSlug;
  const { expanded } = files.treeMem();
  const rels = (slug && expanded.get(slug)) || [];
  const tree = files.rawTree();
  if (!rels.length || !tree) return;
  files._restoringExpansion = true;
  try {
    for (const rel of [...rels].sort((a, b) => a.split("/").length - b.split("/").length)) {
      // A project switch mid-replay: this list no longer describes the tree.
      if (slug !== files.$store.projects.openSlug || !files._tree) return;
      const node = tree.findFirst((n: WunderbaumNode) => files.relPath(n) === rel);
      if (node && !node.expanded) await node.setExpanded(true);
    }
  } finally {
    files._restoringExpansion = false;
  }
}

export function destroyTree(files: MountFiles) {
  try {
    files._treeSub?.close();
  } catch {}
  files._treeSub = null;
  files._treeCheckout = null;
  try {
    files._tree?.destroy?.();
  } catch {}
  files._tree = null;
  document.querySelectorAll(".wb-host").forEach((h) => (h.innerHTML = ""));
  // These states describe a tree that no longer exists.
  files.treeLoading = false;
  files.treeError = "";
  files.treeStale = "";
  files.treeNotLive = "";
  // A search describes THIS tree; the field stays open.
  files.resetFileSearch();
  files.$store.tabs.hideMenu();
}
