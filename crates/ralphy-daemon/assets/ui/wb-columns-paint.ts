/* ---------------------------------------------------------------------------
   The rules of a column paint that `wbColumns` (`wb-consoles-tab.ts`) asks:
   the lead console, the gap in the head, the cap, the focus after a paint
   and the write-back of the stored grid.
   --------------------------------------------------------------------------- */
import { WBColumns } from "./wb-columns.ts";
import type { Grid } from "./wb-columns.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

// The keys are free when nothing outside a console has them: no search box,
// no modal. Read at call time.
function keysFree() {
  const el = document.activeElement;
  return !el || el === document.body || el.closest(".session-window") !== null;
}

// The console the columns grow from: the first column, else the console that
// is maximized on the stage.
export function columnLead(ids: readonly string[]): string | undefined {
  return ids[0] ?? document.querySelector<ConsoleWin>("#stage .session-window.maximized")?._deskId;
}

// What the first column's console asks before a paint: "restore" when it is on
// the stage but neither maximized nor a column, "wait" when it is off the
// stage for a relaunch, else nothing. At a cap of 1 the first console is
// painted as a plain maximize, so its Restore took the maximize path; it is
// still a column restore. A console in a relaunch comes back under the same id.
export function headGap(
  head: string | undefined,
  headWin: ConsoleWin | null | undefined,
  relaunching: (id: string) => boolean,
): "restore" | "wait" | null {
  if (
    headWin &&
    !headWin.classList.contains("maximized") &&
    !headWin.classList.contains("column")
  ) {
    return "restore";
  }
  if (head !== undefined && head !== "" && !headWin && relaunching(head)) return "wait";
  return null;
}

// A paint raises the windows when the caller asks or when the painted columns
// changed.
export function shouldRaise(opts: { raise?: boolean } | undefined, moved: boolean) {
  return opts?.raise === true || moved;
}

// After a paint, the focus goes back to the column that had it, or to the
// last painted one, while the keys are free on the Consoles tab.
export function refocusAfterPaint(
  tabActive: boolean,
  columnIds: readonly (string | null)[],
  ids: string[],
  before: string | null,
  moved: boolean,
) {
  if (!tabActive || !keysFree() || !columnIds.includes(before)) return;
  const want = WBColumns.focusAfter(ids, before);
  if (want !== null && want !== "" && (want !== before || moved)) WBConsole.focusColumn(want);
}

// `setColumns` writes only a change, and an ignored grid meets an empty one:
// the restored grid `next` is written back when it differs from the stored
// `raw`. A flat list stored before rows is written back as a grid.
export function writeBackColumns(raw: unknown, next: Grid) {
  if (next.length > 0 && JSON.stringify(raw) !== JSON.stringify(next)) {
    window.WBView.patch({ columns: WBColumns.toStored(next) });
  }
  if (next.length === 0 && raw !== null) window.WBView.patch({ columns: null });
}

// The number of columns a paint shows: the measured cap when a console leads
// the columns, else 1. Null while the consoles tab is hidden: it measures 0
// wide, which reads as a cap of 1, so the painted columns stay as they are
// until the tab shows again.
export function paintCap(lead: string | undefined, columnCap: () => number) {
  const led = lead !== undefined && lead !== "";
  if (led && !WBConsole.columnMeasure().viewport) return null;
  return led ? columnCap() : 1;
}
