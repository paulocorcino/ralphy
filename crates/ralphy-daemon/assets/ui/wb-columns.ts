/* ---------------------------------------------------------------------------
   Columns (ADR-0051 §5, CONTEXT.md → *Columns*) as pure functions of their
   arguments.

   A maximized console can open other consoles beside it or below it. The
   columns are a GRID: a list of columns, each a list of console ids, one row
   per id, top to bottom. There are two levels and never more. The first id in
   reading order (column by column, top to bottom) is the maximized console,
   and it is the only one the desk records as maximized: a move of the
   maximize is written to the desk as an ordinary maximize. The grid itself
   is per-client view state (ADR-0051 §8): no column writes a desk rect or a
   verb. An empty grid is a lone maximized console, or none.

   Nothing here reads the DOM, the store or a module-scope binding: `app.ts`
   holds the grid, feeds it through these functions, and `wb-console.ts`
   paints the answer. Same shape as `wb-split.ts`.

   Imported by the workbench entry module BEFORE `app.ts` runs. The detached-fence
   popup imports it too, for its own grid, which it never stores (ADR-0051 §8,
   amended 2026-10-05).
   --------------------------------------------------------------------------- */
import type { Rect } from "./wb-types.d.ts";

/** The grid: a list of columns, each a list of console ids. */
export type Grid = string[][];

/** One painted console: its place in the grid. */
export type Painted = { id: string; index: number; count: number; row: number; rows: number };

/** A row of the "Open in a column" list. */
export type ColumnRow = {
  id: string;
  agent: string | null;
  name: string | null;
  repo: string | null;
  kind: string | null;
  state: string | null;
  running: boolean;
  enabled: boolean;
  reason: string | null;
  swappable: boolean;
};

/** A fence of the list, as a group head. */
export type ColumnFence = { id: string; name: string };

/** The rows of one fence, or the rows outside every fence (`fence: null`). */
export type ColumnGroup = { fence: ColumnFence | null; rows: ColumnRow[] };

/** A console the list names: a member a detached fence's popup told us about. */
export type DetachedMember = { id: string; agent: string | null; name?: string | null; repo?: string | null; kind: string | null };

/** A console the list is folded from: a window on this stage. */
type RosterRow = DetachedMember & { state?: string | null; running?: boolean };

/** The input of `listFold`. */
type ListFoldInput = {
  rows?: RosterRow[] | null;
  fences?: ColumnFence[] | null;
  /** Fence id to window ids. */
  membership?: Record<string, string[]> | null;
  /** Fence id to the members of a fence that went to a popup. */
  detached?: Record<string, DetachedMember[]> | null;
  columns?: string[] | null;
  from?: string | null;
  full?: boolean;
};

/** A change that came from outside this client. */
type ExternalEvent = { type?: string; ids?: string[] } | null | undefined;

/** The answer of `open`: the new grid, or why there is none (`null`: no reason to show). */
type OpenResult = { ok: false; reason: string | null } | { ok: true; columns: Grid };

/** The answer of `swap`. */
type SwapResult =
  | { ok: false }
  | { ok: true; columns: Grid; ended: boolean; unmax: string | null };

/** A console at its stage rect, for `fromRects`. */
type StageItem = { id: string; rect?: Partial<Rect> | null };

