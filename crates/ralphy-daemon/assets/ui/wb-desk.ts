/* ---------------------------------------------------------------------------
   The console's desk: sending its desk changes to the daemon, and restoring
   the desk layout when a page opens (ADR-0075 D7, #611).

   `createDesk(deps)` is the desk of one console: the desk view (`desk`,
   `fences`, `notes`, `checkouts`, filled from `sync`), the desk read
   (`reloadDesk`) and what follows it (the converge, a record another device
   removed, the desk failure), the upload and its answer (`flushed`), and the
   `pagehide` flush, the boot (`boot`) and the read after a login
   (`afterLogin`). It owns the desk state: whether the page has read the
   desk (`deskLoaded`), the failure, the upload timer and the batch on the
   wire. Only this module writes them; the console reads them through the
   getters it returns. It starts the desk read while it is built. `DeskDeps`
   lists every read, and the popup registry is reached through it.

   The restore is `createDeskRestore` (`wb-desk-restore.ts`), built inside
   `createDesk`; the writes of the desk records are `createDeskRecords`
   (`wb-desk-records.ts`), built by the console.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { createDeskRestore } from "./wb-desk-restore.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { paintTabShadow } from "./wb-console-place.ts";
import { apiFetch } from "./wb-api.ts";
import type { ApiRefusal } from "./wb-api.ts";
import type { WBDeskSync } from "./wb-desk-sync.ts";
import type { WBView } from "./wb-client-view.ts";
import type { Group } from "./wb-fleet.ts";
import type { OpenerLink } from "./wb-desk-detach.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { PopupRegistry } from "./wb-desk-popups.ts";
import type { ConsoleWin, DeskChange, DeskFence, DeskNote, DeskRecord, DeskReply, ExtentOpts, NoteCard, Presentation, Rect, SpawnCarry } from "./wb-types.d.ts";

const { WIN_MIN_W, WIN_MIN_H } = WBGeometry;
const { unheardRef, sessionRowFor } = WBConsoleSession;
const { sessionIdOf } = WBWindowState;

// Where an upload goes. The sink never rejects, and a network failure is one
// more answer to `flushed`.
type DeskSink = Pick<ReturnType<typeof WBDeskSink.daemon>, "put" | "putSync">;
// The answer to one upload, as `flushed` reads it.
type FlushAnswer = { kind: string; status?: number; reply?: DeskReply | null };

// What the desk reads from the console, and nothing else.
export type DeskDeps = {
  // The console's page: a detached note card mounts through it.
  window: Window;
  // The page the restore tells that every stored window is on the stage.
  document: Document;
  // The console's options: whether it may launch a session, and whether it
  // is the popup (`autoBoot: false`), whose stage never converges.
  OPTS: { canLaunch?: boolean; autoBoot?: boolean };
  // Where an upload goes (`wb-desk-sink.ts`).
  deskSink: DeskSink;
  // The desk view and the changes not yet sent (`wb-desk-sync.ts`).
  sync: ReturnType<typeof WBDeskSync.createSync>;
  // The per-client view: whether a restore relaunches agents.
  viewStore: Pick<typeof WBView, "read">;
  // This tab's stored detach registry and the lifecycle channel.
  link: OpenerLink;
  // The fences this tab detached and their popup entries.
  popups: PopupRegistry;
  // The console windows on the stage.
  wins: Set<ConsoleWin>;
  // The stage the converge puts the records on, or null before it exists.
  stage: () => HTMLElement | null;
  // Every console window of the page, also one in a console tab.
  pageWindows: () => ConsoleWin[];
  // The fleet's last answer, grouped by peer, for the held rule.
  peerGroups: () => Map<string, Group>;
  // The window records, as a copy.
  loadDesk: () => DeskRecord[];
  // The daemon's live sessions, and the peers that did not answer.
  readSessions: () => Promise<{ sessions: HostedSession[]; unheard: ReadonlySet<string> }>;
  // Puts a window, a missing worktree or a placeholder on the stage.
  spawnWindow: (
    termOpts: TerminalOpts,
    label: string | null | undefined,
    repo: string | null | undefined,
    desk?: SpawnCarry,
  ) => ConsoleWin;
  spawnOrMissing: (
    req: TerminalOpts,
    label: string | null | undefined,
    repo: string | null | undefined,
    carry: SpawnCarry,
  ) => Promise<ConsoleWin>;
  spawnPlaceholder: (
    record: SpawnCarry,
    missing?: string | null,
    refused?: { message: string } | null,
    held?: { unheard?: boolean },
  ) => ConsoleWin;
  // Puts the fences and the cards on the stage, and lights a detach glyph.
  renderFences: () => void;
  renderNotes: () => void;
  showDetachGlyph: (id: string, on: boolean) => void;
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: ExtentOpts) => void;
  // Raises the maximized windows over the others.
  raiseMaximized: () => void;
  // Scrolls the viewport to its landing place.
  applyLanding: () => void;
  // Whether a window is under a gesture: the converge does not move it.
  inGesture: (el: HTMLElement) => boolean;
  // Names the records that have no name (`createDeskRecords`).
  nameUnnamed: () => void;
  // Writes a window's record (`createDeskRecords`).
  createRecord: (win: ConsoleWin) => void;
  // Locks or unlocks a window, and paints its title.
  applyLock: (win: ConsoleWin, locked: boolean) => void;
  renderTitle: (win: ConsoleWin, title: HTMLElement, presentation: Presentation) => void;
  // Takes a window whose record left the desk off the stage.
  dropClosedElsewhere: (id: string) => void;
  // Stores the per-client view offset not yet written.
  flushPendingOffset: () => void;
  // Registers the stage's own listeners (`createView`): the boot runs it first.
  wireStage: () => void;
};

export function createDesk(deps: DeskDeps) {
  const {
    window,
    document,
    OPTS,
    deskSink,
    sync,
    viewStore,
    link,
    popups,
    wins,
    stage,
    pageWindows,
    peerGroups,
    loadDesk,
    readSessions,
    spawnWindow,
    spawnOrMissing,
    spawnPlaceholder,
    renderFences,
    renderNotes,
    showDetachGlyph,
    applyExtent,
    raiseMaximized,
    applyLanding,
    inGesture,
    nameUnnamed,
    createRecord,
    applyLock,
    renderTitle,
    dropClosedElsewhere,
    flushPendingOffset,
    wireStage,
  } = deps;

  // ---- the desk layout ---------------------------------------------------------
  // What was open, not merely where a session sat: one record per window keyed
  // by a STABLE client-side id (repo, agent, session kind, rect, maximized).
  // The daemon's session id is a volatile ATTRIBUTE — a restarted daemon hands
  // out ids from 1 again. The desk lives in the DAEMON (`GET`/`PUT /api/desk`,
  // ADR-0050), and a page writes it only as desk changes, each one carrying
  // the fields its act changed (ADR-0050 amendment 2026-10-04, changes, not
  // the desk). `sync` (wb-desk-sync.ts) holds the daemon's last desk and this
  // page's unanswered changes; `desk`, `fences`, `notes` and `checkouts` are
  // its view, the SYNCHRONOUS source of truth every read below uses.
  let desk: DeskRecord[] = [];
  // Second record type (#340): named rectangles on the floor tier.
  let fences: DeskFence[] = [];
  // Third record type (ADR-0064 §2): note cards, PLACEMENT only — the note's
  // text and colour live in its `.note` file. The CARD itself (DOM, editor,
  // autosave) is `wb-notes.ts`, which reaches this state through the
  // console's exports.
  let notes: DeskNote[] = [];
  // Fourth record type (#406, ADR-0063 §4): the selected checkout per repo ref,
  // `{ <ref>: <worktree name> }`. The reactive copy the chip and the tree
  // render lives in `app.ts` (a closure variable here is invisible to Alpine).
  let checkouts: Record<string, string> = {};
  function refreshView() {
    const v = sync.view();
    desk = v.windows;
    fences = v.fences;
    notes = v.notes;
    checkouts = v.checkouts;
  }
  // The page has read the desk.
  // Nothing is sent before: a page that never read the desk does not know its
  // generation. Under the `Session` policy the pre-login GET answers 401, so
  // this stays false until `reloadDesk()` succeeds after login.
  let deskLoaded = false;

  // A restore from the desk history since this page read the desk (ADR-0050
  // amendment 2026-10-04, desk history): the daemon refuses this page's next
  // write, and the page reloads to show the restored desk.
  let deskRestored = false;
  function reloadForRestoredDesk() {
    if (deskRestored) return;
    deskRestored = true;
    sync.restored();
    // The `pagehide` flush would send this page's changes on the way out.
    WBDeskSink.setHold(true);
    window.location?.reload?.();
  }
  // ONE desk change: the view takes it at once, the daemon on the next flush.
  function emitDesk(change: DeskChange) {
    sync.emit(change);
    refreshView();
    scheduleDeskFlush();
  }

  // The upload, debounced. WHERE it goes is `deskSink`'s business
  // (wb-desk-sink.ts), which answers a typed result. One batch is on the wire
  // at a time; a batch that failed in a way the daemon may still accept is
  // sent again with the same `seq`, later.
  let deskFlush: ReturnType<typeof setTimeout> | null = null;
  let flushing = false;
  function scheduleDeskFlush(ms = 250) {
    clearTimeout(deskFlush);
    // Cleared when it FIRES too: a spent timer id is still truthy.
    deskFlush = setTimeout(() => {
      deskFlush = null;
      flushDesk();
    }, ms);
  }
  function flushDesk() {
    if (flushing || sync.phase() !== "ready") return;
    const body = sync.nextBatch();
    if (!body) return;
    flushing = true;
    deskSink
      .put(JSON.stringify(body))
      .catch(() => ({ kind: "network" }))
      .then((out) => {
        flushing = false;
        flushed(out);
      });
  }

  // The last batch, as the tab closes: the console calls this from its
  // `pagehide`, after the per-client view and the notes.
  function flushDeskOnClose() {
    if (!deskLoaded || sync.phase() !== "ready") return;
    const body = sync.closingBatch();
    if (!body) return;
    clearTimeout(deskFlush);
    deskFlush = null;
    deskSink.putSync(JSON.stringify(body));
  }

  const { restoreDesk, restoreDetached, mountDetached, isDeskReconciled, isDeskSettled } = createDeskRestore({
    window,
    document,
    OPTS,
    viewStore,
    link,
    popups,
    wins,
    peerGroups,
    loadDesk,
    readSessions,
    spawnWindow,
    spawnOrMissing,
    spawnPlaceholder,
    renderFences,
    renderNotes,
    showDetachGlyph,
    applyExtent,
    raiseMaximized,
    applyLanding,
    fences: currentFences,
    isDeskLoaded,
    whenDeskLoaded,
    reloadDesk,
    currentDeskFailure,
  });

  function ingestDesk(payload: Parameters<typeof sync.take>[0]) {
    const verdict = sync.take(payload);
    if (verdict === "reload") {
      reloadForRestoredDesk();
      return;
    }
    if (verdict !== "taken") return;
    deskLoaded = true;
    refreshView();
    nameUnnamed();
    converge();
  }

  // The screens converge (ADR-0050 amendment 2026-10-04): every desk this page
  // takes puts each window's rect, lock and name, and each fence and card, on
  // the stage. The view already holds this page's own unanswered changes, so
  // a value this page set and the daemon has not answered yet stays. `max` is
  // left alone: a phone that maximizes a console must not maximize it on the
  // PC. A console opened on another device appears at the next load: opening
  // it here would attach or start a process with no act on this page.
  //
  // Only on a stage `restoreDesk` has filled: before that, it puts every
  // record on the stage itself. Never in the popup, whose stage holds one
  // fence's members at translated places.
  function converge() {
    if (OPTS.autoBoot === false || !isDeskReconciled()) return;
    const st = stage();
    if (!st) return;
    const byId = new Map<string, DeskRecord>(desk.map((r) => [r.id, r]));
    for (const w of pageWindows()) {
      const r = byId.get(w._deskId);
      if (!r) {
        if (!w._deskUnrecorded) recordLeft(w);
        continue;
      }
      // Another page wrote this adopted console's record: it is recorded now,
      // and takes the place written there.
      w._deskUnrecorded = false;
      if (r.rect && !inGesture(w)) {
        placeWindow(w, r.rect);
        paintTabShadow(w);
      }
      if (!!r.locked !== !!w._deskLocked) applyLock(w, !!r.locked);
      if (r.consoleName && r.consoleName !== w._deskConsoleName && !w.querySelector(".session-name-input")) {
        w._deskConsoleName = r.consoleName;
        if (w._title && w._presentation) renderTitle(w, w._title, w._presentation);
      }
    }
    keepUnsavedCards(st);
    renderFences();
    renderNotes();
    applyExtent();
  }

  function placeWindow(win: HTMLElement, r: Rect) {
    const at = (prop: "left" | "top" | "width" | "height") => parseInt(win.style[prop], 10);
    if (at("left") === Math.round(r.left) && at("top") === Math.round(r.top) &&
        at("width") === Math.round(r.width) && at("height") === Math.round(r.height)) return;
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
    const listed = new Set<string | undefined>(notes.map((n) => n.id));
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
    if (win.classList.contains("placeholder") || win.classList.contains("ended") || sessionIdOf(win) == null) {
      leaveDesk([win._deskId]);
      return;
    }
    recordChecks.add(win);
    readSessions()
      .catch(() => null)
      .then((read) => {
        recordChecks.delete(win);
        if (!win.isConnected || desk.some((r) => r.id === win._deskId)) return;
        const sessions = read?.sessions;
        // Not known: the next desk this page takes asks again. A list that
        // did not hear from this window's peer does not know either.
        if (!Array.isArray(sessions) || unheardRef(win._deskRepo, read!.unheard)) return;
        const live = sessions.some((s) => s?.record === win._deskId) || !!sessionRowFor(win, sessions);
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

  // Why the daemon cannot read the saved desk, or "" (ADR-0070 D4). Set only
  // by the daemon's own `409 {"state":"unreadable"}`: a transport failure or
  // a pre-login 401 is not a broken desk, and must not offer a new one.
  let deskFailure = "";
  // The shell's hook for a desk failure found by a flush: no push says so.
  let onDeskFailure: (() => void) | null = null;
  // The `409` reply of an unreadable desk, as the reason to show, or null.
  // The daemon's own text is a parser message for a developer: it goes to
  // the browser console, and the operator reads what it means.
  async function unreadableDesk(r: ApiRefusal<"GET /api/desk">) {
    if (r.status !== 409) return null;
    const body = await r.json().catch(() => null);
    if (body?.state !== "unreadable") return null;
    if (body.error) console.warn("saved desk:", body.error);
    return "the file is damaged";
  }

  // Load (or re-load, after a login or a push) the daemon's desk. Never
  // rejects: an unreachable daemon leaves the page as it was. An unreadable
  // desk sets `deskFailure` and stops the sending.
  function reloadDesk() {
    return apiFetch("GET /api/desk")
      .then(async (r) => {
        if (r.ok) return r.json();
        const why = await unreadableDesk(r);
        if (why) {
          deskFailure = why;
          deskLoaded = false;
          sync.fail();
          return null;
        }
        throw new Error("desk unavailable");
      })
      .then((payload) => {
        if (!payload) return;
        deskFailure = "";
        ingestDesk(payload);
        // Changes kept while the daemon could not be reached go now.
        if (sync.hasPending()) scheduleDeskFlush();
      })
      .catch(() => {});
  }
  function currentDeskFailure() {
    return deskFailure;
  }
  function setDeskFailureHook(fn: () => void) {
    onDeskFailure = fn;
  }
  // The one action on an unreadable desk: the daemon renames the old file
  // aside and starts an empty desk; this page then reads and restores it.
  function startNewDesk() {
    return apiFetch("POST /api/desk/new")
      .then(async (r) => {
        if (r.ok) return;
        // 409 "readable": another tab started the new desk first. The desk is
        // readable, so this tab reads it like any other.
        const body = r.status === 409 ? await r.json().catch(() => null) : null;
        if (body && "state" in body && body.state === "readable") return;
        throw new Error(`the daemon answered ${r.status}`);
      })
      .then(() => reloadDesk())
      .then(() => {
        if (!deskLoaded) return;
        // Consoles opened over the unreadable desk queued their records: they
        // go on this flush. With none up, the boot restore never ran, so it
        // runs now.
        if (wins.size) scheduleDeskFlush();
        else restoreDesk();
      });
  }
  // `restoreDesk` awaits this before reconciling, so the layout is never
  // reconciled against a desk that has not landed.
  const deskReady = reloadDesk();

  // Resolves once the boot desk load has settled (landed OR refused) — what
  // `app.ts` awaits before copying the view into its reactive map.
  function whenDeskLoaded() {
    return deskReady;
  }

  // The answer to one upload (`flushDesk` above): a batch
  // that failed in a way the daemon may still accept is sent again with the
  // same `seq`, later.
  let flushBackoff = 1000;
  function flushed(out: FlushAnswer) {
    const kind = out?.kind;
    if (kind === "held") return;
    if (kind === "ok") {
      flushedOk(out);
      return;
    }
    if (kind === "refused" && flushedRefused(out)) return;
    // A network failure, a 401 before a login, a 5xx: kept, and sent again.
    scheduleDeskFlush(flushBackoff);
    flushBackoff = Math.min(flushBackoff * 2, 30000);
  }
  // The daemon took the batch: the desk it sent is the view now.
  function flushedOk(out: FlushAnswer) {
    flushBackoff = 1000;
    for (const r of out.reply?.refused || []) console.warn("desk change refused:", r.error);
    const verdict = sync.acked(out.reply);
    if (verdict === "reload") {
      reloadForRestoredDesk();
      return;
    }
    if (verdict === "taken") {
      refreshView();
      converge();
    }
    if (sync.hasPending()) scheduleDeskFlush();
  }
  // The daemon refused the batch. True when the refusal is handled; false
  // for one that is kept and sent again.
  function flushedRefused(out: FlushAnswer): boolean {
    const state = out.reply?.state;
    if (out.status === 409 && state === "restored") {
      reloadForRestoredDesk();
      return true;
    }
    // The daemon will never accept this batch.
    if (out.status === 400 || out.status === 422) {
      console.warn("desk changes refused:", out.reply?.error);
      sync.dropped();
      if (sync.hasPending()) scheduleDeskFlush();
      return true;
    }
    if (out.status === 409 && state === "unreadable") {
      deskUnreadable(out);
      return true;
    }
    return false;
  }
  // The daemon refuses a write over a desk it cannot read; the page shows
  // the failure and sends nothing until the desk reads again.
  function deskUnreadable(out: FlushAnswer) {
    if (out.reply?.error) console.warn("saved desk:", out.reply.error);
    deskFailure = "the file is damaged";
    deskLoaded = false;
    sync.fail();
    onDeskFailure?.();
  }
  // A mutation in the last 250 ms before the tab closes would otherwise be
  // dropped. The sink's `putSync` rides `keepalive`, which outlives the document.
  window.addEventListener("pagehide", () => {
    // The per-client view first, and NOT behind the desk guard: `WBView`'s store
    // is synchronous and `deskLoaded` says nothing about it — gating it would
    // drop the last pan of every pre-login page.
    flushPendingOffset();
    // Every dirty note, too (ADR-0064 §7): the autosave debounce is 800 ms, so
    // without this the last sentence typed before a close is gone. A best
    // effort — the socket may not finish — for the same reason and with the
    // same bargain as the desk's own last flush below.
    window.WBNotes?.flushAll();
    // NOTHING closes a detached popup here: `pagehide` fires on a RELOAD exactly
    // as on a close, with no reliable discriminator (#347). The popup declares
    // its peer lost after `PEER_WINDOW_MS` without a beat and closes itself,
    // which covers a clean close and a force-kill alike (ADR-0051 §8).
    flushDeskOnClose();
  });

  // Whether the page has read the desk.
  function isDeskLoaded() {
    return deskLoaded;
  }
  // The desk view, as `sync` last filled it: the console's reads.
  function currentDesk() {
    return desk;
  }
  function currentFences() {
    return fences;
  }
  function currentNotes() {
    return notes;
  }
  function currentCheckouts() {
    return checkouts;
  }

  // `autoBoot: false` is the popup, which renders only the members
  // its opener hands over. The entry module calls this once, after its page
  // has started.
  function boot() {
    wireStage();
    restoreDetached();
    if (OPTS.autoBoot !== false) restoreDesk();
  }

  // Re-read the daemon's desk and restore it: the pre-login `/api/desk`
  // answered 401 under `Session`. Called from `rehydrateAfterAuth` (#327).
  function afterLogin() {
    return reloadDesk().then(() => {
      // `popups.size()` counts as "windows already up": detaching every
      // fence drives `wins.size` to 0, and a `restoreDesk` would respawn the
      // popups' members.
      if (isDeskLoaded() && wins.size === 0 && popups.size() === 0) {
        restoreDesk();
        return;
      }
      // Windows already up: nothing else would put the just-loaded fences on
      // the stage. Gated on the permit: a REFUSED load leaves `fences` as
      // whatever this page drew.
      if (!isDeskLoaded()) return;
      renderFences();
      renderNotes();
      applyExtent();
    });
  }

  return {
    emitDesk,
    scheduleDeskFlush,
    flushDeskOnClose,
    restoreDesk,
    restoreDetached,
    mountDetached,
    isDeskReconciled,
    isDeskSettled,
    refreshView,
    reloadForRestoredDesk,
    recordLeft,
    reloadDesk,
    currentDeskFailure,
    setDeskFailureHook,
    setDeskGoneHook,
    startNewDesk,
    whenDeskLoaded,
    isDeskLoaded,
    currentDesk,
    currentFences,
    currentNotes,
    currentCheckouts,
    boot,
    afterLogin,
  };
}
