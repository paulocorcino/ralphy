/* ---------------------------------------------------------------------------
   The context menu of a canvas tab, for the Alpine store `tabs`
   (`showSlotMenu` and `renderMenu` of wb-tabs-store.ts): the entries that
   put a tab beside the active one, the two Close entries every tab menu
   has, and the one `#ctxmenu` element that paints them.

   Each step takes the store as `store`, and calls only its own methods, so
   the store stays the one writer of its tabs.
   --------------------------------------------------------------------------- */
import { setMarkup } from "./wb-html.ts";
import type { CanvasTab, MenuItem, Slot } from "./wb-shell-types.d.ts";
import { required } from "./wb-dom-need.ts";

/** What the tab menu reads or calls on the store `tabs`. */
type MenuTabs = {
  tabs: CanvasTab[];
  active: string;
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
      ? {
          label: "Unpin",
          icon: "bi-pin-angle",
          run: () => {
            store.clearSlot();
          },
        }
      : {
          label: "Open to the side",
          icon: "bi-layout-split",
          disabled: !wide,
          title: narrowTitle,
          run: () => {
            store.pinTab(t.id);
          },
        },
    t.kind === "code" &&
      (mirrored
        ? {
            label: "Close mirror",
            icon: "bi-files",
            run: () => {
              store.toggleMirror();
            },
          }
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
    ...closeItems(store, t),
  ].filter(Boolean);
  store.renderMenu(x, y, items);
}

/** The tabs right of the tab `id` that may close (Consoles never does). */
export function tabsRightOf(tabs: readonly CanvasTab[], id: string) {
  const at = tabs.findIndex((t) => t.id === id);
  return at < 0 ? [] : tabs.slice(at + 1).filter((t) => t.closable);
}

/** Close each tab right of `t`. When the active tab is one of them, `t`
 * is shown first, so no tab that closes is shown on the way. */
export function closeTabsRightOf(store: MenuTabs, t: CanvasTab) {
  const right = tabsRightOf(store.tabs, t.id);
  if (right.some((r) => r.id === store.active)) store.activate(t.id);
  for (const r of right) store.closeTab(r.id);
}

/** The Close entries of the menu of tab `t`. */
export function closeItems(store: MenuTabs, t: CanvasTab): MenuItem[] {
  return [
    {
      label: "Close",
      icon: "bi-x-lg",
      run: () => {
        store.closeTab(t.id);
      },
    },
    {
      label: "Close to the right",
      icon: "bi-arrow-bar-right",
      disabled: tabsRightOf(store.tabs, t.id).length === 0,
      run: () => {
        closeTabsRightOf(store, t);
      },
    },
  ];
}

// Paint `items` into the one `#ctxmenu` and keep it on-screen. An item is
// `{ label, icon, run }` with optional `sep`, `danger`, `disabled`, `title`.
export function renderMenu(store: MenuTabs, x: number, y: number, items: MenuItem[]) {
  const menu = required(document.querySelector<HTMLElement>("#ctxmenu"), "the element #ctxmenu");
  setMarkup(menu, "");
  for (const it of items) {
    if (it.sep === true) {
      const hr = document.createElement("div");
      hr.className = "ctx-sep";
      menu.append(hr);
      continue;
    }
    const b = document.createElement("button");
    b.className = "ctx-item" + (it.danger === true ? " danger" : "");
    // Built as elements: a label can name a file, and a file name is text.
    const icon = document.createElement("i");
    icon.className = "bi " + it.icon;
    const label = document.createElement("span");
    label.textContent = it.label;
    b.append(icon, label);
    if (it.disabled === true) b.disabled = true;
    if (it.title !== undefined && it.title !== "") b.title = it.title;
    b.addEventListener("click", () => {
      store.hideMenu();
      it.run();
    });
    menu.append(b);
  }
  menu.style.display = "block";
  const w = menu.offsetWidth,
    h = menu.offsetHeight;
  menu.style.left = Math.min(x, innerWidth - w - 8) + "px";
  menu.style.top = Math.min(y, innerHeight - h - 8) + "px";
}
