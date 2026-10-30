/* ---------------------------------------------------------------------------
   The canvas tabs, the slot beside the active tab, the split and the tab
   menu: the Alpine store `tabs` (#621; ADR-0073 D6 and the amendment of
   2026-10-11, which moved them out of `shell()`).

   The store is the one owner of these facts. Its fields change only through
   its own methods. `shell()` code reads `this.$store.tabs`, the markup reads
   `$store.tabs`, a component reads it through the `$store` magic, and code
   outside Alpine reads `Alpine.store("tabs")`.

   It reads no `shell()` member and calls no component; it reads the stores
   `projects`, `checkouts` and `modals`. It tells the viewer (`WBViewer`)
   which pane is on screen, and stores the open file tabs in the per-client
   view (`WBView`). Its `init()` hears `workbench:tab-open`: a component asks
   for a tab (the Spend tab). `activate` sends `workbench:spend-shown`.

   `detachFile` opens the popup through the detached file windows that `wire`
   (app.ts) keeps; `main.ts` passes them in `deps`. `main.ts` registers the
   store as `tabs`, after `release`. `tabsStore` builds it without Alpine, so
   `node --test` can test it (ADR-0073 D3, D6). Opening a Changes row into a
   diff tab is in wb-tabs-store-diff.ts.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { openDiff } from "./wb-tabs-store-diff.ts";
import { classify, fileTabId, tabIcon } from "./wb-file-paths.ts";
import { WBSplit } from "./wb-split.ts";
import { sendDocument, sendWindow } from "./wb-events.ts";
import { consoleTab, consoleTabId, freshTabId, isConsoleTabId } from "./wb-console-tabs.ts";
import { WBColumns } from "./wb-columns.ts";
import type { Messages } from "./wb-messages.ts";
import type { StoredTab } from "./wb-client-view.ts";
import type { CanvasTab, ChangeEntry, MenuItem, Slot, TabBody, TabOpen } from "./wb-shell-types.d.ts";

/** The detached file windows, as `wire` (app.ts) keeps them. */
export type DetachedFiles = { watch: (win: Window, desc: FileDescriptor) => void };

/** What `main.ts` gives the store. */
export type TabsStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
  /** The detached file windows: a detached file is watched until it comes home. */
  detached: DetachedFiles;
};

// A file tab of the open project at `from` or under it. A diff tab keeps its
// path: it shows a version, not the file.
function movedBy(t: CanvasTab, from: string, openSlug: string | null): t is CanvasTab & { path: string; project: string } {
  if (t.kind === "diff" || t.project !== openSlug) return false;
  return t.path === from || !!t.path?.startsWith(`${from}/`);
}

// The stored file tabs that can open again: each needs a project and a path.
function storedFileTabs(tabs: StoredTab[] | undefined) {
  return (tabs || []).filter((t) => !!t && !!t.project && !!t.path);
}

// The stored active tab while this store carries it, else Consoles.
function restoredActive(want: string | null | undefined, tabs: CanvasTab[]) {
  return want && tabs.some((t) => t.id === want) ? want : "consoles";
}

