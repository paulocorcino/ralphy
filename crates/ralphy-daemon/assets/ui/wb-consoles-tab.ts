/* ---------------------------------------------------------------------------
   The Consoles tab's own state: two Alpine components, one for each place of
   the page that shows it.

   `wbConsoleMenus` is on the toolbar of the tab bar, in
   `wb-consoles-tab-menus.ts`. `wbColumns`, here, is
   inside the Consoles tab: the columns beside a maximized console
   (ADR-0051 §5), their list, the count and stage pills of the footer, the
   console module's events, and the Alt+Shift+arrow keys.

   Each reaches `shell()` only through the names in its `uses` (ADR-0073 D4).
   One dropdown at a time: every menu trigger sends `workbench:menus-close`,
   and `shell()` and both components hear it and close their own menus.

   `main.ts` registers both (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBColumns } from "./wb-columns.ts";
import type { ColumnGroup, ColumnRow, Grid } from "./wb-columns.ts";
import type { ConsoleWin } from "./wb-types.d.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import { sendWindow } from "./wb-events.ts";

/** The consoles a fence opens as columns, at their stage rects. */
type FenceColumnItems = DocumentEventMap["workbench:fence-columns"]["detail"]["items"];

// The keys are free when nothing outside a console has them: no search box,
// no modal. Read at call time.
function keysFree() {
  const el = document.activeElement;
  return !el || el === document.body || !!el.closest?.(".session-window");
}

// The console the columns grow from: the first column, else the console that
// is maximized on the stage.
function columnLead(ids: readonly string[]): string | undefined {
  return ids[0] ?? document.querySelector<ConsoleWin>("#stage .session-window.maximized")?._deskId;
}

// What the first column's console asks before a paint: "restore" when it is on
// the stage but neither maximized nor a column, "wait" when it is off the
// stage for a relaunch, else nothing. At a cap of 1 the first console is
// painted as a plain maximize, so its Restore took the maximize path; it is
// still a column restore. A console in a relaunch comes back under the same id.
function headGap(
  head: string | undefined,
  headWin: ConsoleWin | null | undefined,
  relaunching: (id: string) => boolean,
): "restore" | "wait" | null {
  if (headWin && !headWin.classList.contains("maximized") && !headWin.classList.contains("column"))
    return "restore";
  if (head && !headWin && relaunching(head)) return "wait";
  return null;
}

// A paint raises the windows when the caller asks or when the painted columns
// changed.
function shouldRaise(opts: { raise?: boolean } | undefined, moved: boolean) {
  return !!opts?.raise || moved;
}

// After a paint, the focus goes back to the column that had it, or to the
// last painted one, while the keys are free on the Consoles tab.
function refocusAfterPaint(
  tabActive: boolean,
  columnIds: readonly (string | null)[],
  ids: string[],
  before: string | null,
  moved: boolean,
) {
  if (!tabActive || !keysFree() || !columnIds.includes(before)) return;
  const want = WBColumns.focusAfter(ids, before);
  if (want && (want !== before || moved)) WBConsole.focusColumn(want);
}

// `setColumns` writes only a change, and an ignored grid meets an empty one:
// the restored grid `next` is written back when it differs from the stored
// `raw`. A flat list stored before rows is written back as a grid.
function writeBackColumns(raw: unknown, next: Grid) {
  if (next.length && JSON.stringify(raw) !== JSON.stringify(next)) {
    window.WBView?.patch({ columns: WBColumns.toStored(next) });
  }
  if (!next.length && raw !== null) window.WBView?.patch({ columns: null });
}

// The number of columns a paint shows: the measured cap when a console leads
// the columns, else 1. Null while the consoles tab is hidden: it measures 0
// wide, which reads as a cap of 1, so the painted columns stay as they are
// until the tab shows again.
function paintCap(lead: string | undefined, columnCap: () => number) {
  if (lead && !WBConsole.columnMeasure().viewport) return null;
  return lead ? columnCap() : 1;
}

