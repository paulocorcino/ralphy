/* ---------------------------------------------------------------------------
   The console's desk: sending its desk changes to the daemon, and restoring
   the desk layout when a page opens (ADR-0075 D7, #611).

   `createDesk(deps)` is the desk of one console: the desk view (`desk`,
   `fences`, `notes`, `checkouts`, filled from `sync`), the desk read
   (`reloadDesk`) and what follows it (the converge, a record another device
   removed, the desk failure), the `pagehide` flush, the boot (`boot`) and the
   read after a login (`afterLogin`). It owns the desk state: whether the page
   has read the desk (`deskLoaded`) and the failure. Only this module writes
   them; the console reads them through the getters it returns. It starts the
   desk read while it is built. `DeskDeps` lists every read, and the popup
   registry is reached through it.

   The restore is `createDeskRestore` (`wb-desk-restore.ts`), the converge is
   `createDeskConverge` (`wb-desk-converge.ts`) and the upload is
   `createDeskFlush` (`wb-desk-flush.ts`), all built inside `createDesk`. The
   writes of the desk records are `createDeskRecords` (`wb-desk-records.ts`),
   built by the console.
   --------------------------------------------------------------------------- */
import { createDeskRestore } from "./wb-desk-restore.ts";
import { createDeskConverge } from "./wb-desk-converge.ts";
import { createDeskFlush } from "./wb-desk-flush.ts";
import type { DeskSink, FlushAnswer } from "./wb-desk-flush.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { apiFetch } from "./wb-api.ts";
import type { ApiRefusal } from "./wb-api.ts";
import type { WBDeskSync } from "./wb-desk-sync.ts";
import type { WBView } from "./wb-client-view.ts";
import type { Group } from "./wb-fleet.ts";
import type { OpenerLink } from "./wb-desk-detach.ts";
import type { TerminalOpts } from "./wb-console-terminal-types.d.ts";
import type { PopupRegistry } from "./wb-desk-popups.ts";
import type {
  ConsoleWin,
  DeskChange,
  DeskFence,
  DeskNote,
  DeskRecord,
  ExtentOpts,
  Presentation,
  SpawnCarry,
} from "./wb-types.d.ts";
import { devWarn } from "./wb-log.ts";

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

