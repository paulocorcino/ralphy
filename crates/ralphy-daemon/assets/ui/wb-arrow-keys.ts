/* ---------------------------------------------------------------------------
   Arrow keys in a menu and in a tab list, and Escape in a menu
   (docs/design-system/keyboard.md).
   Up and Down walk the enabled rows of a menu, Left and Right the tabs of a
   tab list, Home and End go to either end. One listener on the document
   serves every menu and tab list, the ones built later too.
--------------------------------------------------------------------------- */

import { sendWindow } from "./wb-events.ts";

/** A list the arrow keys walk: where it is, what it holds, which keys. */
type ArrowList = { container: string; items: string; back: string; next: string };

const MENU_ROWS = [
  ".menu-item",
  ".ctx-item",
  ".dropdown-item",
  ".window-item",
  ".fence-item",
  ".note-item",
  ".column-item",
  ".console-tab-item",
].join(", ");

/** Every menu: the dropdowns and the context menu. */
const MENUS = [".dropdown", ".ctxmenu"].join(", ");

export const ARROW_LISTS: readonly ArrowList[] = [
  { container: MENUS, items: MENU_ROWS, back: "ArrowUp", next: "ArrowDown" },
  { container: '[role="tablist"]', items: '[role="tab"]', back: "ArrowLeft", next: "ArrowRight" },
];

/** The index the focus moves to, or -1 when the key does not move it. */
export function arrowTarget(count: number, current: number, key: string, list: ArrowList) {
  if (count === 0) return -1;
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  if (key === list.next) return (current + 1) % count;
  if (key === list.back) return (current - 1 + count) % count;
  return -1;
}

// The rows a key can reach: shown and not disabled, in document order.
function reachable(container: Element, selector: string) {
  return Array.from(container.querySelectorAll<HTMLElement>(selector)).filter(
    (el) => !el.matches(":disabled") && el.getClientRects().length > 0,
  );
}

/** Move the focus for one keydown; `true` when the key was used. */
export function arrowStep(e: KeyboardEvent, active: Element | null) {
  if (!active || e.altKey || e.ctrlKey || e.metaKey) return false;
  // A field inside a menu keeps its keys: the arrows move its caret.
  if (active.matches("input, textarea, select, [contenteditable]")) return false;
  for (const list of ARROW_LISTS) {
    const container = active.closest(list.container);
    if (!container) continue;
    const items = reachable(container, list.items);
    const at = arrowTarget(
      items.length,
      items.findIndex((el) => el === active),
      e.key,
      list,
    );
    const target = items[at];
    if (!target) return false;
    target.focus();
    return true;
  }
  return false;
}

/** What Escape reads of the page: the menus on screen and the focus. */
export type MenuPage = {
  menus: () => Element[];
  active: () => Element | null;
  // Closes every menu: each one hears `workbench:menus-close`, and the
  // context menu is hidden.
  closeAll: () => void;
};

/**
 * Escape closes the open menus and gives the focus back to the element that
 * had it before the menu took it; `true` when the key was used. A field
 * inside a menu keeps its own Escape.
 */
export function menuEscape(
  e: Pick<KeyboardEvent, "key">,
  page: MenuPage,
  opener: HTMLElement | null,
) {
  if (e.key !== "Escape") return false;
  const open = page.menus().filter((m) => m.getClientRects().length > 0);
  if (open.length === 0) return false;
  const active = page.active();
  if (active?.matches("input, textarea") === true && active.closest(MENUS)) return false;
  page.closeAll();
  if (open.some((m) => m.contains(active))) opener?.focus();
  return true;
}

function menuPage(doc: Document): MenuPage {
  return {
    menus: () => Array.from(doc.querySelectorAll(MENUS)),
    active: () => doc.activeElement,
    closeAll: () => {
      if (doc.defaultView) sendWindow(doc.defaultView, "workbench:menus-close");
      const ctx = doc.querySelector<HTMLElement>("#ctxmenu");
      if (ctx) ctx.style.display = "none";
    },
  };
}

/** The one document listener of the menu and tab list keys. */
export function wireArrowKeys(doc: Document) {
  doc.addEventListener("keydown", (e) => {
    if (arrowStep(e, doc.activeElement)) e.preventDefault();
  });
  // The element the focus came from when it entered a menu.
  let opener: HTMLElement | null = null;
  doc.addEventListener("focusin", (e) => {
    const into = e.target instanceof Element && e.target.closest(MENUS) !== null;
    const from = e.relatedTarget instanceof HTMLElement ? e.relatedTarget : null;
    if (into && from?.closest(MENUS) === null) opener = from;
  });
  // CAPTURE, and stopped: a terminal would send the Escape to its child, and
  // a modal under the menu would close too (ADR-0087 D9).
  const page = menuPage(doc);
  doc.addEventListener(
    "keydown",
    (e) => {
      if (!menuEscape(e, page, opener)) return;
      e.preventDefault();
      e.stopPropagation();
    },
    true,
  );
}