export function wbColumns() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component([], {
    // Columns beside a maximized console (ADR-0051 §5): per-client view
    // state, never desk state. The ids left to right; empty whenever fewer
    // than two remain, so a lone survivor is an ordinary maximize again.
    columns: [] as Grid,
    columnMenu: false,
    consoleCount: 0,
    // The stage extent, for the footer pill (#338).
    stageW: 0,
    stageH: 0,
    columnDir: "right",
    _columnsRestored: false,
    _paintedKey: "",
    columnGroups: [] as ColumnGroup[],
    columnFilter: "",
    columnFrom: null as string | null,
    // The list was opened on a phone: a row switches the maximized console
    // instead of opening a column (#599).
    columnSwitch: false,
    columnMenuAt: { top: 0, right: 0, maxWidth: 400, maxHeight: 400 },
    // INVARIANT: the columns never write `max` themselves. Each `applyColumns` call
    // passes `persist`, so `setMax` writes it for the first console in reading
    // order (`true`) or one that stopped being first (`false`). `columns` is
    // the grid: a list of columns, each a list of ids (`wb-columns.ts`).
    columnIds() {
      return WBColumns.flat(this.columns);
    },
    columnCap() {
      return WBColumns.cap(WBConsole.columnMeasure().viewport, WBConsole.PHONE_MAX_WIDTH);
    },
    // The ONE writer of `columns`. The view store is written only when the grid
    // changes: `paintColumns` runs on every `consoles-changed` during boot with
    // an empty grid, and an unconditional write would erase the stored grid
    // before `restoreColumns` reads it.
    setColumns(next: Grid) {
      if (JSON.stringify(next) === JSON.stringify(this.columns)) return;
      this.columns = next;
      window.WBView?.patch({ columns: WBColumns.toStored(next) });
    },
    // Once, on `workbench:desk-restored`. A list the desk does not confirm is
    // ignored and cleared from the store; a kept one is painted with `persist`.
    restoreColumns() {
      if (this._columnsRestored) return;
      this._columnsRestored = true;
      const stored = window.WBView?.read();
      this.columnDir = WBColumns.dirOf(stored?.columnDir);
      const raw = stored?.columns ?? null;
      const next = WBColumns.fromStored(raw, WBConsole.deskRecords());
      this.setColumns(next);
      writeBackColumns(raw, next);
      if (next.length) this.paintColumns({ raise: true });
    },
    effectiveColumns(fromId: string): Grid {
      return this.columnIds().includes(fromId) ? this.columns : [[fromId]];
    },
    // The grid the list opened from works on: the grid of the console tab
    // that holds `fromId` (ADR-0037 §3d), else the stage's columns.
    menuGrid(fromId: string): { tab: string | null; grid: Grid } {
      const tab = this.$store.tabs.tabOf(fromId);
      return tab
        ? { tab: tab.id, grid: tab.grid || [] }
        : { tab: null, grid: this.effectiveColumns(fromId) };
    },
    // The consoles of the console tabs, but the ones in `grid`: they are rows
    // of a list that opens from a tab.
    otherTabs(grid: Grid) {
      const own = WBColumns.flat(grid);
      return this.$store.tabs.consoleDeskIds().filter((id) => !own.includes(id));
    },
    // The Right | Down choice at the top of the list, kept in this browser.
    setColumnDir(dir: string | null | undefined) {
      this.columnDir = WBColumns.dirOf(dir);
      window.WBView?.patch({ columnDir: this.columnDir });
    },
    // True when the first console asked for a restore or a wait, so the paint
    // stops here. A restore needs the first console's id: with none, there is
    // nothing to restore.
    settleHeadGap(head: string | undefined, headWin: ConsoleWin | null | undefined) {
      const gap = headGap(head, headWin, WBConsole.isRelaunching);
      if (gap === "restore" && head) this.restoreColumn(head);
      return gap !== null;
    },
    // Re-derive what is painted from the list and the current cap. A console
    // that left the stage (closed, detached) leaves the list.
    paintColumns(opts?: { raise?: boolean }) {
      const byId = new Map(
        [...document.querySelectorAll<ConsoleWin>("#stage .session-window")].map((w) => [
          w._deskId,
          w,
        ]),
      );
      const head = this.columnIds()[0];
      const headWin = head ? byId.get(head) : null;
      if (this.settleHeadGap(head, headWin)) return;
      const kept = WBColumns.keep(this.columns, new Set(byId.keys()));
      const keptIds = WBColumns.flat(kept);
      // The first console left the stage and one is left: it takes the maximize.
      if (head && !headWin && keptIds.length === 1) {
        const cap = this.columnCap();
        this.setColumns([]);
        WBConsole.applyColumns(WBColumns.painted(kept, cap), { unmax: null, persist: true });
        return;
      }
      // The KEPT grid is stored, never the painted slice: a console the cap
      // hides comes back when the cap grows again.
      this.setColumns(keptIds.length >= 2 ? kept : []);
      const cap = paintCap(columnLead(this.columnIds()), () => this.columnCap());
      if (cap === null) return;
      const before = WBConsole.focusedId();
      const painted = WBColumns.painted(this.columns, cap);
      const ids = painted.map((p) => p.id);
      const key = ids.join(" ");
      // A column that stops being painted falls back to its plane rect with its
      // old z-index; raising the painted ones keeps it behind them.
      const moved = this.columnIds().length >= 2 && key !== this._paintedKey;
      this._paintedKey = key;
      WBConsole.applyColumns(painted, {
        unmax: null,
        raise: shouldRaise(opts, moved),
        persist: true,
      });
      refocusAfterPaint(
        this.$store.tabs.active === "consoles",
        this.columnIds(),
        ids,
        before,
        moved,
      );
    },
    // A fence detached to a popup takes its consoles out of the columns.
    leaveColumns(ids: string[]) {
      const r = WBColumns.external(this.columns, { type: "detached", ids });
      if (!r.changed) return;
      const cap = r.columns.length ? this.columnCap() : 1;
      this.setColumns(r.ended ? [] : r.columns);
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), {
        unmax: r.unmax,
        raise: true,
        persist: true,
      });
    },
    // Consoles whose records another client removed (`ids`, from the console
    // module after a desk read) leave the columns, then the stage. A session
    // that ended, a remote maximize and a remote rect or fence change need
    // nothing here: `WBColumns.external` names them as no-ops.
    checkColumnDesk(ids: string[]) {
      const gone = this.columnIds().filter((id) => ids.includes(id));
      const r = WBColumns.external(this.columns, { type: "closed", ids: gone });
      if (r.changed) {
        const cap = r.columns.length ? this.columnCap() : 1;
        this.setColumns(r.ended ? [] : r.columns);
        // Painted BEFORE the drops, so a lone survivor is maximized first.
        WBConsole.applyColumns(WBColumns.painted(r.columns, cap), {
          unmax: null,
          raise: true,
          persist: true,
        });
      }
      for (const id of ids) WBConsole.dropClosedElsewhere(id);
      if (r.changed) this.paintColumns();
    },
    toggleColumnMenu(id: string, rect: DOMRect | undefined) {
      const was = this.columnMenu && this.columnFrom === id;
      const { grid } = this.menuGrid(id);
      const ids = WBColumns.flat(grid);
      const cap = this.columnCap();
      this.columnGroups = WBColumns.listFold({
        ...WBConsole.columnRoster(),
        columns: ids,
        from: id,
        full: ids.length >= cap,
        tabbed: this.otherTabs(grid),
      });
      this.columnSwitch = cap < 2;
      this.columnFrom = id;
      const top = Math.round((rect?.bottom || 0) + 4);
      const right = Math.round(rect?.right || 0);
      // Right edge on the button's right edge, and no larger than the room
      // left of it and under it: the list grows with its longest row, and a
      // fixed guess at its width pushed it past the edge of the window.
      this.columnMenuAt = {
        top,
        right: Math.max(8, window.innerWidth - right),
        maxWidth: Math.max(200, right - 8),
        maxHeight: Math.max(120, window.innerHeight - top - 8),
      };
      this.columnFilter = "";
      this.closeMenus();
      this.columnMenu = !was;
      if (this.columnMenu) void this.$nextTick(() => this.$refs.columnFilter?.focus());
    },
    columnFilterShown() {
      return this.columnGroups.reduce((n, g) => n + g.rows.length, 0) >= WBColumns.FILTER_MIN;
    },
    // `owner/repo` without the environment: the operator already knows where
    // each console runs, and the list is about telling the consoles apart.
    columnRepoLabel(ref: string) {
      const row = this.$store.projects.projects.find(
        (p) => this.$store.projects.repoRef(p) === ref,
      );
      return row ? WBProject.projectName(row) : WBFleet.refLabel(ref);
    },
    columnView() {
      return WBColumns.filterGroups(this.columnGroups, this.columnFilter, (ref) =>
        this.columnRepoLabel(ref),
      );
    },
    columnRowLabel(r: ColumnRow) {
      return WBColumns.rowLabel(r, window.WBConsoleName.consoleLabel);
    },
    // A row's click swaps, on every width: the operator swaps far more often
    // than they open. The icon beside the row opens (ADR-0051 §5, 2026-10-24).
    pickColumn(id: string) {
      this.swapColumn(id);
    },
    columnRowOff(r: ColumnRow) {
      return !r.swappable;
    },
    columnRowTip(r: ColumnRow) {
      return r.swappable ? "" : r.reason || "";
    },
    columnOpenTip(r: ColumnRow) {
      if (!r.enabled) return r.reason || "";
      return this.columnDir === "down" ? "Open in a new row" : "Open in a new column";
    },
    // Enter in the filter does what a click on a row does: it swaps in the
    // first row that can be swapped.
    swapFirstColumn() {
      const swap = this.columnView()
        .flatMap((g) => g.rows)
        .find((r) => r.swappable);
      if (swap) this.swapColumn(swap.id);
    },
    openColumn(id: string) {
      const from = this.columnFrom;
      if (!from) return;
      const { tab, grid } = this.menuGrid(from);
      const out = WBColumns.open(grid, from, id, this.columnCap(), this.columnDir);
      if (!out.ok) {
        if (out.reason) this.$store.modals.flash(out.reason);
        return;
      }
      this.columnMenu = false;
      if (tab) this.$store.tabs.setTabGrid(tab, out.columns);
      else {
        this.setColumns(out.columns);
        this.paintColumns({ raise: true });
      }
      WBConsole.focusColumn(id);
    },
    // Put `id` in the row that opened the list (ADR-0051 §5, swap).
    swapColumn(id: string) {
      const from = this.columnFrom;
      if (!from) return;
      const { tab, grid } = this.menuGrid(from);
      const r = WBColumns.swap(grid, from, id);
      if (!r.ok) return;
      this.columnMenu = false;
      // A tab never writes the desk: the console that leaves goes to the stage.
      if (tab) this.$store.tabs.setTabGrid(tab, r.columns);
      else this.swapStage(r);
      WBConsole.focusColumn(id);
    },
    swapStage(r: { columns: Grid; ended: boolean; unmax: string | null }) {
      const cap = this.columnCap();
      this.setColumns(r.ended ? [] : r.columns);
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), {
        unmax: r.unmax,
        raise: true,
        persist: true,
      });
      this.paintColumns();
    },
    restoreColumn(id: string) {
      const r = WBColumns.restore(this.columns, id);
      const cap = r.columns.length ? this.columnCap() : 1;
      this.setColumns(r.ended ? [] : r.columns);
      // It may promote a lone survivor to the maximize.
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), {
        unmax: r.unmax,
        raise: true,
        persist: true,
      });
      this.paintColumns();
    },
    // A fence opened as columns (ADR-0051 §5, 2026-09-30): the grid follows the
    // members' stage rects and replaces the columns that are open.
    columnsFromFence(items: FenceColumnItems) {
      const grid = WBColumns.fromRects(items);
      const ids = WBColumns.flat(grid);
      const first = ids[0];
      if (first === undefined) return;
      const old =
        this.columnIds()[0] ??
        document.querySelector<ConsoleWin>("#stage .session-window.maximized")?._deskId;
      const cap = ids.length >= 2 ? this.columnCap() : 1;
      this.setColumns(ids.length >= 2 ? grid : []);
      WBConsole.applyColumns(WBColumns.painted(grid, cap), {
        unmax: old && !ids.includes(old) ? old : null,
        raise: true,
        persist: true,
      });
      this.paintColumns();
      WBConsole.focusColumn(first);
    },

    // Alt+Shift+←/→. Returns the fence landed on, or null when the plane has
    // none (the shortcut decides whether to swallow the key). Runs against the
    // LIVE stage.
    stepFence(step: number) {
      if (this.$store.tabs.active !== "consoles") return null;
      return WBConsole.stepFence(step);
    },
    // The grid the arrows walk: the stage's columns on the Consoles tab, the
    // active console tab's grid (ADR-0037 §3d), else none.
    arrowGrid(): Grid {
      const tabs = this.$store.tabs;
      return tabs.active === "consoles" ? this.columns : (tabs.activeGrid() ?? []);
    },
    // Alt+Shift+arrows among the painted consoles: "x" across the columns,
    // "y" along the rows of one column. Returns whether it applied.
    stepColumn(axis: string, step: number) {
      const grid = this.arrowGrid();
      if (WBColumns.flat(grid).length < 2) return false;
      const painted = WBColumns.painted(grid, this.columnCap());
      const to = WBColumns.focusMove(painted, WBConsole.focusedId(), axis, step);
      if (to) WBConsole.focusColumn(to);
      return true;
    },
    // The columns while they are open, the fences otherwise (ADR-0051 §5). The
    // fences have no "y": ↑/↓ with no columns open applies nothing.
    arrowStep(axis: string, step: number) {
      if (WBColumns.flat(this.arrowGrid()).length >= 2) return this.stepColumn(axis, step);
      return axis === "x" && !!this.stepFence(step);
    },

    closeOwnMenus() {
      this.columnMenu = false;
    },
    // ONE dropdown at a time (see `wbConsoleMenus`).
    closeMenus() {
      sendWindow(window, "workbench:menus-close");
    },

    // The listeners, added once when Alpine builds the consoles tab.
    init() {
      window.addEventListener("workbench:menus-close", () => this.closeOwnMenus());
      // A console whose record another client removed leaves the columns
      // before it leaves the stage.
      window.WBConsole?.setDeskGoneHook?.((ids: string[]) => this.checkColumnDesk(ids));

      // The Alpine mirror of the live console count.
      document.addEventListener("workbench:consoles-changed", (e) => {
        this.consoleCount = e.detail.count;
        this.paintColumns();
      });

      // A console's title bar asked for the columns list, or to restore a column;
      // or something changed the cap (maximize, first measurable frame).
      document.addEventListener("workbench:column-open", (e) => {
        this.toggleColumnMenu(e.detail.id, e.detail.rect);
      });
      // A console in a tab goes back to the stage (ADR-0037 §3d).
      document.addEventListener("workbench:column-restore", (e) => {
        if (this.$store.tabs.holds(e.detail.id)) this.$store.tabs.leaveTab(e.detail.id);
        else this.restoreColumn(e.detail.id);
      });
      document.addEventListener("workbench:columns-stale", () => {
        this.paintColumns();
      });
      document.addEventListener("workbench:columns-leave", (e) => {
        this.leaveColumns(e.detail.ids);
      });
      document.addEventListener("workbench:fence-columns", (e) => {
        this.columnsFromFence(e.detail.items);
      });
      document.addEventListener("workbench:desk-restored", () => {
        this.restoreColumns();
      });
      // A narrower or wider viewport changes the cap. One repaint per frame.
      let columnsFrame = 0;
      window.addEventListener("resize", () => {
        if (columnsFrame) return;
        columnsFrame = requestAnimationFrame(() => {
          columnsFrame = 0;
          this.paintColumns();
        });
      });

      // …and of the stage extent, for the footer pill (#338).
      document.addEventListener("workbench:stage-extent", (e) => {
        this.stageW = e.detail.width;
        this.stageH = e.detail.height;
      });

      // Alt+Shift+arrows → walk the columns (←/→) and the rows of a column (↑/↓)
      // while two or more consoles are open, from inside a column's terminal too;
      // otherwise ←/→ walk the fences in reading order (`fenceCycle`) — ADR-0051 §5.
      // With nothing to walk the key is left UNSWALLOWED.
      const ARROW_STEPS = {
        ArrowRight: ["x", 1],
        ArrowLeft: ["x", -1],
        ArrowDown: ["y", 1],
        ArrowUp: ["y", -1],
      } as const;
      document.addEventListener("keydown", (e) => {
        if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
        const move = ARROW_STEPS[e.code as keyof typeof ARROW_STEPS];
        if (!move) return;
        if (
          this.$store.modals.consoleShortcutsBlocked(WBColumns.flat(this.arrowGrid()).length >= 2)
        )
          return;
        if (!this.arrowStep(move[0], move[1])) return;
        e.preventDefault();
      });
    },
  });
}
