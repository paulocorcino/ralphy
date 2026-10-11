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
import { createPlaceholders } from "./wb-console-placeholder.ts";
import { createChrome } from "./wb-stage-chrome.ts";
import { createStack, createGestures } from "./wb-stage-stack.ts";
import { createPopupRegistry } from "./wb-desk-popups.ts";
import { createFences } from "./wb-stage-fences.ts";
import { createStageWindow } from "./wb-stage-window.ts";
import { createStageColumns } from "./wb-stage-columns.ts";
import { createDesk, createDeskRecords } from "./wb-desk.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBDeskSync } from "./wb-desk-sync.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import { WBView } from "./wb-client-view.ts";
import { createMessages } from "./wb-messages.ts";
import { sendDocument } from "./wb-events.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleOpts, ConsoleWin, DeskChange, DeskFence } from "./wb-types.d.ts";

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
  peerOfflineView,
  peerReturnDecision,
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
    revivePlaceholders: () => revivePlaceholders(),
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
    spawnPlaceholder: (...args: Parameters<SpawnPlaceholder>) => spawnPlaceholder(...args),
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
    spawnOrMissing: (...args: Parameters<SpawnOrMissing>) => spawnOrMissing(...args),
    spawnPlaceholder: (...args: Parameters<SpawnPlaceholder>) => spawnPlaceholder(...args),
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
    spawnPlaceholder: (...args: Parameters<SpawnPlaceholder>) => spawnPlaceholder(...args),
    spawnOrMissing: (...args: Parameters<SpawnOrMissing>) => spawnOrMissing(...args),
    markRelaunch: (deskId: string, takeDown: () => void) => markRelaunch(deskId, takeDown),
  });
  type SpawnWindow = typeof spawnWindow;

  // ---- the placeholder --------------------------------------------------------
  // A console the daemon no longer runs, the relaunch marks and the revive are
  // `wb-console-placeholder.ts`. Built after the window birth; the spawn, the
  // desk, the detach and the dormancy take its functions before it, as lazy
  // arrows.
  const { isRelaunching, markRelaunch, spawnOrMissing, revivePlaceholders, spawnPlaceholder } = createPlaceholders({
    window,
    document,
    OPTS,
    wins,
    budget,
    buildChrome,
    spawnWindow,
    deskOf,
    recordBirth,
    readSessions,
    peerGroups,
    wakePeerNow,
    reloadDesk,
    loadDesk,
    forgetRecord,
    askConfirm,
    applyExtent,
    changed,
  });
  type SpawnOrMissing = typeof spawnOrMissing;
  type SpawnPlaceholder = typeof spawnPlaceholder;

  // The column paint is `wb-stage-columns.ts` (ADR-0051 §5), built with the window
  // states above; this console keeps the desk records it paints.

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
