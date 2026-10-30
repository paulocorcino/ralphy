/* ---------------------------------------------------------------------------
   The desk restore (ADR-0075 D7, #621): when a page opens, `restoreDesk`
   reconciles the saved layout against the daemon's live sessions and puts one
   window per verdict on the stage; `restoreDetached` adopts the fences this
   tab detached before its reload; `mountDetached` is the popup's side.

   `createDeskRestore(deps)` owns whether the restore has reconciled the
   layout (`deskReconciled`) and finished (`deskSettled`). The desk state it
   reads (the desk read, the fences, the failure) is `createDesk`'s and comes
   in as reads.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { sendDocument } from "./wb-events.ts";
import type { DeskDeps } from "./wb-desk.ts";
import type { PopupEntry, PopupMember } from "./wb-desk-popups.ts";
import type { ConsoleWin, DeskFence, DeskRecord, NoteSource, Rect } from "./wb-types.d.ts";

const { fenceMembership } = WBGeometry;
const { unheardRef, peerHeld, relaunchRequest } = WBConsoleSession;
const { reconcileDesk } = WBDeskFolds;

// One verdict of the restore fold on this page's records and sessions.
type Verdict = ReturnType<typeof reconcileDesk<HostedSession, DeskRecord>>[number];

// What the restore reads from the desk and the console, and nothing else.
export type DeskRestoreDeps = Pick<
  DeskDeps,
  | "window"
  | "document"
  | "OPTS"
  | "viewStore"
  | "link"
  | "popups"
  | "wins"
  | "peerGroups"
  | "loadDesk"
  | "readSessions"
  | "spawnWindow"
  | "spawnOrMissing"
  | "spawnPlaceholder"
  | "renderFences"
  | "renderNotes"
  | "showDetachGlyph"
  | "applyExtent"
  | "raiseMaximized"
  | "applyLanding"
> & {
  // The fences of the desk view (`createDesk`).
  fences: () => DeskFence[];
  // Whether the page has read the desk.
  isDeskLoaded: () => boolean;
  // Resolves once the boot desk load has settled.
  whenDeskLoaded: () => Promise<unknown>;
  // Loads the daemon's desk again; never rejects.
  reloadDesk: () => Promise<unknown>;
  // Why the daemon cannot read the desk, or "".
  currentDeskFailure: () => string;
};

export function createDeskRestore(deps: DeskRestoreDeps) {
  const {
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
    fences,
    isDeskLoaded,
    whenDeskLoaded,
    reloadDesk,
    currentDeskFailure,
  } = deps;
  const { commitDetached, newPopupEntry } = popups;

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
      void reloadDesk().then(() => {
        if (isDeskLoaded()) restoreDesk();
        else retryDeskLoad();
      });
    }, deskRetryMs);
    deskRetryMs = Math.min(deskRetryMs * 2, 30000);
  }

  // Members of a fence detached BEFORE this reload are live in a popup that
  // survived it: every verdict is skipped for them (#347) — a `relaunch`
  // would be a SECOND PTY. Ids from the REGISTRY first, the membership fold
  // second: a detached fence may have been moved (§7a) while its members kept
  // their old records, so the fold alone answers "no members".
  // The map is window id -> the detached fence holding it.
  function awayMap() {
    const membership = fenceMembership(fences(), loadDesk());
    const away = new Map<string, string>();
    for (const id of popups.detachedIds()) {
      for (const wid of entryMemberIds(popups.entry(id))) away.set(wid, id);
      for (const wid of membership[id] || []) away.set(wid, id);
    }
    return away;
  }

  // The window ids a popup entry names: the ids it stored, then its snapshot.
  function entryMemberIds(entry: PopupEntry | undefined) {
    const snapshot = (entry?.members || []).filter((m) => m.id).map((m) => m.id);
    return [...(entry?.memberIds || []), ...snapshot];
  }

  // The fallback snapshot, so a popup that never answers still has somewhere
  // to come home to. `popup-here` supersedes this.
  function seedAway(record: DeskRecord, fenceId: string) {
    const entry = popups.entry(fenceId);
    if (entry && !entry.members.some((m) => m.id === record.id)) {
      entry.members.push({ ...record, session: record.sessionId ?? null });
    }
  }

  // One window on the stage for one verdict. A relaunch that waits for its
  // worktree check goes in `pending`.
  function dispatchVerdict(
    verdict: Verdict,
    unheard: ReadonlySet<string>,
    onPlane: Set<string>,
    pending: Promise<ConsoleWin>[],
  ) {
    if (verdict.action === "attach") {
      const { session } = verdict;
      spawnWindow(
        { id: session.id, repo: session.repo },
        session.agent || "console",
        session.repo,
        verdict.record,
      );
    } else if (verdict.action === "relaunch") {
      relaunch(verdict.record, unheard, pending);
    } else if (verdict.action === "placeholder") {
      spawnPlaceholder(verdict.record, null, null, {
        unheard: unheardRef(verdict.record.repo, unheard),
      });
    } else if (!(verdict.id && onPlane.has(verdict.id))) {
      adopt(verdict.session, verdict.id);
    }
  }

  function relaunch(
    record: DeskRecord,
    unheard: ReadonlySet<string>,
    pending: Promise<ConsoleWin>[],
  ) {
    if (unheardRef(record.repo, unheard)) {
      // The list did not hear from its peer: the console may still run
      // there, and a launch on a peer without the record join would be
      // a SECOND shell. The box asks again on the next fleet read.
      spawnPlaceholder(record, null, null, { unheard: true });
    } else if (peerHeld(record.repo, peerGroups())) {
      // Its peer cannot serve it: a launch would only be refused.
      spawnPlaceholder(record);
    } else {
      // `relaunchRequest` carries the worktree the record was in (#411).
      pending.push(spawnOrMissing(relaunchRequest(record), record.agent, record.repo, record));
    }
  }

  // `adopt`: a cascaded window, keeping the live session's own kind so the
  // desk relaunches it correctly next time, under the record the session
  // names when it names one.
  function adopt(session: HostedSession, id: string | null) {
    spawnWindow({ id: session.id, repo: session.repo }, session.agent || "console", session.repo, {
      id: id || undefined,
      kind: session.kind,
      daemonId: session.daemon_id,
      environment: session.environment,
      checkout: session.checkout ?? null,
      unrecorded: true,
    });
  }

  // Every window of the restore is on the stage: the fences, the cards and
  // the glyphs follow, then the stage is sized and the restore is settled.
  function settleRestore() {
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
  }

  // The restore relaunches agent consoles that no longer run. Read at restore
  // time, not module load. `canLaunch === false` (the popup) refuses too: it
  // must never author a session.
  function relaunchesAgents() {
    return OPTS.canLaunch !== false && viewStore?.read()?.relaunch === true;
  }

  // Restore the desk: reconcile the saved layout against the daemon's live
  // sessions and dispatch one window per verdict. A REJECTED fetch leaves the
  // desk untouched — no relaunch, no phantom placeholders.
  function restoreDesk() {
    Promise.all([whenDeskLoaded(), readSessions()])
      .then(async ([, { sessions, unheard }]) => {
        if (!isDeskLoaded()) {
          retryDeskLoad();
          return;
        }
        if (deskReconciled) return;
        deskReconciled = true;
        const away = awayMap();
        // Not restored twice: `reattachFence` can run while this fetch is in
        // flight (a `popup-gone` mid-boot) and spawn these very members.
        const onPlane = new Set([...wins].map((w) => w._deskId));
        // A relaunch into a recorded worktree first asks whether the tree is
        // still there; the stage is sized only once every such window landed.
        const pending: Promise<ConsoleWin>[] = [];
        for (const verdict of reconcileDesk({
          layout: loadDesk(),
          sessions,
          relaunchAgents: relaunchesAgents(),
        })) {
          // `adopt` carries NO record, so every read below is guarded — a
          // throw here is swallowed by the catch below and nothing renders.
          const { record } = verdict;
          if (record && onPlane.has(record.id)) continue;
          if (record && away.has(record.id)) {
            const fenceId = away.get(record.id);
            if (fenceId !== undefined) seedAway(record, fenceId);
            continue;
          }
          dispatchVerdict(verdict, unheard, onPlane, pending);
        }
        await Promise.allSettled(pending);
        settleRestore();
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
    const origin = fenceOrigin(fence);
    for (const m of members || []) mountMember(m, shiftedRecord(m, origin));
    applyExtent();
  }

  // The top-left corner of the fence in the opener's plane; no rect is 0, 0.
  function fenceOrigin(fence: { rect?: Partial<Rect> | null } | null | undefined) {
    return { left: fence?.rect?.left || 0, top: fence?.rect?.top || 0 };
  }

  // The member's record, moved to sit near this window's top-left.
  function shiftedRecord(m: PopupMember, origin: { left: number; top: number }) {
    return {
      ...m,
      rect: {
        ...m.rect,
        left: (m.rect?.left || 0) - origin.left + 12,
        top: (m.rect?.top || 0) - origin.top + 12,
      },
    };
  }

  // A note, a console with a live session, or the placeholder of one without.
  function mountMember(m: PopupMember, record: ReturnType<typeof shiftedRecord>) {
    if (m.kind === "note") {
      window.WBNotes?.mountDetached(record as NoteSource);
    } else if (m.session != null) {
      spawnWindow({ id: m.session, repo: m.repo }, m.agent || "console", m.repo, record);
    } else {
      spawnPlaceholder(record);
    }
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

  return { restoreDesk, restoreDetached, mountDetached, isDeskReconciled, isDeskSettled };
}