export const WBColumns = (function () {
  const REASON_OPEN = "Already in a column";
  const REASON_DETACHED = "In a detached fence";
  const REASON_FULL = "No room for another console";
  // From this many rows, the list opens with a filter box.
  const FILTER_MIN = 8;
  // Where "Slice" puts the console it opens (ADR-0051 §5, rows).
  const DIRS: readonly unknown[] = ["right", "down"];

  // How many consoles the viewport paints. Wider than a phone there is no
  // limit: how small a column or a row gets is the operator's choice, with the
  // font size (ADR-0051 §5, 2026-09-28 amendment). At a phone width, or on a
  // viewport not measured yet, only the maximized console. The open button
  // shows only when this is 2 or more.
  function cap(viewportWidth: number, phoneWidth: number) {
    if (!Number.isFinite(viewportWidth) || viewportWidth <= 0) return 1;
    return viewportWidth <= phoneWidth ? 1 : Infinity;
  }

  // The ids in reading order.
  function flat(grid: Grid): string[] {
    return grid.flat();
  }

  // [column, row] of `id`, or null.
  function where(grid: readonly (readonly (string | null)[])[], id: string | null): [number, number] | null {
    for (let c = 0; c < grid.length; c++) {
      const r = grid[c].indexOf(id);
      if (r >= 0) return [c, r];
    }
    return null;
  }

  // A copy with no empty column.
  function tidy(grid: Grid): Grid {
    return grid.map((col) => [...col]).filter((col) => col.length);
  }

  // Open `id` next to `callerId`: "right" as a new column directly right of
  // the caller's column, "down" as a new row directly below the caller. An
  // empty grid is a lone maximized console: the caller is then the only one.
  function open(grid: Grid, callerId: string, id: string, capValue: number, dir: string): OpenResult {
    const list = grid.length ? grid : [[callerId]];
    const ids = flat(list);
    if (ids.includes(id)) return { ok: false, reason: REASON_OPEN };
    if (ids.length >= capValue) return { ok: false, reason: REASON_FULL };
    const at = where(list, callerId);
    if (!at) return { ok: false, reason: null };
    const [c, r] = at;
    const next = list.map((col) => [...col]);
    if (dir === "down") next[c].splice(r + 1, 0, id);
    else next.splice(c + 1, 0, [id]);
    return { ok: true, columns: next };
  }

  // Put `id` in the row `atId` holds (ADR-0051 §5, swap). An `id` already in
  // another row changes places with `atId`; any other `id` replaces it, and
  // `atId` goes back to its rect. An empty grid is a lone maximized console,
  // as in `open`. `unmax` names the old first console when it leaves the grid;
  // one that moves to another row is repainted, as a column.
  function swap(grid: Grid, atId: string, id: string): SwapResult {
    const list = grid.length ? grid : [[atId]];
    const at = where(list, atId);
    if (!at || id === atId) return { ok: false };
    const from = where(list, id);
    const next = list.map((col) => [...col]);
    next[at[0]][at[1]] = id;
    if (from) next[from[0]][from[1]] = atId;
    return {
      ok: true,
      columns: next,
      ended: flat(next).length < 2,
      unmax: flat(list)[0] === atId && !from ? atId : null,
    };
  }

  // `grid` without `ids`, in the shape `restore` returns.
  function without(grid: Grid, ids: string[]) {
    const next = tidy(grid.map((col) => col.filter((id) => !ids.includes(id))));
    const first = flat(next)[0] ?? null;
    return { columns: next, maximized: first, ended: flat(next).length < 2 };
  }

  // Remove `id`. A column left with no row goes. `unmax` names the old first
  // console when it was the one removed: it stops being the maximized console,
  // and `maximized` takes its place.
  function restore(grid: Grid, id: string) {
    if (!where(grid, id)) {
      return {
        columns: grid,
        maximized: flat(grid)[0] ?? null,
        ended: flat(grid).length < 2,
        unmax: null,
      };
    }
    return { ...without(grid, [id]), unmax: id === flat(grid)[0] ? id : null };
  }

  // A change that came from outside this client, in the shape of `restore`.
  // `ended` (the session exited), `maximized` (another device maximized a
  // console) and `moved` (another client changed a rect or a fence) leave the
  // grid alone. `closed` (another client closed the console) and `detached`
  // (its fence went to a popup) remove `event.ids`. A closed console is gone
  // from the desk, so it is never unmaximized: that would write it back.
  function external(grid: Grid, event: ExternalEvent) {
    const same = {
      columns: grid,
      maximized: flat(grid)[0] ?? null,
      ended: flat(grid).length < 2,
      unmax: null,
      changed: false,
    };
    if (event?.type !== "closed" && event?.type !== "detached") return same;
    const ids = event.ids || [];
    const head = flat(grid)[0];
    if (!flat(grid).some((id) => ids.includes(id))) return same;
    return {
      ...without(grid, ids),
      unmax: event.type === "detached" && ids.includes(head) ? head : null,
      changed: true,
    };
  }

  // The grid with only the ids in `live` (a Set), and no empty column.
  function keep(grid: Grid, live: Set<string>): Grid {
    return tidy(grid.map((col) => col.filter((id) => live.has(id))));
  }

  // What the viewport can show now: the first `cap` ids in reading order, as
  // `{id, index, count, row, rows}` (index and count of the columns, row and
  // rows inside that column). The rest stay in the grid and come back when the
  // cap grows again.
  function painted(grid: Grid, capValue: number): Painted[] {
    const shown = new Set(flat(grid).slice(0, Math.max(1, capValue)));
    const cols = keep(grid, shown);
    return cols.flatMap((col, index) =>
      col.map((id, row) => ({ id, index, count: cols.length, row, rows: col.length })),
    );
  }

  // The grid for a fence opened as columns (ADR-0051 §5, 2026-09-30):
  // `items` is `[{ id, rect }]` at their stage rects. A console joins the
  // column whose FIRST console it overlaps by more than half the narrower
  // width, the largest overlap winning; otherwise it starts a column. Only the
  // first console is compared, so a staircase does not chain into one column.
  function fromRects(items: StageItem[] | null | undefined): Grid {
    const box = (r: Partial<Rect> | null | undefined) => ({
      left: Number(r?.left) || 0,
      top: Number(r?.top) || 0,
      width: Math.max(0, Number(r?.width) || 0),
    });
    const list = (items || [])
      .map((it) => ({ id: it.id, ...box(it.rect) }))
      .sort((a, b) => a.left - b.left || a.top - b.top || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
    const cols = [];
    for (const it of list) {
      let best = null;
      let bestOverlap = 0;
      for (const col of cols) {
        const a = col[0];
        const overlap = Math.min(a.left + a.width, it.left + it.width) - Math.max(a.left, it.left);
        if (overlap > Math.min(a.width, it.width) / 2 && overlap > bestOverlap) {
          best = col;
          bestOverlap = overlap;
        }
      }
      if (best) best.push(it);
      else cols.push([it]);
    }
    // Columns stay in the order of their first console's left edge.
    return cols.map((col) =>
      [...col].sort((a, b) => a.top - b.top || a.left - b.left).map((it) => it.id),
    );
  }

  // Where the focus goes when the painted set changes: it stays on a painted
  // console, or moves to the last painted one. A key never goes to a console
  // that is not painted.
  function focusAfter(ids: readonly (string | null)[], focusedId: string | null): string | null {
    if (ids.includes(focusedId)) return focusedId;
    return ids[ids.length - 1] ?? null;
  }

  // Alt+Shift+arrows among the painted consoles (ADR-0051 §5). "x" walks the
  // columns and lands on the row at the same position, or on the last row of
  // a shorter column; "y" walks the rows of one column. Both wrap at the ends;
  // from outside the grid, a forward step takes the first console and a
  // backward step the last.
  function focusMove(paintedList: Painted[], focusedId: string | null, axis: string, step: number): string | null {
    if (!paintedList.length) return null;
    const cols: Grid = [];
    for (const p of paintedList) (cols[p.index] ||= []).push(p.id);
    const at = where(cols, focusedId);
    if (!at) return step > 0 ? paintedList[0].id : paintedList[paintedList.length - 1].id;
    const [c, r] = at;
    if (axis === "y") {
      const col = cols[c];
      return col[(r + step + col.length) % col.length];
    }
    const to = cols[(c + step + cols.length) % cols.length];
    return to[Math.min(r, to.length - 1)];
  }

  // The grid this client keeps in `wb.view.v1` (ADR-0051 §8): window ids only,
  // and nothing below two consoles.
  function toStored(grid: Grid): Grid | null {
    return flat(grid).length >= 2 ? grid.map((col) => [...col]) : null;
  }

  // The stored grid, checked against the desk (`[{id, max}]`) on restore. A
  // flat list of ids, stored before rows existed, reads as one row per
  // column. The FIRST stored id must be the desk's maximized console, or the
  // grid is ignored (ADR-0051 §8); then ids no longer on the desk drop.
  function fromStored(stored: unknown, desk: { id: string; max?: boolean }[] | null | undefined): Grid {
    if (!Array.isArray(stored)) return [];
    const seen = new Set<string>();
    const cols: Grid = [];
    for (const item of stored) {
      const col = [];
      for (const id of Array.isArray(item) ? item : [item]) {
        if (typeof id === "string" && !seen.has(id)) {
          seen.add(id);
          col.push(id);
        }
      }
      if (col.length) cols.push(col);
    }
    const records = desk || [];
    if (!records.some((r) => r.id === flat(cols)[0] && r.max === true)) return [];
    const next = keep(cols, new Set(records.map((r) => r.id)));
    return flat(next).length >= 2 ? next : [];
  }

  // The stored direction, "right" when there is none.
  function dirOf(stored: string | null | undefined): string {
    return DIRS.includes(stored) ? (stored as string) : "right";
  }

  // The "Slice" list: consoles outside every fence first, then each
  // fence that holds a console, in the order of `fences` (the Fence menu
  // order). `membership` is `WBGeometry.fenceMembership`'s shape: fence id →
  // window ids. A group is `{ fence, rows }`: every row names its console, so
  // no group head prints a repo (ADR-0066 §4).
  // `columns` is the grid's ids in reading order (`flat`). `from` is the
  // console that opened the list; `full` says no console can be added. A list
  // row can still be swapped in when it cannot open.
  function listFold({ rows, fences, membership, detached, columns, from, full }: ListFoldInput): ColumnGroup[] {
    const inColumns = new Set(columns || []);
    // A console in no fence looks up `undefined`.
    const byFence = new Map<string | undefined, ColumnRow[]>((fences || []).map((f) => [f.id, []]));
    const fenceOfWin = new Map<string, string>();
    for (const [fenceId, ids] of Object.entries(membership || {})) {
      for (const wid of ids || []) if (!fenceOfWin.has(wid)) fenceOfWin.set(wid, fenceId);
    }
    const loose: ColumnRow[] = [];
    for (const r of rows || []) {
      if (r.id === from) continue;
      const open = inColumns.has(r.id);
      const row = {
        id: r.id,
        agent: r.agent,
        name: r.name ?? null,
        repo: r.repo ?? null,
        kind: r.kind,
        state: r.state ?? null,
        running: r.running !== false,
        enabled: !open && !full,
        reason: open ? REASON_OPEN : full ? REASON_FULL : null,
        swappable: true,
      };
      const into = byFence.get(fenceOfWin.get(r.id));
      (into || loose).push(row);
    }
    for (const [fenceId, members] of Object.entries(detached || {})) {
      const into = byFence.get(fenceId);
      if (!into) continue;
      for (const m of members) {
        into.push({
          id: m.id,
          agent: m.agent,
          name: m.name ?? null,
          repo: m.repo ?? null,
          kind: m.kind,
          state: null,
          running: true,
          enabled: false,
          reason: REASON_DETACHED,
          swappable: false,
        });
      }
    }
    const group = (fence: ColumnFence | null, rows: ColumnRow[]): ColumnGroup => ({ fence, rows });
    const groups: ColumnGroup[] = [];
    if (loose.length) groups.push(group(null, loose));
    for (const f of fences || []) {
      const members = byFence.get(f.id) as ColumnRow[];
      if (members.length) groups.push(group({ id: f.id, name: f.name }, members));
    }
    return groups;
  }

  // A row's text: the console name and its label, from the one builder the
  // title uses (`labelOf` is `WBConsoleName.consoleLabel`).
  function rowLabel(row: ColumnRow, labelOf: (name: string, agent: string | null) => string) {
    return labelOf(row.name || "", row.agent);
  }

  // The list with only the rows that match `query`, case-insensitive, against
  // the console name, the agent, the repo text and the fence name. A fence
  // whose name matches keeps all its rows. Groups left with no rows drop.
  function filterGroups(groups: ColumnGroup[], query: string | null | undefined, repoText: (ref: string) => string) {
    const q = String(query || "").trim().toLowerCase();
    if (!q) return groups;
    const has = (text: string | null | undefined) => String(text || "").toLowerCase().includes(q);
    const out = [];
    for (const g of groups) {
      const rows = has(g.fence?.name)
        ? g.rows
        : g.rows.filter((r) => has(r.name) || has(r.agent) || has(r.repo ? repoText(r.repo) : ""));
      if (rows.length) out.push({ ...g, rows });
    }
    return out;
  }

  return {
    REASON_OPEN,
    REASON_DETACHED,
    REASON_FULL,
    FILTER_MIN,
    cap,
    open,
    swap,
    restore,
    painted,
    fromRects,
    external,
    keep,
    flat,
    focusAfter,
    focusMove,
    toStored,
    fromStored,
    dirOf,
    listFold,
    rowLabel,
    filterGroups,
  };
})();
