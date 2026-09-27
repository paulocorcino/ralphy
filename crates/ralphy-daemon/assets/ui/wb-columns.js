/* ---------------------------------------------------------------------------
   Columns (ADR-0051 §5, CONTEXT.md → *Columns*) as pure functions of their
   arguments.

   A maximized console can open other consoles beside it. The columns are an
   ordered list of console ids; the leftmost is the maximized console, and it
   is the only one the desk records as maximized. The list is per-client view
   state (ADR-0051 §8): no column writes a desk rect, a desk field or a verb.

   Nothing here reads the DOM, the store or a module-scope binding: `app.js`
   holds the list, feeds it through these functions, and `wb-console.js`
   paints the answer. Same shape as `wb-split.js`.

   Load order: BEFORE `app.js`; nothing else reads this namespace. The
   detached-fence popup does not load it — a popup offers no columns.
   --------------------------------------------------------------------------- */
window.WBColumns = (function () {
  // A column is never narrower than this many terminal cells (ADR-0051 §5).
  const CELLS = 80;
  const REASON_OPEN = "Already in a column";
  const REASON_DETACHED = "In a detached fence";
  const REASON_FULL = "No room for another column";
  // From this many rows, the list opens with a filter box.
  const FILTER_MIN = 8;

  // How many columns fit the viewport. A maximized console alone always fits,
  // so the floor is 1; the open button shows only when this is 2 or more.
  function cap(viewportWidth, cellWidth) {
    if (!Number.isFinite(viewportWidth) || !Number.isFinite(cellWidth)) return 1;
    if (viewportWidth <= 0 || cellWidth <= 0) return 1;
    return Math.max(1, Math.floor(viewportWidth / (CELLS * cellWidth)));
  }

  // Open `id` directly right of `callerId`. An empty list is a lone maximized
  // console: the caller is then the only column.
  function open(columns, callerId, id, capValue) {
    const list = columns.length ? columns : [callerId];
    if (list.includes(id)) return { ok: false, reason: REASON_OPEN };
    if (list.length >= capValue) return { ok: false, reason: REASON_FULL };
    const at = list.indexOf(callerId);
    if (at < 0) return { ok: false, reason: null };
    return { ok: true, columns: [...list.slice(0, at + 1), id, ...list.slice(at + 1)] };
  }

  // Put `id` in the column `atId` holds (ADR-0051 §5, swap). An `id` already
  // in another column changes places with `atId`; any other `id` replaces it,
  // and `atId` goes back to its rect. An empty list is a lone maximized
  // console, as in `open`. `unmax` names the old leftmost when it leaves the
  // list; one that moves to another column is repainted, as a column.
  function swap(columns, atId, id) {
    const list = columns.length ? columns : [atId];
    const at = list.indexOf(atId);
    if (at < 0 || id === atId) return { ok: false };
    const from = list.indexOf(id);
    const next = [...list];
    next[at] = id;
    if (from >= 0) next[from] = atId;
    return {
      ok: true,
      columns: next,
      ended: next.length < 2,
      unmax: at === 0 && from < 0 ? atId : null,
    };
  }

  // Remove `id`. `unmax` names the old leftmost when it was the one removed:
  // it stops being the maximized console, and `maximized` takes its place.
  function restore(columns, id) {
    if (!columns.includes(id)) {
      return {
        columns,
        maximized: columns[0] ?? null,
        ended: columns.length < 2,
        unmax: null,
      };
    }
    const next = columns.filter((c) => c !== id);
    return {
      columns: next,
      maximized: next[0] ?? null,
      ended: next.length < 2,
      unmax: id === columns[0] ? id : null,
    };
  }

  // A change that came from outside this client, in the shape of `restore`.
  // `ended` (the session exited), `maximized` (another device maximized a
  // console) and `moved` (another client changed a rect or a fence) leave the
  // columns alone. `closed` (another client closed the console) and `detached`
  // (its fence went to a popup) remove `event.ids`. A closed console is gone
  // from the desk, so it is never unmaximized: that would write it back.
  function external(columns, event) {
    const same = {
      columns,
      maximized: columns[0] ?? null,
      ended: columns.length < 2,
      unmax: null,
      changed: false,
    };
    if (event?.type !== "closed" && event?.type !== "detached") return same;
    const ids = event.ids || [];
    const next = columns.filter((id) => !ids.includes(id));
    if (next.length === columns.length) return same;
    return {
      columns: next,
      maximized: next[0] ?? null,
      ended: next.length < 2,
      unmax: event.type === "detached" && ids.includes(columns[0]) ? columns[0] : null,
      changed: true,
    };
  }

  // The columns the viewport can show now. The rest stay in the list and come
  // back when the cap grows again.
  function painted(columns, capValue) {
    return columns
      .slice(0, Math.max(1, capValue))
      .map((id, index, a) => ({ id, index, count: a.length }));
  }

  // Where the focus goes when the painted set changes: it stays on a painted
  // column, or moves to the rightmost painted one. A key never goes to a
  // console that is not painted.
  function focusAfter(ids, focusedId) {
    if (ids.includes(focusedId)) return focusedId;
    return ids[ids.length - 1] ?? null;
  }

  // Alt+Shift+←/→ among the painted columns (ADR-0051 §5). It wraps at both
  // ends; from outside the columns, → takes the first and ← the last.
  function focusStep(ids, focusedId, step) {
    if (!ids.length) return null;
    const i = ids.indexOf(focusedId);
    if (i < 0) return step > 0 ? ids[0] : ids[ids.length - 1];
    return ids[(i + step + ids.length) % ids.length];
  }

  // The list this client keeps in `wb.view.v1` (ADR-0051 §8): window ids only,
  // and nothing below two columns.
  function toStored(columns) {
    return columns.length >= 2 ? [...columns] : null;
  }

  // The stored list, checked against the desk (`[{id, max}]`) on restore. The
  // FIRST stored id must be the desk's maximized console, or the list is
  // ignored (ADR-0051 §8); then ids no longer on the desk drop.
  function fromStored(stored, desk) {
    if (!Array.isArray(stored)) return [];
    const list = [];
    for (const id of stored) {
      if (typeof id === "string" && !list.includes(id)) list.push(id);
    }
    const records = desk || [];
    if (!records.some((r) => r.id === list[0] && r.max === true)) return [];
    const onDesk = new Set(records.map((r) => r.id));
    const next = list.filter((id) => onDesk.has(id));
    return next.length >= 2 ? next : [];
  }

  // The "Open in a column" list: consoles outside every fence first, then each
  // fence that holds a console, in the order of `fences` (the Fence menu
  // order). `membership` is `WBGeometry.fenceMembership`'s shape: fence id →
  // window ids. A group whose rows all have one repo carries it as `repo` with
  // `shared: true`, so the repo is printed once, in the group head.
  // `from` is the column that opened the list; `full` says no column can be
  // added. A row can still be swapped in when it cannot open a column.
  function listFold({ rows, fences, membership, detached, columns, from, full }) {
    const inColumns = new Set(columns || []);
    const byFence = new Map((fences || []).map((f) => [f.id, []]));
    const fenceOfWin = new Map();
    for (const [fenceId, ids] of Object.entries(membership || {})) {
      for (const wid of ids || []) if (!fenceOfWin.has(wid)) fenceOfWin.set(wid, fenceId);
    }
    const loose = [];
    for (const r of rows || []) {
      if (r.id === from) continue;
      const open = inColumns.has(r.id);
      const row = {
        id: r.id,
        agent: r.agent,
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
    const group = (fence, rows) => {
      const shared = rows.every((r) => r.repo === rows[0].repo);
      return { fence, rows, shared, repo: shared ? rows[0].repo : null };
    };
    const groups = [];
    if (loose.length) groups.push(group(null, loose));
    for (const f of fences || []) {
      const members = byFence.get(f.id);
      if (members.length) groups.push(group({ id: f.id, name: f.name }, members));
    }
    return groups;
  }

  // A row's text: the agent, and the repo only when the group head does not
  // already print it. `label` turns a repo ref into its display text.
  function rowLabel(row, group, label) {
    if (group?.shared) return row.agent;
    return `${row.agent} · ${row.repo ? label(row.repo) : "home"}`;
  }

  // The list with only the rows that match `query`, case-insensitive, against
  // the agent, the repo text and the fence name. A fence whose name matches
  // keeps all its rows. Groups left with no rows drop.
  function filterGroups(groups, query, label) {
    const q = String(query || "").trim().toLowerCase();
    if (!q) return groups;
    const has = (text) => String(text || "").toLowerCase().includes(q);
    const out = [];
    for (const g of groups) {
      const rows = has(g.fence?.name)
        ? g.rows
        : g.rows.filter((r) => has(r.agent) || has(r.repo ? label(r.repo) : "home"));
      if (rows.length) out.push({ ...g, rows });
    }
    return out;
  }

  return {
    CELLS,
    REASON_OPEN,
    REASON_DETACHED,
    REASON_FULL,
    FILTER_MIN,
    cap,
    open,
    swap,
    restore,
    painted,
    external,
    focusAfter,
    focusStep,
    toStored,
    fromStored,
    listFold,
    rowLabel,
    filterGroups,
  };
})();
