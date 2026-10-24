/* ---------------------------------------------------------------------------
   Consoles in their own tab (ADR-0037 §3d): where each window is.

   A console tab has its own pane, `#console-tabs-pane`. The window of a
   console that has a tab leaves the stage and goes into that pane; the × puts
   it back. The window is moved, never copied: the terminal, its socket and
   the writer slot stay the same. `.in-tab` marks a window in the pane, and
   `.tab-shown` one the active tab paints. A tab holds a grid, as the stage's
   columns do (ADR-0051 §5): `--tab-col-*` and `--tab-row-*` place a window in
   it, and `.tab-grid` marks a window of a tab with two consoles or more. The
   window keeps its inline rect, which is its desk rect, so nothing about the
   stage is written. In its place the stage shows its shadow, `.console-shadow`
   (`win._tabShadow`): the desk rect, the tab's icon and the console name. A
   click on it asks for the console's tab.
   --------------------------------------------------------------------------- */
import { sendDocument } from "./wb-events.ts";
import { WBColumns } from "./wb-columns.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import { PHONE_MAX_WIDTH } from "./wb-viewport.ts";
import type { Grid, Painted } from "./wb-columns.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

// Whether the window is in the pane of the console tabs.
export function inTab(win: Element) {
  return win.classList.contains("in-tab");
}

// The shadow follows the window's desk rect and its name. A window with no
// shadow is left as it is.
export function paintTabShadow(win: ConsoleWin) {
  const sh = win._tabShadow;
  if (!sh) return;
  for (const side of ["left", "top", "width", "height"] as const) sh.style[side] = win.style[side];
  const name = sh.querySelector(".console-shadow-name");
  if (name) name.textContent = WBConsoleName.consoleLabel(win._deskConsoleName || "", win._deskAgent);
}

function buildShadow(document: Document, win: ConsoleWin) {
  const sh = document.createElement("div");
  sh.className = "console-shadow";
  sh.title = "In its own tab. Click to show the tab.";
  const icon = document.createElement("i");
  icon.className = "bi bi-terminal";
  const name = document.createElement("span");
  name.className = "console-shadow-name";
  sh.append(icon, name);
  sh.addEventListener("click", () => sendDocument(document, "workbench:console-tab-show", { id: win._deskId }));
  return sh;
}

export type PlaceDeps = {
  document: Document;
  // The stage, and the pane of the console tabs; null before the page has them.
  stage: () => HTMLElement | null;
  tabsPane: () => HTMLElement | null;
  // Every console window of the page.
  pageWindows: () => ConsoleWin[];
  focusWin: (win: ConsoleWin) => void;
  // A window changed place, or its place in the tab changed.
  moved: (win: ConsoleWin) => void;
};

// A window's place in a tab: `p` is its box in the active tab's grid, null
// when the active tab does not paint it. `many`: its tab holds two consoles
// or more.
type Slot = { p: Painted | null; many: boolean };

// The CSS variables of a box in the tab's grid.
const VARS = ["--tab-col-index", "--tab-col-count", "--tab-row-index", "--tab-row-count"] as const;

function slotKey(slot: Slot | null) {
  if (!slot) return "";
  if (!slot.p) return "held";
  const { index, count, row, rows } = slot.p;
  return `${index}/${count}/${row}/${rows}/${slot.many}`;
}

function paint(win: ConsoleWin, slot: Slot | null, key: string) {
  const p = slot?.p ?? null;
  if (key) win.dataset.tabSlot = key;
  else delete win.dataset.tabSlot;
  win.classList.toggle("in-tab", !!slot);
  win.classList.toggle("tab-shown", !!p);
  win.classList.toggle("tab-grid", !!p && !!slot?.many);
  const values = p ? [p.index, p.count, p.row, p.rows] : [];
  VARS.forEach((name, i) => {
    if (p) win.style.setProperty(name, String(values[i]));
    else win.style.removeProperty(name);
  });
}

// Whether a window that had the place `was` is shown now and was not before.
function newlyShown(was: string, slot: Slot | null) {
  return !!slot?.p && (was === "" || was === "held");
}

// The place of each console: null on the stage, else its slot in a tab.
// `width` is the pane's, for the phone's cap of one console.
function slotsOf(held: readonly string[], grid: Grid | null, width: number) {
  const painted = grid ? WBColumns.painted(grid, WBColumns.cap(width, PHONE_MAX_WIDTH)) : [];
  const many = !!grid && WBColumns.flat(grid).length >= 2;
  const boxes = new Map(painted.map((p) => [p.id, p]));
  const slot = (id: string): Slot | null => (held.includes(id) ? { p: boxes.get(id) ?? null, many } : null);
  return { slot, head: painted[0]?.id ?? null };
}

export function createPlace(deps: PlaceDeps) {
  const { document, stage, tabsPane, pageWindows, focusWin, moved } = deps;

  // `held`: the consoles that have a tab. `grid`: the grid of the active
  // tab, or null when the active tab is not a console tab.
  function placeTabs(held: readonly string[], grid: Grid | null) {
    const st = stage();
    const pane = tabsPane();
    if (!st || !pane) return;
    const { slot, head } = slotsOf(held, grid, pane.clientWidth);
    const entered: string[] = [];
    let shown = false;
    for (const win of pageWindows()) {
      const s = slot(win._deskId);
      const r = placeOne(win, s ? pane : st, s);
      if (r.entered) entered.push(win._deskId);
      shown ||= r.shown;
    }
    // A tab just shown, or a console just added to it: the first console of
    // the tab takes the focus mark.
    const first = shown ? pageWindows().find((w) => w._deskId === head) : null;
    if (first) focusWin(first);
    // A column that goes into a tab leaves the grid, as a detached one does.
    if (entered.length) sendDocument(document, "workbench:columns-leave", { ids: entered });
    keepShadows(st);
  }

  // Each window in a tab has its shadow on the stage. A shadow whose window
  // went back to the stage, or left the page, goes.
  function keepShadows(st: HTMLElement) {
    const kept = new Set<Element>();
    for (const win of pageWindows()) {
      if (inTab(win)) kept.add(shadowOn(st, win));
      else dropShadow(win);
    }
    for (const sh of st.querySelectorAll(".console-shadow")) if (!kept.has(sh)) sh.remove();
  }

  function shadowOn(st: HTMLElement, win: ConsoleWin) {
    const sh = win._tabShadow || buildShadow(document, win);
    win._tabShadow = sh;
    if (sh.parentElement !== st) st.append(sh);
    paintTabShadow(win);
    return sh;
  }

  function dropShadow(win: ConsoleWin) {
    win._tabShadow?.remove();
    win._tabShadow = null;
  }

  // Whether the window went into the pane, and whether it is shown now and
  // was not before.
  function placeOne(win: ConsoleWin, parent: HTMLElement, slot: Slot | null) {
    const moves = win.parentElement !== parent;
    if (moves) parent.append(win);
    const key = slotKey(slot);
    const was = win.dataset.tabSlot ?? "";
    if (key !== was) paint(win, slot, key);
    if (key !== was || moves) moved(win);
    return { entered: moves && !!slot, shown: key !== was && newlyShown(was, slot) };
  }

  return { placeTabs };
}
