/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Consoles tab)

   Consoles are draggable, resizable windows on the STAGE, a plane the VIEWPORT
   (`#workspace`, `overflow:auto`) scrolls over. This module builds the console
   from its parts and returns them as `WBConsole`. The parts are: the desk
   records `wb-desk-records.ts`; the desk `wb-desk.ts`; the feeds
   `wb-console-feeds.ts`; the title and the worktree switcher
   `wb-console-title.ts`; the popups `wb-desk-popups.ts`; sleep and resume
   `wb-console-dormancy.ts`; the window states `wb-stage-window.ts`; the columns
   `wb-stage-columns.ts`; the fence list `wb-stage-fence-list.ts`; the GPU budget
   `wb-console-gpu.ts`; the opener's side of a detach `wb-desk-detach.ts`; the
   view `wb-stage-view.ts`; the key bar `wb-console-keys.ts`; the terminal (a
   live xterm.js on a PTY) `wb-console-terminal.ts`; the chrome
   `wb-stage-chrome.ts`; the fences `wb-stage-fences.ts`; the window birth
   `wb-console-spawn.ts`; the placeholder `wb-console-placeholder.ts`; the
   `WBConsole` object `wb-console-api.ts`.

   Importing this module does nothing. Each entry module calls `createConsole`
   once per page, sets `window.WBConsole`, and calls `boot` after the page has
   started; each call starts with new state (ADR-0075 D7).
--------------------------------------------------------------------------- */
import { createGpuBudget } from "./wb-console-gpu.ts";
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
import { createDesk } from "./wb-desk.ts";
import { createDeskRecords } from "./wb-desk-records.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBDeskSync } from "./wb-desk-sync.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import { WBView } from "./wb-client-view.ts";
import { createMessages } from "./wb-messages.ts";
import { buildConsoleApi } from "./wb-console-api.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleOpts, ConsoleWin, DeskChange, DeskFence } from "./wb-types.d.ts";

