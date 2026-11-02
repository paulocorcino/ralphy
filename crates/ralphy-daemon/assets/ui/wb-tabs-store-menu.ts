/* ---------------------------------------------------------------------------
   The context menu of a canvas tab, for the Alpine store `tabs`
   (`showSlotMenu` and `renderMenu` of wb-tabs-store.ts): the entries that
   put a tab beside the active one, and the one `#ctxmenu` element that
   paints them.

   Each step takes the store as `store`, and calls only its own methods, so
   the store stays the one writer of its tabs.
   --------------------------------------------------------------------------- */
import type { CanvasTab, MenuItem, Slot } from "./wb-shell-types.d.ts";

/** What the tab menu reads or calls on the store `tabs`. */
type MenuTabs = {
  slot: Slot;
  splitAvailable(): boolean;
  clearSlot(): void;
  pinTab(id: string): void;
  toggleMirror(): void;
  activate(id: string): void;
  closeTab(id: string): void;
  renderMenu(x: number, y: number, items: MenuItem[]): void;
  hideMenu(): void;
};

// The slot's entries of the menu of tab `t`, and Close.
export function showSlotMenu(store: MenuTabs, x: number, y: number, t: CanvasTab) {
  const pinned = store.slot?.kind === "pin" && store.slot.id === t.id;
  const mirrored = store.slot?.kind === "mirror";
  const wide = store.splitAvailable();
  const narrowTitle = wide ? "" : "Needs a wider canvas";
  const items = [
    pinned
      ? { label: "Unpin", icon: "bi-pin-angle", run: () => store.clearSlot() }
      : {
          label: "Open to the side",
          icon: "bi-layout-split",
          disabled: !wide,
          title: narrowTitle,
          run: () => store.pinTab(t.id),
        },
    t.kind === "code" &&
      (mirrored
        ? { label: "Close mirror", icon: "bi-files", run: () => store.toggleMirror() }
        : {
            label: "Mirror editor",
            icon: "bi-files",
            disabled: !wide,
            title: narrowTitle,
            run: () => {
              store.activate(t.id);
              store.toggleMirror();
            },
          }),
    { sep: true as const },
    { label: "Close", icon: "bi-x-lg", run: () => store.closeTab(t.id) },
  ].filter(Boolean);
  store.renderMenu(x, y, items);
}

// Paint `items` into the one `#ctxmenu` and keep it on-screen. An item is
// `{ label, icon, run }` with optional `sep`, `danger`, `disabled`, `title`.
export function renderMenu(store: MenuTabs, x: number, y: number, items: MenuItem[]) {
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
      store.hideMenu();
      it.run();
    };
    menu.append(b);
  }
  menu.style.display = "block";
  const w = menu.offsetWidth,
    h = menu.offsetHeight;
  menu.style.left = Math.min(x, innerWidth - w - 8) + "px";
  menu.style.top = Math.min(y, innerHeight - h - 8) + "px";
}
