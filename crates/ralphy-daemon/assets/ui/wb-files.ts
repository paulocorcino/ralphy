/* ---------------------------------------------------------------------------
   The FILES tree of the open project: an Alpine component on the one element
   after the project rows of the sidebar, shown for the open project.

   It owns the Wunderbaum tree and its `/ws/tree` socket, the level cache, the
   FILES search, the panel's states (loading, refused, stale, not live, the
   peer down), the tree's context menu, and the move and create gestures. A
   file opens through the shell's `openTab` and a note through `openNote`: the
   canvas tabs stay in `shell()` (ADR-0073 D2). The move picker is its own
   component (wb-move-dialog.ts).

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
import { classify, newEntryTitle, parentRel, underProtectedDir } from "./wb-file-paths.ts";

// The tree hosts that already carry the context-menu listener (`mountTree`).
// A raw DOM element, kept outside the component: Alpine's Proxy would wrap it.
const menuHosts = new WeakSet<Element>();

export function wbFiles() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["_flashAction", "checkoutOf", "fleetGroups", "readFleetNow", "wakePeer", "tabHidden", "hideMenu", "renderMenu", "openNote", "openTab", "repathTabs", "tabs", "hasWorktrees", "openCheckoutChip", "checkoutTitle", "branchError"], {
    // --- accordion --------------------------------------------------------

    // Wunderbaum copies source keys it does not define into `node.data`, so
    // `folder:true` lands at `node.data.folder` and `node.folder` is forever
    // `undefined`; `node.children` is `null` on a lazy or empty folder. Either
    // alone made EVERY collapsed folder answer "file".
    isFolder(node: any) {
      if (!node) return false;
      return !!(node.data?.folder || node.lazy || Array.isArray(node.children));
    },

    // --- file-type icons (Devicon font; folders use Wunderbaum defaults) ---
    fileIcon(title: any) {
      const name = title.toLowerCase();
      if (name.endsWith("lock") || name === "package-lock.json") return "devicon-json-plain colored";
      const ext = name.includes(".") ? name.split(".").pop() : "";
      const map: Record<string, string> = {
        ts: "devicon-typescript-plain colored",
        tsx: "devicon-typescript-plain colored",
        js: "devicon-javascript-plain colored",
        mjs: "devicon-javascript-plain colored",
        cjs: "devicon-javascript-plain colored",
        json: "devicon-json-plain colored",
        md: "devicon-markdown-plain md-glyph",
        rs: "devicon-rust-plain rs-glyph",
        css: "devicon-css3-plain colored",
        html: "devicon-html5-plain colored",
        prisma: "devicon-prisma-plain colored",
        png: "bi bi-image",
        jpg: "bi bi-image",
        jpeg: "bi bi-image",
        gif: "bi bi-image",
        svg: "bi bi-image",
        toml: "bi bi-gear",
        yml: "bi bi-gear",
        yaml: "bi bi-gear",
        note: "bi bi-sticky",
      };
      return map[ext] || "bi bi-file-earmark";
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
    mountTree() {
      const host = document.querySelector(".files-pane .wb-host");
      const project = this.$store.projects.projects.find((p) => this.$store.projects.repoRef(p) === this.$store.projects.openSlug);
      if (!host || !project) return;
      this.treeMem();
      // Freshness is per-open: the watch that kept a level honest died with
      // the last close.
      this._treeValidated.clear();
      // The spinner is armed only when the root has to come off the daemon; a
      // re-open paints from memory in the same frame.
      this.treeError = "";
      this.treeStale = "";
      this.treeNotLive = "";
      // The checkout this tree is built for (#406): cache key, every level
      // read and the watch carry it.
      this._treeCheckout = this.checkoutOf(this.$store.projects.openSlug);
      // A mount generation: a root read failing AFTER this tree was replaced
      // must not paint its error onto the fresh mount.
      const gen = (this._treeGen = (this._treeGen || 0) + 1);
      this.treeLoading = this.useDaemonTree() && !this._treeCache.has(this.treeKey(""));

      this._tree = new mar10.Wunderbaum({
        element: host,
        header: false,
        // The FILES search narrows the tree through this (`applyFileSearch`).
        // `autoApply` lets `updateFilter()` re-run after a level (re)loads;
        // without it a reloaded level vanishes.
        filter: { autoApply: true, mode: "hide" },
        // The root level from `tree.list`, folders `lazy`. A failed root read
        // says so and renders nothing.
        source: this.useDaemonTree()
          ? this.loadTreeLevel("").catch(() => {
              if (gen === this._treeGen) this.treeError = "Could not read the files of this project.";
              return [];
            })
          : [],
        // A failed level returns `false`: Wunderbaum then leaves the folder
        // unloaded, so the next expand reads it again, and draws no row for
        // the failure. A rethrow drew an "Error (…)" row inside the folder.
        // The footer says why; the folder closes, because it shows nothing.
        lazyLoad: (e: any) =>
          this.loadTreeLevel(this.relPath(e.node)).catch((err: any) => {
            this.treeWentStale(err);
            if (WBFleet.refDaemon(this.$store.projects.openSlug || "")) this.readFleetNow();
            setTimeout(() => e.node.setExpanded(false));
            return false;
          }),
        // While the fleet calls the open project's peer down, a folder with
        // no level read before does not open: its read would fail. A level
        // read before opens from memory. A restore of the expanded folders
        // stops here too.
        beforeExpand: (e: any) => {
          if (!e.flag || !this.openPeerDown() || e.node.children) return undefined;
          return this._treeCache.has(this.treeKey(this.relPath(e.node))) ? undefined : false;
        },
        // A level (re)loaded while a search is on carries no match marks, and
        // in `hide` mode an unmarked row is not painted.
        load: (e: any) => {
          if (e.tree.isFilterActive?.()) e.tree.updateFilter();
        },
        // The content-search badge: hit count beside the title; nothing once
        // the filter is gone (the map is empty by then).
        render: (e: any) => {
          const count = this._fileHits?.get(this.relPath(e.node));
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
        init: (e: any) => {
          if (gen !== this._treeGen) return;
          this.treeLoading = false;
          if (e.error) this.treeError = "Could not read the files of this project.";
          else this.restoreExpansion();
        },
        edit: {
          trigger: ["F2", "macEnter"],
          apply: (e: any) => {
            // The shared listener takes full rel paths.
            const parent = parentRel(this.relPath(e.node));
            this.emit("rename", e.node, {
              from: parent ? `${parent}/${e.oldValue}` : e.oldValue,
              to: parent ? `${parent}/${e.newValue}` : e.newValue,
            });
            return true; // let the tree reflect it optimistically
          },
        },
        // Live watch-set (#196): the daemon watches only the expanded set.
        expand: (e: any) => {
          if (!this.isFolder(e.node)) return;
          const rel = this.relPath(e.node);
          if (e.flag) this._treeSub?.watch(rel);
          else this._treeSub?.unwatch(rel);
          this.rememberExpansion();
        },
        // Double-click / Enter on a leaf = "open this file".
        dblclick: (e: any) => {
          if (!this.isFolder(e.node)) this.openFile(e.node);
          return false;
        },
      });

      // One `/ws/tree` subscription per open project; the root is always
      // watched. A `tree.dirty` push refetches only the affected subtree; a
      // `head.dirty` push re-reads the branch.
      if (this.useDaemonTree() && window.WBDaemon?.subscribeTree) {
        this._treeSub = WBDaemon.subscribeTree(
          this.$store.projects.openSlug,
          (rel: any) => {
            if (!this.tabHidden()) this.onTreeDirty(rel);
          },
          this._treeCheckout,
          () => {
            if (!this.tabHidden()) this.onHeadMoved();
          },
          (reason: any) => this.treeWatchFailed(reason),
        );
        this._treeSub.watch("");
      }

      // Right-click → our own context menu. Empty space below the rows is the
      // repo root: the only gesture that can create a top-level entry.
      // INVARIANT: one listener per host. The host outlives every mount
      // (`destroyTree` only empties it), so a second add would run the menu
      // once more per mount.
      if (!menuHosts.has(host)) {
        menuHosts.add(host);
        host.addEventListener("contextmenu", (ev: any) => {
          const node = mar10.Wunderbaum.getNode(ev);
          ev.preventDefault();
          node?.setActive();
          this.showMenu(ev.clientX, ev.clientY, node || null);
        });
      }
    },

    // Snapshot which folders are open, on every expand/collapse rather than
    // at close: a sidebar refresh or a peer going away is not a close we see.
    rememberExpansion() {
      // A restore expands nodes itself; recording a half-restored tree would
      // truncate the list being replayed.
      if (this._restoringExpansion || !this._tree || !this.$store.projects.openSlug) return;
      this.treeMem();
      const rels: any[] = [];
      this.rawTree().root.visit((n: any) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      this._treeExpanded.set(this.$store.projects.openSlug, rels);
    },

    // Re-expand the folders this project was left with, shallow-first (a
    // child cannot be found before its parent loads). Each level comes from
    // `_treeCache`; every expand still re-registers the daemon watch.
    async restoreExpansion() {
      const slug = this.$store.projects.openSlug;
      this.treeMem();
      const rels = this._treeExpanded.get(slug) || [];
      if (!rels.length || !this._tree) return;
      this._restoringExpansion = true;
      try {
        for (const rel of [...rels].sort((a, b) => a.split("/").length - b.split("/").length)) {
          // A project switch mid-replay: this list no longer describes the tree.
          if (slug !== this.$store.projects.openSlug || !this._tree) return;
          const node = this.rawTree().findFirst((n: any) => this.relPath(n) === rel);
          if (node && !node.expanded) await node.setExpanded(true);
        }
      } finally {
        this._restoringExpansion = false;
      }
    },

    // False only when the daemon client script is not loaded (a unit test).
    useDaemonTree() {
      return !!window.WBDaemon?.observe;
    },

    // One directory level from `tree.list`, folders lazy. Cache-FIRST
    // (stale-while-revalidate): a level read once is painted from memory and
    // re-read in the BACKGROUND, touching the DOM only when the directory
    // changed. Nothing is cached on the daemon (ADR-0036); the revalidation
    // is not optional, since the watch is dropped when the project closes.
    loadTreeLevel(rel: any) {
      this.treeMem();
      const key = this.treeKey(rel);
      const hit = this._treeCache.get(key);
      if (!hit) return this.fetchTreeLevel(rel);
      // Revalidate once per level per open, deferred so the paint happens first.
      if (!this._treeValidated.has(key)) {
        this._treeValidated.add(key);
        setTimeout(() => this.revalidateLevel(rel), 0);
      }
      return Promise.resolve(this.treeNodes(hit));
    },

    // Lazily create the three tree-memory collections (see the note there).
    treeMem() {
      this._treeCache ||= new Map();
      this._treeValidated ||= new Set();
      this._treeExpanded ||= new Map();
      // path → count (content) or `true` (name): what the filter predicate reads.
      this._fileHits ||= new Map();
    },

    // The un-cached read, always fresh: a reconcile served from the cache it
    // corrects would validate itself. INVARIANT: a read that FAILED throws; it
    // does NOT resolve `[]`, which is the real statement "no entries" every
    // caller acts on by replacing what is on screen.
    fetchTreeLevel(rel: any) {
      this.treeMem();
      const key = this.treeKey(rel);
      const payload = WBDaemon.withCheckout(
        { repo: this.$store.projects.openSlug, path: rel },
        this.checkoutOf(this.$store.projects.openSlug),
      );
      return WBDaemon.observe("tree.list", payload).then((reply) => {
        if (!reply || reply.status !== "ok" || !Array.isArray(reply.entries)) {
          throw new Error(WBFail.message(reply, ""));
        }
        this.treeFresh();
        this.pruneTreeCache(rel, reply.entries);
        this._treeCache.set(key, reply.entries);
        this._treeValidated.add(key);
        return this.treeNodes(reply.entries);
      });
    },

    // Evict every remembered level the fresh listing of `rel` CONTRADICTS: a
    // cache key is a path, not an identity, so a renamed-then-recreated
    // directory would inherit the old one's children. A name no longer among
    // the subdirectories cannot have children, so its remembered subtree goes.
    pruneTreeCache(rel: any, entries: any) {
      this.treeMem();
      const dirs = new Set(entries.filter((en: any) => en.dir).map((en: any) => en.name));
      // The keys DESCENDING from `rel`; its own key (empty `child`) is what
      // this listing replaces.
      const prefix = this.treeKey(rel === "" ? "" : `${rel}/`);
      for (const key of [...this._treeCache.keys()]) {
        if (!key.startsWith(prefix)) continue;
        const child = key.slice(prefix.length).split("/")[0];
        if (!child || dirs.has(child)) continue;
        this._treeCache.delete(key);
        this._treeValidated.delete(key);
      }
    },

    // Mark every cached level BELOW `rel` as not validated, so the next
    // `loadTreeLevel` of each paints from the cache and re-reads it.
    forgetValidatedBelow(rel: any) {
      this.treeMem();
      const prefix = this.treeKey(rel === "" ? "" : `${rel}/`);
      const own = this.treeKey(rel);
      for (const key of [...this._treeValidated]) {
        if (key !== own && key.startsWith(prefix)) this._treeValidated.delete(key);
      }
    },

    // Cache key, scoped by REPO and by CHECKOUT (#406).
    treeKey(rel: any) {
      return `${this.$store.projects.openSlug}\n${this.checkoutOf(this.$store.projects.openSlug) || ""}\n${rel}`;
    },

    // Daemon entries → fresh Wunderbaum node specs, rebuilt on every call: the
    // tree OWNS and mutates the objects it is given. An ignored entry carries
    // `wb-ignored`, set once when the row is created. An older peer sends no
    // `ignored`, and its rows are simply not dimmed.
    treeNodes(entries: any) {
      return entries.map((en: any) => {
        const node: any = en.dir
          ? { title: en.name, folder: true, lazy: true }
          : { title: en.name, icon: this.fileIcon(en.name) };
        if (en.ignored) node.classes = "wb-ignored";
        return node;
      });
    },

    // Re-read a level painted from cache and reconcile ONLY if the directory
    // changed: the common case costs one read and zero DOM work.
    revalidateLevel(rel: any) {
      if (!this._tree || !this.useDaemonTree()) return Promise.resolve();
      this.treeMem();
      const key = this.treeKey(rel);
      const before = JSON.stringify(this._treeCache.get(key) ?? null);
      const slug = this.$store.projects.openSlug;
      return this.fetchTreeLevel(rel)
        .then(() => {
          // A project switch in flight: another project's tree.
          if (slug !== this.$store.projects.openSlug || !this._tree) return;
          if (JSON.stringify(this._treeCache.get(key) ?? null) === before) return;
          const node = rel === "" ? this.rawTree().root : this.findFolderByRel(rel);
          if (node) return this.reconcileLevel(node, rel);
        })
        // A dropped read leaves the cached level on screen, but says so.
        .catch((err: any) => this.treeWentStale(err));
    },

    // The tree shows a listing it could not confirm: record the reason, leave
    // every row alone. Cleared by `treeFresh`.
    treeWentStale(err: any) {
      if (!this.useDaemonTree()) return;
      const failure = WBFail.failed({ message: err?.message }, "Could not refresh the file list: the daemon gave no reason.");
      this.treeStale = `${failure} The list shown is the last one read.`;
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
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      return group && !WBFleet.available(group) ? group : null;
    },
    peerDownText(g: any) {
      return `${WBFleet.peerName(g)} is not connected. The list shown is the last one read.`;
    },
    peerDownAction(g: any) {
      return WBFleet.wakeable(g) ? "Wake" : "Try again";
    },
    peerDownAct() {
      const g = this.openPeerDown();
      if (!g) return;
      if (WBFleet.wakeable(g)) this.wakePeer(g.daemon);
      else this.readFleetNow();
    },

    // After each good fleet read: the read that calls the open project's peer
    // back reads again the levels the tree shows.
    filesFollowFleet() {
      const down = !!this.openPeerDown();
      if (!down && this._filesPeerDown && this._filesPeerDown === this.$store.projects.openSlug && this._tree) {
        this.treeFresh();
        this.revalidateLevel("");
        this.rawTree().root.visit((n: any) => {
          if (this.isFolder(n) && n.expanded) this.revalidateLevel(this.relPath(n));
        });
      }
      this._filesPeerDown = down ? this.$store.projects.openSlug : null;
    },
    _filesPeerDown: null,

    // The daemon says it could not watch a dir of this tree (`reason`), or
    // `null` when a new socket holds every dir again.
    treeWatchFailed(reason: any) {
      this.treeNotLive = reason
        ? `The file list no longer updates by itself: ${reason}. Reopen the project to try again.`
        : "";
    },

    _treeSub: null as any, // the live `/ws/tree` subscription for the open project, if any
    // Tree memory, `null` until `treeMem` creates it:
    //   _treeCache     directory levels already shown, keyed `repo\nrel`.
    //                  Survives closing a project. Memory only.
    //   _treeValidated cached levels re-read against the disk during THIS open.
    //                  Cleared on every mount.
    //   _treeExpanded  folders expanded when a project was last closed, by repo.
    //   _fileHits      what the FILES search filter reads (`treeMem`).
    _treeCache: null as any,
    _treeValidated: null as any,
    _treeExpanded: null as any,
    _fileHits: null as any,
    // The checkout this tree was built for (#406): `mountTree` sets it.
    _treeCheckout: null as any,
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
    _tree: null as any, // the live Wunderbaum instance, if any

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

    setFileSearchMode(mode: any) {
      if (this.fileSearch.mode === mode) return Promise.resolve();
      this.fileSearch.mode = mode;
      this.$refs.fileSearch?.focus?.();
      return this.fileSearchNow();
    },

    _fileSearchTimer: null as any,
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

    // One Observe read, dated by `seq`: a reply that is not the newest, or
    // arrives after a project switch, is dropped.
    fileSearchNow() {
      clearTimeout(this._fileSearchTimer);
      const query = String(this.fileSearch.query ?? "").trim();
      if (!WBFileSearch.worthSearching(query)) {
        return this.clearFileSearch();
      }
      const seq = ++this.fileSearch.seq;
      if (!this.useDaemonTree()) {
        this.fileSearch.note = "search needs a daemon";
        return Promise.resolve();
      }
      const slug = this.$store.projects.openSlug;
      const verb = WBFileSearch.verbFor(this.fileSearch.mode);
      this.fileSearch.note = "searching…";
      // Find and grep walk the SELECTED tree (#406).
      const payload = window.WBDaemon.withCheckout({ repo: slug, query }, this.checkoutOf(slug));
      return window.WBDaemon.observe(verb, payload)
        .then((reply) => {
          if (seq !== this.fileSearch.seq || slug !== this.$store.projects.openSlug) return;
          if (WBFail.isError(reply) || !Array.isArray(reply?.hits)) {
            this.fileSearch.note = WBFail.failed(reply, "Could not search: the daemon gave no reason.");
            return;
          }
          return this.applyFileSearch(reply.hits, !!reply.truncated, seq);
        })
        .catch((err: any) => {
          if (seq !== this.fileSearch.seq) return;
          this.fileSearch.note = WBFail.failed({ message: err?.message }, "Could not search: the daemon did not answer.");
        });
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
      const rels: any[] = [];
      this.rawTree()?.root?.visit((n: any) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      return rels;
    },

    // Narrow the tree to `hits`: snapshot the expansion once per search
    // session, load every ancestor level (shallow-first), then filter. The
    // expands run under `_restoringExpansion`: the remembered expansion must
    // not learn them.
    async applyFileSearch(hits: any, truncated: any, seq: any) {
      this.treeMem();
      this.fileSearch.hits = hits;
      this.fileSearch.truncated = truncated;
      this.fileSearch.note = WBFileSearch.note({ hits, truncated });
      const tree = this.rawTree();
      if (!tree) return;
      if (this.fileSearch.expandedBefore === null) this.fileSearch.expandedBefore = this.expandedRels();
      // ONE paint, at the end: every lazy level repaints on its own, and one
      // landing between the old marks cleared and the new ones set left a
      // blank tree that nothing repainted (MEASURED 2026-09-15).
      this._restoringExpansion = true;
      tree.enableUpdate(false);
      try {
        for (const dir of WBFileSearch.dirsToLoad(hits)) {
          // A newer search, or a torn-down tree, owns the screen now.
          if (seq !== this.fileSearch.seq || tree !== this.rawTree()) return;
          const f = tree.findFirst((n: any) => this.relPath(n) === dir);
          if (f && this.isFolder(f) && !f.expanded) await f.setExpanded(true);
        }
        if (seq !== this.fileSearch.seq || tree !== this.rawTree()) return;
        this._fileHits = WBFileSearch.hitMap(hits);
        // `autoExpand: false`: the extension would also open every MATCHED
        // folder, a burst of lazy loads.
        tree.filterNodes((n: any) => this._fileHits.has(this.relPath(n)), {
          mode: "hide",
          autoExpand: false,
          matchBranch: false,
          noData: false,
        });
      } finally {
        this._restoringExpansion = false;
        // Set BEFORE the paint: the row window is computed from `scrollTop`,
        // and a leftover offset painted two rows at the bottom of a 52-row tree.
        if (tree === this.rawTree()) {
          tree.element.scrollTop = 0;
          // Re-enabling paints immediately and in full.
          tree.enableUpdate(true);
        }
      }
    },

    // Take the filter off and fold back what the search opened, deepest first.
    async clearFileSearch() {
      this.treeMem();
      this._fileHits = new Map();
      this.fileSearch.hits = [];
      this.fileSearch.truncated = false;
      this.fileSearch.note = "";
      const before = this.fileSearch.expandedBefore;
      this.fileSearch.expandedBefore = null;
      const tree = this.rawTree();
      if (!tree) return;
      // Same one-paint discipline as `applyFileSearch`.
      this._restoringExpansion = true;
      tree.enableUpdate(false);
      try {
        if (tree.isFilterActive?.()) tree.clearFilter();
        const fold = WBFileSearch.toCollapse(before, this.expandedRels());
        for (const rel of fold) {
          if (tree !== this.rawTree()) return;
          const f = tree.findFirst((n: any) => this.relPath(n) === rel);
          if (f && f.expanded) await f.setExpanded(false);
        }
      } finally {
        this._restoringExpansion = false;
        if (tree === this.rawTree()) {
          tree.element.scrollTop = 0;
          tree.enableUpdate(true);
        }
      }
    },

    // The search's memory of a tree that no longer exists (see `destroyTree`).
    resetFileSearch() {
      clearTimeout(this._fileSearchTimer);
      this.fileSearch.seq++;
      this.fileSearch.query = "";
      this.fileSearch.hits = [];
      this.fileSearch.truncated = false;
      this.fileSearch.note = "";
      this.fileSearch.expandedBefore = null;
      this._fileHits = new Map();
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
        window.dispatchEvent(new CustomEvent("workbench:head-moved", { detail: { ref } }));
      }, this.HEAD_SETTLE_MS);
    },
    HEAD_SETTLE_MS: 250,
    _headTimer: null as any,

    // A `tree.dirty` nudge for `rel`: refetch that level IF it is on screen. A
    // nudge for a collapsed/absent dir is DROPPED (ADR-0036 §4).
    onTreeDirty(rel: any) {
      const tree = this.rawTree();
      if (!tree) return;
      const node = rel === "" ? tree.root : this.findFolderByRel(rel);
      if (!node) return; // not in the tree → invisible, drop
      if (rel !== "" && !node.expanded) return; // collapsed → invisible, drop
      // Reconcile in place, then freshen the open tabs in this directory. A
      // reconcile failure leaves every row in place (`_reconcileOnce` resolves
      // the listing BEFORE touching the tree) and must still refresh viewers.
      return this.reconcileLevel(node, rel)
        .catch((err: any) => this.treeWentStale(err))
        .then((): any => this.refreshOpenViewers(rel));
    },

    // Re-list one level WITHOUT duplicating nodes, preserving descendant
    // expansion and the active selection. `node.load` appends, so
    // `removeChildren()` first, then re-expand and re-activate by captured rel.
    // The reconcile passes in flight, those to run again, and the callers
    // waiting on each, by rel: `null` until the first pass.
    _reconciling: null as any,
    _reconcilePending: null as any,
    _reconcileWaiters: null as any,
    async reconcileLevel(node: any, rel: any) {
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
        return new Promise((resolve) => {
          if (!this._reconcileWaiters.has(rel)) this._reconcileWaiters.set(rel, []);
          this._reconcileWaiters.get(rel).push(resolve);
        });
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
        // Drain by SPLICE: waiters arriving during the nested re-run land on
        // THIS list and are drained here once.
        const waiters = this._reconcileWaiters.get(rel);
        if (waiters?.length) for (const r of waiters.splice(0)) r();
      }
    },

    async _reconcileOnce(nodeAtCall: any, rel: any) {
      let node = nodeAtCall;
      // The selection restored is a SNAPSHOT; a reveal landing mid-pass must
      // not be undone by it. `_revealSeq` dates the snapshot.
      const seq = this._revealSeq || 0;

      // Resolve the fresh level BEFORE touching the tree: `node.load` given a
      // PROMISE appends (Wunderbaum quirk); a resolved ARRAY after
      // `removeChildren()` replaces cleanly. FRESH, never the cache.
      const source = await this.fetchTreeLevel(rel);
      // A reload of an ANCESTOR can land inside the fetch: its
      // `removeChildren()` unregisters this node (`node.tree` null) and a
      // teardown on the dead node throws inside Wunderbaum (MEASURED
      // 2026-09-16, Safari). Re-resolve by rel: none is a level that no longer
      // exists; one still loading is the ancestor's own re-expansion, and a
      // teardown under an in-flight load doubles the children.
      if (!node.tree) {
        const raw = this.rawTree();
        node = rel === "" ? raw?.root : raw?.findFirst((n: any) => this.relPath(n) === rel);
        if (!node || node.isLoading?.()) return;
      }
      const hasGitignore = source.some((n: any) => !n.folder && n.title === ".gitignore");
      // A write to a file already listed nudges its directory too. When the
      // rows on screen already match the fresh listing, the teardown would
      // change nothing but the operator's scroll position.
      if (this.levelShows(node, source)) {
        if (hasGitignore) this.revalidateBelow(node, rel);
        return;
      }
      // Read AFTER the fetch, right before the teardown: read before it, the
      // snapshot missed every folder a FILES search opened meanwhile.
      const expandedRels: any[] = [];
      node.visit((n: any) => {
        if (this.isFolder(n) && n.expanded) expandedRels.push(this.relPath(n));
      });
      const activeRel = this.relPath(this.rawTree()?.getActiveNode?.() || null) || null;
      // The teardown shrinks the list under the viewport, the browser clamps
      // `scrollTop` to the shorter list, and the re-activation below scrolls
      // to the selected row. Nothing else puts the offset back.
      const scrollTop = this.rawTree()?.element?.scrollTop ?? 0;
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

      // Shallow-first. Match by rel path (NOT findFolderByRel): a freshly
      // reloaded folder has neither `folder` nor loaded `children` yet.
      expandedRels.sort((a, b) => a.split("/").length - b.split("/").length);
      for (const r of expandedRels) {
        const f = this.rawTree()?.findFirst((n: any) => this.relPath(n) === r);
        if (f && !f.expanded) await f.setExpanded(true);
      }
      const target = (this._revealSeq || 0) > seq ? this._revealedRel : activeRel;
      if (target) await this.revealRel(target, { restore: true });
      // A search is on: the reloaded level has no match marks yet.
      const raw = this.rawTree();
      if (raw?.isFilterActive?.()) raw.updateFilter();
      // A reveal that landed mid-pass owns the scroll position.
      if (raw?.element && (this._revealSeq || 0) === seq) {
        // The list must have its full height first, or the offset is clamped.
        raw.updatePendingModifications?.();
        raw.element.scrollTop = scrollTop;
      }
    },

    // Whether the rows under `node` already show `specs` (from `treeNodes`):
    // the same names, kinds and ignore marks, in the same order. A level still
    // loading has a status row, so it never matches.
    levelShows(node: any, specs: any) {
      const rows = node.children;
      if (!Array.isArray(rows) || rows.length !== specs.length) return false;
      return specs.every((spec: any, i: any) => {
        const row = rows[i];
        return (
          row.title === spec.title &&
          this.isFolder(row) === !!spec.folder &&
          !!row.hasClass?.("wb-ignored") === (spec.classes === "wb-ignored")
        );
      });
    },

    // An unchanged level with a `.gitignore` may still have changed the ignore
    // marks of the levels below it: re-read each expanded one, which repaints
    // only a level whose listing changed.
    revalidateBelow(node: any, rel: any) {
      this.forgetValidatedBelow(rel);
      node.visit((n: any) => {
        if (this.isFolder(n) && n.expanded) this.revalidateLevel(this.relPath(n));
      });
    },

    // After a directory nudge, re-read any open tab whose file lives in `rel`
    // and push the bytes to its viewer. A failure keeps the tab's bytes.
    refreshOpenViewers(rel: any) {
      if (!this.useDaemonTree()) return Promise.resolve();
      const dirOf = (p: any) => {
        if (typeof p !== "string") return null;
        const i = p.lastIndexOf("/");
        return i < 0 ? "" : p.slice(0, i);
      };
      const reads: any[] = [];
      for (const t of this.tabs) {
        if (t.project !== this.$store.projects.openSlug || dirOf(t.path) !== rel) continue;
        // A tab pinned to another checkout (#406) is not this nudge's.
        if ((t.checkout ?? null) !== (this._treeCheckout ?? null)) continue;
        // An image tab re-reads through its OWN verb (ADR-0049 §1). A text
        // tab re-reads with ITS encoding as the hint: the encoding is sticky
        // once opened, so a nudge never silently re-detects it.
        const payload = WBDaemon.withCheckout({ repo: t.project, path: t.path }, t.checkout);
        const enc = WBViewer.encodingOf?.(t.id)?.encoding;
        if (enc) payload.encoding = enc;
        const fresh =
          t.kind === "image"
            ? WBDaemon.readImage(t.project, t.path, undefined, t.checkout)
            : WBDaemon.observe("file.read", payload).then((reply) =>
                reply?.status === "ok" ? reply.content : null,
              );
        reads.push(
          fresh
            .then((content: any) => {
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
    _revealedRel: null as any,
    async revealRel(rel: any, opts: any = {}) {
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
        const f = tree.findFirst((n: any) => this.relPath(n) === prefix);
        if (!f) return null; // an unmounted ancestor: nothing to reveal
        if (!f.expanded) await f.setExpanded(true);
      }
      const node = tree.findFirst((n: any) => this.relPath(n) === rel);
      if (!node) return null;
      node.setActive();
      return node;
    },

    // The folder node whose rel path is `rel`, or `null` if none is mounted.
    findFolderByRel(rel: any) {
      return this.rawTree()?.findFirst((n: any) => this.isFolder(n) && this.relPath(n) === rel) || null;
    },

    destroyTree() {
      try {
        this._treeSub?.close();
      } catch {}
      this._treeSub = null;
      this._treeCheckout = null;
      try {
        this._tree?.destroy?.();
      } catch {}
      this._tree = null;
      document.querySelectorAll(".wb-host").forEach((h) => (h.innerHTML = ""));
      // These states describe a tree that no longer exists.
      this.treeLoading = false;
      this.treeError = "";
      this.treeStale = "";
      this.treeNotLive = "";
      // A search describes THIS tree; the field stays open.
      this.resetFileSearch();
      this.hideMenu();
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
      hits: [] as any[],
      truncated: false,
      note: "",
      expandedBefore: null as string[] | null,
    },

    openFile(node: any) {
      const path = this.relPath(node);
      const ftype = classify(node.title);
      this.emit("open", node, { ftype });
      // A note opens as a CARD, not as a tab (ADR-0064 §11): on the stage if
      // it is not there yet, and by a jump if it is.
      if (ftype === "note") {
        this.openNote(path);
        return;
      }
      if (ftype === "binary") {
        // Flash it too: a click that silently does nothing reads as a broken
        // tree.
        window.WB.emit("open-refused", { project: this.$store.projects.openSlug, path, reason: "binary" });
        this._flashAction?.("Cannot open binary files.");
        return;
      }
      // Out of a CONTENT search: the tab lands on the first occurrence
      // (ADR-0036 amendment 2026-09-15).
      const find = this.fileSearchFindTerm();
      this.openTab({ project: this.$store.projects.openSlug, path, title: node.title, ftype, find });
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
      if (ref && this._treeCheckout !== this.checkoutOf(ref)) {
        this.destroyTree();
        this.mountTree();
      }
    },
    // On `workbench:peer-woken`: a row opened against the sleeping peer has an
    // empty tree: remount.
    filesFollowWake(daemon: any) {
      if (WBFleet.refDaemon(this.$store.projects.openSlug) === daemon) {
        this.destroyTree();
        this.mountTree();
      }
    },

    // `node` is null for empty tree space, which addresses the repo root: the
    // create items apply, the per-node items drop out.
    showMenu(x: any, y: any, node: any) {
      const isFolder = this.isFolder(node);
      const items = [
        node && !isFolder && { label: "Open", icon: "bi-box-arrow-up-right", run: () => this.openFile(node) },
        node && { label: "Rename…", icon: "bi-pencil", run: () => node.startEditTitle() },
        // FLAT rows, not a submenu, for a two-item choice made constantly.
        node && { label: "Copy full path", icon: "bi-clipboard", run: () => this.copyPath(node, true) },
        node && { label: "Copy relative path", icon: "bi-clipboard", run: () => this.copyPath(node, false) },
        node && !isFolder && { label: "Duplicate", icon: "bi-files", run: () => this.duplicateNode(node) },
        // Folders too. Dropped for a node inside `.git`/`.ralphy`, whose every
        // move the daemon refuses on the SOURCE.
        node && !underProtectedDir(this.relPath(node)) && {
          label: "Move to…",
          icon: "bi-arrow-right-square",
          run: () => this.moveNode(node),
        },
        node && { sep: true },
        // Creating targets the folder itself, or the folder CONTAINING the
        // clicked file.
        { label: "New file…", icon: "bi-file-earmark-plus", run: () => this.emitCreate(node, "file") },
        { label: "New folder…", icon: "bi-folder-plus", run: () => this.emitCreate(node, "folder") },
        node && { sep: true },
        node && { label: "Delete", icon: "bi-trash", danger: true, run: () => this.emit("delete", node) },
      ].filter(Boolean);
      this.renderMenu(x, y, items);
    },

    // --- the backend seam -------------------------------------------------
    // Build the repo-relative path by walking parent titles.
    relPath(node: any) {
      const parts: any[] = [];
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
    copyPath(node: any, full = false) {
      const rel = this.relPath(node);
      const root = full ? this.$store.projects.projects.find((p) => p.slug === this.$store.projects.openSlug)?.root : "";
      let path = rel;
      if (root) {
        // Windows by SHAPE (drive letter or UNC lead), not by "contains a
        // backslash".
        const sep = /^[A-Za-z]:/.test(root) || root.startsWith("\\\\") ? "\\" : "/";
        // Trim a trailing separator so a drive root (`C:\`) does not double it.
        const base = root.replace(/[\\/]+$/, "");
        path = base + sep + rel.split("/").join(sep);
      }
      navigator.clipboard?.writeText(path).catch(() => {});
      this.emit("copy-path", node, { path });
    },

    // Duplicate a file beside itself, NO prompt. `file.copy` refuses an
    // existing dst, so the free-name search happens HERE: the first of `<stem>
    // copy<ext>`, `<stem> copy 2<ext>`, … not taken.
    async duplicateNode(node: any) {
      const rel = this.relPath(node);
      if (!rel) return;
      const parent = parentRel(rel);
      const name = rel.slice(parent ? parent.length + 1 : 0);
      const dot = name.lastIndexOf(".");
      // A leading dot is the whole name of a dotfile, not an extension.
      const stem = dot > 0 ? name.slice(0, dot) : name;
      const ext = dot > 0 ? name.slice(dot) : "";

      // The tree's gestures speak the tree's checkout (#406); a refusal beats
      // a primary-aimed copy that silently misfires.
      const checkout = this.checkoutOf(this.$store.projects.openSlug);
      const listing = await WBDaemon.observe(
        "tree.list",
        WBDaemon.withCheckout({ repo: this.$store.projects.openSlug, path: parent }, checkout),
      ).catch(() => null);
      // A refused listing must NOT degrade to an empty `taken` set.
      if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
        this._flashAction?.("couldn't list the folder");
        return;
      }
      const taken = new Set(listing.entries.map((e) => e.name));
      let candidate = `${stem} copy${ext}`;
      for (let i = 2; taken.has(candidate); i++) candidate = `${stem} copy ${i}${ext}`;
      const to = parent ? `${parent}/${candidate}` : candidate;

      const reply = await WBDaemon.write(
        "file.copy",
        WBDaemon.withCheckout({ repo: this.$store.projects.openSlug, path: rel, to }, checkout),
      ).catch(() => null);
      if (!reply || WBFail.isError(reply)) {
        this._flashAction?.(
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
    moveNode(node: any) {
      const rel = this.relPath(node);
      if (!rel) return;
      window.dispatchEvent(new CustomEvent("workbench:move-open", { detail: { from: rel } }));
    },

    // Through `WBDaemon.write`, not the fire-and-forget `window.WB.emit("rename")`:
    // the reveal, the flash and the tab re-path need the reply. INVARIANT: no
    // tab is re-pathed and no reveal happens on a refusal.
    async performMove(from: any, to: any) {
      const reply = await WBDaemon.write(
        "file.rename",
        WBDaemon.withCheckout(
          { repo: this.$store.projects.openSlug, path: from, to },
          this.checkoutOf(this.$store.projects.openSlug),
        ),
      ).catch(() => null);
      if (!reply) {
        this._flashAction?.("Could not move: the daemon did not answer.");
        return;
      }
      if (WBFail.isError(reply)) {
        this._flashAction?.(WBFail.failed(reply, "Could not move: the daemon gave no reason."));
        return;
      }
      await this.onTreeDirty(parentRel(from));
      await this.onTreeDirty(parentRel(to));
      this.repathTabs(from, to);
      // LAST, so its `setActive()` is the final write.
      await this.revealRel(to);
    },

    // A `create` intent carries the DIRECTORY, already resolved (`createDir`).
    emitCreate(node: any, kind: any) {
      window.WB.emit("create", { project: this.$store.projects.openSlug, path: this.createDir(node), kind, isFolder: true });
    },

    // The directory a create addressed at `node` lands in: the folder itself,
    // the folder CONTAINING a file, or the repo root ("") for no node at all.
    createDir(node: any) {
      const rel = node ? this.relPath(node) : "";
      return !node || this.isFolder(node) ? rel : parentRel(rel);
    },

    // The Files-header buttons create relative to the tree's active node.
    createHere(kind: any) {
      this.emitCreate(this.rawTree()?.getActiveNode() || null, kind);
    },

    // The header buttons' tooltip: the directory a create lands in.
    createTitle(kind: any) {
      return newEntryTitle(kind, this.createDir(this.rawTree()?.getActiveNode() || null));
    },

    // Node-shaped gestures funnel through the shared WB.emit.
    emit(action: any, node: any, extra: any = {}) {
      window.WB.emit(action, {
        project: this.$store.projects.openSlug,
        path: this.relPath(node),
        title: node.title,
        isFolder: this.isFolder(node),
        ...extra,
      });
    },

    // The listeners, added once when Alpine builds the files.
    init() {
      window.addEventListener("workbench:project-changed", () => this.filesFollowProject());
      window.addEventListener("workbench:fleet-read", () => this.filesFollowFleet());
      window.addEventListener("workbench:peer-woken", (e: any) => this.filesFollowWake(e.detail.daemon));
      window.addEventListener("workbench:checkout-changed", () => this.filesFollowCheckout());
      // `resumeSockets`: the shell's heartbeat verdict, for this socket too.
      window.addEventListener("workbench:sockets-resume", (e: any) => this._treeSub?.resume?.(e.detail.stale));
      // The tab became visible, or a login: the socket sends what it missed.
      window.addEventListener("workbench:panels-reread", () => this._treeSub?.replay?.());
      // `/` and Ctrl/Cmd+Shift+F (`wire()` in app.ts).
      window.addEventListener("workbench:file-search-open", () => this.openFileSearch());
      window.addEventListener("workbench:move-confirmed", (e: any) => this.performMove(e.detail.from, e.detail.to));
      // A create or a delete of the write seam (`wire()` in app.ts): re-list the
      // level, THEN reveal, so `setActive()` is the last write. `revealRel`
      // expands the ancestors: a nudge for a COLLAPSED dir is dropped, so a
      // nudge alone would leave a new entry invisible.
      window.addEventListener("workbench:tree-dirty", async (e: any) => {
        await this.onTreeDirty(e.detail.rel);
        if (e.detail.reveal) await this.revealRel(e.detail.reveal);
      });
    },
  });
}
