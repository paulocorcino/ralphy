/* ---------------------------------------------------------------------------
   The writes of the desk records (ADR-0075 D7, #621): a window's create, set
   and forget, a fence list or a card list committed as changes, the caps, and
   the reads of the checkouts. `createDeskRecords(deps)` holds no state of its
   own: the desk view is `createDesk`'s (`wb-desk.ts`), built later, and the
   console hands it in as reads.
   --------------------------------------------------------------------------- */
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import { WBWindowState } from "./wb-window-state.ts";
import type { WBDeskSync } from "./wb-desk-sync.ts";
import type {
  ConsoleWin,
  DeskChange,
  DeskFence,
  DeskNote,
  DeskRecord,
  DeskWindowFields,
  Rect,
} from "./wb-types.d.ts";

const { atCap, DESK_MAX, NOTE_MAX } = WBDeskFolds;
const { sessionIdOf } = WBWindowState;

// What the record writes read from the console, and nothing else. The desk
// view (`desk`, `fences`, `notes`, `checkouts`) is `createDesk`'s: it
// reassigns each list when `sync` takes a change, so each is read when used.
export type DeskRecordsDeps = {
  // The desk view and the changes not yet sent (`wb-desk-sync.ts`).
  sync: ReturnType<typeof WBDeskSync.createSync>;
  desk: () => DeskRecord[];
  fences: () => DeskFence[];
  notes: () => DeskNote[];
  checkouts: () => Record<string, string>;
  // The view takes the changes `sync` holds.
  refreshView: () => void;
  // ONE desk change, and the flush that sends it (`createDesk`, built later).
  emitDesk: (change: DeskChange) => void;
  scheduleDeskFlush: () => void;
  // A window's box before it was maximized.
  restoreRect: (win: HTMLElement) => Rect;
  // The fence chrome says which windows a fence holds (`createFenceList`).
  refreshFenceChrome: () => void;
  // The default name prefix of a repo (`createTitle`).
  consolePrefix: (repo: string | null | undefined) => string;
};

