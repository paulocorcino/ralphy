/* ---------------------------------------------------------------------------
   The screens converge (ADR-0050 amendment 2026-10-04): every desk a page
   takes puts each window's rect, lock and name, and each fence and card, on
   the stage. `createDeskConverge(deps)` is built by `createDesk`
   (`wb-desk.ts`) and reads the desk only through `deps`.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { paintTabShadow } from "./wb-console-place.ts";
import type {
  ConsoleWin,
  DeskChange,
  DeskNote,
  DeskRecord,
  ExtentOpts,
  NoteCard,
  Presentation,
  Rect,
} from "./wb-types.d.ts";

const { WIN_MIN_W, WIN_MIN_H } = WBGeometry;
const { unheardRef, sessionRowFor } = WBConsoleSession;
const { sessionIdOf } = WBWindowState;

// What the converge reads from the desk and the console, and nothing else.
export type ConvergeDeps = {
  // The popup (`autoBoot: false`) never converges: its stage holds one fence.
  OPTS: { autoBoot?: boolean };
  stage: () => HTMLElement | null;
  pageWindows: () => ConsoleWin[];
  // The desk view and the cards, as the desk last filled them.
  currentDesk: () => DeskRecord[];
  currentNotes: () => DeskNote[];
  isDeskReconciled: () => boolean;
  emitDesk: (change: DeskChange) => void;
  readSessions: () => Promise<{ sessions: HostedSession[]; unheard: ReadonlySet<string> }>;
  renderFences: () => void;
  renderNotes: () => void;
  applyExtent: (opts?: ExtentOpts) => void;
  inGesture: (el: HTMLElement) => boolean;
  applyLock: (win: ConsoleWin, locked: boolean) => void;
  renderTitle: (win: ConsoleWin, title: HTMLElement, presentation: Presentation) => void;
  createRecord: (win: ConsoleWin) => void;
  dropClosedElsewhere: (id: string) => void;
};

export function createDeskConverge(deps: ConvergeDeps) {
  const {
    OPTS,
    stage,
    pageWindows,
    currentDesk,
    currentNotes,
    isDeskReconciled,
    emitDesk,
    readSessions,
    renderFences,
    renderNotes,
    applyExtent,
    inGesture,
    applyLock,
    renderTitle,
    createRecord,
    dropClosedElsewhere,
  } = deps;

  // The view already holds this page's own unanswered changes, so a value this
  // page set and the daemon has not answered yet stays. `max` is left alone: a
  // phone that maximizes a console must not maximize it on the PC. A console
  // opened on another device appears at the next load: opening it here would
  // attach or start a process with no act on this page.
  //
  // Only on a stage `restoreDesk` has filled: before that, it puts every
  // record on the stage itself.
  function converge() {
    if (OPTS.autoBoot === false || !isDeskReconciled()) return;
    const st = stage();
    if (!st) return;
    const byId = new Map<string, DeskRecord>(currentDesk().map((r) => [r.id, r]));
    for (const w of pageWindows()) {
      const r = byId.get(w._deskId);
      if (!r) {
        if (!w._deskUnrecorded) recordLeft(w);
        continue;
      }
      // Another page wrote this adopted console's record: it is recorded now,
      // and takes the place written there.
      w._deskUnrecorded = false;
      convergeRect(w, r);
      if (!!r.locked !== !!w._deskLocked) applyLock(w, !!r.locked);
      convergeName(w, r);
    }
    keepUnsavedCards(st);
    renderFences();
    renderNotes();
    applyExtent();
  }

  // The window takes the rect of its record, unless a gesture moves it now.
  function convergeRect(w: ConsoleWin, r: DeskRecord) {
    if (r.rect && !inGesture(w)) {
      placeWindow(w, r.rect);
      paintTabShadow(w);
    }
  }

  // A name written on another device, unless the name is being edited here.
  function convergeName(w: ConsoleWin, r: DeskRecord) {
    if (
      r.consoleName &&
      r.consoleName !== w._deskConsoleName &&
      !w.querySelector(".session-name-input")
    ) {
      w._deskConsoleName = r.consoleName;
      if (w._title && w._presentation) renderTitle(w, w._title, w._presentation);
    }
  }

  function placeWindow(win: HTMLElement, r: Rect) {
    const at = (prop: "left" | "top" | "width" | "height") => parseInt(win.style[prop], 10);
    if (
      at("left") === Math.round(r.left) &&
      at("top") === Math.round(r.top) &&
      at("width") === Math.round(r.width) &&
      at("height") === Math.round(r.height)
    )
      return;
    win.style.left = r.left + "px";
    win.style.top = r.top + "px";
    win.style.width = r.width + "px";
    win.style.height = r.height + "px";
    // A tile below the CSS floor (`arrangeFence`): relaxed to the cell.
    win.style.minWidth = r.width < WIN_MIN_W ? r.width + "px" : "";
    win.style.minHeight = r.height < WIN_MIN_H ? r.height + "px" : "";
  }

  // A card whose record another device removed, holding text not yet saved,
  // stays: what was typed here wins, so its record is created again.
  function keepUnsavedCards(st: HTMLElement) {
    const listed = new Set<string | undefined>(currentNotes().map((n) => n.id));
    for (const el of st.querySelectorAll<NoteCard>(".note-card")) {
      if (!el._noteDirty || !el._noteRecord || listed.has(el.dataset.noteId)) continue;
      emitDesk({ op: "create", type: "note", record: el._noteRecord });
    }
  }

  // Another device removed this window's record. A placeholder or an ended
  // console leaves this page too. A running console keeps its window and gets
  // its record back, because a running console always has one. Whether it
  // runs is asked of the daemon, not of the socket: a close on another device
  // ends the session a moment before its record goes, and this page's socket
  // may not have heard yet.
  const recordChecks = new WeakSet();
  function recordLeft(win: ConsoleWin) {
    if (recordChecks.has(win)) return;
    if (
      win.classList.contains("placeholder") ||
      win.classList.contains("ended") ||
      sessionIdOf(win) == null
    ) {
      leaveDesk([win._deskId]);
      return;
    }
    recordChecks.add(win);
    void readSessions()
      .catch(() => null)
      .then((read) => {
        recordChecks.delete(win);
        if (!win.isConnected || currentDesk().some((r) => r.id === win._deskId)) return;
        const sessions = read?.sessions;
        // Not known: the next desk this page takes asks again. A list that
        // did not hear from this window's peer does not know either.
        if (!Array.isArray(sessions) || unheardRef(win._deskRepo, read?.unheard)) return;
        const live =
          sessions.some((s) => s?.record === win._deskId) || !!sessionRowFor(win, sessions);
        if (live) createRecord(win);
        else leaveDesk([win._deskId]);
      });
  }

  // Windows whose records left the desk. The shell's hook takes them out of
  // the columns first (a lone survivor is maximized before the drops) and
  // calls `dropClosedElsewhere`; without a hook they are dropped here.
  let onDeskGone: ((ids: string[]) => void) | null = null;
  function setDeskGoneHook(fn: (ids: string[]) => void) {
    onDeskGone = fn;
  }
  function leaveDesk(ids: string[]) {
    if (typeof onDeskGone === "function") onDeskGone(ids);
    else for (const id of ids) dropClosedElsewhere(id);
  }

  return { converge, recordLeft, setDeskGoneHook };
}
