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
import { createChrome } from "./wb-stage-chrome.ts";
import { createStack, createGestures } from "./wb-stage-stack.ts";
import { createPopupRegistry } from "./wb-desk-popups.ts";
import { createFences } from "./wb-stage-fences.ts";
import { createStageWindow } from "./wb-stage-window.ts";
import { createStageColumns } from "./wb-stage-columns.ts";
import { createDesk, createDeskRecords } from "./wb-desk.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBDeskSync } from "./wb-desk-sync.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import { WBView } from "./wb-client-view.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { createMessages } from "./wb-messages.ts";
import { apiFetch } from "./wb-api.ts";
import { sendDocument } from "./wb-events.ts";
import type { TerminalDeps, TerminalOpts } from "./wb-console-terminal.ts";
import type { Group } from "./wb-fleet.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleOpts, ConsoleTerm, ConsoleWin, DeskChange, DeskFence, DeskWindowFields, SpawnCarry, WindowSnapshot } from "./wb-types.d.ts";

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
  pasteAfterRead,
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
  consoleCommand,
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
  const { sessionIdOf, watchingOf, windowCheckout } = WBWindowState;

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
    lastSessions: () => lastSessions,
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
    spawnWindow,
    spawnPlaceholder,
    deskOf,
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

  // ---- the terminal ----------------------------------------------------------
  // The terminal and its session socket are `wb-console-terminal.ts`; this
  // console keeps the font size, the plane and the clipboard, and hands them
  // to it.
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
  });

  // Coming back from a suspend. Registered in EVERY document that runs this
  // module (shell and each popup): each owns the sockets of the windows it
  // paints, and nothing else would revive them.
  let hiddenAt = 0;

  // The verdict the probe gives, or — with no probe, which is the popup — how
  // long this document was hidden. `Infinity` for the network events: `online`
  // fires precisely because the link the sockets ran over is a different link now.
  function isStale(hiddenMs: number) {
    if (staleProbe) {
      try {
        return staleProbe() === true;
      } catch {
        return true;
      }
    }
    return hiddenMs > RESUME_HIDDEN_MS;
  }

  function resumeAll(stale: boolean) {
    let woke = 0;
    for (const w of wins) {
      // A placeholder has no terminal, and a window whose session ended latches
      // itself — both decline from inside `resume`.
      if (w._term?.resume(stale)) woke += 1;
    }
    return woke;
  }

  // Dispose the renderer, keep the window: frame, title, state dot (fed by the
  // `/api/sessions` poll, not this socket) and desk record are untouched.
  //
  // The `.session-body` is REPLACED, not reused: `attachTerminal` registers its
  // touch handlers on the body node and `term.dispose()` does not remove them,
  // so attaching twice into one div would double every gesture.
  function sleepWindow(win: ConsoleWin) {
    const t = win._term;
    if (!t) return false;
    // Carried across the gap, because the handle that knows them is about to go.
    win._dormantSession = t.sessionId;
    win._dormantWatch = t.watching;
    // `reach` finds a window by `_term.sessionId` OR `_wantsSession`. Without
    // this a "go to session" on a sleeping console would miss its own window
    // and spawn a SECOND one against the same id, which the daemon would park.
    if (t.sessionId != null) win._wantsSession = t.sessionId;
    t.dispose("dormant");
    win._term = null;
    win._dormant = true;
    win.classList.add("dormant");
    const stale = win.querySelector(".session-body");
    if (stale) {
      const fresh = document.createElement("div");
      fresh.className = "session-body";
      stale.replaceWith(fresh);
    }
    return true;
  }

  // Rebuild through the shipped factory with the wiring this window was born
  // with. NOT a takeover: the reattach is the ordinary one, so a session another
  // client claimed while this one slept parks visibly instead of being stolen
  // back (ADR-0051 §9).
  function wakeWindow(win: ConsoleWin) {
    if (!win._dormant) return false;
    const body = win.querySelector<HTMLElement>(".session-body");
    const wiring = win._termWiring;
    if (!body || !wiring || win._dormantSession == null) return false;
    win._dormant = false;
    win.classList.remove("dormant");
    win._term = attachTerminal(body, {
      ...wiring,
      id: win._dormantSession,
      watch: win._dormantWatch,
      takeover: false,
    });
    win._dormantSession = null;
    win._rewire?.(win._term);
    return true;
  }

  // Carry out `dormancyDecision` for one window. The fold owns the rule; this
  // owns the clock, and re-asks when the timer fires — the window may have been
  // focused, maximized or closed meanwhile.
  function applyDormancy(win: ConsoleWin) {
    const verdict = dormancyDecision(dormancyInputs(win));
    if (win._dormantTimer) {
      clearTimeout(win._dormantTimer);
      win._dormantTimer = null;
    }
    if (verdict === "wake") wakeWindow(win);
    budget.scheduleGpu();
    if (verdict === "sleep") {
      win._dormantTimer = setTimeout(() => {
        win._dormantTimer = null;
        if (dormancyDecision(dormancyInputs(win)) === "sleep") sleepWindow(win);
      }, DORMANT_AFTER_MS);
    }
    return verdict;
  }

  function isCovered(win: ConsoleWin) {
    if (fillsViewport(win)) return false;
    for (const other of wins) if (other !== win && fillsViewport(other)) return true;
    return false;
  }

  // Re-ask only the windows whose cover changed: `applyDormancy` restarts the
  // grace period, and the callers run on every columns repaint.
  function refreshCover() {
    for (const win of wins) {
      const covered = isCovered(win);
      if (covered === !!win._covered) continue;
      win._covered = covered;
      applyDormancy(win);
    }
  }

  // The live reading of one window, handed to the pure fold.
  function dormancyInputs(win: ConsoleWin) {
    return {
      // Unobserved windows have never been told; treat them as visible, which
      // is the reading that changes nothing.
      intersecting: win._visible !== false,
      covered: isCovered(win),
      dormant: !!win._dormant,
      maximized: win.classList.contains("maximized") || win.classList.contains("column"),
      fullscreen: isFull(win),
      focused: win.classList.contains("focused"),
      hasTerminal: !!win._term,
      ended: win.classList.contains("ended"),
      // NOT `sessionIdOf`: only these two sources survive `sleepWindow`. A
      // spawned-but-silent console with only `_wantsSession` would sleep with
      // nothing for `wakeWindow` to reattach to, and never come back.
      sessionId: win._term?.sessionId ?? win._dormantSession ?? null,
    };
  }

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") {
      hiddenAt = Date.now();
      return;
    }
    const hiddenMs = hiddenAt ? Date.now() - hiddenAt : 0;
    hiddenAt = 0;
    resumeAll(isStale(hiddenMs));
    revivePlaceholders();
  });
  // No `pageshow`: a document holding an open WebSocket is not bfcache-eligible,
  // so the restore this would catch cannot happen here.
  window.addEventListener("online", () => {
    resumeAll(true);
    revivePlaceholders();
    // Desk changes kept while the link was down go now.
    if (sync.hasPending()) scheduleDeskFlush(0);
  });

  // The key-bar setting changed. The shell re-emits every save on
  // `workbench:action`. A detached popup never receives it (its `WB.emit` posts
  // to the opener) and reads no store, so its bar stays on auto.
  document.addEventListener("workbench:action", (e: DocumentEventMap["workbench:action"]) => {
    if (e.detail?.action !== "setting-change" || e.detail.key !== "consoles.key_bar") return;
    for (const w of wins) applyKeyBar(w);
  });

  // Publish the keyboard inset. The popup and the node harness load this module
  // without `visualViewport`; absent, `--kb-inset` keeps the stylesheet's
  // `:root` default of 0px.
  const vv = window.visualViewport;
  if (vv) {
    const publishInset = () => {
      // iOS PANS rather than resizes: with the visual viewport scrolled down, a
      // window sized to the remaining height starts above the visible region,
      // titlebar included. Scroll back first, then measure; `offsetTop` is still
      // subtracted because the scroll lands a frame later.
      if (vv.offsetTop > 0) window.scrollTo(0, 0);
      const px = keyboardInset({
        innerHeight: window.innerHeight,
        height: vv.height,
        offsetTop: vv.offsetTop,
        scale: vv.scale,
      });
      document.documentElement.style.setProperty("--kb-inset", px + "px");
    };
    vv.addEventListener("resize", publishInset);
    vv.addEventListener("scroll", publishInset);
    // The inset must HEAL: on an iPad the keyboard rising kicks the document
    // out of fullscreen, a transition the visual viewport does not always
    // report, and a stale `--kb-inset` keeps every maximized console short.
    document.addEventListener("fullscreenchange", publishInset);
    window.addEventListener("orientationchange", publishInset);
    window.addEventListener("resize", publishInset);
    // A terminal that has LOST focus cannot be the reason a keyboard is up.
    document.addEventListener("focusout", () => setTimeout(publishInset, 250));
  }

  function newId(prefix: string) {
    // `crypto.randomUUID` is undefined in a non-secure context and the daemon can
    // bind a plain-http LAN address (ADR-0032), so build the id by hand.
    return prefix + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 10);
  }
  function newDeskId() {
    return newId("w-");
  }
  function newFenceId() {
    return newId("f-");
  }
  // `/api/sessions` and the peers it did not hear from (`unheard`, a Set of
  // daemon ids). Rejects when the list cannot be read. On this read a console
  // of an unheard peer is neither running nor gone: nothing relaunches, adopts
  // or forgets it (ADR-0050 amendment 2026-10-04).
  // Reads in flight at the same moment share one request: every held box
  // asks on the same fleet read, and each request asks every peer.
  let sessionsRead: Promise<{ sessions: HostedSession[]; unheard: ReadonlySet<string> }> | null = null;
  function readSessions() {
    if (!sessionsRead) {
      sessionsRead = (async () => {
        const r = await apiFetch("GET /api/sessions");
        if (!r.ok) throw new Error("sessions unavailable");
        const route = WBSessionRoute;
        return {
          sessions: await r.json(),
          unheard: route.unanswered(r.headers?.get?.(route.UNANSWERED_HEADER)),
        };
      })().finally(() => {
        sessionsRead = null;
      });
    }
    return sessionsRead;
  }
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
  // The project name and tooltip text per repo ref, fed by the shell
  // (`ingestProjects`) from the daemon's project list. A ref the shell has not
  // named yet (the detached popup, a desk read before the list) falls back to
  // its slug, never to the ref: a peer ref carries a `<daemon_id>/` routing
  // head (ADR-0052 §5).
  const projectNames = new Map<string | null | undefined, { name: string; title: string }>();
  function projectNameOf(ref: string | null | undefined) {
    const known = projectNames.get(ref);
    if (known) return known.name;
    return WBFleet ? WBFleet.refSlug(ref) : ref!;
  }
  function projectTitleOf(ref: string | null | undefined) {
    if (ref === "~") return ref;
    return projectNames.get(ref)?.title || projectNameOf(ref);
  }
  // `rows` is `[{ ref, name, title }]`. Titles and tooltips already drawn are
  // drawn again; a console NAME already given is the operator's and stays.
  function ingestProjects(rows: { ref: string; name: string; title?: string }[] | null | undefined) {
    projectNames.clear();
    for (const r of rows || []) {
      if (r && r.ref && r.name) projectNames.set(r.ref, { name: r.name, title: r.title || r.name });
    }
    for (const win of wins) {
      if (!win._title || !win._presentation) continue;
      const p = win._presentation;
      p.tooltip = WBConsoleName.tooltipLines(projectTitleOf(win._deskRepo), p.environment, p.name).join("\n");
      win._title.title = p.tooltip;
      renderTitle(win, win._title, p);
    }
  }

  // Each peer's fleet group by daemon id, and the shell's wake action, fed by
  // the shell (`ingestFleet`) after every fleet read. The shell owns the fleet;
  // this is only its last answer, for the placeholders of peer projects.
  // `readFleet` is the shell's fleet read on demand, for a window that saw
  // its peer fail.
  const peerGroups = new Map<string, Group>();
  let wakePeer: ((daemon: string) => Promise<boolean>) | null = null;
  let readFleet: (() => void) | null = null;
  function ingestFleet(groups: Group[] | null | undefined, hooks: { wake?: (daemon: string) => Promise<boolean>; read?: () => void } | null | undefined) {
    peerGroups.clear();
    for (const g of groups || []) if (g && g.daemon && !g.local) peerGroups.set(g.daemon, g);
    if (typeof hooks?.wake === "function") wakePeer = hooks.wake;
    if (typeof hooks?.read === "function") readFleet = hooks.read;
    for (const win of [...wins]) if (typeof win._peerRefresh === "function") win._peerRefresh();
  }

  // The shell's last `/api/sessions` poll, kept for the menus' state dots.
  let lastSessions: HostedSession[] = [];

  // The column paint is `wb-stage-columns.ts` (ADR-0051 §5), built with the window
  // states above; this console keeps the desk records it paints.

  // Every window on the plane, for the Go-to picker. Reads the DOM, not `wins`:
  // a snapshot at menu open, not reactive state.
  function list() {
    const st = stage();
    if (!st) return [];
    return [...st.querySelectorAll<ConsoleWin>(".session-window")].map((w) => ({
      id: w._deskId,
      agent: w._deskAgent,
      name: w._deskConsoleName ?? null,
      // The title's tooltip, so the Go-to row says what the title says.
      tooltip: w._title?.title || "",
      // `"~"` is the desk's "no repo".
      repo: w._deskRepo === "~" ? null : w._deskRepo,
      kind: w._deskKind,
      running: !w.classList.contains("placeholder"),
      state: w._agentState ?? null,
    }));
  }

  // The agent-state dot per window, from the shell's `/api/sessions` poll
  // (ADR-0059 §5): the state word as a class, hidden when the row says
  // nothing. A placeholder has no session and keeps no dot.
  function ingestSessions(sessions: HostedSession[] | null | undefined) {
    lastSessions = Array.isArray(sessions) ? sessions : [];
    for (const win of wins) {
      const dot = win._stateDot;
      if (!dot) continue;
      const row = sessionRowFor(win, sessions);
      const state = row?.agent_state?.state ?? null;
      win._agentState = state;
      dot.className = "session-state" + (state ? ` ${state}` : "");
      dot.title = agentStateTitle(state, row?.agent_state?.detail);
      dot.hidden = !state;
    }
  }

  // `visibilitychange` and `online` both land on one iOS resume; without the
  // probe seam the popup would have no verdict at all.
  let staleProbe = OPTS.isStale || null;
  function setStaleProbe(fn: (() => boolean) | null) {
    staleProbe = typeof fn === "function" ? fn : null;
  }

  function hasTouchSurface() {
    try {
      return !!window.matchMedia?.("(any-pointer: coarse)")?.matches;
    } catch {
      return false;
    }
  }

  // Per browser profile (wb-settings.ts `scope: client`, stored by
  // `wb-client-view.ts`). Absent — the popup reads nothing — means auto.
  function keyBarMode() {
    return viewStore?.read()?.keys ?? null;
  }

  function applyKeyBar(win: ConsoleWin) {
    win.classList.toggle("keys", keyBarVisible(keyBarMode(), hasTouchSurface()));
    win._applyInputMode?.();
  }

  function fontSize() {
    return viewStore?.read()?.font ?? FONT_DEFAULT;
  }

  // Every window at once. `fit` is required: the BOX does not change, so the
  // ResizeObserver never fires and the daemon would never learn the new size.
  function setFont(px: number) {
    viewStore?.patch({ font: px });
    for (const w of wins) {
      const t = w._term;
      if (!t) continue;
      t.term.options.fontSize = px;
      try {
        t.fit.fit();
      } catch {}
    }
    return px;
  }

  // Put `text` on the system clipboard and give the terminal its focus back.
  // `navigator.clipboard` exists only in a SECURE CONTEXT (loopback, https), so
  // the write degrades to a hidden textarea + `execCommand`. Every path is
  // best-effort and SILENT: `writeText` rejects with "Document is not focused"
  // whenever the workbench is not the focused window, and Chrome can refuse
  // `execCommand` outside a user gesture; the copy is dropped, not queued.
  function writeClipboard(text: string, term: XtermTerminal) {
    if (!text) return;
    // The fallback moves focus to the textarea; without giving it back, the
    // operator's next keystroke goes nowhere and the console looks dead.
    const fallback = () => {
      // A detached popup has its OWN document — write into the one this terminal
      // actually lives in, not the shell's.
      const doc = term?.element?.ownerDocument || document;
      const ta = doc.createElement("textarea");
      ta.value = text;
      // Off-screen rather than `display:none`: a hidden element cannot be selected.
      ta.style.position = "fixed";
      ta.style.left = "-9999px";
      doc.body.append(ta);
      try {
        ta.select();
        doc.execCommand("copy");
      } catch {}
      ta.remove();
      try {
        term?.focus();
      } catch {}
    };
    if (!navigator.clipboard) {
      fallback();
      return;
    }
    navigator.clipboard.writeText(text).catch(fallback);
  }

  // The read half: resolves `{ image: Blob }` or `{ text }`. Always a promise:
  // an insecure origin has no `navigator.clipboard` and the API throws
  // synchronously. `read()` first, because `readText()` resolves "" for an
  // image-only clipboard (an iOS screenshot). ONE call: each read raises
  // Safari's "Paste" callout, so a text fallback would ask twice.
  function readClipboard(): ReturnType<TerminalDeps["readClipboard"]> {
    try {
      const clip = navigator.clipboard;
      if (typeof clip.read !== "function") {
        return Promise.resolve(clip.readText()).then((text) => ({ text }));
      }
      return Promise.resolve(clip.read()).then(clipboardContent);
    } catch {
      return Promise.resolve({ text: "" });
    }
  }

  // Build the chrome and attach a live terminal. Shared by `open()` and the
  // load-time restore; `termOpts` is the `attachTerminal` opts, `desk` the
  // record this window continues (absent for a fresh launch).
  function spawnWindow(termOpts: TerminalOpts, label: string | null | undefined, repo: string | null | undefined, desk?: SpawnCarry) {
    const kind = termOpts.console ? "console" : "agent";
    const { win, body, title, restartBtn, closeBtn } = buildChrome(label, repo, desk, kind);
    // Read at launch, never later: a rename reaches the next restart and never
    // restarts the running session (ADR-0066 §6).
    if (termOpts.id == null && !termOpts.console) termOpts = { ...termOpts, name: win._deskConsoleName };
    // Every NEW launch names its record: the daemon keeps one session per
    // record, so a second page relaunching it joins this one (ADR-0050
    // amendment 2026-10-04).
    if (termOpts.id == null) termOpts = { ...termOpts, record: win._deskId };
    // A launch that names a worktree records the intent NOW, so a daemon that
    // dies mid-launch still leaves it behind.
    if (termOpts.checkout !== undefined) win._deskCheckout = termOpts.checkout ?? null;

    // The terminal owns the Ctrl and Shift latches and the selection arming;
    // the key-bar buttons (assigned below) only REFLECT them.
    let ctrlBtn: HTMLButtonElement | null = null;
    let shiftBtn: HTMLButtonElement | null = null;
    let selBtn: HTMLButtonElement | null = null;

    // Debounced nudge for a keystroke typed into a parked window (#335):
    // repeated typing EXTENDS the pulse rather than stacking timers.
    let nudgeTimer: ReturnType<typeof setTimeout> | null = null;
    function clearNudge() {
      if (nudgeTimer) {
        clearTimeout(nudgeTimer);
        nudgeTimer = null;
      }
      const strip = win.querySelector(".session-parked");
      if (!strip) return;
      strip.classList.remove("is-nudged");
      const hintEl = strip.querySelector(".session-parked-hint");
      if (hintEl) hintEl.textContent = "";
    }

    // A held window's strip: the fleet's sentence for the peer, its action, and
    // the daemon's diagnosis under Details. Built once, then reworded on each
    // fleet read.
    const peerDaemon = WBFleet?.refDaemon(repo) || "";
    const PEER_BUTTON: Partial<Record<NonNullable<WBConsoleSession.PeerAction>, string>> = { wake: "Wake", retry: "Try again" };
    const showPeerDown = (group: Group | null) => {
      let strip: HTMLElement | null = win.querySelector<HTMLElement>(".session-peer-down");
      if (!strip) {
        strip = document.createElement("div") as HTMLElement;
        strip.className = "session-peer-down";
        const text = document.createElement("span");
        text.className = "session-peer-down-text";
        const detail = document.createElement("details");
        detail.className = "session-detail";
        const summary = document.createElement("summary");
        summary.textContent = "Details";
        detail.append(summary, document.createElement("p"));
        const btn = document.createElement("button");
        btn.className = "session-reconnect";
        btn.addEventListener("click", (e: MouseEvent) => {
          e.stopPropagation();
          if (btn.dataset.act === "wake") wakePeer?.(peerDaemon);
          else readFleet?.();
        });
        strip.append(text, detail, btn);
        win.insertBefore(strip, body);
      }
      const view = peerOfflineView(group, null, win._deskEnvironment);
      strip.querySelector(".session-peer-down-text")!.textContent = view.text;
      const detail = strip.querySelector<HTMLElement>(".session-detail")!;
      detail.querySelector("p")!.textContent = view.detail || "";
      detail.hidden = !view.detail;
      const btn = strip.querySelector<HTMLElement>(".session-reconnect")!;
      btn.dataset.act = view.action || "";
      const label = view.action && PEER_BUTTON[view.action];
      btn.hidden = !label;
      if (label) btn.textContent = label;
    };

    // NAMED, not inline: a dormant console rebuilds its terminal (`wakeWindow`)
    // and the rebuild must be wired to the same chrome. Everything closes over
    // `win`, never a particular terminal.
    const termWiring: TerminalOpts = {
      ...termOpts,
      onCtrlLatch: (on) => {
        if (ctrlBtn) ctrlBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      onShiftLatch: (on) => {
        if (shiftBtn) shiftBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      onSelecting: (on) => {
        if (selBtn) selBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      // The daemon assigned/echoed this window's session id: record it.
      onSession: (_id, owner) => {
        const presentation = sessionPresentation(label, repo, desk, owner);
        win._sessionCheckout = presentation.checkout;
        win._deskDaemonId = presentation.daemonId;
        win._deskEnvironment = presentation.environment;
        // The announcement wins over the request and the record.
        win._deskCheckout = presentation.checkout;
        renderTitle(win, title, presentation);
        title.title = presentation.tooltip;
        recordSession(win);
        // Without an id the window could not sleep (`dormancyDecision` D5).
        applyDormancy(win);
      },
      // Parked: watching a session another window drives. It KEEPS its window
      // and output; the strip's button is the only way `takeover` is ever sent
      // (#334, ADR-0051 §9).
      onPark: () => {
        if (win.querySelector(".session-parked")) return;
        const strip = document.createElement("div");
        strip.className = "session-parked";
        const text = document.createElement("span");
        // The titlebar already names the agent and the repo; repeating them here
        // wraps the strip on a phone.
        text.textContent = "Read-only. Another window has control.";
        const hint = document.createElement("span");
        hint.className = "session-parked-hint";
        const btn = document.createElement("button");
        btn.className = "session-reconnect";
        btn.dataset.act = "take-over";
        btn.textContent = "Take over";
        btn.addEventListener("click", (e: MouseEvent) => {
          e.stopPropagation();
          win._term?.takeOver();
        });
        strip.append(text, hint, btn);
        win.insertBefore(strip, body);
      },
      // A watcher's keystroke never reaches the child (gated in
      // `attachTerminal`); this pulses the strip so the refusal is SEEN (#335).
      onWatchedInput: () => {
        const strip = win.querySelector(".session-parked");
        if (!strip) return;
        clearTimeout(nudgeTimer);
        strip.classList.add("is-nudged");
        const hintEl = strip.querySelector(".session-parked-hint");
        if (hintEl) hintEl.textContent = "Input is read-only. Take over to type.";
        nudgeTimer = setTimeout(() => {
          nudgeTimer = null;
          strip.classList.remove("is-nudged");
          if (hintEl) hintEl.textContent = "";
        }, 2000);
      },
      onResume: () => {
        clearNudge();
        win.querySelector(".session-parked")?.remove();
      },
      // A session that ENDED: the parked strip's "take over" would only spin
      // at a dead id. The window stays (its scrollback is the last thing the
      // agent said) and the restart control is tinted as the next action.
      // A launch a PEER refused becomes that peer's placeholder, which says why
      // in words and comes back when the peer does.
      onEnded: (announced, refusal) => {
        clearNudge();
        win.querySelector(".session-parked")?.remove();
        if (announced === "refused" && WBFleet?.refDaemon(repo)) {
          const carry = deskOf(win);
          discard();
          spawnPlaceholder(carry, null, { message: refusal });
          return;
        }
        win.classList.add("ended");
      },
      // A peer project: the fleet's word on its peer gates every reattach.
      ...(peerDaemon
        ? {
            peerGroup: () => peerGroups.get(peerDaemon) || null,
            readFleet: () => readFleet?.(),
          }
        : {}),
      onPeerHold: (group) => showPeerDown(group),
      onPeerBack: () => win.querySelector(".session-peer-down")?.remove(),
    };
    win._termWiring = termWiring;
    // The fleet reaches a live window here; `wakeWindow` replaces `_term`, so
    // it is read at each call. A dormant window has none, and its wake asks.
    win._peerRefresh = () => win._term?.peerRefresh();
    if (birthDecision({ id: termOpts.id, observed: !!budget.dormancyWatch() }) === "dormant") {
      // The state `sleepWindow` leaves. `_visible` is false until the observer
      // reports: a window not yet reported reads as visible, and any
      // `applyDormancy` before the report would wake it.
      win._dormantSession = termOpts.id!;
      win._dormantWatch = termOpts.watch;
      win._dormant = true;
      win._visible = false;
      win.classList.add("dormant");
    } else {
      // On the window, not in a local: after a sleep/wake cycle a captured
      // local would name a disposed terminal.
      win._term = attachTerminal(body, termWiring);
    }
    // The placeholder's relaunch path: carry this window's record, drop the
    // dead window, spawn a FRESH session — never the old `id`/`watch` opts,
    // which would reattach to a torn-down session. `checkout` CHOSEN by the
    // title's switcher (#412); `undefined` means "the recorded one".
    // Take this window off the stage without announcing a gap: a window with
    // the same desk id takes its place at once.
    const discard = () => {
      clearNudge();
      closeCheckoutMenu();
      win._term?.dispose("window-closed");
      win.remove();
      budget.untrackDormancy(win);
      wins.delete(win);
      applyExtent();
    };
    const relaunchIn = (checkout: string | null | undefined) => {
      // A restart is the operator's act: an adopted console gets its record.
      const carry = { ...deskOf(win), unrecorded: false };
      markRelaunch(carry.id, discard);
      // `win._deskKind`, not the local `kind`: a window reattached at load was
      // spawned with `{id, repo}` only. `~` is the daemon's repo-less label.
      const plain = win._deskKind === "console";
      const at = repo === "~" ? undefined : repo;
      // A window reattached at load has no `termOpts.checkout`; the recorded
      // announcement, else the desk record, keeps the restart in its tree (#411).
      const fresh = plain
        ? { console: true, repo: at, command: consoleCommand(label) }
        : {
            repo: at,
            agent: termOpts.agent ?? label,
            // Explicit target, else `windowCheckout`: announcement beats
            // request beats record.
            checkout:
              checkout !== undefined ? checkout : windowCheckout(win, termOpts.checkout),
          };
      spawnOrMissing(fresh, label, repo, carry);
      WB.emit("console-restart", { repo: at || null, agent: plain ? null : label });
    };
    win._relaunchIn = relaunchIn;
    restartBtn.addEventListener("click", (e: MouseEvent) => {
      e.stopPropagation();
      restartWin(win);
    });
    // The switcher needs the repo's listing; one read per ref, cached.
    if (kind === "agent") ensureListing(repo);
    // The id this window is attaching to, known before the terminal reports one.
    if (termOpts.id != null) win._wantsSession = termOpts.id;

    // THE KEY BAR. Here, not in `buildChrome`: a placeholder has no session
    // for the buttons to talk to.
    {
      const bar = document.createElement("div");
      bar.className = "session-keys";
      // The strip refuses focus: on iOS losing the textarea's caret dismisses
      // the keyboard. `pointerdown` + `mousedown`; `touchstart` is NOT
      // prevented — that would suppress the synthesized click.
      const holdFocus = (e: Event) => e.preventDefault();
      bar.addEventListener("pointerdown", holdFocus);
      bar.addEventListener("mousedown", holdFocus);

      // `icon` (a Bootstrap Icons class) draws the key as that glyph; without
      // it the key shows `text`.
      const key = (name: string, text: string, title: string, cls?: string, icon?: string) => {
        const b = document.createElement("button");
        b.type = "button";
        // Not in the tab order: a keyboard user already has these keys.
        b.tabIndex = -1;
        b.className = "session-key" + (cls ? " " + cls : "");
        b.dataset.key = name;
        b.title = title;
        if (icon) {
          const i = document.createElement("i");
          i.className = "bi " + icon;
          b.append(i);
        } else {
          b.textContent = text;
        }
        bar.append(b);
        return b;
      };

      // The virtual keyboard opens only from here (`terminalInputMode`). Any
      // blur of the field closes it: the phone's own Done key, another window
      // taking focus, the page going to the background.
      let keyboardOpen = false;
      let refocusing = false;
      const kbBtn = key("keyboard", "", "Show or hide the keyboard", "", "bi-keyboard");
      kbBtn.setAttribute("aria-pressed", "false");
      const applyInputMode = () => {
        kbBtn.setAttribute("aria-pressed", keyboardOpen ? "true" : "false");
        const field = win._term?.term.textarea;
        if (!field) return;
        const mode = terminalInputMode(win.classList.contains("keys"), keyboardOpen);
        if (mode) field.setAttribute("inputmode", mode);
        else field.removeAttribute("inputmode");
      };
      win._applyInputMode = applyInputMode;
      const toggleKeyboard = (field: HTMLTextAreaElement) => {
        keyboardOpen = !keyboardOpen;
        applyInputMode();
        // iOS reads `inputmode` only when the field takes focus. The click
        // handler focuses it again, inside the same tap.
        refocusing = true;
        field?.blur();
        refocusing = false;
      };

      // Most used first: on a phone only the keys up to Down show without a
      // scroll (the narrow-screen rule in 06-consoles.css).
      key("esc", "esc", "Escape");
      shiftBtn = key("shift", "shift", "Shift: applies to the next key");
      shiftBtn.setAttribute("aria-pressed", "false");
      key("tab", "tab", "Tab");
      key("enter", "", "Enter", "", "bi-arrow-return-left");
      key("slash", "/", "Slash");
      key("up", "", "Up", "", "bi-arrow-up");
      key("down", "", "Down", "", "bi-arrow-down");
      key("left", "", "Left", "", "bi-arrow-left");
      key("right", "", "Right", "", "bi-arrow-right");
      ctrlBtn = key("ctrl", "ctrl", "Ctrl: applies to the next key");
      ctrlBtn.setAttribute("aria-pressed", "false");
      key("ctrl-c", "^C", "Ctrl-C: interrupt");
      // Arms ONE drag to select whole lines; the gesture's end disarms it.
      selBtn = key("select", "sel", "Select lines: drag across the screen");
      selBtn.setAttribute("aria-pressed", "false");

      const gap = document.createElement("span");
      gap.className = "session-keys-gap";
      bar.append(gap);

      // `writeClipboard`'s textarea fallback runs inside this click (a user
      // gesture), which is what makes it work on an insecure LAN origin.
      // `bi-copy`, not `bi-clipboard`: the clipboard glyph is the PASTE icon.
      const copyBtn = key("copy", "", "Copy selection", "", "bi-copy");
      copyBtn.disabled = true;
      const syncCopy = () => {
        copyBtn.disabled = !win._term?.term.hasSelection();
      };
      // The one piece of chrome bound to a PARTICULAR terminal:
      // `onSelectionChange` is on the xterm instance, so a woken console's new
      // instance needs it again.
      win._rewire = (t: ConsoleTerm) => {
        t.term.onSelectionChange(syncCopy);
        syncCopy();
        t.term.textarea?.addEventListener("blur", () => {
          if (refocusing || !keyboardOpen) return;
          keyboardOpen = false;
          applyInputMode();
        });
        applyInputMode();
      };
      if (win._term) win._rewire(win._term);

      // Paste. The read has no `execCommand` fallback, so on an insecure origin
      // the button is disabled (`pasteOffered`). An image becomes the same
      // `image.write` drop as a keyboard paste (ADR-0055).
      const pasteBtn = key("paste", "", "Paste", "", "bi-clipboard");
      pasteBtn.disabled = !pasteOffered(navigator.clipboard);

      key("font-down", "A−", "Smaller text");
      key("font-up", "A+", "Larger text");

      bar.addEventListener("click", (e: MouseEvent) => {
        const btn = (e.target as Element).closest<HTMLButtonElement>("button[data-key]");
        if (!btn) return;
        e.stopPropagation();
        const name = btn.dataset.key;
        // The clipboard READ goes first, before any focus move: Safari grants
        // it only to a call made synchronously inside the tap. `term.paste`
        // then rides `onData → sendInput`, so a watcher's paste is refused
        // like a keystroke. A refused or empty read is dropped silently.
        // Dormant: the terminal is off and every branch below speaks to one.
        if (!win._term) return;
        const pressed = win._term;
        const read = name === "paste" ? readClipboard() : null;
        focusWin(win);
        if (read) {
          void pasteAfterRead(win, pressed, read);
        } else if (name === "keyboard") {
          toggleKeyboard(win._term.term.textarea);
        } else if (name === "copy") {
          writeClipboard(win._term.term.getSelection(), win._term.term);
        } else if (name === "select") {
          win._term.setSelecting(!win._term.selecting);
        } else if (name === "font-up" || name === "font-down") {
          setFont(stepFont(fontSize(), name === "font-up" ? 1 : -1));
        } else {
          win._term.sendKey(name!);
        }
        // Back to the terminal, inside the gesture, so an open keyboard stays up.
        win._term.term.focus();
      });

      win.append(bar);
      applyKeyBar(win);
    }

    closeBtn.onclick = async () => {
      // A dormant window has no handle; it carried both answers across the gap.
      const id = sessionIdOf(win);
      const watching = watchingOf(win);
      // A watcher's × closes only its own window; the writer's ends the session.
      const ok = await askConfirm({
        title: "Close this console?",
        message: watching
          ? `Closes this window only. ${label} keeps running.`
          : `Ends the ${label} session. You lose the text in this console.`,
        confirmLabel: "Close",
        danger: true,
      });
      if (!ok) return;
      const finish = () => {
        // A window closed mid-pulse must not leave `nudgeTimer` pending.
        clearNudge();
        win._term?.dispose("window-closed");
        // A watcher's × closes this window only: the console still runs, and
        // its record stays on the desk.
        if (!watching) forgetRecord(win._deskId);
        win.remove();
        budget.untrackDormancy(win);
        wins.delete(win);
        applyExtent();
        WB.emit("console-close", { repo: repo || null, agent: label });
        changed();
      };
      // End the daemon-owned session first, then drop the window. A WATCHER
      // closes only its own window: `/api/sessions/close` tree-kills the child
      // another operator is driving.
      if (id != null && !watching) {
        fetch(WBSessionRoute.closeUrl(id, win._deskRepo), {
          method: "POST",
        }).then(
          (response) => {
            if (WBSessionRoute.closeSucceeded(response.status)) finish();
            else
              win._term?.term.write(
                `\r\n[close failed — the daemon refused it]\r\n`,
              );
          },
          () => win._term?.term.write("\r\n[close failed — connection unavailable]\r\n"),
        );
      } else {
        finish();
      }
    };

    wins.add(win);
    budget.trackDormancy(win);
    changed();
    recordBirth(win, desk);
    return win;
  }

  // What a birth writes. A window whose record is in the view writes only the
  // fields its birth changed (a relaunch in the primary tree, a name the
  // record lacked) — never its rect, which came from the record. A window
  // carried from an adopted, unrecorded one stays unrecorded. Any other
  // window is a new console: its record is created.
  function recordBirth(win: ConsoleWin, carry: SpawnCarry | undefined) {
    const r = currentDesk().find((x) => x.id === win._deskId);
    if (r) {
      const fields: DeskWindowFields = {};
      if ((r.checkout ?? null) !== (win._deskCheckout ?? null)) fields.checkout = win._deskCheckout ?? null;
      if (win._deskConsoleName && r.consoleName !== win._deskConsoleName) {
        fields.consoleName = win._deskConsoleName;
      }
      if (Object.keys(fields).length) setWin(win, fields);
      return;
    }
    if (carry?.unrecorded) {
      win._deskUnrecorded = true;
      setTimeout(() => adoptOrphan(win), ADOPT_GRACE_MS);
      return;
    }
    createRecord(win);
  }

  // How long an adopted console waits for the record the page that launched
  // it writes. A launch and its create land within a second; a console whose
  // record no page will write (another device removed it) still gets one.
  const ADOPT_GRACE_MS = 10000;
  function adoptOrphan(win: ConsoleWin) {
    if (!win.isConnected || !win._deskUnrecorded) return;
    // A fresh read first: the record may have landed without a push here.
    reloadDesk().then(() => {
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
    if (
      (r.sessionId ?? null) !== session.sessionId ||
      (r.daemonId ?? null) !== session.daemonId ||
      (r.environment ?? null) !== session.environment
    ) {
      fields.session = session;
    }
    if ((r.checkout ?? null) !== (win._deskCheckout ?? null)) fields.checkout = win._deskCheckout ?? null;
    if (Object.keys(fields).length) setWin(win, fields);
  }

  // The window's placement as a desk record, to carry identity and box across
  // a rebuild (takeover, placeholder → live console).
  function deskOf(win: ConsoleWin): WindowSnapshot {
    return {
      unrecorded: !!win._deskUnrecorded,
      id: win._deskId,
      repo: win._deskRepo,
      agent: win._deskAgent,
      kind: win._deskKind,
      daemonId: win._deskDaemonId,
      environment: win._deskEnvironment,
      checkout: win._deskCheckout ?? null,
      locked: !!win._deskLocked,
      consoleName: win._deskConsoleName ?? null,
      rect: restoreRect(win),
      max: win.classList.contains("maximized"),
    };
  }

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
        const woke = typeof wakePeer === "function" && (await wakePeer(daemon));
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
