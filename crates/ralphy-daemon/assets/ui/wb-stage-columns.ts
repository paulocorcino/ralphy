/* ---------------------------------------------------------------------------
   The column paint: the grid of consoles that fill the viewport.

   `createStageColumns(deps)` returns the functions that paint the answer of
   `wbColumns` (wb-consoles-tab.ts), and the `WBConsole` API members that wrap
   them (ADR-0075 D7). They read the console only through `deps`, and
   `StageColumnsDeps` lists every read, so `tsc` refuses a read outside it.
   `wb-console.ts` creates one per console.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import type { DetachedMember, Painted } from "./wb-columns.ts";
import type { ConsoleOpts, ConsoleWin, DeskRecord, Stacked } from "./wb-types.d.ts";
import type { PopupEntry } from "./wb-desk-popups.ts";
import type { DetachReason } from "./wb-console-session.ts";

// Plane geometry is `wb-geometry.ts` (ADR-0057): pure folds over rects.
const { fenceMembership } = WBGeometry;
// The grid rule is `wb-desk-folds.ts`: a pure function of the painted list.
const { columnClasses } = WBDeskFolds;

// One row of `list`.
export type ListedWindow = {
  id: string;
  agent: string;
  name: string | null;
  tooltip: string;
  repo: string | null;
  kind: string;
  running: boolean;
  state: string | null;
};

// What the column paint reads from the console, and nothing else.
export type StageColumnsDeps = {
  // The console's options: only `autoBoot` is read.
  OPTS: ConsoleOpts;
  // Every console window on this page.
  wins: Set<ConsoleWin>;
  // The viewport (the scrolling box) and the stage (the plane inside it);
  // null before the page has them.
  workspace: () => HTMLElement | null;
  stage: () => HTMLElement | null;
  // The detach registry: which fences are in a popup, and who is in them.
  popups: { entries: () => [string, PopupEntry][] };
  // The desk's window records. The desk is built after this factory, so it is
  // a lazy arrow.
  desk: () => DeskRecord[];
  // Raises and focuses a window.
  focusWin: (win: Stacked) => void;
  // The window states. `setMax` writes the maximize, `paintMaxButton` the
  // button, `syncMaxLock` and `syncMaxPin` the follow-up states.
  setMax: (win: ConsoleWin, on: boolean, persist?: boolean) => void;
  paintMaxButton: (win: ConsoleWin) => void;
  syncMaxLock: () => void;
  syncMaxPin: () => void;
  applyExtent: () => void;
  // Every window on the plane, one row each.
  list: () => ListedWindow[];
  // The fences. The fence list is built after this factory (it takes
  // `columnMeasure`), so each is a lazy arrow.
  paintFenceColumns: () => void;
  fenceList: () => { id: string; name: string }[];
  readFenceRects: (st: HTMLElement) => Parameters<typeof fenceMembership>[0];
  readWindowRects: (st: HTMLElement) => Parameters<typeof fenceMembership>[1];
  // The window of a desk id, from the view (built later: a lazy arrow).
  findWindow: (id: string) => ConsoleWin | null;
  // Closes a console in the detach registry's way (built later: a lazy arrow).
  tearDownMember: (win: ConsoleWin, reason: DetachReason) => void;
};

export function createStageColumns(deps: StageColumnsDeps) {
  const {
    OPTS,
    wins,
    workspace,
    stage,
    popups,
    focusWin,
    setMax,
    paintMaxButton,
    syncMaxLock,
    syncMaxPin,
    applyExtent,
    list,
    paintFenceColumns,
    fenceList,
    readFenceRects,
    readWindowRects,
    findWindow,
    tearDownMember,
  } = deps;

  // `wbColumns` (wb-consoles-tab.ts) owns the column list and folds it with
  // `WBColumns`; this module only paints the answer and never reads
  // `WBColumns`: the detached-fence popup boots this file without it.
  //
  // A column writes no rect to the desk: the painted box is CSS, and
  // `restoreRect` reads the inline rect under it. The first console is the one
  // the desk records as maximized, so a move of the maximize is written when
  // the caller passes `persist` (ADR-0051 §5). A reload that keeps the stored
  // grid writes the desk to match it, so a console another device maximized
  // inside the grid is written back as not maximized. Another page does not
  // apply that `max` while it is open (ADR-0050 amendment 2026-10-04).

  // The width of the viewport the columns share, in px.
  function columnMeasure() {
    return { viewport: workspace()?.clientWidth || 0 };
  }

  function clearColumn(win: ConsoleWin) {
    win.classList.remove("column");
    win.style.removeProperty("--col-index");
    win.style.removeProperty("--col-count");
    win.style.removeProperty("--row-index");
    win.style.removeProperty("--row-count");
    if (!win.classList.contains("maximized")) {
      win.style.removeProperty("--max-left");
      win.style.removeProperty("--max-top");
    }
    paintMaxButton(win);
    try {
      win._term?.fit.fit();
    } catch {}
  }

  // Paint `painted` (`WBColumns.painted`). `unmax` is the old first console
  // after a restore: it stops being the maximized console. `persist` writes
  // each change of the maximize to the desk.
  function applyColumns(painted: Painted[] | null | undefined, opts?: { cap?: number; unmax?: string | null; raise?: boolean; persist?: boolean }) {
    const list = painted || [];
    const cap = opts?.cap ?? 1;
    const persist = !!opts?.persist;
    for (const win of wins) {
      if (win.classList.contains("column") && !columnClasses(list, win._deskId).column) {
        clearColumn(win);
      }
    }
    const gone = opts?.unmax ? findWindow(opts.unmax) : null;
    if (gone && !columnClasses(list, gone._deskId).column) setMax(gone, false, persist);
    const shown = [];
    for (const p of list) {
      const win = findWindow(p.id);
      if (!win) continue;
      const c = columnClasses(list, p.id);
      if (c.column) {
        win.classList.add("column");
        win.style.setProperty("--col-index", String(p.index));
        win.style.setProperty("--col-count", String(p.count));
        win.style.setProperty("--row-index", String(p.row ?? 0));
        win.style.setProperty("--row-count", String(p.rows ?? 1));
        shown.push(win);
      } else if (c.maximized) {
        // The last column left is a plain maximize, and a full bleed must be
        // on top: the console just restored was raised later than it.
        shown.push(win);
      }
      // The class is set FIRST: `restoreRect` must already read a column's
      // inline rect.
      if (c.maximized && !win.classList.contains("maximized")) setMax(win, true, persist);
      else if (!c.maximized && win.classList.contains("maximized")) setMax(win, false, persist);
      paintMaxButton(win);
    }
    syncMaxLock();
    syncMaxPin();
    // Raised in reading order only on an open or a restore: a repaint on every
    // `consoles-changed` would bury a console just spawned, and move the focus
    // mark off the column the operator is typing in.
    for (const win of shown) {
      if (opts?.raise) focusWin(win);
      try {
        win._term?.fit.fit();
      } catch {}
    }
    // Never disabled: at the cap the list still swaps (ADR-0051 §5).
    for (const win of wins) {
      const btn = win._colBtn;
      if (!btn) continue;
      const held = win.classList.contains("maximized") || win.classList.contains("column");
      btn.hidden = !(OPTS.autoBoot !== false && held && cap >= 2);
    }
    paintFenceColumns();
  }

  // A console whose record another client removed: off this stage. Its
  // session is not touched here, and its record is already gone.
  function dropClosedElsewhere(id: string) {
    const win = findWindow(id);
    if (!win) return;
    tearDownMember(win, "window-closed");
    applyExtent();
  }

  function focusedId() {
    return stage()?.querySelector<ConsoleWin>(".session-window.focused")?._deskId ?? null;
  }

  function focusColumn(id: string) {
    const win = findWindow(id);
    if (!win) return;
    focusWin(win);
    win._term?.term.focus();
  }

  // What the "Open in a column" list is folded from. A detached fence's
  // members are not on this stage; its popup told us who they are.
  function columnRoster() {
    const st = stage();
    if (!st) return { rows: [], fences: [], membership: {}, detached: {} };
    const out: Record<string, DetachedMember[]> = {};
    for (const [id, entry] of popups.entries()) {
      out[id] = (entry.members || [])
        .filter((m) => m && m.id && m.kind !== "note")
        .map((m) => ({
          id: m.id,
          agent: m.agent ?? null,
          name: deps.desk().find((r) => r.id === m.id)?.consoleName ?? m.consoleName ?? null,
          repo: m.repo === "~" ? null : (m.repo ?? null),
          kind: m.kind ?? null,
        }));
    }
    return {
      rows: list(),
      fences: fenceList().map(({ id, name }) => ({ id, name })),
      membership: fenceMembership(readFenceRects(st), readWindowRects(st)),
      detached: out,
    };
  }

  return {
    columnMeasure,
    applyColumns,
    dropClosedElsewhere,
    focusedId,
    focusColumn,
    columnRoster,
  };
}