// The `409` reply of an unreadable desk, as the reason to show, or null.
// The daemon's text is for a developer: it goes to the browser console.
async function unreadableDesk(r: ApiRefusal<"GET /api/desk">) {
  if (r.status !== 409) return null;
  const body = await r.json().catch(() => null);
  if (body?.state !== "unreadable") return null;
  if (body.error) devWarn("saved desk:", body.error);
  return "the file is damaged";
}

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
  // One record per window keyed by a STABLE client-side id (repo, agent,
  // session kind, rect, maximized). The daemon's session id is a volatile
  // ATTRIBUTE: a restarted daemon hands out ids from 1 again. The desk lives in
  // the DAEMON (`GET`/`PUT /api/desk`, ADR-0050), and a page writes it only as
  // desk changes (ADR-0050 amendment 2026-10-04). `sync` (wb-desk-sync.ts)
  // holds the daemon's last desk and this page's unanswered changes; `desk`,
  // `fences`, `notes` and `checkouts` are its view, the SYNCHRONOUS source of
  // truth every read below uses.
  let desk: DeskRecord[] = [];
  // Second record type (#340): named rectangles on the floor tier.
  let fences: DeskFence[] = [];
  // Third record type (ADR-0064 §2): note cards, PLACEMENT only. The card
  // itself is `wb-notes.ts`.
  let notes: DeskNote[] = [];
  // Fourth record type (#406, ADR-0063 §4): the selected checkout per repo ref,
  // `{ <ref>: <worktree name> }`. The reactive copy lives in `app.ts`.
  let checkouts: Record<string, string> = {};
  function refreshView() {
    const v = sync.view();
    desk = v.windows;
    fences = v.fences;
    notes = v.notes;
    checkouts = v.checkouts;
  }
  // The page has read the desk. Nothing is sent before: a page that never read
  // it does not know its generation. Under the `Session` policy the pre-login
  // GET answers 401, so this stays false until `reloadDesk()` succeeds.
  let deskLoaded = false;

  // A restore from the desk history since this page read the desk (ADR-0050
  // amendment 2026-10-04): the daemon refuses this page's next write, and the
  // page reloads to show the restored desk.
  let deskRestored = false;
  function reloadForRestoredDesk() {
    if (deskRestored) return;
    deskRestored = true;
    sync.restored();
    // The `pagehide` flush would send this page's changes on the way out.
    WBDeskSink.setHold(true);
    window.location.reload();
  }
  // ONE desk change: the view takes it at once, the daemon on the next flush.
  function emitDesk(change: DeskChange) {
    sync.emit(change);
    refreshView();
    scheduleDeskFlush();
  }

  const { restoreDesk, restoreDetached, mountDetached, isDeskReconciled, isDeskSettled } =
    createDeskRestore({
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

  const { converge, recordLeft, setDeskGoneHook } = createDeskConverge({
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
  });

  const { scheduleDeskFlush, flushDeskOnClose } = createDeskFlush({
    deskSink,
    sync,
    isDeskLoaded,
    refreshView,
    converge,
    reloadForRestoredDesk,
    deskUnreadable,
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

  // Why the daemon cannot read the saved desk, or "" (ADR-0070 D4). Set only
  // by the daemon's own `409 {"state":"unreadable"}`: a transport failure or a
  // pre-login 401 is not a broken desk.
  let deskFailure = "";
  // The shell's hook for a desk failure found by a flush: no push says so.
  let onDeskFailure: (() => void) | null = null;

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
        if (!payload) return null;
        deskFailure = "";
        ingestDesk(payload);
        // Changes kept while the daemon could not be reached go now.
        if (sync.hasPending()) scheduleDeskFlush();
        return null;
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
        if (body && "state" in body) return;
        throw new Error(`the daemon answered ${r.status}`);
      })
      .then(() => reloadDesk())
      .then(() => {
        if (!deskLoaded) return null;
        // Consoles opened over the unreadable desk queued their records: they
        // go on this flush. With none up, the boot restore never ran, so it
        // runs now.
        if (wins.size > 0) scheduleDeskFlush();
        else restoreDesk();
        return null;
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

  // The daemon refuses a write over a desk it cannot read; the page shows
  // the failure and sends nothing until the desk reads again.
  function deskUnreadable(out: FlushAnswer) {
    const error = out.reply?.error ?? "";
    if (error !== "") devWarn("saved desk:", error);
    deskFailure = "the file is damaged";
    deskLoaded = false;
    sync.fail();
    onDeskFailure?.();
  }
  // A mutation in the last 250 ms before the tab closes would otherwise be
  // dropped. The sink's `putSync` rides `keepalive`, which outlives the document.
  window.addEventListener("pagehide", () => {
    // The per-client view first, and NOT behind the desk guard: its store is
    // synchronous, and gating it would drop the last pan of a pre-login page.
    flushPendingOffset();
    // Every dirty note, too (ADR-0064 §7): the autosave debounce is 800 ms. A
    // best effort, as the desk's own last flush below.
    window.WBNotes.flushAll();
    // NOTHING closes a detached popup here: `pagehide` fires on a RELOAD as on
    // a close (#347). The popup declares its peer lost after `PEER_WINDOW_MS`
    // without a beat and closes itself (ADR-0051 §8).
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

  // `autoBoot: false` is the popup, which renders only the members its opener
  // hands over. The entry module calls this once, after its page has started.
  function boot() {
    wireStage();
    restoreDetached();
    if (OPTS.autoBoot !== false) restoreDesk();
  }

  // Re-read the daemon's desk and restore it: the pre-login `/api/desk`
  // answered 401 under `Session`. Called from `rehydrateAfterAuth` (#327).
  function afterLogin() {
    return reloadDesk().then(() => {
      // `popups.size()` counts as "windows already up": detaching every fence
      // drives `wins.size` to 0, and a `restoreDesk` would respawn the members.
      if (isDeskLoaded() && wins.size === 0 && popups.size() === 0) {
        restoreDesk();
        return null;
      }
      // Windows already up: nothing else would put the just-loaded fences on
      // the stage. A REFUSED load leaves `fences` as this page drew them.
      if (!isDeskLoaded()) return null;
      renderFences();
      renderNotes();
      applyExtent();
      return null;
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
