/* ---------------------------------------------------------------------------
   The "Open in a column" list of the columns (`listFold`, `rowLabel` and
   `filterGroups` of wb-columns.ts): the rows of the consoles on the stage and
   of the members of detached fences, grouped by fence, with the reason a row
   cannot be opened, and the filter of the list.

   Pure functions of their arguments: nothing here reads the DOM or a store.
   --------------------------------------------------------------------------- */

export const REASON_OPEN = "Already in a column";
export const REASON_DETACHED = "In a detached fence";
export const REASON_FULL = "No room for another console";
export const REASON_TAB = "In its own tab";
export const REASON_SELF = "This console";

/** A row of the "Open in a column" list. */
export type ColumnRow = {
  id: string;
  agent: string | null;
  name: string | null;
  repo: string | null;
  kind: string | null;
  state: string | null;
  /** What the agent-state dot says beside its state, such as why a console is silent. */
  detail: string | null;
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
export type DetachedMember = {
  id: string;
  agent: string | null;
  name?: string | null;
  repo?: string | null;
  kind: string | null;
};

/** A console the list is folded from: a window on this stage. */
type RosterRow = DetachedMember & {
  state?: string | null;
  detail?: string | null | undefined;
  running?: boolean;
};

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
  /** The consoles in their own tab on this page (ADR-0037 §3d). */
  tabbed?: readonly string[] | null;
};

// The "Slice" list: consoles outside every fence first, then each
// fence that holds a console, in the order of `fences` (the Fence menu
// order). `membership` is `WBGeometry.fenceMembership`'s shape: fence id →
// window ids. A group is `{ fence, rows }`: every row names its console, so
// no group head prints a repo (ADR-0066 §4).
// `columns` is the grid's ids in reading order (`flat`). `from` is the
// console that opened the list; `full` says no console can be added. A list
// row can still be swapped in when it cannot open.
// Window id → the fence that holds it. A window two fences name goes to
// the first one.
function fenceOfWindow(membership: Record<string, string[]> | null | undefined) {
  const fenceOfWin = new Map<string, string>();
  for (const [fenceId, ids] of Object.entries(membership ?? {})) {
    for (const wid of ids) if (!fenceOfWin.has(wid)) fenceOfWin.set(wid, fenceId);
  }
  return fenceOfWin;
}

function idSet(ids: readonly string[] | null | undefined) {
  return new Set(ids ?? []);
}

// The reason a console on this page cannot be opened as a column, or null.
function rowReason(inGrid: boolean, full: boolean | undefined, tabbed: boolean) {
  if (tabbed) return REASON_TAB;
  if (inGrid) return REASON_OPEN;
  return full === true ? REASON_FULL : null;
}

// The row of a console on this page. `inGrid`: it is in the grid already.
// `tabbed`: it is in its own tab, out of the stage.
function liveRow(
  r: RosterRow,
  inGrid: boolean,
  full: boolean | undefined,
  tabbed: boolean,
): ColumnRow {
  const reason = rowReason(inGrid, full, tabbed);
  return {
    id: r.id,
    agent: r.agent,
    name: r.name ?? null,
    repo: r.repo ?? null,
    kind: r.kind,
    state: r.state ?? null,
    detail: r.detail ?? null,
    running: r.running !== false,
    enabled: !reason,
    reason,
    swappable: !tabbed,
  };
}

// The row of the console that opened the list (ADR-0051 §5): named, never
// opened or swapped.
function selfRow(r: RosterRow): ColumnRow {
  return {
    ...liveRow(r, false, false, false),
    enabled: false,
    reason: REASON_SELF,
    swappable: false,
  };
}

// The row of a console in a detached fence: named, never opened or swapped.
function detachedRow(m: DetachedMember): ColumnRow {
  return {
    id: m.id,
    agent: m.agent,
    name: m.name ?? null,
    repo: m.repo ?? null,
    kind: m.kind,
    state: null,
    detail: null,
    running: true,
    enabled: false,
    reason: REASON_DETACHED,
    swappable: false,
  };
}

export function listFold({
  rows,
  fences,
  membership,
  detached,
  columns,
  from,
  full,
  tabbed,
}: ListFoldInput): ColumnGroup[] {
  const inColumns = idSet(columns);
  const inTabs = idSet(tabbed);
  // A console in no fence looks up `undefined`.
  const byFence = new Map<string | undefined, ColumnRow[]>((fences ?? []).map((f) => [f.id, []]));
  const fenceOfWin = fenceOfWindow(membership);
  const loose: ColumnRow[] = [];
  for (const r of rows ?? []) {
    const into = byFence.get(fenceOfWin.get(r.id));
    const row =
      r.id === from ? selfRow(r) : liveRow(r, inColumns.has(r.id), full, inTabs.has(r.id));
    (into ?? loose).push(row);
  }
  addDetached(byFence, detached);
  return listGroups(loose, fences, byFence);
}

// The members of each detached fence join its rows, after its live ones.
// A fence that is not listed takes none.
function addDetached(
  byFence: Map<string | undefined, ColumnRow[]>,
  detached: ListFoldInput["detached"],
) {
  for (const [fenceId, members] of Object.entries(detached ?? {})) {
    const into = byFence.get(fenceId);
    if (!into) continue;
    for (const m of members) into.push(detachedRow(m));
  }
}

// The groups of the list: the loose consoles first, then each fence with a
// row, in the order of `fences`.
function listGroups(
  loose: ColumnRow[],
  fences: ListFoldInput["fences"],
  byFence: Map<string | undefined, ColumnRow[]>,
) {
  const group = (fence: ColumnFence | null, rows: ColumnRow[]): ColumnGroup => ({ fence, rows });
  const groups: ColumnGroup[] = [];
  if (loose.length > 0) groups.push(group(null, loose));
  for (const f of fences ?? []) {
    const members = byFence.get(f.id) as ColumnRow[];
    if (members.length > 0) groups.push(group({ id: f.id, name: f.name }, members));
  }
  return groups;
}

// A row's text: the console name and its label, from the one builder the
// title uses (`labelOf` is `WBConsoleName.consoleLabel`).
export function rowLabel(row: ColumnRow, labelOf: (name: string, agent: string | null) => string) {
  return labelOf(row.name ?? "", row.agent);
}

// The list with only the rows that match `query`, case-insensitive, against
// the console name, the agent, the repo text and the fence name. A fence
// whose name matches keeps all its rows. Groups left with no rows drop.
export function filterGroups(
  groups: ColumnGroup[],
  query: string | null | undefined,
  repoText: (ref: string) => string,
) {
  const q = (query ?? "").trim().toLowerCase();
  if (!q) return groups;
  const has = (text: string | null | undefined) => (text ?? "").toLowerCase().includes(q);
  const repoHas = (repo: string | null) => repo !== null && repo !== "" && has(repoText(repo));
  const out = [];
  for (const g of groups) {
    const rows = has(g.fence?.name)
      ? g.rows
      : g.rows.filter((r) => has(r.name) || has(r.agent) || repoHas(r.repo));
    if (rows.length > 0) out.push({ ...g, rows });
  }
  return out;
}
