/* ---------------------------------------------------------------------------
   The console's desk and fence folds — the restore decision, the fence readouts
   and walk, the detach registry and the popup rules, as pure functions of their
   arguments (ADR-0075 phase 5).

   Nothing here reads the DOM, the clock, a timer, `window` or a module-scope
   `let`. `wb-console.ts` owns the desk state and the stage, feeds them through
   these functions, and acts on the answer.

   Every function and the constants read outside the module are members of
   `window.WBConsole` under their own name.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import type { Rect, Offset, Size, DeskRecord, DeskFence, DeskNote } from "./wb-types.d.ts";

const { fenceMembership, fenceSpawnRect, rectsOverlap } = WBGeometry;

// A row of the live session list (`/api/sessions`), as the restore reads it.
// The folds are generic over the row, so a caller gets its own row back.
type LiveSession = {
  id: number;
  repo: string;
  agent: string;
  kind: string;
  daemon_id?: string;
  environment?: string;
  checkout?: string | null;
  record?: string | null;
};

// One restore verdict: what to do with one record, or with one session no
// record claims (`adopt`, with the record id it names, if it is free).
type DeskVerdict<S, R> =
  | { record: R; session: S; action: "attach"; id?: undefined }
  | { record: R; session: null; action: "relaunch"; id?: undefined }
  | { record: R; session: null; action: "placeholder"; id?: undefined }
  | { record: null; session: S; action: "adopt"; id: string | null };

// A window of the desk as a fence readout reads it.
type WindowRect = { id: string; repo?: string; rect: Rect };

// One event of the detach registry, and one effect it asks for.
type DetachEvent = { type: "detach" | "reattach" | "focus"; fenceId: string };
type DetachEffect =
  | { type: "focus" | "open" | "close"; fenceId: string }
  | { type: "refuse"; fenceId: string; reason: "cap" };

// The caps of the desk's record types: consoles, fences (#340) and note cards
// (ADR-0064 §2). The daemon refuses a record past each one too.
export const DESK_MAX = 30;
export const FENCE_MAX = 12;
export const NOTE_MAX = 32;

// The restore decision, a pure fold of the saved layout over the live session
// list. Each live session is consumed by AT MOST ONE record (first in layout
// order): a restarted daemon reuses ids, hence the full
// `sessionId`+`repo`+`agent`+`kind` tuple.
//
// `relaunchAgents` is the operator's per-client opt-in (Settings → Consoles),
// default OFF and passed in so the fold stays pure and the popup can never
// turn it on.
type ReconcileInput<S, R> = {
  layout: readonly R[] | null | undefined;
  sessions: readonly S[] | null | undefined;
  relaunchAgents?: boolean;
};
// What the fold reads of a record. A placeholder whose record another page
// deleted stands in with the record it was spawned from, which has no rect.
export type FoldRecord = Pick<DeskRecord, "id" | "sessionId" | "checkout"> & {
  repo?: string | null;
  agent?: string | null;
  kind?: string | null;
};
export function reconcileDesk<S extends LiveSession, R extends FoldRecord = DeskRecord>({ layout, sessions, relaunchAgents = false }: ReconcileInput<S, R>) {
  const live = sessions || [];
  const used = new Set<number>();
  const out: DeskVerdict<S, R>[] = [];
  for (const record of layout || []) {
    // A session that names its record is that record's, whatever a stale
    // `sessionId` says, and never another record's (ADR-0050 amendment
    // 2026-10-04). The tuple below is for sessions from an older daemon.
    let i = live.findIndex((s, idx) => !used.has(idx) && s.record === record.id);
    if (i < 0) {
      i = live.findIndex(
        (s, idx) =>
          !used.has(idx) &&
          s.record == null &&
          s.id === record.sessionId &&
          s.repo === record.repo &&
          s.agent === record.agent &&
          s.kind === record.kind,
      );
    }
    if (i >= 0) {
      used.add(i);
      out.push({ record, session: live[i], action: "attach" });
    } else {
      // A shell is free and idempotent, so it comes back by itself; an agent
      // console waits for a click — loading a page must never spawn a vendor
      // CLI and spend quota nobody authorized. Only `relaunchAgents` lifts
      // this. The placeholder's button is a LAUNCH, not a reconnect: the old
      // PTY and its scrollback are gone.
      out.push({
        record,
        session: null,
        action: record.kind === "console" || relaunchAgents ? "relaunch" : "placeholder",
      });
    }
  }
  // A live session no record claims first looks for the record waiting for
  // it (a placeholder or would-be relaunch on the same repo, vendor, kind and
  // worktree): its `sessionId` was lost to a lost flush or reissued by a
  // restarted daemon, and attaching there keeps one console from coming back
  // as two — or, for a shell, from spawning a SECOND PTY. Only with no such
  // record is it adopted into a fresh one, so it stays visible and closable.
  // The ids an adopted window may take: one window per record id.
  const taken = new Set((layout || []).map((r) => r.id));
  live.forEach((s, idx) => {
    if (used.has(idx)) return;
    // A session that names a record this page has not read is adopted under
    // that id, so the page that launched it and this one write one record.
    if (s.record != null) {
      const id = taken.has(s.record) ? null : s.record;
      if (id) taken.add(id);
      out.push({ record: null, session: s, action: "adopt", id });
      return;
    }
    // Widened to write: a `relaunch` or `placeholder` verdict becomes `attach`.
    const waiting: { session: S | null; action: DeskVerdict<S, R>["action"] } | undefined = out.find(
      ({ record, action }) =>
        action !== "attach" &&
        // An `adopt` entry pushed for an earlier session has no record.
        record != null &&
        record.repo === s.repo &&
        record.agent === s.agent &&
        record.kind === s.kind &&
        (record.checkout ?? null) === (s.checkout ?? null),
    );
    if (waiting) {
      waiting.session = s;
      waiting.action = "attach";
      return;
    }
    out.push({ record: null, session: s, action: "adopt", id: null });
  });
  return out;
}

// The live session a placeholder should attach to, or null: `reconcileDesk`'s
// own verdict for `recordId`, so a session another record owns is never taken.
// `layout` is a FRESH desk — another device may have relaunched this record
// and written its new `sessionId` there. `held` lists the `{id, repo}` of the
// sessions this page already shows: attaching one again would be a second
// window on one session. Ids repeat across repos and peers, hence the pair.
export function placeholderSession<S extends LiveSession>({
  layout,
  sessions,
  recordId,
  held = [],
}: {
  layout: readonly FoldRecord[] | null | undefined;
  sessions: readonly S[] | null | undefined;
  recordId: string;
  held?: readonly { id: number | null; repo: string }[];
}) {
  const shown = (s: S) =>
    held.some(
      (h) =>
        h.id === s.id && (h.repo === "~" ? !s.repo || s.repo === "~" : s.repo === h.repo),
    );
  const verdict = reconcileDesk({
    layout,
    sessions: (sessions || []).filter((s) => s && !shown(s)),
  }).find(({ record }) => record?.id === recordId);
  return verdict?.action === "attach" ? verdict.session : null;
}

export function isUnknownCheckout(reply: { status: string; message?: string } | null | undefined) {
  return !!reply && reply.status === "error" && reply.message === "unknown checkout";
}

// Pure. What one window is, given the painted consoles. `maximized: null`
// means "not a column: leave its maximize alone". Two rows of one column
// are columns too: what counts is how many consoles are painted.
export function columnClasses(
  painted: readonly { id: string; index: number; row?: number }[] | null | undefined,
  id: string,
) {
  const list = painted || [];
  const entry = list.find((p) => p.id === id);
  if (!entry) return { column: false, maximized: null };
  return { column: list.length >= 2, maximized: entry.index === 0 && !entry.row };
}

// The repos a fence's members belong to, for the fence's chrome. Deduped,
// sorted (DOM order is not stable), `"~"` rendered as `home` like `list()`.
export function fenceRepos(members: readonly ({ repo?: string } | undefined)[]) {
  const names = new Set<string | null | undefined>((members || []).map((m) => (m?.repo === "~" ? "home" : m?.repo)));
  names.delete(undefined);
  names.delete(null);
  names.delete("");
  return [...names].sort().join(" · ");
}

// One fence readout for BOTH the fence's chrome and the toolbar list (#343):
// `[{id, name, rect}]` + `[{id, repo, rect}]` in, one entry per fence IN ORDER
// out. Folding membership here is what makes "the list and the fence never
// disagree" a property of the code.
export function fenceSummaries(fences: readonly DeskFence[], windows: readonly WindowRect[]) {
  const list = fences || [];
  const all = windows || [];
  const byId = new Map(all.map((w): [string, WindowRect] => [w?.id, w]));
  const membership = fenceMembership(list, all);
  return list.map((f) => {
    const members = (membership[f.id] || []).map((wid) => byId.get(wid));
    return {
      id: f.id,
      name: f.name || "",
      count: members.length,
      repos: fenceRepos(members),
      locked: !!f.locked,
    };
  });
}

// Which grid slot a NEW fence takes: the first no existing fence occupies.
// Indexing by `fences.length` would reuse a slot after a removal and land on
// a survivor (the overlap ADR-0051 §6 does not enforce away). The scan runs
// PAST the fence count: one big fence covering the viewport occupies the
// first `taken.length + 1` slots while free plane sits below. `-1` means
// nowhere, and the caller refuses rather than nudging into a gap.
const FENCE_SLOT_SCAN = 64;
export function nextFenceSlot(rects: readonly Rect[], offset: Offset, viewport: Size) {
  const taken = rects || [];
  const cap = Math.max(taken.length, FENCE_SLOT_SCAN);
  for (let i = 0; i <= cap; i++) {
    const candidate = fenceSpawnRect(offset, viewport, i);
    if (!taken.some((t) => rectsOverlap(candidate, t))) return i;
  }
  return -1;
}

// ---- walking the fences from the keyboard ------------------------------------
// Pure. `[{id, rect}]` + the id in hand + a step (+1/-1) yields the next id in
// READING ORDER (top band first, left to right) — the desk array is creation
// order, which would teleport across the stage. The band is what keeps a row
// a row: side-by-side fences are never pixel-aligned on `top`.
const FENCE_BAND = 120;

function fenceOrder(fences: readonly { id: string; rect: Rect }[]) {
  return [...(fences || [])].sort((a, b) => {
    const at = Math.floor((a?.rect?.top || 0) / FENCE_BAND);
    const bt = Math.floor((b?.rect?.top || 0) / FENCE_BAND);
    if (at !== bt) return at - bt;
    const al = a?.rect?.left || 0;
    const bl = b?.rect?.left || 0;
    if (al !== bl) return al - bl;
    // Total order, so the walk is the same on every client: two fences at the
    // very same point would otherwise cycle in whatever order `sort` picked.
    return String(a?.id).localeCompare(String(b?.id));
  });
}

// `null` when there is nothing to walk. With no fence in hand the step decides
// which end to enter from, so the first Alt+Shift+→ lands on the top-left
// fence and the first Alt+Shift+← on the bottom-right one.
export function fenceCycle(
  fences: readonly { id: string; rect: Rect }[],
  currentId: string | null | undefined,
  step: number,
) {
  const order = fenceOrder(fences);
  if (!order.length) return null;
  const d = step < 0 ? -1 : 1;
  const at = order.findIndex((f) => f.id === currentId);
  if (at < 0) return (d > 0 ? order[0] : order[order.length - 1]).id;
  return order[(at + d + order.length) % order.length].id;
}

// How many fences may be detached at once: a popup is a real OS window with
// its own sockets and renderers; the cap stops a stuck key opening forty.
export const DETACH_MAX = 4;

// The detach registry's fold: (registry, event) -> { registry, effects }.
// Pure — no DOM, no storage, no `window`. `registry` (fence ids) is never
// mutated: a new array comes back, so the caller commits only once the
// effects have run (a popup the browser blocked leaves the registry as was).
// Three events: detach, reattach, focus. A new event is a new case here.
export function detachFold(
  registry: string[],
  event: DetachEvent,
): { registry: string[]; effects: DetachEffect[] } {
  const reg = Array.isArray(registry) ? registry : [];
  const id = event?.fenceId;
  const held = reg.includes(id);
  switch (event?.type) {
    case "detach":
      // Already detached: one popup per fence, so this is a request to SEE
      // the window that already exists, not to open a second one.
      if (held) return { registry: reg.slice(), effects: [{ type: "focus", fenceId: id }] };
      if (reg.length >= DETACH_MAX)
        return { registry: reg.slice(), effects: [{ type: "refuse", fenceId: id, reason: "cap" }] };
      return { registry: reg.concat([id]), effects: [{ type: "open", fenceId: id }] };
    case "reattach":
      // Idempotent on purpose: BOTH the popup's `beforeunload` and the
      // opener's `closed` poll report a re-attach, so the second one must be
      // a no-op rather than a second spawn of the same consoles.
      if (!held) return { registry: reg.slice(), effects: [] };
      return { registry: reg.filter((f) => f !== id), effects: [{ type: "close", fenceId: id }] };
    case "focus":
      return { registry: reg.slice(), effects: held ? [{ type: "focus", fenceId: id }] : [] };
    default:
      return { registry: reg.slice(), effects: [] };
  }
}

// Is a popup's note-name report one this tab may record? Pure. The
// note must be a card the popup holds, its desk record must have no path
// yet (a name is given once, ADR-0064 §4), and the path must name a note.
// `msg` comes from the popup: `path` is checked below before it is used.
export function noteNameOk(
  entry: { members?: readonly { kind?: string | null; id?: string }[] } | null | undefined,
  record: DeskNote | null | undefined,
  msg: { noteId?: unknown; path?: unknown },
) {
  const held = (entry?.members || []).some((x) => x?.kind === "note" && x.id === msg?.noteId);
  if (!held || !record || record.id !== msg.noteId || record.path) return false;
  const path = msg.path;
  // Relative to the repo: no parent step, no root, no drive letter.
  return (
    typeof path === "string" &&
    path.endsWith(".note") &&
    !path.includes("..") &&
    !/^[\\/]/.test(path) &&
    !/^[a-zA-Z]:/.test(path)
  );
}

// Does a lifecycle message come from the popup this entry holds? Pure. A
// popup's `pid` is given at detach and rides every message it sends. An
// entry restored after a reload has no `pid` until the popup's first
// `popup-here`, and until then it hears any popup of its fence.
// `m` is a message from a popup: its `pid` is only compared.
export function popupMatches(
  entry: { pid?: string | null } | null | undefined,
  m: { pid?: unknown } | null | undefined,
) {
  if (!entry) return false;
  if (entry.pid == null) return true;
  return m?.pid === entry.pid;
}

const FENCE_NAME_RE = /^Fence (\d+)$/;
export function nextFenceName(existing: readonly { name?: string }[]) {
  const highest = (existing || []).reduce((max: number, f) => {
    const m = FENCE_NAME_RE.exec(String(f?.name ?? ""));
    return m ? Math.max(max, Number(m[1])) : max;
  }, 0);
  return `Fence ${highest + 1}`;
}

// The cap rule of every record list: a list at its cap is full. One rule for
// the refusal before a record is born (`atNoteCap`, `atDeskCap`) and for
// `addNote`.
export function atCap(list: readonly unknown[], cap: number) {
  return list.length >= cap;
}

// The card list with one more card at the end, or `null` when the list is at
// its cap: the cap refuses, it never evicts. Pure; the caller writes the
// result through `saveNotes`.
export function addNote(list: readonly DeskNote[], note: DeskNote, cap: number) {
  return atCap(list, cap) ? null : list.concat([note]);
}

// The card list without the card `id`, in the same order.
export function removeNote(list: readonly DeskNote[], id: string) {
  return list.filter((n) => n.id !== id);
}
