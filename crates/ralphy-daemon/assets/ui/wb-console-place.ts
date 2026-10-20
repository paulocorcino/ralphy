/* ---------------------------------------------------------------------------
   A console in its own tab (ADR-0037 §3d): where its window is.

   A console tab has its own pane, `#console-tabs-pane`. The window of a
   console that has a tab leaves the stage and goes into that pane; the × puts
   it back. The window is moved, never copied: the terminal, its socket and
   the writer slot stay the same. `.in-tab` marks a window in the pane, and
   `.tab-shown` the one of the active tab. The window keeps its inline rect,
   which is its desk rect, so nothing about the stage is written.
   --------------------------------------------------------------------------- */
import { sendDocument } from "./wb-events.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

// Whether the window is in the pane of the console tabs.
export function inTab(win: Element) {
  return win.classList.contains("in-tab");
}

export type PlaceDeps = {
  document: Document;
  // The stage, and the pane of the console tabs; null before the page has them.
  stage: () => HTMLElement | null;
  tabsPane: () => HTMLElement | null;
  // Every console window of the page.
  pageWindows: () => ConsoleWin[];
  focusWin: (win: ConsoleWin) => void;
  // A window changed place, or its tab was shown or hidden.
  moved: (win: ConsoleWin) => void;
};

export function createPlace(deps: PlaceDeps) {
  const { document, stage, tabsPane, pageWindows, focusWin, moved } = deps;

  // `held`: the consoles that have a tab. `shown`: the console of the active
  // tab, or null.
  function placeTabs(held: readonly string[], shown: string | null) {
    const st = stage();
    const pane = tabsPane();
    if (!st || !pane) return;
    const entered: string[] = [];
    for (const win of pageWindows()) {
      const want = held.includes(win._deskId);
      if (placeOne(win, want ? pane : st, want, want && win._deskId === shown)) entered.push(win._deskId);
    }
    // A column that goes into a tab leaves the grid, as a detached one does.
    if (entered.length) sendDocument(document, "workbench:columns-leave", { ids: entered });
  }

  // Returns whether the window went into the pane.
  function placeOne(win: ConsoleWin, parent: HTMLElement, held: boolean, show: boolean) {
    const moves = win.parentElement !== parent;
    if (moves) parent.append(win);
    if (paint(win, held, show) || moves) moved(win);
    return moves && held;
  }

  // Returns whether the window's state changed.
  function paint(win: ConsoleWin, held: boolean, show: boolean) {
    const was = inTab(win) === held && win.classList.contains("tab-shown") === show;
    win.classList.toggle("in-tab", held);
    win.classList.toggle("tab-shown", show);
    if (show && !was) focusWin(win);
    return !was;
  }

  return { placeTabs };
}
