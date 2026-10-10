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
import { createChrome, createGestures } from "./wb-stage-chrome.ts";
import { createPopupRegistry } from "./wb-desk-popups.ts";
import { createFences } from "./wb-stage-fences.ts";
import { createDesk } from "./wb-desk.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBDeskSync } from "./wb-desk-sync.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import { WBView } from "./wb-client-view.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { apiFetch } from "./wb-api.ts";
import type { ApiRefusal } from "./wb-api.ts";
import { sendDocument } from "./wb-events.ts";
import type { DetachedMember, Painted } from "./wb-columns.ts";
import type { TerminalDeps, TerminalOpts } from "./wb-console-terminal.ts";
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { Group } from "./wb-fleet.ts";
import type { ConsoleOpts, ConsoleTerm, ConsoleWin, DeskChange, DeskFence, DeskNote, DeskRecord, DeskReply, DeskWindowFields, ExtentOpts, NoteCard, Rect, SpawnCarry, Stacked, WindowSnapshot } from "./wb-types.d.ts";

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
    STAGE_MARGIN,
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
    WIN_MIN_W,
    WIN_MIN_H,
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
  // Injection point, read once per instance. `main.ts` passes nothing (every
  // default below is the shell's behaviour); `detached-fence-main.ts` overrides all five,
  // which is what makes the popup unable to author the desk or the viewport.
  const OPTS = opts || {};
  const deskSink = OPTS.deskSink || WBDeskSink.daemon();
  const viewStore = OPTS.viewStore || WBView;
  // Detach registry + lifecycle channel (#347). Denied in the popup: `window.open`
  // hands it a COPY of the opener's session-scoped store, so a read there drifts.
  // This module names no browser store of its own — pinned in lib.rs.
  const link = OPTS.detachLink || WBDetachLink.link();
  const wins = new Set<ConsoleWin>();

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
    desk: () => desk,
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
  // The fence records' chrome, the fence verbs and the focused fence are
  // `wb-stage-fence-list.ts`; this console keeps the fence records and hands
  // them to it. Built before the view and the fences, which it reaches
  // through lazy arrows.
  const fenceFloor = createFenceList({
    window,
    document,
    OPTS,
    popups,
    fences: () => fences,
    notes: () => notes,
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
    fences: () => fences,
    notes: () => notes,
    stage,
    changed,
    applyExtent,
    forgetRecord,
    loadDesk,
    saveNotes,
    spawnWindow,
    spawnPlaceholder,
    deskOf,
    whenDeskLoaded,
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
  // The elements under a gesture of the operator have one owner,
  // `createGestures`: the chrome's drag and resize and the fence gestures
  // begin and end them, and a desk this page takes asks it (`inGesture`).
  // The titlebar, the drag, the resize and the free cascade are
  // `wb-stage-chrome.ts`; this console hands it the window state.
  const gestures = createGestures();
  const { buildChrome, makeDraggable, startResize } = createChrome({
    window,
    document,
    OPTS,
    gestures,
    stage,
    workspace,
    fences: () => fences,
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
    fences: () => fences,
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

  // Focus stacking. `z` climbs each time a window is raised; when it reaches the
  // ceiling the whole stack is renormalized back down (preserving order) so the
  // console z-index never overtakes the runs overlay (z 150) or the tabbar.
  const Z_BASE = 60;
  const Z_CEIL = 120;
  let z = Z_BASE;

  function changed() {
    // A close takes a full bleed away without moving the windows under it, so
    // the observer has nothing to report.
    refreshCover();
    sendDocument(document, "workbench:consoles-changed", { count: wins.size });
  }

  // Ask before a click that cannot be taken back: tiling moves every console in
  // a fence, removing a fence takes the region out from under them, a console's
  // × ends a live session — each one pixel from something harmless.
  //
  // Not the shell's Alpine dialog: this module also runs in the detached-fence
  // popup, which has no Alpine and no modal markup. It borrows the shell's
  // CLASSES (styles.css is loaded in both). Not `window.confirm`: it blocks the
  // thread, and an automated browser dismisses it by default — every guarded
  // click would silently cancel.
  // `notice: true` is the one-button form: OK alone, focused, Enter/Escape dismiss.
  function askConfirm({ title, message, confirmLabel = "Confirm", danger = false, notice = false }: ConfirmOptions) {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-confirm";
    const modal = document.createElement("div");
    modal.className = "modal confirm-modal";
    modal.setAttribute("role", "alertdialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", title);
    const head = document.createElement("div");
    head.className = "modal-head";
    const mark = document.createElement("i");
    mark.className = "bi " + (danger ? "bi-exclamation-triangle" : "bi-question-circle");
    if (danger) mark.style.color = "var(--danger)";
    head.append(mark);
    const heading = document.createElement("span");
    heading.className = "modal-title";
    heading.textContent = title;
    head.append(heading);
    const body = document.createElement("p");
    body.className = "confirm-body";
    body.textContent = message;
    const foot = document.createElement("div");
    foot.className = "modal-foot";
    const cancel = document.createElement("button");
    cancel.className = "btn";
    cancel.type = "button";
    cancel.textContent = "Cancel";
    const go = document.createElement("button");
    go.className = danger ? "btn danger" : "btn accent";
    go.type = "button";
    go.textContent = confirmLabel;
    if (notice) foot.append(go);
    else foot.append(cancel, go);
    modal.append(head, body, foot);
    scrim.append(modal);
    document.body.append(scrim);
    // CANCEL takes the keyboard: the dialog exists because a click went astray,
    // and a destructive button under a stray Enter would repeat the slip. A
    // notice has nothing to protect: OK takes the focus.
    if (notice) go.focus();
    else cancel.focus();

    return new Promise<boolean>((resolve) => {
      let settled = false;
      const done = (ok: boolean) => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        scrim.remove();
        resolve(ok);
      };
      const onKey = (e: KeyboardEvent) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          done(false);
        } else if (e.key === "Enter" && document.activeElement === go) {
          e.stopPropagation();
          done(true);
        }
      };
      // CAPTURE: a console's terminal swallows keystrokes, and Escape is one it
      // forwards to the child — the dialog must hear it first.
      document.addEventListener("keydown", onKey, true);
      cancel.addEventListener("click", () => done(false));
      go.addEventListener("click", () => done(true));
    });
  }

  // A message with an OK and nothing else (a verb's refusal, verbatim).
  function askNotice({ title, message, danger = true }: { title: string; message: string; danger?: boolean }) {
    return askConfirm({ title, message, confirmLabel: "OK", danger, notice: true });
  }

  // A transient line with ONE optional verb — the undo a close needs (ADR-0064
  // §11). Not a dialog: it asks nothing, takes no focus and never blocks the
  // stage, because the act it reports already happened and the file it reports
  // on is still there. One at a time: a second replaces the first, so a burst
  // of closes cannot stack a column over the plane.
  //
  // DOM-built like `askConfirm` and appended to `document.body`, so it works in
  // the detached-fence popup too, which has no shell around it.
  let toastEl: HTMLElement | null = null;
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  const TOAST_MS = 6000;
  function toast({ text, action, onAction, ms = TOAST_MS }: { text: string; action?: string; onAction?: () => void; ms?: number }) {
    dismissToast();
    const el = document.createElement("div");
    el.className = "wb-toast";
    el.setAttribute("role", "status");
    const line = document.createElement("span");
    line.className = "wb-toast-text";
    line.textContent = text;
    el.append(line);
    if (action && onAction) {
      const btn = document.createElement("button");
      btn.className = "wb-toast-action";
      btn.type = "button";
      btn.textContent = action;
      btn.addEventListener("click", () => {
        dismissToast();
        onAction();
      });
      el.append(btn);
    }
    const close = document.createElement("button");
    close.className = "wb-toast-close";
    close.type = "button";
    close.title = "Dismiss";
    close.textContent = "×";
    close.addEventListener("click", dismissToast);
    el.append(close);
    document.body.append(el);
    toastEl = el;
    toastTimer = setTimeout(dismissToast, ms);
    return el;
  }
  function dismissToast() {
    if (toastTimer != null) clearTimeout(toastTimer);
    toastTimer = null;
    toastEl?.remove();
    toastEl = null;
  }

  // The tooltip of an agent-state dot: the Go-to menu, the checkout switcher
  // and the title bar all say it the same way. `waiting` carries the question
  // as its detail; `unknown` is a stale observation (agent_state.rs).
  function agentStateTitle(state: string | null | undefined, detail?: string) {
    if (!state) return "";
    if (state === "unknown") return "Agent state unknown";
    return detail ? `Agent is ${state}: ${detail}` : `Agent is ${state}`;
  }

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
  const sync = WBDeskSync.createSync();
  let desk: DeskRecord[] = [];
  // Second record type (#340): named rectangles on the floor tier.
  let fences: DeskFence[] = [];
  // Third record type (ADR-0064 §2): note cards, PLACEMENT only — the note's
  // text and colour live in its `.note` file. The CARD itself (DOM, editor,
  // autosave) is `wb-notes.ts`, which reaches this state through the exports
  // below.
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

  // Sending the desk changes and restoring the desk layout are
  // `wb-desk.ts`, which owns the desk flush state; this console keeps
  // the desk view, the desk read (`reloadDesk`) and the answer to an upload
  // (`flushed`), and hands them to it.
  const {
    emitDesk,
    scheduleDeskFlush,
    flushDeskOnClose,
    restoreDesk,
    restoreDetached,
    mountDetached,
    isDeskReconciled,
    isDeskSettled,
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
    fences: () => fences,
    peerGroups: () => peerGroups,
    deskLoaded: () => deskLoaded,
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
  });

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
    commitList("fence", fences, next);
  }
  function saveNotes(next: DeskNote[]) {
    commitList("note", notes, next);
  }

  // A record without a name gets one, in desk order (ADR-0066 §2), so two
  // pages that read one desk agree, and the name is stored as a change.
  function nameUnnamed() {
    const named = WBConsoleName.nameDesk(desk, consolePrefix);
    let emitted = false;
    named.forEach((r, i) => {
      if (desk[i].consoleName || !r.consoleName) return;
      sync.emit({ op: "set", type: "window", id: r.id, fields: { consoleName: r.consoleName } });
      emitted = true;
    });
    if (!emitted) return;
    refreshView();
    scheduleDeskFlush();
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

  // A desk this page takes never moves an element under a gesture.
  function inGesture(el: HTMLElement) {
    return gestures.active(el);
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

  function loadDesk() {
    return desk.slice();
  }
  // The desk as the column restore reads it (ADR-0051 §8): ids and `max` only.
  function deskRecords() {
    return loadDesk().map((r) => ({ id: r.id, max: !!r.max }));
  }

  // Whether a NEW console would be over the window cap — asked before it is
  // born, so the open is refused instead of cutting a record in silence
  // (ADR-0050 amendment 2026-10-04).
  function atDeskCap() {
    return desk.length >= DESK_MAX;
  }
  // Whether another card would be over the cap — asked before a card is born,
  // so the open is refused instead of quietly evicting one that is on screen.
  function atNoteCap() {
    return notes.length >= NOTE_MAX;
  }
  // The card records, as a copy: `wb-notes.ts` reads them and hands a NEW
  // array back to `saveNotes`, never mutates this one.
  function loadNotes() {
    return notes.slice();
  }
  // The selected checkout for one repo ref, or `null` — the primary tree.
  function checkoutOf(ref: string) {
    return checkouts[ref] || null;
  }
  // Select (`name`) or clear (`null`) a project's checkout. A clear names the
  // tree it clears, so it never erases a tree another device picked since.
  function setCheckout(ref: string, name: string | null | undefined) {
    if (name) emitDesk({ op: "checkout", repo: ref, name: String(name) });
    else if (checkouts[ref]) emitDesk({ op: "checkout-clear", repo: ref, ifName: checkouts[ref] });
  }
  function allCheckouts() {
    return { ...checkouts };
  }
  // Resolves once the boot desk load has settled (landed OR refused) — what
  // `app.ts` awaits before copying the view into its reactive map.
  function whenDeskLoaded() {
    return deskReady;
  }

  // The answer to one upload (`flushDesk`, `wb-desk.ts`): a batch
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

  // Columns, a maximize and the physical screen each fill the whole viewport,
  // so every other console is under them.
  function fillsViewport(win: ConsoleWin) {
    return win.classList.contains("maximized") || win.classList.contains("column") || isFull(win);
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
  // A window's RESTORE box, from the inline styles. Three states make a live
  // read lie: `.maximized` pins all four offsets via `!important` (a read would
  // persist 0,0); fullscreen sizes the element to the DISPLAY at 0,0 (a read
  // would grow the stage to screen size and persist a monitor-sized box); a
  // `display:none` ancestor — the Consoles tab is Alpine `x-show` and
  // `restoreDesk` runs whatever tab is showing — measures 0×0 at 0,0 (measured
  // 2026-09-09: a reload from a file tab wrote `0,0,0,0`, the next load rendered
  // the CSS floor 240×150 at the origin and stored THAT). The inline rect is
  // untouched by all three; it is what `buildChrome` wrote from the record.
  function measurable(win: HTMLElement) {
    return !!(win.offsetWidth || win.offsetHeight);
  }
  function restoreRect(win: HTMLElement) {
    const inline = (prop: "left" | "top" | "width" | "height", fallback: number) => parseInt(win.style[prop], 10) || fallback;
    const fromInline = () => ({
      left: inline("left", win.offsetLeft),
      top: inline("top", win.offsetTop),
      width: inline("width", win.offsetWidth),
      height: inline("height", win.offsetHeight),
    });
    if (!measurable(win)) return fromInline();
    // A column's painted box is view state (ADR-0051 §5): like a maximize, the
    // inline rect is the desk rect underneath it. So is a card on top's
    // (ADR-0064, 2026-09-26 amendment).
    if (win.classList.contains("on-top")) return fromInline();
    if (!win.classList.contains("maximized") && !win.classList.contains("column") && !isFull(win)) {
      return {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
    }
    return fromInline();
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

  // Toggle a console between its floating rect and a full-VIEWPORT bleed. The
  // pre-maximize rect stays in the inline styles, so restoring drops the class.
  //
  // On a scrollable stage the bleed is pinned to what the operator is looking
  // at: `--max-left`/`--max-top` carry the viewport's scroll offsets, re-derived
  // by `syncMaxPin`. Re-asserted after the class flip because `maxlock`
  // (`overflow:hidden`) drops the scrollbars, which can clamp the offsets.
  //
  // `persist` writes `max`: the operator's own toggle, and the columns
  // (`wbColumns`) moving the maximize to another console (ADR-0051 §5). A restore
  // (`buildChrome`) and the torn-off fence window's columns write nothing: a
  // restore already reads the record, and that window's grid is never stored.
  function setMax(win: ConsoleWin, on: boolean, persist = false) {
    if (win.classList.contains("maximized") === on) return;
    const ws = workspace();
    const offsets = ws ? { left: ws.scrollLeft, top: ws.scrollTop } : null;
    const maxed = win.classList.toggle("maximized", on);
    if (!maxed && !win.classList.contains("column")) {
      win.style.removeProperty("--max-left");
      win.style.removeProperty("--max-top");
    }
    syncMaxLock();
    if (ws && offsets) {
      ws.scrollLeft = offsets.left;
      ws.scrollTop = offsets.top;
    }
    // AFTER the restore: the pin must come from the offsets that SURVIVED the
    // `maxlock` flip, not the pair read before it.
    syncMaxPin();
    paintMaxButton(win);
    focusWin(win);
    try {
      win._term?.fit.fit();
    } catch {}
    applyExtent();
    if (persist) setWin(win, { max: on });
  }

  function toggleMax(win: ConsoleWin) {
    setMax(win, !win.classList.contains("maximized"), true);
  }

  // A column restores like a maximize, so it shows the same control.
  function paintMaxButton(win: ConsoleWin) {
    const btn = win._maxBtn;
    if (!btn) return;
    const on = win.classList.contains("maximized") || win.classList.contains("column");
    btn.title = on ? "Restore" : "Maximize";
    btn.innerHTML = on
      ? '<i class="bi bi-fullscreen-exit"></i>'
      : '<i class="bi bi-fullscreen"></i>';
  }

  // ---- columns (ADR-0051 §5) --------------------------------------------------
  // `wbColumns` (wb-consoles-tab.ts) owns the column list and folds it with
  // `WBColumns`; this module only paints the answer and never reads
  // `WBColumns`: the detached-fence popup boots this file without it.
  //
  // A column writes no rect to the desk: the painted box is CSS, and
  // `restoreRect` reads the inline rect under it. The first console is the one
  // the desk records as maximized, so a move of the maximize is written when
  // the caller passes `persist` (ADR-0051 §5). A reload that keeps the stored
  // grid writes the desk to match it, so a console another device maximized
  // inside the grid is written back as not maximized. Another page does not
  // apply that `max` while it is open (ADR-0050 amendment 2026-10-04).

  // The width of the viewport the columns share, in px.
  function columnMeasure() {
    return { viewport: workspace()?.clientWidth || 0 };
  }

  function clearColumn(win: ConsoleWin) {
    win.classList.remove("column");
    win.style.removeProperty("--col-index");
    win.style.removeProperty("--col-count");
    win.style.removeProperty("--row-index");
    win.style.removeProperty("--row-count");
    if (!win.classList.contains("maximized")) {
      win.style.removeProperty("--max-left");
      win.style.removeProperty("--max-top");
    }
    paintMaxButton(win);
    try {
      win._term?.fit.fit();
    } catch {}
  }

  // Paint `painted` (`WBColumns.painted`). `unmax` is the old first console
  // after a restore: it stops being the maximized console. `persist` writes
  // each change of the maximize to the desk.
  function applyColumns(painted: Painted[] | null | undefined, opts?: { cap?: number; unmax?: string | null; raise?: boolean; persist?: boolean }) {
    const list = painted || [];
    const cap = opts?.cap ?? 1;
    const persist = !!opts?.persist;
    for (const win of wins) {
      if (win.classList.contains("column") && !columnClasses(list, win._deskId).column) {
        clearColumn(win);
      }
    }
    const gone = opts?.unmax ? findWindow(opts.unmax) : null;
    if (gone && !columnClasses(list, gone._deskId).column) setMax(gone, false, persist);
    const shown = [];
    for (const p of list) {
      const win = findWindow(p.id);
      if (!win) continue;
      const c = columnClasses(list, p.id);
      if (c.column) {
        win.classList.add("column");
        win.style.setProperty("--col-index", String(p.index));
        win.style.setProperty("--col-count", String(p.count));
        win.style.setProperty("--row-index", String(p.row ?? 0));
        win.style.setProperty("--row-count", String(p.rows ?? 1));
        shown.push(win);
      } else if (c.maximized) {
        // The last column left is a plain maximize, and a full bleed must be
        // on top: the console just restored was raised later than it.
        shown.push(win);
      }
      // The class is set FIRST: `restoreRect` must already read a column's
      // inline rect.
      if (c.maximized && !win.classList.contains("maximized")) setMax(win, true, persist);
      else if (!c.maximized && win.classList.contains("maximized")) setMax(win, false, persist);
      paintMaxButton(win);
    }
    syncMaxLock();
    syncMaxPin();
    // Raised in reading order only on an open or a restore: a repaint on every
    // `consoles-changed` would bury a console just spawned, and move the focus
    // mark off the column the operator is typing in.
    for (const win of shown) {
      if (opts?.raise) focusWin(win);
      try {
        win._term?.fit.fit();
      } catch {}
    }
    // Never disabled: at the cap the list still swaps (ADR-0051 §5).
    for (const win of wins) {
      const btn = win._colBtn;
      if (!btn) continue;
      const held = win.classList.contains("maximized") || win.classList.contains("column");
      btn.hidden = !(OPTS.autoBoot !== false && held && cap >= 2);
    }
    paintFenceColumns();
  }

  // A console whose record another client removed: off this stage. Its
  // session is not touched here, and its record is already gone.
  function dropClosedElsewhere(id: string) {
    const win = findWindow(id);
    if (!win) return;
    tearDownMember(win, "window-closed");
    applyExtent();
  }

  function focusedId() {
    return stage()?.querySelector<ConsoleWin>(".session-window.focused")?._deskId ?? null;
  }

  function focusColumn(id: string) {
    const win = findWindow(id);
    if (!win) return;
    focusWin(win);
    win._term?.term.focus();
  }

  // What the "Open in a column" list is folded from. A detached fence's
  // members are not on this stage; its popup told us who they are.
  function columnRoster() {
    const st = stage();
    if (!st) return { rows: [], fences: [], membership: {}, detached: {} };
    const out: Record<string, DetachedMember[]> = {};
    for (const [id, entry] of popups.entries()) {
      out[id] = (entry.members || [])
        .filter((m) => m && m.id && m.kind !== "note")
        .map((m) => ({
          id: m.id,
          agent: m.agent ?? null,
          name: desk.find((r) => r.id === m.id)?.consoleName ?? m.consoleName ?? null,
          repo: m.repo === "~" ? null : (m.repo ?? null),
          kind: m.kind ?? null,
        }));
    }
    return {
      rows: list(),
      fences: fenceList().map(({ id, name }) => ({ id, name })),
      membership: fenceMembership(readFenceRects(st), readWindowRects(st)),
      detached: out,
    };
  }

  // Raise ONE console to the physical screen, or drop it back. A different axis
  // from `toggleMax`: maximize fills the workspace VIEWPORT, fullscreen fills the
  // DISPLAY. They compose — entering fullscreen never touches `.maximized`.
  //
  // Nothing here writes geometry: the top layer's UA `!important` sizing
  // outranks every author rule, the `.maximized` pin included, and the
  // per-window ResizeObserver refits the terminal.
  // The live browser fact, not our class: the guards must hold in the instant
  // between the state change and the event that mirrors it.
  function isFull(win: HTMLElement) {
    return document.fullscreenElement === win;
  }

  // ---- locked in place (ADR-0050 / ADR-0051 lock amendment) -----------------
  // A lock is a property of the DESK, honoured on every device: the gesture
  // handlers consult it and refuse; nothing else changes (maximize, fullscreen
  // and close do not rewrite the rect). A console is locked by its own record
  // OR by the fence holding its centre — the same `fenceOf` fold as membership.
  function isLocked(win: ConsoleWin) {
    if (win._deskLocked) return true;
    return heldByFence(win);
  }
  // The one place a window's own lock is set: flag, class, glyph.
  function applyLock(win: ConsoleWin, locked: boolean) {
    win._deskLocked = !!locked;
    win.classList.toggle("locked", !!locked);
    paintLockGlyph(win);
  }
  // The glyph shows the EFFECTIVE lock: a console held by a locked fence reads
  // closed like its fence. Held-only, the button is disabled — its own toggle
  // would change nothing the operator can see; the fence's lock is the one to
  // open.
  function paintLockGlyph(win: ConsoleWin) {
    const btn = win.querySelector<HTMLButtonElement>(".session-lock");
    if (!btn) return;
    const own = !!win._deskLocked;
    const held = !own && heldByFence(win);
    const locked = own || held;
    btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
    btn.title = held ? "Locked by its fence. Unlock the fence first." : own ? "Unlock" : "Lock in place";
    btn.setAttribute("aria-pressed", locked ? "true" : "false");
    btn.disabled = held;
  }
  function toggleLock(win: ConsoleWin) {
    applyLock(win, !win._deskLocked);
    setWin(win, { locked: !!win._deskLocked });
  }

  function toggleFull(win: ConsoleWin) {
    if (document.fullscreenElement === win) {
      // The promise rejects if we are already leaving; there is nothing to
      // recover, and `syncFullState` runs off the event either way.
      document.exitFullscreen().catch(() => {});
      return;
    }
    focusWin(win);
    // Requesting while ANOTHER element is fullscreen is a legal swap — browsers
    // move the top layer without a round trip through the exit.
    win.requestFullscreen().catch((err) => {
      // The browser refusing (no user gesture, a policy header). Say so; the
      // console is still usable maximized.
      console.warn("fullscreen refused", err);
    });
  }

  // The fullscreen control's LOOK, derived from `document.fullscreenElement`,
  // never written by the click handler: the operator leaves fullscreen by paths
  // this code never sees (Esc, the iPad swipe, a tab switch, a permission
  // prompt), and on a tablet a stale "exit" button is the only exit there is.
  function syncFullState() {
    const el = document.fullscreenElement;
    for (const btn of document.querySelectorAll<HTMLElement>(".session-full")) {
      const win = btn.closest(".session-window");
      const on = !!win && el === win;
      btn.title = on ? "Exit full screen" : "Full screen";
      btn.innerHTML = on
        ? '<i class="bi bi-fullscreen-exit"></i>'
        : '<i class="bi bi-arrows-fullscreen"></i>';
      // The touch-target grow lives on the WINDOW, not the button: the whole
      // titlebar is the operator's exit affordance on a tablet.
      win?.classList.toggle("fullscreen", on);
    }
    refreshCover();
  }

  // A maximized console is a FULL BLEED: anything stacked on top of it is a
  // window the operator cannot see the rest of. A restore spawns windows in
  // record order and each raises itself, so a maximized record restored early
  // ended up underneath the rest. Fixed at the END of the restore rather than
  // by a fixed z-index on `.maximized`, which would have to out-rank the focus
  // ladder and then nothing could be raised over it on purpose.
  function raiseMaximized() {
    const st = stage();
    if (!st) return;
    // Columns are a maximize too (ADR-0051 §5): all of them, in reading order.
    const at = (w: HTMLElement, v: string) => parseInt(w.style.getPropertyValue(v), 10) || 0;
    const cols = [...st.querySelectorAll<HTMLElement>(".session-window.column")].sort(
      (a, b) => at(a, "--col-index") - at(b, "--col-index") || at(a, "--row-index") - at(b, "--row-index"),
    );
    if (cols.length) {
      for (const w of cols) focusWin(w);
      return;
    }
    // The LAST one, if a desk somehow carries two: it is the one whose record
    // was written most recently, and exactly one window can usefully be on top.
    const all = st.querySelectorAll<HTMLElement>(".session-window.maximized");
    const win = all[all.length - 1];
    if (win) focusWin(win);
  }

  // The scroll freeze that keeps a maximized window's viewport pin honest.
  // DERIVED from the DOM at every layout mutation, never toggled by hand:
  // closing a maximized console never passes through `toggleMax`, and a
  // hand-held lock stranded `overflow:hidden` on the viewport for the rest of
  // the page's life (the unreachable-window state of ADR-0051 §4).
  function syncMaxLock() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    const maxed = !!st.querySelector(".session-window.maximized");
    ws.classList.toggle("maxlock", maxed);
    // The body-level mirror, for the phone bleed: the rail and the sidebar are
    // `#workspace`'s cousins, unreachable from `maxlock`. The width gate is CSS's.
    document.body?.classList.toggle("console-max", maxed);
    refreshCover();
  }

  // The maximize pin, DERIVED the same way: `--max-left`/`--max-top` are only
  // honest while they equal the viewport's CURRENT offsets.
  // INVARIANT: every path that changes `#workspace`'s scroll offsets ends here.
  // The viewport's `scroll` event (`wireStage`) covers gesture, wheel, scrollbar
  // AND `reveal`'s programmatic write; `toggleMax` calls it after the flip. Only
  // ever writes to `.maximized` and `.column` windows — un-maximize REMOVES both
  // properties.
  function syncMaxPin() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    for (const win of st.querySelectorAll<HTMLElement>(".session-window.maximized, .session-window.column")) {
      win.style.setProperty("--max-left", ws.scrollLeft + "px");
      win.style.setProperty("--max-top", ws.scrollTop + "px");
    }
  }

  // The last extent published to the shell, so the dispatch below can be an
  // edge and not a level.
  let lastExtent = { width: 0, height: 0 };

  // Size the stage to hold every window. NOTHING here moves or resizes a
  // window: the plane grows under them and a small viewport scrolls (#336
  // deleted the clamp-and-refit). `grow` floors each axis at the current pixels
  // — shrinking mid-drag would clamp `scrollLeft` under the cursor; the exact
  // recompute runs on mouseup.
  // INVARIANT: every path that creates, moves, resizes, closes or restores a
  // window ends here.
  function applyExtent(opts?: ExtentOpts) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // The one place the freeze is kept in step with what is on screen.
    syncMaxLock();
    // Read the DOM, not `wins`: a window is on the stage from `buildChrome`'s
    // append (before `spawnWindow` registers it) to its removal. Fences count
    // too — ADR-0051 §2 sizes the plane to windows AND fences.
    const rects = [...st.querySelectorAll<HTMLElement>(".session-window, .fence, .note-card")].map(restoreRect);
    const ext = stageExtent(
      rects,
      { width: ws.clientWidth, height: ws.clientHeight },
      STAGE_MARGIN,
    );
    const width = opts?.grow ? Math.max(ext.width, st.offsetWidth) : ext.width;
    const height = opts?.grow ? Math.max(ext.height, st.offsetHeight) : ext.height;
    st.style.width = width + "px";
    st.style.height = height + "px";
    // Publish the extent to the footer pill (#338), ONLY on a real change: a
    // drag folds the extent per mousemove and would re-render Alpine per frame.
    if (width !== lastExtent.width || height !== lastExtent.height) {
      lastExtent = { width, height };
      sendDocument(document, "workbench:stage-extent", { width, height });
    }
  }

  // Give a surface a place in the window tier WITHOUT focusing it. A restore
  // builds its consoles through `buildChrome`, which ends in `focusWin` and so
  // hands every window a z; a note card is built by `WBNotes.render` and had
  // none, which put it at `auto` — BELOW every console (z ≥ 61). MEASURED: a
  // card restored beside a console was visible where nothing overlapped and
  // deaf where something did, because the click landed on the terminal's
  // canvas and the keystrokes went to the shell. A surface on the plane is in
  // the tier or it is under it; there is no third state.
  function stackWin(win: Stacked) {
    if (win.style.zIndex) return;
    // At the ceiling the counter stops and the newcomers tie: a tie among
    // cards is a stacking order, while a number past the ceiling would put a
    // card over the tab bar. The next `focusWin` renormalises the lot.
    if (z < Z_CEIL) z += 1;
    win.style.zIndex = z;
  }

  function focusWin(win: Stacked) {
    z += 1;
    if (z > Z_CEIL) {
      // Renormalize: re-stack the existing windows by their current z, resetting
      // the counter so focus never pushes a console over the overlay/tabbar tier.
      const ordered = [...workspace()!.querySelectorAll<Stacked>(".session-window, .note-card")].sort(
        (a, b) => (parseInt(a.style.zIndex, 10) || 0) - (parseInt(b.style.zIndex, 10) || 0),
      );
      z = Z_BASE;
      for (const w of ordered) {
        if (w === win) continue;
        z += 1;
        w.style.zIndex = z;
      }
      z += 1;
    }
    win.style.zIndex = z;
    for (const w of workspace()!.querySelectorAll<ConsoleWin>(".session-window.focused, .note-card.focused")) {
      if (w === win) continue;
      w.classList.remove("focused");
      // Focus held it awake (`dormancyDecision` D2); the observer will not
      // report a window that did not move.
      if (wins.has(w)) applyDormancy(w);
    }
    win.classList.add("focused");
    // On top now, so first in line for a context. The `applyDormancy` above
    // asks too, but not when the focus came from a note card.
    budget.scheduleGpu();
  }

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
    const r = desk.find((x) => x.id === win._deskId);
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
      if (desk.some((r) => r.id === win._deskId)) return;
      recordLeft(win);
    });
  }

  // The session the daemon announced, and the worktree with it, written only
  // when they differ from the record: a reconnect changes nothing else.
  function recordSession(win: ConsoleWin) {
    const r = desk.find((x) => x.id === win._deskId);
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
    lastExtent = { width: -1, height: -1 };
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
      if (deskLoaded && wins.size === 0 && popups.size() === 0) {
        restoreDesk();
        return;
      }
      // Windows already up: nothing else would put the just-loaded fences on
      // the stage. Gated on the permit: a REFUSED load leaves `fences` as
      // whatever this page drew.
      if (!deskLoaded) return;
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
