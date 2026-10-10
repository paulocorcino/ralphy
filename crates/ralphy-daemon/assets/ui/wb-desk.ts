/* ---------------------------------------------------------------------------
   The console's desk: sending its desk changes to the daemon, and restoring
   the desk layout when a page opens (ADR-0075 D7, #611).

   `createDesk(deps)` is the desk of one console: the desk view (`desk`,
   `fences`, `notes`, `checkouts`, filled from `sync`), the desk read
   (`reloadDesk`) and what follows it (the converge, a record another device
   removed, the desk failure), the upload and its answer (`flushed`), the
   `pagehide` flush, and the restore. It owns the desk state: whether the page
   has read the desk (`deskLoaded`), the failure, the upload timer and the
   batch on the wire, and whether the restore has reconciled the layout
   (`deskReconciled`) and finished (`deskSettled`). Only this module writes
   them; the console reads them through the getters it returns. It starts the
   desk read while it is built. `DeskDeps` lists every read, and the popup
   registry is reached through it.

   `createDeskRecords(deps)` is the other half: the writes of the desk
   records (a window's create, set and forget, a fence list or a card list
   committed as changes, the caps) and the reads of the checkouts. It holds no
   state of its own: the desk view is `createDesk`'s, built later, and the
   console hands it in as reads.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { sendDocument } from "./wb-events.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { apiFetch } from "./wb-api.ts";
import type { ApiRefusal } from "./wb-api.ts";
import type { WBDeskSync } from "./wb-desk-sync.ts";
import type { WBView } from "./wb-client-view.ts";
import type { Group } from "./wb-fleet.ts";
import type { OpenerLink } from "./wb-desk-detach.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { PopupMember, PopupRegistry } from "./wb-desk-popups.ts";
import type { ConsoleWin, DeskChange, DeskFence, DeskNote, DeskRecord, DeskReply, DeskWindowFields, ExtentOpts, NoteCard, NoteSource, Presentation, Rect, SpawnCarry } from "./wb-types.d.ts";

const { fenceMembership, WIN_MIN_W, WIN_MIN_H } = WBGeometry;
const { unheardRef, peerHeld, relaunchRequest, sessionRowFor } = WBConsoleSession;
const { reconcileDesk, atCap, DESK_MAX, NOTE_MAX } = WBDeskFolds;
const { sessionIdOf } = WBWindowState;

// Where an upload goes. The sink never rejects, and a network failure is one
// more answer to `flushed`.
type DeskSink = Pick<ReturnType<typeof WBDeskSink.daemon>, "put" | "putSync">;

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
  } = deps;
  const { commitDetached, newPopupEntry } = popups;

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

  // Set once the saved layout has been reconciled: `restoreDesk` has three
  // callers (boot, `afterLogin`, `startNewDesk`) and the retry below, and a
  // second reconcile would spawn every window twice.
  let deskReconciled = false;
  // Whether `restoreDesk` has finished on ANY of its exits (a failed fetch
  // included): distinguishes "empty because nothing restored
  // YET" from "empty because there is nothing to restore".
  let deskSettled = false;
  // A desk that did not load is NOT an empty desk: reconciled against `[]`,
  // every live session that names a record is adopted at the cascade under
  // that record's id. So a transport failure reads the desk again and
  // restores only once it lands. A refused (pre-login) read retries too, harmlessly;
  // an unreadable desk waits for the operator (`startNewDesk`).
  let deskRetryMs = 1000;
  function retryDeskLoad() {
    if (currentDeskFailure()) return;
    setTimeout(() => {
      reloadDesk().then(() => {
        if (deskLoaded) restoreDesk();
        else retryDeskLoad();
      });
    }, deskRetryMs);
    deskRetryMs = Math.min(deskRetryMs * 2, 30000);
  }

  // Restore the desk: reconcile the saved layout against the daemon's live
  // sessions and dispatch one window per verdict. A REJECTED fetch leaves the
  // desk untouched — no relaunch, no phantom placeholders.
  function restoreDesk() {
    Promise.all([whenDeskLoaded(), readSessions()])
      .then(async ([, { sessions, unheard }]) => {
        if (!deskLoaded) {
          retryDeskLoad();
          return;
        }
        if (deskReconciled) return;
        deskReconciled = true;
        // Members of a fence detached BEFORE this reload are live in a popup
        // that survived it: every verdict is skipped for them (#347) — a
        // `relaunch` would be a SECOND PTY. Ids from the REGISTRY first, the
        // membership fold second: a detached fence may have been moved (§7a)
        // while its members kept their old records, so the fold alone answers
        // "no members".
        const membership = fenceMembership(fences, loadDesk());
        const away = new Map<string, string>(); // window id -> the detached fence holding it
        for (const id of popups.detachedIds()) {
          const entry = popups.entry(id);
          for (const wid of entry?.memberIds || []) away.set(wid, id);
          for (const m of entry?.members || []) if (m.id) away.set(m.id, id);
          for (const wid of membership[id] || []) away.set(wid, id);
        }
        // Not restored twice: `reattachFence` can run while this fetch is in
        // flight (a `popup-gone` mid-boot) and spawn these very members.
        const onPlane = new Set([...wins].map((w) => w._deskId));
        // A relaunch into a recorded worktree first asks whether the tree is
        // still there; the stage is sized only once every such window landed.
        const pending: Promise<ConsoleWin>[] = [];
        for (const { record, session, action, id } of reconcileDesk({
          layout: loadDesk(),
          sessions,
          // Read at restore time, not module load. `canLaunch === false` (the
          // popup) refuses too: it must never author a session.
          relaunchAgents: OPTS.canLaunch !== false && viewStore?.read()?.relaunch === true,
        })) {
          // `adopt` carries NO record, so every read below is guarded — a
          // throw here is swallowed by the catch below and nothing renders.
          if (record && onPlane.has(record.id)) continue;
          if (record && away.has(record.id)) {
            // The fallback snapshot, so a popup that never answers still has
            // somewhere to come home to. `popup-here` supersedes this.
            const entry = popups.entry(away.get(record.id)!);
            if (entry && !entry.members.some((m) => m.id === record.id)) {
              entry.members.push({ ...record, session: record.sessionId ?? null });
            }
            continue;
          }
          if (action === "attach") {
            spawnWindow(
              { id: session.id, repo: session.repo },
              session.agent || "console",
              session.repo,
              record,
            );
          } else if (action === "relaunch" && unheardRef(record.repo, unheard)) {
            // The list did not hear from its peer: the console may still run
            // there, and a launch on a peer without the record join would be
            // a SECOND shell. The box asks again on the next fleet read.
            spawnPlaceholder(record, null, null, { unheard: true });
          } else if (action === "relaunch" && peerHeld(record.repo, peerGroups())) {
            // Its peer cannot serve it: a launch would only be refused.
            spawnPlaceholder(record);
          } else if (action === "relaunch") {
            // `relaunchRequest` carries the worktree the record was in (#411).
            pending.push(
              spawnOrMissing(relaunchRequest(record), record.agent, record.repo, record),
            );
          } else if (action === "placeholder") {
            spawnPlaceholder(record, null, null, { unheard: unheardRef(record.repo, unheard) });
          } else if (!(id && onPlane.has(id))) {
            // `adopt`: a cascaded window, keeping the live session's own kind
            // so the desk relaunches it correctly next time, under the record
            // the session names when it names one.
            spawnWindow(
              { id: session.id, repo: session.repo },
              session.agent || "console",
              session.repo,
              {
                id: id || undefined,
                kind: session.kind,
                daemonId: session.daemon_id,
                environment: session.environment,
                checkout: session.checkout ?? null,
                unrecorded: true,
              },
            );
          }
        }
        await Promise.allSettled(pending);
        // A desk saved on a larger screen keeps its rects verbatim (#336): the
        // STAGE grows to hold them. Fences go on the stage BEFORE that fold.
        // Re-persist the member ids the fallback seeded, so a fence whose popup
        // never answers still skips its members on the NEXT reload.
        if (popups.detachedIds().length) commitDetached(popups.detachedIds());
        renderFences();
        renderNotes();
        // The glyph is DOM the fences own: lit only once they are on the stage.
        for (const id of popups.detachedIds()) showDetachGlyph(id, true);
        applyExtent();
        raiseMaximized();
        // Every stored id that can be on this stage is on it now: the shell
        // restores the columns from here, once.
        sendDocument(document, "workbench:desk-restored");
        deskSettled = true;
        applyLanding();
      })
      .catch(() => {
        // A refused/unreachable desk restores nothing. The glyph is lit ANYWAY:
        // a live popup with no way home is worse than an unreachable daemon.
        for (const id of popups.detachedIds()) showDetachGlyph(id, true);
        deskSettled = true;
        applyLanding();
      });
  }

  // Adopt what this tab left behind before its reload, SYNCHRONOUSLY and
  // BEFORE `restoreDesk` is issued, so `isDetached` already answers true when that
  // fetch decides which records to put on the plane.
  function restoreDetached() {
    const saved = link.readRegistry();
    if (!saved.length) return;
    const savedMembers = link.readMembers();
    for (const id of saved) {
      // No handle (it died with the document), only member ids. `popup-here`
      // hands the snapshot back; `restoreDesk` seeds a fallback from the
      // records it skips.
      popups.put(id, newPopupEntry(savedMembers[id] || []));
    }
    commitDetached(saved);
    link.post({ type: "origin-here", tab: link.tab });
  }

  // The POPUP's side: render the members the opener handed over, translated by
  // the fence origin to sit near this window's top-left. The untranslated
  // snapshot stays in the OPENER — nothing measured here ever goes back.
  function mountDetached(
    fence: { rect?: Partial<Rect> | null } | null | undefined,
    members: PopupMember[] | null | undefined,
  ) {
    const originLeft = fence?.rect?.left || 0;
    const originTop = fence?.rect?.top || 0;
    for (const m of members || []) {
      const record = {
        ...m,
        rect: {
          ...m.rect,
          left: (m.rect?.left || 0) - originLeft + 12,
          top: (m.rect?.top || 0) - originTop + 12,
        },
      };
      if (m.kind === "note") {
        window.WBNotes?.mountDetached(record as NoteSource);
      } else if (m.session != null) {
        spawnWindow(
          { id: m.session, repo: m.repo },
          m.agent || "console",
          m.repo,
          record,
        );
      } else {
        spawnPlaceholder(record);
      }
    }
    applyExtent();
  }

  // Whether the restore has reconciled the saved layout: a desk this page
  // takes converges on the stage only after it.
  function isDeskReconciled() {
    return deskReconciled;
  }
  // Whether the restore has finished: the landing latches only after it.
  function isDeskSettled() {
    return deskSettled;
  }

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
    for (const w of [...st.querySelectorAll<ConsoleWin>(".session-window")]) {
      const r = byId.get(w._deskId);
      if (!r) {
        if (!w._deskUnrecorded) recordLeft(w);
        continue;
      }
      // Another page wrote this adopted console's record: it is recorded now,
      // and takes the place written there.
      w._deskUnrecorded = false;
      if (r.rect && !inGesture(w)) placeWindow(w, r.rect);
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
  function flushed(out: { kind: string; status?: number; reply?: DeskReply | null }) {
    const kind = out?.kind;
    if (kind === "held") return;
    if (kind === "ok") {
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
      return;
    }
    if (kind === "refused") {
      const state = out.reply?.state;
      if (out.status === 409 && state === "restored") {
        reloadForRestoredDesk();
        return;
      }
      // The daemon will never accept this batch.
      if (out.status === 400 || out.status === 422) {
        console.warn("desk changes refused:", out.reply?.error);
        sync.dropped();
        if (sync.hasPending()) scheduleDeskFlush();
        return;
      }
      // The daemon refuses a write over a desk it cannot read; the page shows
      // the failure and sends nothing until the desk reads again.
      if (out.status === 409 && state === "unreadable") {
        if (out.reply?.error) console.warn("saved desk:", out.reply.error);
        deskFailure = "the file is damaged";
        deskLoaded = false;
        sync.fail();
        onDeskFailure?.();
        return;
      }
    }
    // A network failure, a 401 before a login, a 5xx: kept, and sent again.
    scheduleDeskFlush(flushBackoff);
    flushBackoff = Math.min(flushBackoff * 2, 30000);
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
  };
}

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
  const { sync, desk, fences, notes, checkouts, refreshView, emitDesk, scheduleDeskFlush, restoreRect, refreshFenceChrome, consolePrefix } = deps;
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
  const rectOnly = (r: Rect | undefined) => (r ? { left: r.left, top: r.top, width: r.width, height: r.height } : null);
  const SET_FIELDS: Record<"fence" | "note", (r: Partial<DeskFence & DeskNote>) => { [field: string]: JsonValue | undefined }> = {
    fence: (r) => ({ rect: rectOnly(r.rect), name: r.name ?? "", locked: !!r.locked }),
    note: (r) => ({
      rect: rectOnly(r.rect),
      locked: !!r.locked,
      file: { repo: r.repo, path: r.path ?? "", checkout: r.checkout ?? null },
    }),
  };
  function commitList(type: "fence" | "note", before: (DeskFence | DeskNote)[], next: (DeskFence | DeskNote)[]) {
    const was = new Map<string, DeskFence | DeskNote>(before.map((r) => [r.id, r]));
    const kept = new Set(next.map((r) => r.id));
    const changes = before.filter((r) => !kept.has(r.id)).map((r): DeskChange => ({ op: "remove", type, id: r.id }));
    for (const r of next) {
      const old = was.get(r.id);
      if (!old) {
        changes.push({ op: "create", type, record: r } as DeskChange);
        continue;
      }
      const a = SET_FIELDS[type](old);
      const b = SET_FIELDS[type](r);
      const fields: { [field: string]: JsonValue | undefined } = {};
      for (const k of Object.keys(b)) if (JSON.stringify(a[k]) !== JSON.stringify(b[k])) fields[k] = b[k];
      if (Object.keys(fields).length) changes.push({ op: "set", type, id: r.id, fields } as DeskChange);
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
      if (desk()[i].consoleName || !r.consoleName) return;
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
    if (name) emitDesk({ op: "checkout", repo: ref, name: String(name) });
    else if (checkouts()[ref]) emitDesk({ op: "checkout-clear", repo: ref, ifName: checkouts()[ref] });
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
