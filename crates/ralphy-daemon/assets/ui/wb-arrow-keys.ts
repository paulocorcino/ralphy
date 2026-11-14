/* ---------------------------------------------------------------------------
   Arrow keys in a menu and in a tab list (docs/design-system/keyboard.md).
   Up and Down walk the enabled rows of a menu, Left and Right the tabs of a
   tab list, Home and End go to either end. One listener on the document
   serves every menu and tab list, the ones built later too.
--------------------------------------------------------------------------- */

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

export const ARROW_LISTS: readonly ArrowList[] = [
  { container: ".dropdown, .ctxmenu", items: MENU_ROWS, back: "ArrowUp", next: "ArrowDown" },
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

/** The one document listener. */
export function wireArrowKeys(doc: Document) {
  doc.addEventListener("keydown", (e) => {
    if (arrowStep(e, doc.activeElement)) e.preventDefault();
  });
}
