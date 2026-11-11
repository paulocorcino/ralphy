/* ---------------------------------------------------------------------------
   The per-client view of the open tabs, for the Alpine store `tabs`
   (`persistView` and `restoreView` of wb-tabs-store.ts): the tabs half of
   `wb.view.v1` that is written after a change, and read once to open the
   stored file tabs again.

   Each step takes the store as `store`, and calls only its own methods, so
   the store stays the one writer of its tabs.
   --------------------------------------------------------------------------- */
import { fileTabId } from "./wb-file-paths.ts";
import { WBSplit } from "./wb-split.ts";
import type { StoredTab } from "./wb-client-view.ts";
import type { CanvasTab, Slot, TabOpen } from "./wb-shell-types.d.ts";

/** What the view reads, writes or calls on the store `tabs`. */
type ViewTabs = {
  tabs: CanvasTab[];
  active: string;
  slot: Slot;
  splitRatio: number | null;
  _restoring: boolean;
  _viewRestored: boolean;
  consoleTabs(): CanvasTab[];
  restoreConsoleTabs(): void;
  openTab(open: TabOpen): void;
  activate(id: string): void;
};

// The stored file tabs that can open again: each needs a project and a path.
// A test seeds tabs that miss a field, and null.
function storedFileTabs(tabs: readonly (Partial<StoredTab> | null)[] | undefined) {
  return (tabs ?? []).filter(
    (t): t is StoredTab => t !== null && (t.project ?? "") !== "" && (t.path ?? "") !== "",
  );
}

// The stored active tab while this store carries it, else Consoles.
function restoredActive(want: string | null | undefined, tabs: CanvasTab[]) {
  return want != null && want !== "" && tabs.some((t) => t.id === want) ? want : "consoles";
}

// The tabs half of `wb.view.v1` (`wb-console.ts` owns the offset half;
// `patch` merges). Only `file:` tabs are stored: a `diff:` tab's sides are
// LIVE git state.
export function persistView(store: ViewTabs) {
  if (store._restoring) return;
  const files = store.tabs
    .filter((t) => t.id.startsWith("file:"))
    .map((t) => ({
      project: t.project,
      path: t.path,
      title: t.title,
      kind: t.kind,
      // The pin (#406) survives a reload.
      checkout: t.checkout ?? null,
    }));
  const consoleTabs = store.consoleTabs().map((t) => ({
    id: t.id,
    deskId: t.deskId ?? "",
    title: t.title,
    grid: t.grid ?? [],
  }));
  // A stored `active` naming a tab this store does not carry would leave
  // the canvas blank: degrade to Consoles.
  const alive =
    store.active === "consoles" ||
    consoleTabs.some((t) => t.id === store.active) ||
    files.some(
      (f) =>
        f.project !== undefined &&
        f.path !== undefined &&
        fileTabId(f.project, f.path, f.checkout) === store.active,
    );
  window.WBView.patch({
    tabs: files,
    consoleTabs,
    active: alive ? store.active : "consoles",
    // ALWAYS written, null included: `patch` is read-modify-write, and a
    // key left out would let a stale pin outlive the tab it named.
    split: WBSplit.toStored(store.slot, store.splitRatio, store.tabs),
  });
}

// Latched, and AUTH-GATED by its callers: a pre-login `file.read` is
// refused, `fetchContent` closes the tab, and that close persists the loss.
export function restoreView(store: ViewTabs) {
  if (store._viewRestored) return;
  store._viewRestored = true;
  store.restoreConsoleTabs();
  const stored = window.WBView.read();
  if (!stored) return;
  // Set while the stored tabs open: `fetchContent` closes a tab whose read
  // fails, and those closes would otherwise REWRITE the store.
  store._restoring = true;
  try {
    for (const t of storedFileTabs(stored.tabs)) {
      store.openTab({
        project: t.project,
        path: t.path,
        title: (t.title ?? "") || t.path,
        ftype: t.kind,
        checkout: t.checkout ?? null,
      });
    }
    // After the tab entries exist (the panes land async; the viewer
    // paints single until they do) and before the activation that paints.
    store.slot = WBSplit.fromStored(stored.split, store.tabs);
    store.splitRatio = stored.split?.ratio ?? null;
    store.activate(restoredActive(stored.active, store.tabs));
  } finally {
    // The reads are async: hold the suppressor so a refusal landing later
    // cannot rewrite the store. A LATER close persists normally.
    setTimeout(() => {
      store._restoring = false;
    }, 3000);
  }
}