export function createDeskRecords(deps: DeskRecordsDeps) {
  const {
    sync,
    desk,
    fences,
    notes,
    checkouts,
    refreshView,
    emitDesk,
    scheduleDeskFlush,
    restoreRect,
    refreshFenceChrome,
    consolePrefix,
  } = deps;
  // A window's record as a `create` carries it. A maximized window stores its
  // *pre-maximize* rect (the class drives the full-bleed via CSS), so `max`
  // restores the full-screen state while the stored rect still restores the
  // underlying box.
  function recordOf(win: ConsoleWin) {
    return {
      id: win._deskId,
      repo: win._deskRepo,
      agent: win._deskAgent,
      kind: win._deskKind,
      rect: restoreRect(win),
      max: win.classList.contains("maximized"),
      // A DORMANT window has no handle; `null` here would demote its record to
      // a placeholder, and the next reload would rebuild it as "not running".
      sessionId: sessionIdOf(win),
      daemonId: win._deskDaemonId ?? null,
      environment: win._deskEnvironment ?? null,
      checkout: win._deskCheckout ?? null,
      locked: !!win._deskLocked, // a bool on the wire: the daemon refuses null
      consoleName: win._deskConsoleName || null,
    };
  }
  function createRecord(win: ConsoleWin) {
    win._deskUnrecorded = false;
    emitDesk({ op: "create", type: "window", record: recordOf(win) });
  }
  // The fields one act changed on one window. A window adopted from a session
  // this page found with no record (`_deskUnrecorded`) gets its record with the
  // operator's first act on it, in the same batch: until then the cascade
  // place it was given is not a place anybody chose, and must not be written
  // over the record another page may be writing.
  function setWin(win: ConsoleWin, fields: DeskWindowFields) {
    // A window taken off the page (a late pointerup after a close) must not
    // write.
    if (!win?._deskId || !win.isConnected) return;
    if (win._deskUnrecorded) createRecord(win);
    emitDesk({ op: "set", type: "window", id: win._deskId, fields });
    // A moved window may have joined or left a region; membership is derived.
    refreshFenceChrome();
  }
  function forgetRecord(deskId: string) {
    if (!deskId) return;
    emitDesk({ op: "remove", type: "window", id: deskId });
    refreshFenceChrome();
  }

  // The fields a `set` may carry per type, read off a record, so a list of
  // records handed back (`saveFences`, `saveNotes`) becomes the changes that
  // tell it from the view: a create, a remove, or a set of the fields that
  // differ, and nothing for a record left as it was.
  const rectOnly = (r: Rect | undefined) =>
    r ? { left: r.left, top: r.top, width: r.width, height: r.height } : null;
  const SET_FIELDS: Record<
    "fence" | "note",
    (r: Partial<DeskFence & DeskNote>) => { [field: string]: JsonValue | undefined }
  > = {
    fence: (r) => ({ rect: rectOnly(r.rect), name: r.name ?? "", locked: !!r.locked }),
    note: (r) => ({
      rect: rectOnly(r.rect),
      locked: !!r.locked,
      file: { repo: r.repo, path: r.path ?? "", checkout: r.checkout ?? null },
    }),
  };
  function commitList(
    type: "fence" | "note",
    before: (DeskFence | DeskNote)[],
    next: (DeskFence | DeskNote)[],
  ) {
    const was = new Map<string, DeskFence | DeskNote>(before.map((r) => [r.id, r]));
    const kept = new Set(next.map((r) => r.id));
    const changes = before
      .filter((r) => !kept.has(r.id))
      .map((r): DeskChange => ({ op: "remove", type, id: r.id }));
    for (const r of next) {
      const old = was.get(r.id);
      if (!old) {
        changes.push({ op: "create", type, record: r });
        continue;
      }
      const a = SET_FIELDS[type](old);
      const b = SET_FIELDS[type](r);
      const fields: { [field: string]: JsonValue | undefined } = {};
      for (const k of Object.keys(b))
        if (JSON.stringify(a[k]) !== JSON.stringify(b[k])) fields[k] = b[k];
      if (Object.keys(fields).length) changes.push({ op: "set", type, id: r.id, fields });
    }
    if (!changes.length) return;
    for (const c of changes) sync.emit(c);
    refreshView();
    scheduleDeskFlush();
  }
  // The cap REFUSES a new fence or card before it is born (`atFenceCap`,
  // `atNoteCap`); nothing here drops a record to make room.
  function saveFences(next: DeskFence[]) {
    commitList("fence", fences(), next);
  }
  function saveNotes(next: DeskNote[]) {
    commitList("note", notes(), next);
  }

  // A record without a name gets one, in desk order (ADR-0066 §2), so two
  // pages that read one desk agree, and the name is stored as a change.
  function nameUnnamed() {
    const named = WBConsoleName.nameDesk(desk(), consolePrefix);
    let emitted = false;
    named.forEach((r, i) => {
      if (desk()[i]?.consoleName || !r.consoleName) return;
      sync.emit({ op: "set", type: "window", id: r.id, fields: { consoleName: r.consoleName } });
      emitted = true;
    });
    if (!emitted) return;
    refreshView();
    scheduleDeskFlush();
  }

  function loadDesk() {
    return desk().slice();
  }
  // The desk as the column restore reads it (ADR-0051 §8): ids and `max` only.
  function deskRecords() {
    return loadDesk().map((r) => ({ id: r.id, max: !!r.max }));
  }

  // Whether a NEW console would be over the window cap — asked before it is
  // born, so the open is refused instead of cutting a record in silence
  // (ADR-0050 amendment 2026-10-04).
  function atDeskCap() {
    return atCap(desk(), DESK_MAX);
  }
  // Whether another card would be over the cap — asked before a card is born,
  // so the open is refused instead of quietly evicting one that is on screen.
  function atNoteCap() {
    return atCap(notes(), NOTE_MAX);
  }
  // The card records, as a copy: `wb-notes.ts` reads them and hands a NEW
  // array back to `saveNotes`, never mutates this one.
  function loadNotes() {
    return notes().slice();
  }
  // The selected checkout for one repo ref, or `null` — the primary tree.
  function checkoutOf(ref: string) {
    return checkouts()[ref] || null;
  }
  // Select (`name`) or clear (`null`) a project's checkout. A clear names the
  // tree it clears, so it never erases a tree another device picked since.
  function setCheckout(ref: string, name: string | null | undefined) {
    const current = checkouts()[ref];
    if (name) emitDesk({ op: "checkout", repo: ref, name: String(name) });
    else if (current) emitDesk({ op: "checkout-clear", repo: ref, ifName: current });
  }
  function allCheckouts() {
    return { ...checkouts() };
  }

  return {
    createRecord,
    setWin,
    forgetRecord,
    saveFences,
    saveNotes,
    nameUnnamed,
    loadDesk,
    deskRecords,
    atDeskCap,
    atNoteCap,
    loadNotes,
    checkoutOf,
    setCheckout,
    allCheckouts,
  };
}
