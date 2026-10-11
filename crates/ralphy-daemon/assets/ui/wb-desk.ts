/* ---------------------------------------------------------------------------
   The console's desk: sending its desk changes to the daemon, and restoring
   the desk layout when a page opens (ADR-0075 D7, #611).

   `createDesk(deps)` returns `emitDesk`, `scheduleDeskFlush`,
   `flushDeskOnClose`, `restoreDesk`, `restoreDetached` and `mountDetached`
   for one console. It owns the desk flush state: the upload timer and the
   batch on the wire (`deskFlush`, `flushing`), and whether the restore has
   reconciled the layout (`deskReconciled`) and finished (`deskSettled`).
   Only this module writes them; the console reads the last two through
   `isDeskReconciled` and `isDeskSettled`. `DeskDeps` lists every read, and
   the popup registry is reached through it. `wb-console.ts` keeps the desk
   view, the desk read (`reloadDesk`) and the answer to an upload (`flushed`).

   `createDeskRecords(deps)` is the other half: the writes of the desk
   records (a window's create, set and forget, a fence list or a card list
   committed as changes, the caps) and the reads of the checkouts. It holds no
   state of its own: the console keeps the desk view and hands it in as reads.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { sendDocument } from "./wb-events.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import { WBWindowState } from "./wb-window-state.ts";
import type { WBDeskSink } from "./wb-desk-sink.ts";
import type { WBDeskSync } from "./wb-desk-sync.ts";
import type { WBView } from "./wb-client-view.ts";
import type { Group } from "./wb-fleet.ts";
import type { OpenerLink } from "./wb-desk-detach.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { PopupMember, PopupRegistry } from "./wb-desk-popups.ts";
import type { ConsoleWin, DeskChange, DeskFence, DeskNote, DeskRecord, DeskWindowFields, ExtentOpts, NoteSource, Rect, SpawnCarry } from "./wb-types.d.ts";

const { fenceMembership } = WBGeometry;
const { unheardRef, peerHeld, relaunchRequest } = WBConsoleSession;
const { reconcileDesk, DESK_MAX, NOTE_MAX } = WBDeskFolds;
const { sessionIdOf } = WBWindowState;

// Where an upload goes, and what it answers: the sink never rejects, and a
// network failure is one more answer.
type DeskSink = Pick<ReturnType<typeof WBDeskSink.daemon>, "put" | "putSync">;
type DeskUpload = Awaited<ReturnType<DeskSink["put"]>> | { kind: "network" };

// What the desk reads from the console, and nothing else.
export type DeskDeps = {
  // The console's page: a detached note card mounts through it.
  window: Window;
  // The page the restore tells that every stored window is on the stage.
  document: Document;
  // The console's options: whether it may launch a session.
  OPTS: { canLaunch?: boolean };
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
  // The fence records; the console reassigns the list, so it is read when
  // the restore needs it.
  fences: () => DeskFence[];
  // The fleet's last answer, grouped by peer, for the held rule.
  peerGroups: () => Map<string, Group>;
  // Whether the page has read the desk, and why the daemon cannot read it
  // ("" when it can).
  deskLoaded: () => boolean;
  currentDeskFailure: () => string;
  // Resolves once the boot desk read has settled.
  whenDeskLoaded: () => Promise<unknown>;
  // Reads the desk again.
  reloadDesk: () => Promise<unknown>;
  // The view takes the changes `sync` holds.
  refreshView: () => void;
  // The answer to one upload.
  flushed: (out: DeskUpload) => void;
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
    fences,
    peerGroups,
    deskLoaded,
    currentDeskFailure,
    whenDeskLoaded,
    reloadDesk,
    refreshView,
    flushed,
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
  } = deps;
  const { commitDetached, newPopupEntry } = popups;
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
    if (!deskLoaded() || sync.phase() !== "ready") return;
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
        if (deskLoaded()) restoreDesk();
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
        if (!deskLoaded()) {
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
        const membership = fenceMembership(fences(), loadDesk());
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

  return {
    emitDesk,
    scheduleDeskFlush,
    flushDeskOnClose,
    restoreDesk,
    restoreDetached,
    mountDetached,
    isDeskReconciled,
    isDeskSettled,
  };
}

// What the record writes read from the console, and nothing else. The desk
// view (`desk`, `fences`, `notes`, `checkouts`) is the console's: it
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
    return desk().length >= DESK_MAX;
  }
  // Whether another card would be over the cap — asked before a card is born,
  // so the open is refused instead of quietly evicting one that is on screen.
  function atNoteCap() {
    return notes().length >= NOTE_MAX;
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