// `deps` is the shape every store takes; the tabs tell the operator through
// the store `modals`, as they did from `shell()`.
export function tabsStore(deps: TabsStoreDeps) {
  const { detached } = deps;
  return {
    // Alpine calls this once, when `main.ts` registers the store.
    init() {
      // A canvas tab that a component asks for (the Spend tab): added once,
      // then brought to the front.
      window.addEventListener("workbench:tab-open", (e) => {
        const tab = e.detail.tab;
        if (!this.tabs.some((t) => t.id === tab.id)) this.tabs.push(tab);
        this.activate(tab.id);
      });
      this.restoreConsoleTabs();
      // The console tabs are checked against the stage once the desk put
      // every console on it, and on every change of the stage after that.
      document.addEventListener("workbench:desk-restored", () => {
        this._stageRestored = true;
        this.syncConsoleTabs();
      });
      document.addEventListener("workbench:consoles-changed", () => {
        if (this._stageRestored) this.syncConsoleTabs();
      });
      // The shadow of a console in its own tab shows that tab.
      document.addEventListener("workbench:console-tab-show", (e) => {
        const tab = this.tabOf(e.detail.id);
        if (tab) this.activate(tab.id);
      });
      // A narrower or wider window changes how many consoles a tab paints
      // (one at a phone width). One placement per frame.
      let frame = 0;
      window.addEventListener("resize", () => {
        if (frame || !this.consoleTabShown()) return;
        frame = requestAnimationFrame(() => {
          frame = 0;
          this.placeConsoleTabs();
        });
      });
    },

    // --- console tabs (ADR-0037 §3d) ----------------------------------------
    // Whether the active tab shows the stage, or the pane of a console tab.
    stageShown() {
      return this.active === "consoles";
    },
    consoleTabShown() {
      return isConsoleTabId(this.active);
    },
    consoleTabs(): CanvasTab[] {
      return this.tabs.filter((t) => t.kind === "console");
    },
    // The console tab that holds the console `deskId`, if one does.
    tabOf(deskId: string): CanvasTab | undefined {
      return this.consoleTabs().find((t) => (t.grid || []).some((col) => col.includes(deskId)));
    },
    holds(deskId: string) {
      return !!this.tabOf(deskId);
    },
    consoleDeskIds(): string[] {
      return this.consoleTabs().flatMap((t) => WBColumns.flat(t.grid || []));
    },
    // The grid of the active console tab, or null.
    activeGrid(): string[][] | null {
      return this.consoleTabs().find((t) => t.id === this.active)?.grid ?? null;
    },
    // A console that has a tab shows that tab; any other gets a new one.
    openConsoleTab(deskId: string, title: string) {
      const held = this.tabOf(deskId);
      if (held) return this.activate(held.id);
      const id = freshTabId(deskId, (x) => this.tabs.some((t) => t.id === x));
      this.tabs.push(consoleTab(id, deskId, title, [[deskId]]));
      this.activate(id);
    },
    // The one writer of a console tab's grid. A tab with no console left
    // closes; its windows go back to the stage.
    setTabGrid(tabId: string, grid: string[][]) {
      const tab = this.consoleTabs().find((t) => t.id === tabId);
      if (!tab) return;
      if (!WBColumns.flat(grid).length) return this.closeTab(tabId);
      tab.grid = grid;
      this.persistView();
      this.placeConsoleTabs();
    },
    // Restore of a console in a tab: it goes back to the stage, and the tab
    // keeps the others.
    leaveTab(deskId: string) {
      const tab = this.tabOf(deskId);
      if (tab) this.setTabGrid(tab.id, WBColumns.restore(tab.grid || [], deskId).columns);
    },
    // Each console with a tab goes into the tab's pane, and every other one
    // goes back to the stage. The console module moves the windows.
    placeConsoleTabs() {
      window.WBConsole?.placeTabs(this.consoleDeskIds(), this.activeGrid());
    },
    // Read before the desk, so `restoreView` knows a stored active console
    // tab. The windows go into their tabs at `syncConsoleTabs`. A console
    // that two stored tabs name stays in the first.
    _consoleTabsRestored: false,
    _stageRestored: false,
    restoreConsoleTabs() {
      if (this._consoleTabsRestored || !window.WBView) return;
      this._consoleTabsRestored = true;
      for (const t of window.WBView.read()?.consoleTabs || []) {
        const id = t.id ?? consoleTabId(t.deskId);
        const seen = new Set(this.consoleDeskIds());
        const grid = t.grid.map((col) => col.filter((x) => !seen.has(x))).filter((col) => col.length);
        if (grid.length && !this.tabs.some((x) => x.id === id)) this.tabs.push(consoleTab(id, t.deskId, t.title, grid));
      }
    },
    // A console that left the page leaves its tab's grid, and a tab with no
    // console left closes. A console that relaunches keeps its place.
    syncConsoleTabs() {
      const rows = new Map(window.WBConsole.list().map((r) => [r.id, r]));
      const here = (id: string) => rows.has(id) || window.WBConsole.isRelaunching(id);
      for (const tab of this.consoleTabs()) {
        const grid = tab.grid || [];
        const kept = WBColumns.keep(grid, new Set(WBColumns.flat(grid).filter(here)));
        if (JSON.stringify(kept) !== JSON.stringify(grid)) this.setTabGrid(tab.id, kept);
        else if (tab.deskId && WBColumns.flat(grid).includes(tab.deskId)) this.renameConsoleTab(tab, rows.get(tab.deskId));
      }
      this.placeConsoleTabs();
    },
    // The tab follows a rename of the console it opened for, while that
    // console is in it; no other console names it (ADR-0037 §3d).
    renameConsoleTab(tab: CanvasTab, row: { name: string | null; agent: string } | undefined) {
      if (!row) return;
      const title = window.WBConsoleName.consoleLabel(row.name || "", row.agent);
      if (!title || tab.title === title) return;
      tab.title = title;
      this.persistView();
    },

    // --- canvas tabs ------------------------------------------------------
    // The SAME terminal glyph as the New-console button and its menu rows.
    tabs: [{ id: "consoles", kind: "consoles", title: "Consoles", icon: "bi bi-terminal", closable: false }] as CanvasTab[],
    active: "consoles",
    // The secondary pane (ADR-0037 §3c): `{ kind: "pin", id }` shows that tab
    // beside whichever is active, `{ kind: "mirror" }` shows the active code
    // tab twice. One slot, ever; `WBSplit.resolve` folds it against `active`
    // in `syncViewer`. `splitRatio` is the left column's share (null = half);
    // `lastLeft` the tab last read on the left, so activating the pinned tab
    // itself keeps its neighbour rather than emptying the canvas.
    slot: null as Slot,
    splitRatio: null as number | null,
    lastLeft: null as string | null,

    // Open a `.note` as a card (ADR-0064 §11). The module decides whether this
    // is a jump to a card already on the plane or a new one; this layer only
    // puts the operator on the tab that holds the stage.
    openNote(path: string, project?: string | null, checkout?: string | null) {
      const repo = project || window.Alpine.store("projects").openSlug;
      if (!repo) return;
      if (this.active !== "consoles") this.activate("consoles");
      const tree = checkout === undefined ? window.WBConsole.checkoutOf(repo) : checkout;
      void window.Alpine.nextTick(() =>
        window.WBNotes.openFromExplorer({
          repo,
          checkout: tree,
          path,
          viewport: {
            width: document.getElementById("workspace")?.clientWidth || 0,
            height: document.getElementById("workspace")?.clientHeight || 0,
          },
          offset: {
            left: document.getElementById("workspace")?.scrollLeft || 0,
            top: document.getElementById("workspace")?.scrollTop || 0,
          },
        }),
      );
    },

    // `file.read`, resolving to what the pane opens with: `{content, encoding,
    // bom}` (ADR-0036 amendment 2026-09-22), or `{refused: reason}` when the
    // daemon served nothing for a reason the pane can state (`binary`, `too
    // large`, …) — the tab KEEPS its place and says so — or `null` when there
    // is nothing to hold a tab open for (`not found`, a transport drop): the
    // tab closes. `checkout` is the tab's PINNED checkout (#406).
    fetchContent(project: string, path: string, ftype: string, checkout: string | null | undefined): Promise<TabBody> {
      const refuse = (reason: string) => {
        window.WB.emit("open-refused", { project, path, reason });
        window.Alpine.store("modals").flash?.(WBFail.failed({ reason }, "Could not open the file: the daemon gave no reason."));
        if (reason === "not found" || reason === "transport") {
          this.closeTab(fileTabId(project, path, checkout));
          return null;
        }
        return { refused: reason };
      };
      // An image is `file.image` (ADR-0049): a `data:` URL. Same refusal shape.
      if (ftype === "image") {
        let refusal: TabBody = null;
        return WBDaemon.readImage(project, path, (reason) => (refusal = refuse(reason)), checkout)
          .then((url) => (url == null ? refusal : { content: url }))
          .catch(() => refuse("transport"));
      }
      return WBDaemon.observe("file.read", WBDaemon.withCheckout({ repo: project, path }, checkout))
        .then((reply) => {
          if (reply.status === "ok") {
            return { content: reply.content, encoding: reply.encoding, bom: reply.bom };
          }
          return refuse(WBFail.message(reply, "refused"));
        })
        .catch(() => refuse("transport"));
    },

    // --- opening a file into a tab ----------------------------------------
    // A markdown link to another repo file: same viewer choice and binary
    // refusal as a tree click. `checkout` is the SOURCE pane's pin (#406): a
    // link in a worktree's document names that worktree's file.
    // `as: "bytes"` is the ONE caller that refuses the note routing below: a
    // `.note` whose bytes are not a container (ADR-0064 §11) must not become a
    // card, and classifying it again would be the loop the card just escaped.
    // The viewer serves text and images and refuses anything else, so what is
    // honest here is the refusal, named — not a pane that would show nothing.
    openLink({ project, path, fragment, checkout, as }: OpenRequest) {
      const title = path.split("/").pop()!;
      if (as === "bytes") {
        window.WB.emit("open-refused", { project, path, reason: "not a note" });
        window.Alpine.store("modals").flash?.(`${path} is not a note.`);
        return;
      }
      const ftype = classify(title);
      // A note linking to a note lands on the plane, exactly as a double-click
      // in the explorer does.
      if (ftype === "note") {
        this.openNote(path, project, checkout);
        return;
      }
      if (ftype === "binary") {
        window.WB.emit("open-refused", { project, path, reason: "binary" });
        window.Alpine.store("modals").flash?.("Cannot open binary files.");
        return;
      }
      this.openTab({ project: project!, path, title, ftype, fragment, checkout });
    },

    // `content`: a re-attached popup passes its (possibly edited) bytes back.
    // `fragment`/`find`: a `#heading` or search term to land on once the bytes
    // are shown. `checkout` PINS the tab to the tree it was opened in (#406):
    // a Save from a tab showing worktree bytes must never land on the primary.
    // `encoding`/`bom` come back with a re-attached popup's bytes, so the
    // reattached pane saves the way the detached one would have.
    openTab({ project, path, title, ftype, content, fragment, find, checkout, encoding, bom }: TabOpen) {
      const ck = checkout !== undefined ? checkout : window.Alpine.store("checkouts").checkoutOf(project);
      const id = fileTabId(project, path, ck);
      if (this.tabs.some((t) => t.id === id)) {
        this.activate(id);
        if (fragment) WBViewer.jumpTo(id, fragment);
        if (find) WBViewer.find(id, find);
        return;
      }
      this.tabs.push({ id, kind: ftype, title, path, project, icon: tabIcon(ftype), closable: true, checkout: ck });
      this.showTab(id);
      this.persistView();
      void window.Alpine.nextTick(() => {
        // A re-attach passes bytes in; a fresh open reads `file.read`.
        const bytes: Promise<TabBody> =
          content != null
            ? Promise.resolve({ content, encoding, bom })
            : this.fetchContent(project, path, ftype, ck);
        void bytes.then((body) => {
          if (body == null) return; // nothing to show: fetchContent closed the tab
          WBViewer.open({
            id,
            project,
            label: window.Alpine.store("projects").projectLabel(project),
            path,
            ftype,
            checkout: ck,
            ...body,
          });
          // NOT `setActive(id)`: `restoreView` opens N tabs in one burst and
          // THEN activates the stored one, so the last read to answer must not
          // own the screen.
          this.syncViewer();
          if (fragment) WBViewer.jumpTo(id, fragment);
          if (find) WBViewer.find(id, find);
        });
      });
    },

    // Re-point every open tab under the moved path: a tab's id IS its path,
    // and saves would write to the old location.
    repathTabs(from: string, to: string) {
      // Snapshot: the collision branch CLOSES a tab, which mutates `this.tabs`.
      for (const t of [...this.tabs]) {
        if (!movedBy(t, from, window.Alpine.store("projects").openSlug)) continue;
        const newPath = to + t.path.slice(from.length);
        const newId = fileTabId(t.project, newPath, t.checkout);
        if (this.tabs.some((x) => x.id === newId)) {
          // The destination is ALREADY open: a live editor on the old path
          // would recreate the file on its next save. Close it.
          this.closeTab(t.id);
          continue;
        }
        window.WBViewer?.repath(t.id, { id: newId, path: newPath });
        this.renameTabId(t.id, newId);
        t.path = newPath;
        t.id = newId;
      }
      // The viewer's own "what is shown" is keyed by id too.
      void window.Alpine.nextTick(() => this.syncViewer());
      this.persistView();
    },

    // The places that name a tab by id follow its new id: the active tab,
    // the pinned slot and the last tab on the left.
    renameTabId(oldId: string, newId: string) {
      if (this.active === oldId) this.showTab(newId);
      if (this.slot?.kind === "pin" && this.slot.id === oldId) this.slot = { kind: "pin", id: newId };
      if (this.lastLeft === oldId) this.lastLeft = newId;
    },

    // --- opening a Changes row into a diff tab ----------------------------
    // HEAD on one side, the working tree on the other (#311)
    // (wb-tabs-store-diff.ts).
    openDiff(project: string, entry: ChangeEntry) {
      openDiff(this, deps.messages, project, entry);
    },

    // Pop a file tab out into a standalone popup; the in-app tab closes.
    detachFile(desc: FileDescriptor) {
      const id = fileTabId(desc.project, desc.path, desc.checkout);
      // The descriptor is handed over by postMessage with `targetOrigin =
      // location.origin`, NOT in the URL hash: a hash let anyone render content
      // of their choosing on the daemon's origin. Passing the bytes keeps
      // unsaved edits alive across a detach.
      // `popup` is the daemon's route for the page (`Shell` in `assets.rs`).
      const win = window.open("popup", "_blank", "popup,width=920,height=760");
      if (!win) {
        window.WB.emit("detach-blocked", { project: desc.project, path: desc.path });
        return;
      }
      detached.watch(win, desc);
      window.WB.emit("detach", { project: desc.project, path: desc.path });
      this.closeTab(id);
      this.activate("consoles");
    },

    // Puts a tab on the left. Every change of `active` goes through here: the
    // consoles sleep while their tab is hidden and wake when it shows.
    showTab(id: string) {
      this.active = id;
      sendDocument(document, "workbench:tab-shown", { id });
    },

    activate(id: string) {
      this.showTab(id);
      // The Spend tab's subject can change while it sits in the background.
      if (id === "spend") sendWindow(window, "workbench:spend-shown");
      void window.Alpine.nextTick(() => {
        this.syncViewer();
        // A console opened while another tab was active measured 0×0.
        if (id === "consoles") window.WBConsole?.refitAll?.();
        this.placeConsoleTabs();
      });
      this.persistView();
    },

    // The ONE place that tells the viewer which pane is on screen — and which
    // sits beside it: an async opener calling it late can only converge.
    // Paneless tabs map to `null`; the slot waits in state while one is up.
    PANELESS_TABS: ["consoles", "spend"],
    syncViewer() {
      const r = WBSplit.resolve({
        active: this.active,
        slot: this.slot,
        tabs: this.tabs,
        lastLeft: this.lastLeft,
        width: WBViewer.width(),
        paneless: this.PANELESS_TABS.includes(this.active) || isConsoleTabId(this.active),
      });
      // Remembered where the decision is made, not in `activate`: `openTab`
      // and `openDiff` put a tab on the left without going through it.
      if (r.left && r.left !== this.slot?.id) this.lastLeft = r.left;
      WBViewer.setActive(
        r.left,
        r.right && { id: r.right, mirror: r.mirror, focus: r.focus === "right", ratio: this.splitRatio },
      );
    },

    // --- the slot: pin a tab beside the active one, or mirror the active one --
    pinTab(id: string) {
      this.slot = { kind: "pin", id };
      this.syncLater();
    },
    toggleMirror() {
      this.slot = this.slot?.kind === "mirror" ? null : { kind: "mirror" };
      this.syncLater();
    },
    clearSlot() {
      this.slot = null;
      this.syncLater();
    },
    syncLater() {
      void window.Alpine.nextTick(() => this.syncViewer());
      this.persistView();
    },
    // Whether the canvas is wide enough for two panes right now: the tab menu
    // greys its slot items below the floor rather than pinning into nothing.
    splitAvailable() {
      return WBSplit.available(WBViewer.width());
    },
    // The divider between the two panes was dragged (`wire`, app.ts): the
    // ratio is view state, and this store has one writer.
    splitRatioChanged(ratio: number | null) {
      this.splitRatio = ratio;
    },

    closeTab(id: string) {
      const idx = this.tabs.findIndex((t) => t.id === id);
      const tab = this.tabs[idx];
      if (!tab || !tab.closable) return; // Consoles never closes
      // A closed tab takes its pin with it (a mirror follows the active tab).
      this.slot = WBSplit.afterClose(this.slot, id);
      if (this.lastLeft === id) this.lastLeft = null;
      WBViewer.close(id);
      this.tabs.splice(idx, 1);
      // The consoles go back to the stage and keep running.
      if (tab.kind === "console") this.placeConsoleTabs();
      if (this.active === id) {
        // fall back to the neighbour, else the Consoles tab
        const next = this.tabs[idx] || this.tabs[idx - 1] || this.tabs[0];
        if (next) this.activate(next.id);
      } else {
        // A background close can still be the slot's pane: repaint.
        void window.Alpine.nextTick(() => this.syncViewer());
      }
      this.persistView();
    },

    // --- the per-client view: the open file tabs (issue #339) ----------------
    // The tabs half of `wb.view.v1` (`wb-console.ts` owns the offset half;
    // `patch` merges). Only `file:` tabs are stored: a `diff:` tab's sides are
    // LIVE git state.
    // Set while `restoreView` opens the stored tabs: `fetchContent` closes a
    // tab whose read fails, and those closes would otherwise REWRITE the store.
    _restoring: false,
    persistView() {
      if (this._restoring) return;
      const files = this.tabs
        .filter((t) => t.id.startsWith("file:"))
        .map((t) => ({
          project: t.project,
          path: t.path,
          title: t.title,
          kind: t.kind,
          // The pin (#406) survives a reload.
          checkout: t.checkout ?? null,
        }));
      const consoleTabs = this.consoleTabs().map((t) => ({ id: t.id, deskId: t.deskId || "", title: t.title, grid: t.grid || [] }));
      // A stored `active` naming a tab this store does not carry would leave
      // the canvas blank: degrade to Consoles.
      const alive =
        this.active === "consoles" ||
        consoleTabs.some((t) => t.id === this.active) ||
        files.some((f) => fileTabId(f.project!, f.path!, f.checkout) === this.active);
      window.WBView?.patch({
        tabs: files,
        consoleTabs,
        active: alive ? this.active : "consoles",
        // ALWAYS written, null included: `patch` is read-modify-write, and a
        // key left out would let a stale pin outlive the tab it named.
        split: WBSplit.toStored(this.slot, this.splitRatio, this.tabs),
      });
    },

    _viewRestored: false,
    // Latched, and AUTH-GATED by its callers: a pre-login `file.read` is
    // refused, `fetchContent` closes the tab, and that close persists the loss.
    restoreView() {
      if (this._viewRestored) return;
      this._viewRestored = true;
      this.restoreConsoleTabs();
      const stored = window.WBView?.read();
      if (!stored) return;
      this._restoring = true;
      try {
        for (const t of storedFileTabs(stored.tabs)) {
          this.openTab({
            project: t.project,
            path: t.path,
            title: t.title || t.path,
            ftype: t.kind,
            checkout: t.checkout ?? null,
          });
        }
        // After the tab entries exist (the panes land async; the viewer
        // paints single until they do) and before the activation that paints.
        this.slot = WBSplit.fromStored(stored.split, this.tabs);
        this.splitRatio = stored.split?.ratio ?? null;
        this.activate(restoredActive(stored.active, this.tabs));
      } finally {
        // The reads are async: hold the suppressor so a refusal landing later
        // cannot rewrite the store. A LATER close persists normally.
        setTimeout(() => {
          this._restoring = false;
        }, 3000);
      }
    },
    // --- context menu -----------------------------------------------------
    // A tab's menu (ADR-0037 §3c): the slot's two entry points and Close.
    // Consoles and Spend have no pane to put beside another, so no menu.
    showTabMenu(x: number, y: number, t: CanvasTab) {
      if (!t?.closable) return;
      // A console tab shows the stage, which has no pane to put beside another.
      if (t.kind === "console") this.renderMenu(x, y, [{ label: "Close", icon: "bi-x-lg", run: () => this.closeTab(t.id) }]);
      else this.showSlotMenu(x, y, t);
    },
    showSlotMenu(x: number, y: number, t: CanvasTab) {
      const pinned = this.slot?.kind === "pin" && this.slot.id === t.id;
      const mirrored = this.slot?.kind === "mirror";
      const wide = this.splitAvailable();
      const narrowTitle = wide ? "" : "Needs a wider canvas";
      const items = [
        pinned
          ? { label: "Unpin", icon: "bi-pin-angle", run: () => this.clearSlot() }
          : {
              label: "Open to the side",
              icon: "bi-layout-split",
              disabled: !wide,
              title: narrowTitle,
              run: () => this.pinTab(t.id),
            },
        t.kind === "code" &&
          (mirrored
            ? { label: "Close mirror", icon: "bi-files", run: () => this.toggleMirror() }
            : {
                label: "Mirror editor",
                icon: "bi-files",
                disabled: !wide,
                title: narrowTitle,
                run: () => {
                  this.activate(t.id);
                  this.toggleMirror();
                },
              }),
        { sep: true as const },
        { label: "Close", icon: "bi-x-lg", run: () => this.closeTab(t.id) },
      ].filter(Boolean);
      this.renderMenu(x, y, items);
    },

    // Paint `items` into the one `#ctxmenu` and keep it on-screen. An item is
    // `{ label, icon, run }` with optional `sep`, `danger`, `disabled`, `title`.
    renderMenu(x: number, y: number, items: MenuItem[]) {
      const menu = document.getElementById("ctxmenu") as HTMLElement;
      menu.innerHTML = "";
      for (const it of items) {
        if (it.sep) {
          const hr = document.createElement("div");
          hr.className = "ctx-sep";
          menu.append(hr);
          continue;
        }
        const b = document.createElement("button");
        b.className = "ctx-item" + (it.danger ? " danger" : "");
        // Built as elements: a label can name a file, and a file name is text.
        const icon = document.createElement("i");
        icon.className = "bi " + it.icon;
        const label = document.createElement("span");
        label.textContent = it.label;
        b.append(icon, label);
        if (it.disabled) b.disabled = true;
        if (it.title) b.title = it.title;
        b.onclick = () => {
          this.hideMenu();
          it.run();
        };
        menu.append(b);
      }
      menu.style.display = "block";
      const w = menu.offsetWidth,
        h = menu.offsetHeight;
      menu.style.left = Math.min(x, innerWidth - w - 8) + "px";
      menu.style.top = Math.min(y, innerHeight - h - 8) + "px";
    },

    hideMenu() {
      const menu = document.getElementById("ctxmenu") as HTMLElement;
      if (menu) menu.style.display = "none";
    },
  };
}

export type TabsStore = ReturnType<typeof tabsStore>;
