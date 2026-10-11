/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Consoles tab)

   Consoles are draggable, resizable windows on the STAGE, a plane the VIEWPORT
   (`#workspace`, `overflow:auto`) scrolls over. This module owns the window
   state (tiling, the stage's extent); the titlebar, the drag and the resize
   are `wb-stage-chrome.ts`; the console name, the title's text and the
   worktree switcher are `wb-console-title.ts`; the fence records' chrome, the
   fence verbs and the focused fence are `wb-stage-fence-list.ts`; the
   fences are `wb-stage-fences.ts`, the
   fences this tab detached are `wb-desk-popups.ts`, and the opener's
   side of a detached fence is `wb-desk-detach.ts`; sending the desk
   changes and restoring the desk layout are `wb-desk.ts`; the
   landing, the reveal, the slide and the plane's pan are
   `wb-stage-view.ts`; the body
   is a live xterm.js on a PTY over the daemon's `/ws/session` WebSocket,
   made by `wb-console-terminal.ts`.

   Opening/closing a console spawns/closes a daemon-owned session; on load the
   live sessions re-open as windows, so a reload reattaches with scrollback.

   Importing this module does nothing. Each entry module calls `createConsole`
   once per page, sets `window.WBConsole`, and calls `boot` after the page has
   started; each call starts with new state (ADR-0075 D7).
--------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { resumeDecision, CONNECT_TIMEOUT_MS, RESUME_DEBOUNCE_MS } from "./wb-resume.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { createGpuBudget, gpuHolders, DORMANT_AFTER_MS, DORMANT_MARGIN_PX } from "./wb-console-gpu.ts";
import { createTitle } from "./wb-console-title.ts";
import { createFenceList } from "./wb-stage-fence-list.ts";
import { createDetach } from "./wb-desk-detach.ts";
import { createView } from "./wb-stage-view.ts";
import { createTerminal } from "./wb-console-terminal.ts";
import { createConsoleKeys } from "./wb-console-keys.ts";
import { createFeeds } from "./wb-console-feeds.ts";
import { createDormancy } from "./wb-console-dormancy.ts";
import { createSpawn } from "./wb-console-spawn.ts";
import { createChrome } from "./wb-stage-chrome.ts";
import { createStack, createGestures } from "./wb-stage-stack.ts";
import { createPopupRegistry } from "./wb-desk-popups.ts";
import { createFences } from "./wb-stage-fences.ts";
import { createStageWindow } from "./wb-stage-window.ts";
import { createStageColumns } from "./wb-stage-columns.ts";
import { createDesk, createDeskRecords } from "./wb-desk.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBDeskSync } from "./wb-desk-sync.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import { WBView } from "./wb-client-view.ts";
import { WBFleet } from "./wb-fleet.ts";
import { createMessages } from "./wb-messages.ts";
import { sendDocument } from "./wb-events.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleOpts, ConsoleWin, DeskChange, DeskFence, SpawnCarry } from "./wb-types.d.ts";

// The input folds are `wb-console-input.ts`: pure functions of their arguments.
const {
  isWebKit,
  prefersDomRenderer,
  fullscreenOffered,
  touchScrollLines,
  touchScrollTarget,
  touchGesture,
  touchCentroid,
  dragThreshold,
  dragBegins,
  isDoubleTap,
  flingStep,
  keySequence,
  barKey,
  applyCtrlLatch,
  isTerminalReply,
  keyBarVisible,
  terminalInputMode,
  pasteOffered,
  rightClickAction,
  pressRoute,
  forceSelectionKeys,
  holdMoveReport,
  phoneBleed,
  selectionRow,
  stepFont,
  keyboardInset,
  pasteDecision,
  clipboardContent,
  DOUBLE_TAP_MS,
  PHONE_MAX_WIDTH,
  FONT_MIN,
  FONT_MAX,
  FONT_DEFAULT,
} = WBConsoleInput;

// The session folds are `wb-console-session.ts`: the wire codec, the reconnect
// and dormancy rules, and the peer rules, pure functions of their arguments.
const {
  encodeResize,
  encodeDetach,
  birthDecision,
  dormancyDecision,
  endNotice,
  reconnectDecision,
  heldReturnDecision,
  peerGate,
  peerHost,
  peerOfflineView,
  peerReturnDecision,
  unheardRef,
  peerHeld,
  relaunchRequest,
  sessionRowFor,
  RESUME_HIDDEN_MS,
} = WBConsoleSession;

// The desk and fence folds are `wb-desk-folds.ts`: the restore decision, the fence
// readouts and walk, the detach registry and the popup rules, pure functions.
const {
  reconcileDesk,
  placeholderSession,
  isUnknownCheckout,
  fenceRepos,
  fenceSummaries,
  nextFenceSlot,
  detachFold,
  popupMatches,
  nextFenceName,
  columnClasses,
  fenceCycle,
  noteNameOk,
  DESK_MAX,
  FENCE_MAX,
  NOTE_MAX,
  DETACH_MAX,
} = WBDeskFolds;

export function createConsole(window: Window, document: Document, location: Pick<Location, "protocol" | "host" | "origin">, opts: ConsoleOpts) {
  // Plane geometry is `wb-geometry.ts` (ADR-0057): pure folds over rects.
  const {
    stageExtent,
    fenceSpawnRect,
    rectHolds,
    fenceMembership,
    fenceFits,
    fenceMoveDelta,
    tileIntoRect,
    resizeRect,
    bringIntoView,
    anchorIntoView,
    viewLanding,
    panNudge,
    spawnRectIn,
    freeSpawnRect,
    fenceHolds,
  } = WBGeometry;

  // What a console window IS and what it HOLDS (`wb-window-state.ts`).
  const { sessionIdOf } = WBWindowState;

  // The viewport (the scrolling box) and the stage (the sized plane inside it).
  const workspace = () => document.getElementById("workspace");
  // Defensive about the DOM ITSELF, not just about the element: the ui-tests
  // evaluate this module against a document that answers nothing (and a page
  // ingests its first desk before the stage exists), and every caller already
  // handles a null stage.
  const stage = () =>
    typeof document?.getElementById === "function" ? document.getElementById("stage") : null;
  // Scheme-match the session socket to the page (see wb-daemon.ts WS_ORIGIN):
  // `wss://` over a TLS dev-tunnel/proxy, `ws://` for a plain-http localhost bind.
  const WS_ORIGIN =
    (location.protocol === "https:" ? "wss://" : "ws://") + location.host;
  // Injection point, read once per instance. `main.ts` passes only the door
  // for operator messages (every other default below is the shell's
  // behaviour); `detached-fence-main.ts` overrides all five,
  // which is what makes the popup unable to author the desk or the viewport.
  const OPTS = opts || {};
  const deskSink = OPTS.deskSink || WBDeskSink.daemon();
  const viewStore = OPTS.viewStore || WBView;
  // The confirm, the notice and the toast are the door's (`wb-messages.ts`);
  // a page that passes no door gets one with no shell.
  const messages = OPTS.messages || createMessages(window, document, { shell: () => null });
  const { askConfirm, askNotice, toast, dismissToast } = messages;
  // The z stack and the gestures are the document's (`wb-stage-stack.ts`),
  // shared with the note cards; a page that passes none gets its own.
  const stack = OPTS.stack || createStack(document);
  const { focusWin, stackWin } = stack;
  const gestures = OPTS.gestures || createGestures();
  // Detach registry + lifecycle channel (#347). Denied in the popup: `window.open`
  // hands it a COPY of the opener's session-scoped store, so a read there drifts.
  // This module names no browser store of its own — pinned in lib.rs.
  const link = OPTS.detachLink || WBDetachLink.link();
  const wins = new Set<ConsoleWin>();

  // ---- the desk records -------------------------------------------------------
  // The writes of the desk records and the checkout reads are `wb-desk.ts`
  // (`createDeskRecords`); the desk view they read is `createDesk`'s, built
  // below, and this console keeps the sync both fill. Built first: the title, the
  // window states, the fence list and the chrome take `setWin` and
  // `saveFences`. What is built later goes in as lazy arrows.
  const sync = WBDeskSync.createSync();
  const {
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
  } = createDeskRecords({
    sync,
    desk: () => currentDesk(),
    fences: () => currentFences(),
    notes: () => currentNotes(),
    checkouts: () => currentCheckouts(),
    refreshView: () => refreshView(),
    emitDesk: (change: DeskChange) => emitDesk(change),
    scheduleDeskFlush: () => scheduleDeskFlush(),
    restoreRect: (win: HTMLElement) => restoreRect(win),
    refreshFenceChrome: () => refreshFenceChrome(),
    consolePrefix: (repo: string | null | undefined) => consolePrefix(repo),
  });

  // ---- the feeds -------------------------------------------------------------
  // The last session list, the project names, the fleet and the stale probe the
  // shell fed are `wb-console-feeds.ts`. Built before the title, which takes
  // the project names and the session list.
  const {
    newDeskId,
    newFenceId,
    readSessions,
    projectNameOf,
    projectTitleOf,
    ingestProjects,
    peerGroups,
    ingestFleet,
    ingestSessions,
    list,
    setStaleProbe,
    lastSessions,
    wakePeerNow,
    readFleetNow,
    staleProbeNow,
  } = createFeeds({
    OPTS,
    wins,
    stage,
    agentStateTitle,
    renderTitle: (win, title, presentation) => renderTitle(win, title, presentation),
  });

  // ---- the title -------------------------------------------------------------
  // The console name, the title's text and the worktree switcher are
  // `wb-console-title.ts`; this console keeps the desk, the sessions poll and
  // the project names, and hands them to it. Built before the chrome, which
  // takes six of its functions.
  const {
    consolePrefix,
    takenNames,
    sessionPresentation,
    ingestWorktrees,
    ensureListing,
    checkoutMenuRows,
    renderTitle,
    canRename,
    startRename,
    closeCheckoutMenu,
    checkoutMenu,
    restartWin,
  } = createTitle({
    window,
    document,
    OPTS,
    wins,
    stage,
    desk: () => currentDesk(),
    lastSessions,
    agentStateTitle,
    askConfirm,
    projectNameOf,
    projectTitleOf,
    setWin,
  });

  // ---- the popup registry and the fence list ---------------------------------
  // The fences this tab detached and their popup entries have one owner,
  // `createPopupRegistry`; the fence list, the fences, the desk restore and
  // the opener's side of the detach read and change them only through it.
  // The heartbeat is the detach's, built after the GPU budget: lazy arrows.
  const popups = createPopupRegistry({
    link,
    startBeat: () => detach.startBeat(),
    stopBeat: () => detach.stopBeat(),
  });
  const { isDetached } = popups;
  // ---- dormancy and resume ---------------------------------------------------
  // Sleep, wake, the cover rule and the resume are `wb-console-dormancy.ts`.
  // Built before the window states, which take `refreshCover`; what is built
  // later goes in as lazy arrows. Its listeners wait for `listen()`.
  const dormancy = createDormancy({
    window,
    document,
    wins,
    sync,
    staleProbeNow,
    applyKeyBar: (win: ConsoleWin) => applyKeyBar(win),
    budget: () => budget,
    fillsViewport: (win: ConsoleWin) => fillsViewport(win),
    isFull: (win: HTMLElement) => isFull(win),
    attachTerminal: (body: HTMLElement, termOpts: TerminalOpts) => attachTerminal(body, termOpts),
    scheduleDeskFlush: (ms?: number) => scheduleDeskFlush(ms),
    revivePlaceholders,
  });
  const { isCovered, applyDormancy, refreshCover, resumeAll } = dormancy;
  // ---- the window states ------------------------------------------------------
  // Maximize, the lock, full screen, the stage's extent and a window's restore
  // box are `wb-stage-window.ts`; this console keeps the columns and the desk
  // records, and hands them the window state. Built before the fence list,
  // which takes three of its functions.
  const {
    fillsViewport,
    restoreRect,
    setMax,
    toggleMax,
    paintMaxButton,
    isFull,
    isLocked,
    applyLock,
    paintLockGlyph,
    toggleLock,
    toggleFull,
    syncFullState,
    raiseMaximized,
    syncMaxLock,
    syncMaxPin,
    applyExtent,
    resetExtentEdge,
  } = createStageWindow({
    document,
    workspace,
    stage,
    focusWin,
    setWin,
    heldByFence: (win: HTMLElement) => heldByFence(win),
    refreshCover,
  });
  // ---- the column paint -------------------------------------------------------
  // Built before the fence list, which takes `columnMeasure`; the fence list,
  // the view and the detach registry come later, so they go in as lazy arrows.
  const { columnMeasure, applyColumns, dropClosedElsewhere, focusedId, focusColumn, columnRoster } = createStageColumns({
    OPTS,
    wins,
    workspace,
    stage,
    popups,
    desk: () => currentDesk(),
    focusWin,
    setMax,
    paintMaxButton,
    syncMaxLock,
    syncMaxPin,
    applyExtent,
    list,
    paintFenceColumns: () => paintFenceColumns(),
    fenceList: () => fenceList(),
    readFenceRects: (st: HTMLElement) => readFenceRects(st),
    readWindowRects: (st: HTMLElement) => readWindowRects(st),
    findWindow: (id: string) => findWindow(id),
    tearDownMember: (win: ConsoleWin, reason: DetachReason) => tearDownMember(win, reason),
  });
  // The fence records' chrome, the fence verbs and the focused fence are
  // `wb-stage-fence-list.ts`; this console keeps the fence records and hands
  // them to it. Built before the view and the fences, which it reaches
  // through lazy arrows.
  const fenceFloor = createFenceList({
    window,
    document,
    OPTS,
    popups,
    fences: () => currentFences(),
    notes: () => currentNotes(),
    stage,
    workspace,
    restoreRect,
    applyExtent,
    columnMeasure,
    inGesture,
    newFenceId,
    paintLockGlyph,
    saveFences,
    jumpToFence: (id: string) => jumpToFence(id),
    buildFence: (f: DeskFence) => buildFence(f),
  });
  const {
    fenceLocked,
    heldByFence,
    fenceEl,
    refreshFenceChrome,
    paintFenceColumns,
    readFenceRects,
    readWindowRects,
    fenceRecords,
    renderNotes,
    fenceList,
    stepFence,
    renderFences,
    atFenceCap,
    createFence,
    renameFence,
    removeFence,
    focusedFenceId,
    focusFence,
    clearFenceFocus,
    showDetachGlyph,
  } = fenceFloor;

  // ---- dormant consoles and the GPU budget ---------------------------------
  // The watch, its constants and the GPU budget are `wb-console-gpu.ts`; this
  // console keeps sleeping and waking a window (`applyDormancy`) and the
  // covered rule (`isCovered`), and hands them to its budget.
  const budget = createGpuBudget({
    wins,
    workspace,
    isCovered,
    applyDormancy,
    IntersectionObserver:
      typeof IntersectionObserver === "function" ? IntersectionObserver : undefined,
    queueMicrotask: (task) => queueMicrotask(task),
  });

  // ---- the opener's side of the detach ----------------------------------------
  // The heartbeat, the probe of a quiet popup, the re-attach and the two
  // listeners that hear the popups are `wb-desk-detach.ts`; this console
  // keeps the desk and the windows, and hands them to it. Built after the
  // GPU budget and before the fences, which take five of its functions.
  const detach = createDetach({
    window,
    location,
    link,
    popups,
    wins,
    budget,
    fences: () => currentFences(),
    notes: () => currentNotes(),
    stage,
    changed,
    applyExtent,
    forgetRecord,
    loadDesk,
    saveNotes,
    spawnWindow: (...args: Parameters<SpawnWindow>) => spawnWindow(...args),
    spawnPlaceholder,
    deskOf: (win: ConsoleWin) => deskOf(win),
    whenDeskLoaded: () => whenDeskLoaded(),
    fenceFloor,
  });
  const { peerFold, newPid, fenceSnapshot, tearDownMember, reattachFence, glyphClick } = detach;

  // ---- the view -------------------------------------------------------------
  // The landing and the stored offset, the reveal, the slide, the auto-pan and
  // the plane's own pan and wheel are `wb-stage-view.ts`; this console keeps
  // the stage's extent and `refitAll`, and hands it the window state. Built
  // before the terminal, which takes `cancelSlide`.
  const {
    applyLanding,
    flushPendingOffset,
    reveal,
    findWindow,
    jumpToFence,
    jumpToNote,
    cancelSlide,
    autoPan,
    wireStage,
  } = createView({
    window,
    document,
    workspace,
    stage,
    viewStore,
    restoreRect,
    focusWin,
    syncFullState,
    syncMaxPin,
    fenceEl,
    focusFence,
    clearFenceFocus,
    focusedFence: focusedFenceId,
    isDeskSettled: () => isDeskSettled(),
  });

  // ---- the input -------------------------------------------------------------
  // The key bar, the font size and the clipboard are `wb-console-keys.ts`.
  const { fontSize, setFont, writeClipboard, readClipboard, applyKeyBar, buildKeyBar } = createConsoleKeys({
    window,
    document,
    viewStore,
    wins,
    focusWin,
  });

  // ---- the terminal ----------------------------------------------------------
  // The terminal and its session socket are `wb-console-terminal.ts`; this
  // console keeps the plane and hands it the font size and the clipboard.
  const { attachTerminal } = createTerminal({
    window,
    WS_ORIGIN,
    fontSize,
    stage,
    workspace,
    cancelSlide,
    writeClipboard,
    readClipboard,
  });

  // ---- the window chrome -----------------------------------------------------
  // The titlebar, the drag, the resize and the free cascade are
  // `wb-stage-chrome.ts`; this console hands it the window state and the
  // gestures.
  const { buildChrome, makeDraggable, startResize } = createChrome({
    window,
    document,
    OPTS,
    gestures,
    stage,
    workspace,
    fences: () => currentFences(),
    focusedFence: focusedFenceId,
    applyExtent,
    applyLock,
    autoPan,
    canRename,
    consolePrefix,
    fenceEl,
    fenceLocked,
    focusWin,
    isFull,
    isLocked,
    newDeskId,
    renderTitle,
    restoreRect,
    reveal,
    sessionPresentation,
    setMax,
    setWin,
    startRename,
    takenNames,
    toggleFull,
    toggleLock,
    toggleMax,
  });

  // ---- the fences -------------------------------------------------------------
  // The fence element, its gestures, tiling and detaching are
  // `wb-stage-fences.ts`; they take the fence list as one object.
  const { buildFence, arrangeFence, detachFence } = createFences({
    window,
    document,
    gestures,
    popups,
    link,
    wins,
    fences: () => currentFences(),
    fenceFloor,
    applyExtent,
    askConfirm,
    autoPan,
    fenceSnapshot,
    focusWin,
    glyphClick,
    newPid,
    reattachFence,
    restoreRect,
    saveFences,
    setWin,
    stage,
    tearDownMember,
  });

  // This console's work when the stack moves the focus.
  stack.setFocusHook({
    blurred: (w) => {
      // Focus held it awake (`dormancyDecision` D2); the observer will not
      // report a window that did not move.
      if (wins.has(w)) applyDormancy(w);
    },
    // On top now, so first in line for a context. The `applyDormancy` above
    // asks too, but not when the focus came from a note card.
    focused: () => budget.scheduleGpu(),
  });

  function changed() {
    // A close takes a full bleed away without moving the windows under it, so
    // the observer has nothing to report.
    refreshCover();
    sendDocument(document, "workbench:consoles-changed", { count: wins.size });
  }

  // The tooltip of an agent-state dot: the Go-to menu, the checkout switcher
  // and the title bar all say it the same way. `waiting` carries the question
  // as its detail; `unknown` is a stale observation (agent_state.rs).
  function agentStateTitle(state: string | null | undefined, detail?: string) {
    if (!state) return "";
    if (state === "unknown") return "Agent state unknown";
    return detail ? `Agent is ${state}: ${detail}` : `Agent is ${state}`;
  }

  // A desk this page takes never moves an element under a gesture.
  function inGesture(el: HTMLElement) {
    return gestures.active(el);
  }

  // ---- the desk ----------------------------------------------------------------
  // The desk view, the desk read and what follows it (the converge, the answer
  // to an upload, the `pagehide` flush), sending the desk changes and
  // restoring the desk layout are `wb-desk.ts`, which owns the desk state.
  // `createDesk` starts the desk read at once: `restoreDesk` and
  // `whenDeskLoaded` wait for it.
  const {
    emitDesk,
    scheduleDeskFlush,
    restoreDesk,
    restoreDetached,
    mountDetached,
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
  } = createDesk({
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
    peerGroups: () => peerGroups,
    loadDesk,
    readSessions,
    spawnWindow: (...args: Parameters<SpawnWindow>) => spawnWindow(...args),
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
  });

  dormancy.listen();

  // ---- the window birth -------------------------------------------------------
  // A live console window and the desk record writes of its birth are
  // `wb-console-spawn.ts`. Built after the desk; the desk and the detach take
  // `spawnWindow` (and the detach `deskOf`) before it, as lazy arrows.
  const { spawnWindow, recordBirth, recordSession, deskOf } = createSpawn({
    document,
    wins,
    peerGroups,
    wakePeerNow,
    readFleetNow,
    buildChrome,
    sessionPresentation,
    renderTitle,
    closeCheckoutMenu,
    restartWin,
    ensureListing,
    buildKeyBar,
    applyDormancy,
    attachTerminal,
    budget,
    askConfirm,
    createRecord,
    setWin,
    forgetRecord,
    currentDesk,
    reloadDesk,
    recordLeft,
    restoreRect,
    applyExtent,
    changed,
    spawnPlaceholder,
    spawnOrMissing,
    markRelaunch,
  });
  type SpawnWindow = typeof spawnWindow;

  // Whether the worktree a relaunch asks for is still there (#411). The daemon
  // refuses a launch into a missing worktree with a `400` BEFORE the socket
  // upgrades, and a browser cannot read that status. So ask first, through the
  // cheapest Observe read that takes a checkout (no spawn): the ONE reply that
  // means the tree is gone is `unknown checkout`. Anything else — an
  // unreachable daemon included — lets the launch decide.
  async function checkoutStillThere(repo: string | null | undefined, checkout: string | null | undefined) {
    const daemon = window.WBDaemon;
    if (!checkout || !repo || typeof daemon?.observe !== "function") return true;
    let reply;
    try {
      reply = await daemon.observe("tree.list", { repo, path: "", checkout });
    } catch {
      return true;
    }
    return !isUnknownCheckout(reply);
  }
  // The column paint is `wb-stage-columns.ts` (ADR-0051 §5), built with the window
  // states above; this console keeps the desk records it paints.

  // Spawn an agent console — unless the worktree it asks for is gone, in which
  // case a placeholder SAYS so (#411). A console must never silently land on
  // the primary tree because its own vanished (the #409 gates).
  // Desk ids whose window a relaunch took off the stage and has not put back
  // yet: the columns (`wbColumns`) wait for them instead of dropping them.
  const relaunching = new Set<string | undefined>();
  function isRelaunching(deskId: string) {
    return relaunching.has(deskId);
  }
  // Marks `deskId`, then takes its window off the stage. `spawnOrMissing`
  // clears the mark; a take-down that throws clears it here.
  function markRelaunch(deskId: string, takeDown: () => void) {
    relaunching.add(deskId);
    try {
      takeDown();
    } catch (e) {
      relaunching.delete(deskId);
      throw e;
    }
  }

  async function spawnOrMissing(req: TerminalOpts, label: string | null | undefined, repo: string | null | undefined, carry: SpawnCarry) {
    try {
      if (req.checkout && !(await checkoutStillThere(repo, req.checkout))) {
        return spawnPlaceholder({ ...carry, checkout: req.checkout }, req.checkout);
      }
      return spawnWindow(req, label, repo, carry);
    } finally {
      relaunching.delete(carry?.id);
    }
  }

  // The live session a placeholder should attach to, read NOW: the page was
  // loaded before another device started this console, so neither the mirror
  // nor the load-time session list knows it. `null` when the list says it does
  // not run; `undefined` when that is not known — the list cannot be read, or
  // it did not hear from the record's peer.
  async function liveSessionFor(win: ConsoleWin, record: SpawnCarry) {
    await reloadDesk();
    let sessions;
    try {
      const read = await readSessions();
      if (unheardRef(record.repo, read.unheard)) return undefined;
      sessions = read.sessions;
    } catch {
      return undefined;
    }
    const layout: WBDeskFolds.FoldRecord[] = loadDesk();
    // Deleted by another page: this window still stands for it.
    if (!layout.some((rec) => rec.id === win._deskId)) {
      layout.push({ ...record, id: win._deskId, sessionId: null });
    }
    const held = [...wins]
      .filter((w) => w !== win && !w.classList.contains("placeholder"))
      .map((w) => ({ id: sessionIdOf(w), repo: w._deskRepo }))
      .filter((h) => h.id != null);
    return placeholderSession({ layout, sessions, recordId: win._deskId, held });
  }

  // Bring every placeholder whose console now runs somewhere back as an attached
  // window. ATTACH only — a resume must never launch a vendor CLI. One at a
  // time, so an attach counts as `held` for the next placeholder; one pass at a
  // time, because `visibilitychange` and `online` land together on an iOS resume.
  let reviving: Promise<void> | null = null;
  function revivePlaceholders() {
    if (reviving) return reviving;
    reviving = (async () => {
      for (const w of [...wins]) {
        if (typeof w._revive === "function" && w.isConnected) await w._revive();
      }
    })().finally(() => {
      reviving = null;
    });
    return reviving;
  }

  // An agent console the daemon no longer runs: same chrome, same box, no
  // session — one click relaunches into this very record, unless the console
  // runs by now (another device started it), in which case it attaches.
  // `missing` names a worktree that no longer exists (#411): the button
  // relaunches on the PRIMARY tree, explicitly by its label. `refused` (a
  // `{ message }`) is a launch a peer refused. A box for a peer project says
  // what its peer's fleet state is, and redraws on every fleet read.
  // `held.unheard`: restored while the session list did not hear from its
  // peer, so whether it runs is not known; it asks again on each fleet read.
  function spawnPlaceholder(record: SpawnCarry, missing?: string | null, refused?: { message: string | null } | null, held?: { unheard?: boolean }) {
    const { win, body, restartBtn, closeBtn } = buildChrome(
      record.agent,
      record.repo,
      record,
      record.kind,
    );
    win.classList.add("placeholder");
    // Nothing runs here to restart: Relaunch is the one action.
    restartBtn.hidden = true;

    const note = document.createElement("div");
    note.className = "session-offline";
    const text = document.createElement("p");
    text.textContent =
      record.kind === "console" ? "This console is not running." : "This agent console is not running.";
    const btn = document.createElement("button");
    btn.className = "session-reconnect";
    btn.textContent = "Relaunch";
    // The popup offers it too: a relaunch reuses this record's id, so the
    // popup's members stay the fence's snapshot (ADR-0051 §8, amended
    // 2026-10-05). Only a click launches there; `canLaunch` keeps every
    // automatic relaunch out of the popup.
    note.append(text, btn);
    body.append(note);

    // The peer's words. `peerAction` is the button's action ("wake", "retry",
    // "relaunch"); the box starts as "not running" until the fleet says more.
    const daemon = WBFleet?.refDaemon(record.repo) || "";
    const canLaunch = OPTS.canLaunch !== false;
    type BoxAction = WBConsoleSession.PeerAction | "relaunch";
    const BUTTON: Partial<Record<NonNullable<BoxAction>, string>> = { wake: "Wake", retry: "Try again", relaunch: "Relaunch" };
    let peerAction: BoxAction = "relaunch";
    let wasOffline = false;
    let shown: Parameters<typeof show>[0] | null = null;
    let detail: HTMLDetailsElement | null = null;
    let detailText: HTMLParagraphElement | null = null;
    const show = (view: { text: string; detail: string; action: BoxAction }) => {
      shown = view;
      text.textContent = view.text;
      peerAction = view.action;
      const label = view.action && BUTTON[view.action];
      btn.hidden = !label;
      if (label) btn.textContent = label;
      if (!detail) {
        detail = document.createElement("details");
        detail.className = "session-detail";
        const summary = document.createElement("summary");
        summary.textContent = "Details";
        detailText = document.createElement("p");
        detail.append(summary, detailText);
        note.append(detail);
      }
      detailText!.textContent = view.detail || "";
      detail.hidden = !view.detail;
    };
    const showPeer = () => {
      // A click in flight owns the box; a missing worktree says something else.
      if (!daemon || missing || btn.disabled || !win.isConnected) return;
      const group = peerGroups.get(daemon);
      if (!group) return;
      const available = WBFleet.available(group);
      // The fleet may call the peer available while its session list timed
      // out: a held box asks the list itself, and never waits for `wasOffline`.
      if (unheard && available) {
        askHeld();
        return;
      }
      if (!available) wasOffline = true;
      const turn = peerReturnDecision({ kind: record.kind, canLaunch, available, wasOffline });
      if (turn === "relaunch") {
        btn.click();
        return;
      }
      if (turn === "offer") {
        show({ text: `${peerHost(group, record.environment)} is back.`, detail: "", action: "relaunch" });
        return;
      }
      if (available && !refused) return;
      show(peerOfflineView(available ? null : group, refused?.message, peerHost(group, record.environment)));
    };
    if (refused) show(peerOfflineView(null, refused.message, peerHost(peerGroups.get(daemon), record.environment)));
    let unheard = !!(daemon && held?.unheard);
    // The box says why it waits: the list did not hear from the peer. "Try
    // again" asks the list again, and launches only when the list heard from
    // the peer and the console does not run there.
    const showUnheard = () =>
      show({
        text: `${peerHost(peerGroups.get(daemon), record.environment)} does not answer.`,
        detail: "",
        action: "retry",
      });
    if (unheard) showUnheard();
    // One question at a time: fleet reads arrive every 30 s and on every
    // `peers.dirty`, and an answer must be acted on once.
    let asking = false;
    const askHeld = async () => {
      if (asking) return;
      asking = true;
      try {
        const session = await check();
        if (!win.isConnected || btn.disabled || !unheard) return;
        const turn = heldReturnDecision({ kind: record.kind, canLaunch, session });
        if (turn === "stay") {
          showUnheard();
          return;
        }
        unheard = false;
        if (turn === "attach") attach(session!);
        else if (turn === "relaunch") btn.click();
        else show({ text: `${peerHost(peerGroups.get(daemon), record.environment)} is back.`, detail: "", action: "relaunch" });
      } finally {
        asking = false;
      }
    };
    win._peerRefresh = showPeer;

    const markMissing = (name: string | null | undefined) => {
      missing = name;
      win.classList.add("missing-checkout");
      text.textContent = `Worktree ${name} no longer exists.`;
      btn.textContent = "Relaunch in primary";
    };
    if (missing) markMissing(missing);
    // A placeholder restored for a recorded worktree asks whether that tree is
    // still there (no spawn), so the box says "gone" on load, not on the click.
    else if (record.checkout) {
      checkoutStillThere(record.repo, record.checkout).then((there) => {
        if (!there && win.isConnected) markMissing(record.checkout);
      });
    }

    // `respawn`: the same desk id comes back at once, and the spawn announces
    // it. Announcing the gap would take the console out of its column
    // (ADR-0051 §5) and promote the next one to the maximize.
    const drop = (respawn?: boolean) => {
      win.remove();
      budget.untrackDormancy(win);
      wins.delete(win);
      applyExtent();
      if (!respawn) changed();
    };
    // The attach `restoreDesk` makes, into this record's id and rect. A session
    // another window drives parks this one as a watcher (`reconnectDecision`).
    const attach = (session: HostedSession) => {
      const carry = deskOf(win);
      drop(true);
      spawnWindow(
        { id: session.id, repo: session.repo },
        session.agent || "console",
        session.repo,
        carry,
      );
    };
    // One check at a time: a click and a resume can arrive together.
    let checking: Promise<HostedSession | null | undefined> | null = null;
    const check = () => {
      if (!checking) {
        checking = liveSessionFor(win, record)
          .catch(() => undefined)
          .finally(() => {
            checking = null;
          });
      }
      return checking;
    };
    win._revive = async () => {
      const session = await check();
      if (session && win.isConnected) attach(session);
    };
    btn.addEventListener("click", async (e: MouseEvent) => {
      e.stopPropagation();
      if (btn.disabled) return;
      btn.disabled = true;
      // The shell's wake answers when the peer is usable, and reports its own
      // failure; the box then says again what it said.
      if (peerAction === "wake") {
        text.textContent = `Waking ${peerHost(peerGroups.get(daemon), record.environment)}…`;
        const woke = typeof wakePeerNow() === "function" && (await wakePeerNow()!(daemon));
        if (!win.isConnected) return;
        if (!woke) {
          btn.disabled = false;
          show(shown!);
          showPeer();
          return;
        }
      }
      const session = await check();
      if (!win.isConnected) return;
      if (session) {
        attach(session);
        return;
      }
      // A peer console whose list is not known may still run there: a launch
      // would start a second one on a peer without the record join. The box
      // says so and waits for the next read or click.
      if (session === undefined && daemon) {
        btn.disabled = false;
        unheard = true;
        showUnheard();
        return;
      }
      const carry = deskOf(win);
      markRelaunch(carry.id, () => drop(true));
      // The agent menu's launch path, reusing this record's id, rect and
      // maximized state — in the recorded worktree unless that is the one that
      // is gone, in which case the button said "primary". For a local record
      // an unreadable session list launches too: the launch then fails as it
      // always did.
      if (missing) carry.checkout = null;
      spawnOrMissing(
        relaunchRequest({ ...record, checkout: missing ? null : record.checkout }),
        record.agent,
        record.repo,
        carry,
      );
    });
    closeBtn.onclick = async () => {
      // Nothing is running here; the question says so rather than borrowing the
      // live console's warning.
      const ok = await askConfirm({
        title: "Close this console?",
        message: `This ${record.agent} console is not running. Close removes its window.`,
        confirmLabel: "Close",
        danger: true,
      });
      if (!ok) return;
      forgetRecord(win._deskId);
      drop();
      WB.emit("console-close", { repo: record.repo || null, agent: record.agent });
    };

    wins.add(win);
    budget.trackDormancy(win);
    changed();
    recordBirth(win, record);
    showPeer();
    return win;
  }

  // `agent` names an adapter (claude/codex/opencode); when `plain` is set there
  // is no agent — a normal shell in the repo dir, labelled "console", or, with
  // `command`, a shell running that command and labelled by it (the same label
  // the daemon gives the session, so the desk record relaunches it as such).
  function open({ repo, agent, plain, checkout, command }: { repo?: string | null; agent?: string | null; plain?: boolean; checkout?: string | null; command?: string }) {
    if (atDeskCap()) {
      toast({ text: `You can have at most ${DESK_MAX} consoles. Close one first.` });
      return false;
    }
    const label = agent || (plain && command) || "console";
    // The plain console ignores the checkout: it rides the repo path (on a
    // peer, `wsl.exe --cd`) and stays on the primary.
    spawnWindow(plain ? { console: true, repo, command } : { repo, agent, checkout }, label, repo);
    WB.emit("console-open", { repo: repo || null, agent: agent || null, plain: !!plain });
    return true;
  }

  // `autoBoot: false` is the popup, which renders only the members
  // its opener hands over. The entry module calls this once, after its page
  // has started.
  function boot() {
    wireStage();
    restoreDetached();
    if (OPTS.autoBoot !== false) restoreDesk();
  }

  // Refit every open console. Called when the Consoles tab returns to view: a
  // terminal opened/reattached while the tab was display:none measured 0×0.
  function refitAll(attempt?: number) {
    // Alpine's `$nextTick` fires BEFORE `x-show` applies the flip (MEASURED):
    // `.consoles-tab` is still `display:none` and everything measures 0.
    // Refitting there collapses the stage to the bare margin (3200×2080 →
    // 200×200) and sizes every terminal to nothing. Wait for a frame that can
    // measure; give up rather than refit blind.
    const ws0 = workspace();
    if (ws0 && (!ws0.clientWidth || !ws0.clientHeight)) {
      const n = attempt || 0;
      if (n < 10) requestAnimationFrame(() => refitAll(n + 1));
      return;
    }
    // First moment the viewport leg of the union can be measured for real.
    // The extent dispatch is an EDGE, so one that fired before Alpine mounted
    // would leave the footer pill at `stage 0 × 0`; force the edge.
    resetExtentEdge();
    applyExtent();
    for (const win of wins) {
      try {
        win._term?.fit.fit();
      } catch {}
    }
    // The chrome folds MEASURED rects: a refresh while hidden wrote `0
    // consoles`. Re-derive on the first frame that can measure.
    refreshFenceChrome();
    // LAST, after `applyExtent`: `x-show` threw the stored offset away.
    applyLanding();
    // The first frame that can measure the column cap.
    sendDocument(document, "workbench:columns-stale");
  }

  function count() {
    return wins.size;
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
    boot,
    open,
    relaunchRequest,
    ingestWorktrees,
    ingestSessions,
    ingestProjects,
    ingestFleet,
    peerOfflineView,
    peerReturnDecision,
    heldReturnDecision,
    peerHeld,
    sessionRowFor,
    arrangeFence,
    count,
    refitAll,
    resizeRect,
    stageExtent,
    bringIntoView,
    anchorIntoView,
    viewLanding,
    panNudge,
    autoPan,
    reconnectDecision,
    peerGate,
    endNotice,
    resumeDecision,
    resumeAll,
    dormancyDecision,
    birthDecision,
    encodeDetach,
    encodeResize,
    DORMANT_AFTER_MS,
    DORMANT_MARGIN_PX,
    keyboardInset,
    raiseMaximized,
    touchScrollLines,
    touchScrollTarget,
    touchGesture,
    touchCentroid,
    dragThreshold,
    dragBegins,
    isDoubleTap,
    DOUBLE_TAP_MS,
    prefersDomRenderer,
    gpuHolders,
    isWebKit,
    fullscreenOffered,
    flingStep,
    keySequence,
    barKey,
    applyCtrlLatch,
    terminalInputMode,
    isTerminalReply,
    keyBarVisible,
    pasteOffered,
    rightClickAction,
    pressRoute,
    forceSelectionKeys,
    holdMoveReport,
    clipboardContent,
    phoneBleed,
    PHONE_MAX_WIDTH,
    selectionRow,
    stepFont,
    setFont,
    fontSize,
    FONT_MIN,
    FONT_MAX,
    FONT_DEFAULT,
    setStaleProbe,
    RESUME_HIDDEN_MS,
    RESUME_DEBOUNCE_MS,
    CONNECT_TIMEOUT_MS,
    pasteDecision,
    reconcileDesk,
    placeholderSession,
    restoreRect,
    columnClasses,
    columnMeasure,
    applyColumns,
    isRelaunching,
    markRelaunch,
    focusColumn,
    focusedId,
    deskRecords,
    reloadDesk,
    deskFailure: currentDeskFailure,
    setDeskFailureHook,
    setDeskGoneHook,
    startNewDesk,
    reloadForRestoredDesk,
    dropClosedElsewhere,
    columnRoster,
    sessionPresentation,
    consolePrefix,
    list,
    reveal,
    afterLogin,
    checkoutOf,
    setCheckout,
    checkouts: allCheckouts,
    checkoutMenu,
    checkoutMenuRows,
    ensureListing,
    askNotice,
    // The note card asks the same question the stage's own verbs do (ADR-0064
    // §11's delete), so there is one dialog in this workbench and not two.
    askConfirm,
    whenDeskLoaded,
    fenceSpawnRect,
    nextFenceSlot,
    rectHolds,
    fenceMembership,
    fenceSummaries,
    fenceFits,
    fenceMoveDelta,
    tileIntoRect,
    fenceRepos,
    fenceList,
    fenceCycle,
    fenceHolds,
    detachFold,
    peerFold,
    DETACH_MAX,
    detachFence,
    reattachFence,
    isDetached,
    mountDetached,
    noteNameOk,
    popupMatches,
    stepFence,
    jumpToFence,
    jumpToNote,
    focusedFence: focusedFenceId,
    spawnRectIn,
    freeSpawnRect,
    createFence,
    atFenceCap,
    nextFenceName,
    FENCE_MAX,
    renameFence,
    removeFence,
    // The note card's seam (ADR-0064 §2): the desk state lives here, the card
    // lives in `wb-notes.ts`, and these are everything it needs.
    notes: loadNotes,
    saveNotes,
    atNoteCap,
    NOTE_MAX,
    atDeskCap,
    DESK_MAX,
    fenceRecords,
    inGesture,
    // The window writes that are not a gesture (a birth, a reconnect, a first
    // act on an adopted console), for the ui-test: each must send only the
    // fields it changed.
    recordBirth,
    recordSession,
    setWin,
    makeDraggable,
    startResize,
    focusWin,
    stackWin,
    toast,
    dismissToast,
    agentStateTitle,
    renderNotes,
  };
}
