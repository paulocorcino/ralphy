/* ---------------------------------------------------------------------------
   The desk record of a console window at its birth.

   `createSpawnRecords(deps)` writes what a window's birth and its session
   change in its desk record, and reads the window back as a record. It reads
   the console only through `deps`. `createSpawn` (`wb-console-spawn.ts`)
   builds it and the window.
--------------------------------------------------------------------------- */
import { WBWindowState } from "./wb-window-state.ts";
import type {
  ConsoleWin,
  DeskRecord,
  DeskSessionFields,
  DeskWindowFields,
  Rect,
  SpawnCarry,
  WindowSnapshot,
} from "./wb-types.d.ts";

const { sessionIdOf } = WBWindowState;

// What the record writes read from the console, and nothing else.
export type SpawnRecordDeps = {
  // The desk record writes (`createDeskRecords`).
  createRecord: (win: ConsoleWin) => void;
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
  // The desk view, a fresh desk read, and a window whose record left (`createDesk`).
  currentDesk: () => DeskRecord[];
  reloadDesk: () => Promise<unknown>;
  recordLeft: (win: ConsoleWin) => void;
  // The box of a window.
  restoreRect: (el: HTMLElement) => Rect;
};

// How long an adopted console waits for the record the page that launched
// it writes. A launch and its create land within a second; a console whose
// record no page will write (another device removed it) still gets one.
const ADOPT_GRACE_MS = 10000;

export function createSpawnRecords(deps: SpawnRecordDeps) {
  const { createRecord, setWin, currentDesk, reloadDesk, recordLeft, restoreRect } = deps;

  // What a birth writes. A window whose record is in the view writes only the
  // fields its birth changed (a relaunch in the primary tree, a name the
  // record lacked) — never its rect, which came from the record. A window
  // carried from an adopted, unrecorded one stays unrecorded. Any other
  // window is a new console: its record is created.
  function recordBirth(win: ConsoleWin, carry: SpawnCarry | undefined) {
    const r = currentDesk().find((x) => x.id === win._deskId);
    if (r) {
      const fields = birthFields(r, win);
      if (Object.keys(fields).length > 0) setWin(win, fields);
      return;
    }
    if (carry?.unrecorded === true) {
      win._deskUnrecorded = true;
      setTimeout(() => adoptOrphan(win), ADOPT_GRACE_MS);
      return;
    }
    createRecord(win);
  }

  function adoptOrphan(win: ConsoleWin) {
    if (!win.isConnected || !win._deskUnrecorded) return;
    // A fresh read first: the record may have landed without a push here.
    void reloadDesk().then(() => {
      if (!win.isConnected || !win._deskUnrecorded) return;
      if (currentDesk().some((r) => r.id === win._deskId)) return;
      recordLeft(win);
    });
  }

  // The session the daemon announced, and the worktree with it, written only
  // when they differ from the record: a reconnect changes nothing else.
  function recordSession(win: ConsoleWin) {
    const r = currentDesk().find((x) => x.id === win._deskId);
    if (!r) return;
    const fields: DeskWindowFields = {};
    const session = {
      sessionId: sessionIdOf(win),
      daemonId: win._deskDaemonId ?? null,
      environment: win._deskEnvironment ?? null,
    };
    if (sessionDiffers(r, session)) fields.session = session;
    if (checkoutDiffers(r, win)) fields.checkout = win._deskCheckout ?? null;
    if (Object.keys(fields).length > 0) setWin(win, fields);
  }

  // The window's placement as a desk record, to carry identity and box across
  // a rebuild (takeover, placeholder → live console).
  function deskOf(win: ConsoleWin): WindowSnapshot {
    return {
      unrecorded: win._deskUnrecorded,
      id: win._deskId,
      repo: win._deskRepo,
      agent: win._deskAgent,
      kind: win._deskKind,
      daemonId: win._deskDaemonId,
      environment: win._deskEnvironment,
      checkout: win._deskCheckout ?? null,
      locked: win._deskLocked,
      consoleName: win._deskConsoleName ?? null,
      rect: restoreRect(win),
      max: win.classList.contains("maximized"),
    };
  }

  return { recordBirth, recordSession, deskOf };
}

// The fields of the record `r` that the birth of `win` changed: a relaunch in
// another tree, a name the record lacked.
function birthFields(r: DeskRecord, win: ConsoleWin): DeskWindowFields {
  const fields: DeskWindowFields = {};
  if (checkoutDiffers(r, win)) fields.checkout = win._deskCheckout ?? null;
  const name = win._deskConsoleName ?? "";
  if (name !== "" && r.consoleName !== name) fields.consoleName = name;
  return fields;
}

// The window is in another checkout than its record names.
function checkoutDiffers(r: DeskRecord, win: ConsoleWin) {
  return (r.checkout ?? null) !== (win._deskCheckout ?? null);
}

// The session the daemon announced is not the one the record names.
function sessionDiffers(r: DeskRecord, session: DeskSessionFields) {
  return (
    (r.sessionId ?? null) !== session.sessionId ||
    (r.daemonId ?? null) !== session.daemonId ||
    (r.environment ?? null) !== session.environment
  );
}