export function createConsole(window: Window, document: Document, location: Pick<Location, "protocol" | "host" | "origin">, opts: ConsoleOpts) {
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
  const messages = OPTS.messages || createMessages(window, document);
  const { askConfirm, toast } = messages;
  // The z stack and the gestures are the document's (`wb-stage-stack.ts`),
  // shared with the note cards; a page that passes none gets its own.
  const stack = OPTS.stack || createStack(document);
  const { focusWin } = stack;
  const gestures = OPTS.gestures || createGestures();
  // Detach registry + lifecycle channel (#347). Denied in the popup: `window.open`
  // hands it a COPY of the opener's session-scoped store, so a read there drifts.
  // This module names no browser store of its own — pinned in lib.rs.
  const link = OPTS.detachLink || WBDetachLink.link();
  const wins = new Set<ConsoleWin>();

  // ---- the desk records: `wb-desk-records.ts` ----------------------------------
  // Built first: the title, window states, fence list and chrome take `setWin` and `saveFences`.
  const sync = WBDeskSync.createSync();
  const records = createDeskRecords({
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
  const { createRecord, setWin, forgetRecord, saveFences, saveNotes, nameUnnamed, loadDesk, atDeskCap } = records;

  // ---- the feeds: `wb-console-feeds.ts` ----------------------------------------
  // Built before the title, which takes the project names and the session list.
  const feeds = createFeeds({
    OPTS,
    wins,
    stage,
    renderTitle: (win, title, presentation) => renderTitle(win, title, presentation),
  });
  const { newDeskId, newFenceId, readSessions, projectNameOf, projectTitleOf, peerGroups, list, lastSessions, wakePeerNow, readFleetNow, staleProbeNow } = feeds;

  // ---- the title: `wb-console-title.ts` ----------------------------------------
  // Built before the chrome, which takes six of its functions.
  const title = createTitle({
    window,
    document,
    OPTS,
    wins,
    stage,
    desk: () => currentDesk(),
    lastSessions,
    askConfirm,
    projectNameOf,
    projectTitleOf,
    setWin,
  });
  const { consolePrefix, takenNames, sessionPresentation, ensureListing, renderTitle, canRename, startRename, closeCheckoutMenu, restartWin } = title;

  // ---- the popup registry: `wb-desk-popups.ts` ---------------------------------
  // The one owner of the detached fences: everything else reads and changes
  // them through it. The heartbeat is the detach's, built later: lazy arrows.
  const popups = createPopupRegistry({
    link,
    startBeat: () => detach.startBeat(),
    stopBeat: () => detach.stopBeat(),
  });
  // ---- dormancy and resume: `wb-console-dormancy.ts` ---------------------------
  // Built before the window states, which take `refreshCover`. Its listeners
  // wait for `listen()`.
  const dormancy = createDormancy({
    window,
    document,
    wins,
    stack,
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
  const { isCovered, applyDormancy, refreshCover, changed } = dormancy;
  // ---- the window states: `wb-stage-window.ts` ---------------------------------
  // Built before the fence list, which takes three of its functions.
  const stageWindow = createStageWindow({
    document,
    workspace,
    stage,
    focusWin,
    setWin,
    heldByFence: (win: HTMLElement) => heldByFence(win),
    refreshCover,
  });
  const { fillsViewport, restoreRect, setMax, toggleMax, paintMaxButton, isFull, isLocked, applyLock, paintLockGlyph, toggleLock, toggleFull, syncFullState, raiseMaximized, syncMaxLock, syncMaxPin, applyExtent, resetExtentEdge } = stageWindow;
  // ---- the column paint: `wb-stage-columns.ts` (ADR-0051 §5) -------------
  // Built before the fence list, which takes `columnMeasure`.
  const columns = createStageColumns({
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
  const { columnMeasure, dropClosedElsewhere } = columns;
  // ---- the fence list: `wb-stage-fence-list.ts` --------------------------------
  // Built before the view and the fences, which it reaches through lazy arrows.
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
    inGesture: gestures.active,
    newFenceId,
    paintLockGlyph,
    saveFences,
    jumpToFence: (id: string) => jumpToFence(id),
    buildFence: (f: DeskFence) => buildFence(f),
  });
  const { fenceLocked, heldByFence, fenceEl, refreshFenceChrome, paintFenceColumns, readFenceRects, readWindowRects, renderNotes, fenceList, renderFences, focusedFenceId, focusFence, clearFenceFocus, showDetachGlyph } = fenceFloor;

  // ---- the GPU budget: `wb-console-gpu.ts` -------------------------------------
  const budget = createGpuBudget({
    wins,
    workspace,
    isCovered,
    applyDormancy,
    IntersectionObserver:
      typeof IntersectionObserver === "function" ? IntersectionObserver : undefined,
    queueMicrotask: (task) => queueMicrotask(task),
  });

  // ---- the opener's side of the detach: `wb-desk-detach.ts` --------------------
  // Built after the GPU budget and before the fences, which take five of its
  // functions.
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
  const { newPid, fenceSnapshot, tearDownMember, reattachFence, glyphClick } = detach;

  // ---- the view: `wb-stage-view.ts` --------------------------------------------
  // Built before the terminal, which takes `cancelSlide`.
  const view = createView({
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
    wins,
    resetExtentEdge,
    applyExtent,
    refreshFenceChrome,
  });
  const { applyLanding, flushPendingOffset, reveal, findWindow, jumpToFence, cancelSlide, autoPan, wireStage } = view;

  // ---- the key bar: `wb-console-keys.ts` ---------------------------------------
  const keys = createConsoleKeys({
    window,
    document,
    viewStore,
    wins,
    focusWin,
  });
  const { fontSize, writeClipboard, readClipboard, applyKeyBar, buildKeyBar } = keys;

  // ---- the terminal: `wb-console-terminal.ts` ----------------------------------
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

  // ---- the window chrome: `wb-stage-chrome.ts` ---------------------------------
  const chrome = createChrome({
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
  const { buildChrome } = chrome;

  // ---- the fences: `wb-stage-fences.ts` ----------------------------------------
  const fences = createFences({
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
  const { buildFence } = fences;

  // ---- the desk: `wb-desk.ts` --------------------------------------------------
  // `createDesk` starts the desk read at once: `restoreDesk` and
  // `whenDeskLoaded` wait for it.
  const desk = createDesk({
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
    inGesture: gestures.active,
    nameUnnamed,
    createRecord,
    applyLock,
    renderTitle,
    dropClosedElsewhere,
    flushPendingOffset,
    wireStage,
  });
  const { emitDesk, scheduleDeskFlush, isDeskSettled, refreshView, recordLeft, reloadDesk, whenDeskLoaded, currentDesk, currentFences, currentNotes, currentCheckouts } = desk;

  dormancy.listen();

  // ---- the window birth: `wb-console-spawn.ts` ---------------------------------
  // Built after the desk; the desk and the detach take `spawnWindow` (and the
  // detach `deskOf`) before it, as lazy arrows.
  const spawn = createSpawn({
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
    toast,
    createRecord,
    atDeskCap,
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
  const { spawnWindow, recordBirth, deskOf } = spawn;
  type SpawnWindow = typeof spawnWindow;

  // ---- the placeholder: `wb-console-placeholder.ts` ----------------------------
  // Built after the window birth; the spawn, the desk, the detach and the
  // dormancy take its functions before it, as lazy arrows.
  const placeholders = createPlaceholders({
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
  const { markRelaunch, spawnOrMissing, revivePlaceholders, spawnPlaceholder } = placeholders;
  type SpawnOrMissing = typeof spawnOrMissing;
  type SpawnPlaceholder = typeof spawnPlaceholder;

  return buildConsoleApi({
    records,
    feeds,
    title,
    popups,
    dormancy,
    stageWindow,
    columns,
    fenceFloor,
    detach,
    view,
    keys,
    fences,
    desk,
    spawn,
    placeholders,
    chrome,
    messages,
    stack,
    gestures,
    wins,
  });
}
