/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Consoles tab)

   Consoles are draggable, resizable windows on the STAGE, a plane the VIEWPORT
   (`#workspace`, `overflow:auto`) scrolls over. This module owns the window
   state (tiling, the stage's extent); the titlebar, the drag and the resize
   are `wb-console-chrome.ts`; the fences are `wb-console-fences.ts`, and the
   fences this tab detached are `wb-console-popups.ts`; the body is a live
   xterm.js on a PTY over the daemon's `/ws/session` WebSocket, made by
   `wb-console-terminal.ts`.

   Opening/closing a console spawns/closes a daemon-owned session; on load the
   live sessions re-open as windows, so a reload reattaches with scrollback.

   Importing this module does nothing. Each entry module calls `createConsole`
   once per page, sets `window.WBConsole`, and calls `boot` after the page has
   started; each call starts with new state (ADR-0075 D7).
--------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { createGpuBudget, gpuHolders, DORMANT_AFTER_MS, DORMANT_MARGIN_PX } from "./wb-console-gpu.ts";
import { createTerminal } from "./wb-console-terminal.ts";
import { createChrome, createGestures } from "./wb-console-chrome.ts";
import { createPopupRegistry } from "./wb-console-popups.ts";
import { createFences } from "./wb-console-fences.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBDeskSync } from "./wb-desk-sync.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import { WBView } from "./wb-view.ts";
import { WBFail } from "./wb-fail.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import { WBSessionRoute } from "./wb-session-route.ts";

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

// The session folds are `wb-console-session.ts`: the wire codec, the reconnect,
// resume and dormancy rules, and the peer rules, pure functions of their arguments.
const {
  encodeResize,
  encodeDetach,
  resumeDecision,
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
  RESUME_DEBOUNCE_MS,
  CONNECT_TIMEOUT_MS,
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

export function createConsole(window: any, document: any, location: any, opts: any) {
  // Plane geometry is `wb-geometry.ts` (ADR-0057): pure folds over rects.
  const {
    STAGE_MARGIN,
    stageExtent,
    fenceSpawnRect,
    rectsOverlap,
    rectHolds,
    fenceMembership,
    fenceOf,
    fenceFits,
    fenceMoveDelta,
    tileIntoRect,
    resizeRect,
    bringIntoView,
    anchorIntoView,
    slideEase,
    viewLanding,
    panNudge,
    spawnRectIn,
    freeSpawnRect,
    fenceHolds,
    PAN_BAND,
    PAN_STEP,
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
  const wins = new Set<any>();

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
  // `wb-console-chrome.ts`; this console hands it the window state.
  const gestures = createGestures();
  const { buildChrome, makeDraggable, startResize } = createChrome({
    window,
    document,
    OPTS,
    gestures,
    stage,
    workspace,
    fences: () => fences,
    focusedFence: () => focusedFence,
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

  // ---- the popup registry and the fences -------------------------------------
  // The fences this tab detached and their popup entries have one owner,
  // `createPopupRegistry`; the fences, the desk restore and the lifecycle
  // channel below read and change them only through it. The fence element,
  // its gestures, tiling and detaching are `wb-console-fences.ts`.
  const popups = createPopupRegistry({ link, startBeat, stopBeat });
  const { isDetached, commitDetached, newPopupEntry, detachedMembers } = popups;
  const { buildFence, arrangeFence, detachFence } = createFences({
    window,
    document,
    gestures,
    popups,
    link,
    wins,
    fences: () => fences,
    applyExtent,
    askConfirm,
    autoPan,
    clearFenceFlash,
    fenceEl,
    fenceLocked,
    fenceNotice,
    fenceSnapshot,
    focusWin,
    glyphClick,
    newPid,
    readFenceRects,
    readWindowRects,
    reattachFence,
    refreshFenceChrome,
    removeFence,
    renameFence,
    renderFences,
    renderNotes,
    restoreRect,
    saveFences,
    setFenceLock,
    setWin,
    showDetachGlyph,
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
    document.dispatchEvent(new CustomEvent("workbench:consoles-changed", { detail: { count: wins.size } }));
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
  function askConfirm({ title, message, confirmLabel = "Confirm", danger = false, notice = false }: any) {
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

    return new Promise((resolve) => {
      let settled = false;
      const done = (ok: any) => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        scrim.remove();
        resolve(ok);
      };
      const onKey = (e: any) => {
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
  function askNotice({ title, message, danger = true }: any) {
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
  let toastEl: any = null;
  let toastTimer: any = null;
  const TOAST_MS = 6000;
  function toast({ text, action, onAction, ms = TOAST_MS }: any) {
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
  function agentStateTitle(state: any, detail?: any) {
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
  let desk: any = [];
  // Second record type (#340): named rectangles on the floor tier.
  let fences: any = [];
  // Third record type (ADR-0064 §2): note cards, PLACEMENT only — the note's
  // text and colour live in its `.note` file. The CARD itself (DOM, editor,
  // autosave) is `wb-notes.ts`, which reaches this state through the exports
  // below.
  let notes: any = [];
  // Fourth record type (#406, ADR-0063 §4): the selected checkout per repo ref,
  // `{ <ref>: <worktree name> }`. The reactive copy the chip and the tree
  // render lives in `app.ts` (a closure variable here is invisible to Alpine).
  let checkouts: any = {};
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
  function emitDesk(change: any) {
    sync.emit(change);
    refreshView();
    scheduleDeskFlush();
  }

  // A window's record as a `create` carries it. A maximized window stores its
  // *pre-maximize* rect (the class drives the full-bleed via CSS), so `max`
  // restores the full-screen state while the stored rect still restores the
  // underlying box.
  function recordOf(win: any) {
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
  function createRecord(win: any) {
    win._deskUnrecorded = false;
    emitDesk({ op: "create", type: "window", record: recordOf(win) });
  }
  // The fields one act changed on one window. A window adopted from a session
  // this page found with no record (`_deskUnrecorded`) gets its record with the
  // operator's first act on it, in the same batch: until then the cascade
  // place it was given is not a place anybody chose, and must not be written
  // over the record another page may be writing.
  function setWin(win: any, fields: any) {
    // A window taken off the page (a late pointerup after a close) must not
    // write.
    if (!win?._deskId || !win.isConnected) return;
    if (win._deskUnrecorded) createRecord(win);
    emitDesk({ op: "set", type: "window", id: win._deskId, fields });
    // A moved window may have joined or left a region; membership is derived.
    refreshFenceChrome();
  }
  function forgetRecord(deskId: any) {
    if (!deskId) return;
    emitDesk({ op: "remove", type: "window", id: deskId });
    refreshFenceChrome();
  }

  // The fields a `set` may carry per type, read off a record, so a list of
  // records handed back (`saveFences`, `saveNotes`) becomes the changes that
  // tell it from the view: a create, a remove, or a set of the fields that
  // differ, and nothing for a record left as it was.
  const rectOnly = (r: any) => (r ? { left: r.left, top: r.top, width: r.width, height: r.height } : null);
  const SET_FIELDS: any = {
    fence: (r: any) => ({ rect: rectOnly(r.rect), name: r.name ?? "", locked: !!r.locked }),
    note: (r: any) => ({
      rect: rectOnly(r.rect),
      locked: !!r.locked,
      file: { repo: r.repo, path: r.path ?? "", checkout: r.checkout ?? null },
    }),
  };
  function commitList(type: any, before: any, next: any) {
    const was = new Map(before.map((r: any) => [r.id, r]));
    const kept = new Set(next.map((r: any) => r.id));
    const changes = before.filter((r: any) => !kept.has(r.id)).map((r: any) => ({ op: "remove", type, id: r.id }));
    for (const r of next) {
      const old = was.get(r.id);
      if (!old) {
        changes.push({ op: "create", type, record: r });
        continue;
      }
      const a = SET_FIELDS[type](old);
      const b = SET_FIELDS[type](r);
      const fields: any = {};
      for (const k of Object.keys(b)) if (JSON.stringify(a[k]) !== JSON.stringify(b[k])) fields[k] = b[k];
      if (Object.keys(fields).length) changes.push({ op: "set", type, id: r.id, fields });
    }
    if (!changes.length) return;
    for (const c of changes) sync.emit(c);
    refreshView();
    scheduleDeskFlush();
  }
  // The cap REFUSES a new fence or card before it is born (`atFenceCap`,
  // `atNoteCap`); nothing here drops a record to make room.
  function saveFences(next: any) {
    commitList("fence", fences, next);
  }
  function saveNotes(next: any) {
    commitList("note", notes, next);
  }

  // A record without a name gets one, in desk order (ADR-0066 §2), so two
  // pages that read one desk agree, and the name is stored as a change.
  function nameUnnamed() {
    const named = WBConsoleName.nameDesk(desk, consolePrefix);
    let emitted = false;
    named.forEach((r: any, i: any) => {
      if (desk[i].consoleName || !r.consoleName) return;
      sync.emit({ op: "set", type: "window", id: r.id, fields: { consoleName: r.consoleName } });
      emitted = true;
    });
    if (!emitted) return;
    refreshView();
    scheduleDeskFlush();
  }

  function ingestDesk(payload: any) {
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
  function inGesture(el: any) {
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
    if (OPTS.autoBoot === false || !deskReconciled) return;
    const st = stage();
    if (!st) return;
    const byId = new Map<any, any>(desk.map((r: any) => [r.id, r]));
    for (const w of [...st.querySelectorAll(".session-window")]) {
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

  function placeWindow(win: any, r: any) {
    const at = (prop: any) => parseInt(win.style[prop], 10);
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
  function keepUnsavedCards(st: any) {
    const listed = new Set(notes.map((n: any) => n.id));
    for (const el of st.querySelectorAll(".note-card")) {
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
  function recordLeft(win: any) {
    if (recordChecks.has(win)) return;
    if (win.classList.contains("placeholder") || win.classList.contains("ended") || sessionIdOf(win) == null) {
      leaveDesk([win._deskId]);
      return;
    }
    recordChecks.add(win);
    readSessions()
      .catch(() => null)
      .then((read: any) => {
        recordChecks.delete(win);
        if (!win.isConnected || desk.some((r: any) => r.id === win._deskId)) return;
        const sessions = read?.sessions;
        // Not known: the next desk this page takes asks again. A list that
        // did not hear from this window's peer does not know either.
        if (!Array.isArray(sessions) || unheardRef(win._deskRepo, read.unheard)) return;
        const live = sessions.some((s) => s?.record === win._deskId) || !!sessionRowFor(win, sessions);
        if (live) createRecord(win);
        else leaveDesk([win._deskId]);
      });
  }

  // Windows whose records left the desk. The shell's hook takes them out of
  // the columns first (a lone survivor is maximized before the drops) and
  // calls `dropClosedElsewhere`; without a hook they are dropped here.
  let onDeskGone: any = null;
  function setDeskGoneHook(fn: any) {
    onDeskGone = fn;
  }
  function leaveDesk(ids: any) {
    if (typeof onDeskGone === "function") onDeskGone(ids);
    else for (const id of ids) dropClosedElsewhere(id);
  }

  // Why the daemon cannot read the saved desk, or "" (ADR-0070 D4). Set only
  // by the daemon's own `409 {"state":"unreadable"}`: a transport failure or
  // a pre-login 401 is not a broken desk, and must not offer a new one.
  let deskFailure = "";
  // The shell's hook for a desk failure found by a flush: no push says so.
  let onDeskFailure: any = null;
  // The `409` reply of an unreadable desk, as the reason to show, or null.
  // The daemon's own text is a parser message for a developer: it goes to
  // the browser console, and the operator reads what it means.
  async function unreadableDesk(r: any) {
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
    return fetch("/api/desk")
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
  function setDeskFailureHook(fn: any) {
    onDeskFailure = fn;
  }
  // The one action on an unreadable desk: the daemon renames the old file
  // aside and starts an empty desk; this page then reads and restores it.
  function startNewDesk() {
    return fetch("/api/desk/new", { method: "POST" })
      .then(async (r) => {
        if (r.ok) return;
        // 409 "readable": another tab started the new desk first. The desk is
        // readable, so this tab reads it like any other.
        const body = r.status === 409 ? await r.json().catch(() => null) : null;
        if (body?.state === "readable") return;
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
    return loadDesk().map((r: any) => ({ id: r.id, max: !!r.max }));
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
  function checkoutOf(ref: any) {
    return checkouts[ref] || null;
  }
  // Select (`name`) or clear (`null`) a project's checkout. A clear names the
  // tree it clears, so it never erases a tree another device picked since.
  function setCheckout(ref: any, name: any) {
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

  // The upload, debounced. WHERE it goes is `deskSink`'s business
  // (wb-desk-sink.ts), which answers a typed result. One batch is on the wire
  // at a time; a batch that failed in a way the daemon may still accept is
  // sent again with the same `seq`, later.
  let deskFlush: any = null;
  let flushing = false;
  let flushBackoff = 1000;
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
      .then((out: any) => {
        flushing = false;
        flushed(out);
      });
  }
  function flushed(out: any) {
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
    if (offsetFlush) {
      clearTimeout(offsetFlush);
      offsetFlush = null;
      flushOffset();
    }
    // Every dirty note, too (ADR-0064 §7): the autosave debounce is 800 ms, so
    // without this the last sentence typed before a close is gone. A best
    // effort — the socket may not finish — for the same reason and with the
    // same bargain as the desk's own last flush below.
    window.WBNotes?.flushAll();
    // NOTHING closes a detached popup here: `pagehide` fires on a RELOAD exactly
    // as on a close, with no reliable discriminator (#347). The popup declares
    // its peer lost after `PEER_WINDOW_MS` without a beat and closes itself,
    // which covers a clean close and a force-kill alike (ADR-0051 §8).
    if (!deskLoaded || sync.phase() !== "ready") return;
    const body = sync.closingBatch();
    if (!body) return;
    clearTimeout(deskFlush);
    deskFlush = null;
    deskSink.putSync(JSON.stringify(body));
  });

  // Coming back from a suspend. Registered in EVERY document that runs this
  // module (shell and each popup): each owns the sockets of the windows it
  // paints, and nothing else would revive them.
  let hiddenAt = 0;

  // The verdict the probe gives, or — with no probe, which is the popup — how
  // long this document was hidden. `Infinity` for the network events: `online`
  // fires precisely because the link the sockets ran over is a different link now.
  function isStale(hiddenMs: any) {
    if (staleProbe) {
      try {
        return staleProbe() === true;
      } catch {
        return true;
      }
    }
    return hiddenMs > RESUME_HIDDEN_MS;
  }

  function resumeAll(stale: any) {
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
  function sleepWindow(win: any) {
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
  function wakeWindow(win: any) {
    if (!win._dormant) return false;
    const body = win.querySelector(".session-body");
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
  function applyDormancy(win: any) {
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
  function fillsViewport(win: any) {
    return win.classList.contains("maximized") || win.classList.contains("column") || isFull(win);
  }

  function isCovered(win: any) {
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
  function dormancyInputs(win: any) {
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
  document.addEventListener("workbench:action", (e: any) => {
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


  function newId(prefix: any) {
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
  function measurable(win: any) {
    return !!(win.offsetWidth || win.offsetHeight);
  }
  function restoreRect(win: any) {
    const inline = (prop: any, fallback: any) => parseInt(win.style[prop], 10) || fallback;
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
  let sessionsRead: any = null;
  function readSessions() {
    if (!sessionsRead) {
      sessionsRead = (async () => {
        const r = await fetch("/api/sessions");
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
  async function checkoutStillThere(repo: any, checkout: any) {
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
  const projectNames = new Map();
  function projectNameOf(ref: any) {
    const known = projectNames.get(ref);
    if (known) return known.name;
    return WBFleet ? WBFleet.refSlug(ref) : ref;
  }
  function projectTitleOf(ref: any) {
    if (ref === "~") return ref;
    return projectNames.get(ref)?.title || projectNameOf(ref);
  }
  // `rows` is `[{ ref, name, title }]`. Titles and tooltips already drawn are
  // drawn again; a console NAME already given is the operator's and stays.
  function ingestProjects(rows: any) {
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
  const peerGroups = new Map();
  let wakePeer: any = null;
  let readFleet: any = null;
  function ingestFleet(groups: any, hooks: any) {
    peerGroups.clear();
    for (const g of groups || []) if (g && g.daemon && !g.local) peerGroups.set(g.daemon, g);
    if (typeof hooks?.wake === "function") wakePeer = hooks.wake;
    if (typeof hooks?.read === "function") readFleet = hooks.read;
    for (const win of [...wins]) if (typeof win._peerRefresh === "function") win._peerRefresh();
  }

  // The console name's prefix is the PROJECT NAME's last segment: a peer ref's
  // routing head never shows, a remoteless repo is named by its folder and not
  // its `path-<hash>` key, and the same repo on two environments shares one
  // count (ADR-0066 §2).
  function consolePrefix(repo: any) {
    return WBConsoleName.prefixOf(projectNameOf(repo));
  }
  // Every console name in use: the desk mirror's and the stage's windows',
  // except `exceptId` (the console being renamed).
  function takenNames(exceptId: any) {
    const names = desk.filter((r: any) => r.id !== exceptId).map((r: any) => r.consoleName);
    const st = typeof document?.getElementById === "function" ? stage() : null;
    for (const w of st ? st.querySelectorAll(".session-window") : []) {
      if (w._deskId !== exceptId) names.push(w._deskConsoleName);
    }
    return names.filter(Boolean);
  }

  // The title is built by `renderTitle` from the window's console name and
  // label (ADR-0066 §4). The environment left the title for the tooltip
  // (ADR-0066 §5), after the full ref.
  function sessionPresentation(label: any, repo: any, prior: any, owner: any) {
    const daemonId = owner?.daemon_id ?? prior?.daemonId ?? null;
    const environment = owner?.environment ?? prior?.environment ?? null;
    // The vendor's own session name (`--name`; Claude only). On the TOOLTIP, not
    // the title: a fourth segment would outrun the titlebar. NO desk fallback:
    // the name dies with the child and the daemon re-announces it on every
    // reattach, so a restored window with no socket yet correctly shows none.
    const name = owner?.name ?? null;
    // The worktree the console lives in (ADR-0063 §3), right after the label.
    // From the `session-open` payload ONLY — no desk fallback, like `name` — so
    // changing the picker's selection can never retitle a live console.
    const checkout = owner?.checkout ?? null;
    return {
      daemonId,
      environment,
      name,
      checkout,
      tooltip: WBConsoleName.tooltipLines(projectTitleOf(repo), environment, name).join("\n"),
    };
  }

  // ---- the title's worktree segment as a switcher (#412) --------------------
  //
  // Listing per repo ref. Only a repo with at least one worktree gets a
  // switcher. Fed by the shell (`ingestWorktrees`, from every `worktree.list` it
  // reads for the picker) and, for a repo the picker never opened, by ONE read
  // of our own per ref at the first agent window — `worktree.list` is a git
  // spawn, so never per render and never periodic.
  const worktreeListings: any = {};
  const listingReads = new Map();
  function ingestWorktrees(ref: any, listing: any) {
    if (!ref) return;
    worktreeListings[ref] = listing || null;
    for (const win of wins) {
      if (win._deskRepo === ref && win._title && win._presentation) {
        renderTitle(win, win._title, win._presentation);
      }
    }
  }
  function ensureListing(ref: any, force = false) {
    if (!ref || ref === "~" || (ref in worktreeListings && !force) || listingReads.has(ref)) return;
    const daemon = window.WBDaemon;
    if (typeof daemon?.observe !== "function" || OPTS.canLaunch === false) return;
    const read = daemon
      .observe("worktree.list", { repo: ref })
      .then((reply: any) => {
        ingestWorktrees(ref, reply && reply.status === "ok" ? reply.checkouts || null : null);
      })
      .catch(() => ingestWorktrees(ref, null))
      .finally(() => listingReads.delete(ref));
    listingReads.set(ref, read);
  }
  // The switcher's rows: `primary` first, then the worktrees in listing order,
  // each with its dirty flag; the current one marked. With `sessions` (rows with
  // `checkout` and `agent_state`) each row also carries the agent's state in
  // that tree (ADR-0059 §5) via `WBProject.worktreeStates`. `primaryBranch`/
  // `primaryDirty` when the caller knows them (the shell does).
  function checkoutMenuRows(listing: any, current: any, sessions: any, primaryBranch = "", primaryDirty = false) {
    const rows = [{ name: "primary", branch: String(primaryBranch || ""), dirty: primaryDirty === true, primary: true }];
    for (const w of listing?.worktrees || []) {
      rows.push({
        name: String(w.name || ""),
        branch: String(w.branch || ""),
        dirty: w.dirty === true,
        primary: false,
      });
    }
    const states = sessions && WBProject?.worktreeStates ? WBProject.worktreeStates(rows, sessions) : {};
    return rows.map((r) => ({ ...r, current: (current ?? "primary") === r.name, state: states[r.name] || null }));
  }
  // The shell's last `/api/sessions` poll, kept for the menus' state dots.
  let lastSessions: any = [];
  function sessionsOfRepo(ref: any) {
    const route = WBSessionRoute;
    return (lastSessions || []).filter((s: any) => s && (route ? route.matchesRepo(s, ref) : s.repo === ref));
  }

  // The title: `<console name> (<label>)`, then ` · <checkout>`, then
  // ` · <repo slug>` (ADR-0066 §4).
  // On an agentic console the checkout segment is ALWAYS a button (`primary` on
  // the primary tree): it is where the first worktree is born via `+ new
  // worktree…`, so it cannot wait for one to exist (ADR-0063, amendment
  // 2026-09-16 b). Never on a plain shell (stays on the primary, #408), a
  // placeholder (no `_relaunchIn`) or the detached popup (`canLaunch === false`).
  function renderTitle(win: any, title: any, presentation: any) {
    win._presentation = presentation;
    // A rename in progress keeps its input; `endEdit` draws with the latest.
    if (title.querySelector(".session-name-input")) return;
    const switchable =
      win._deskKind !== "console" &&
      typeof win._relaunchIn === "function" &&
      OPTS.canLaunch !== false;
    title.textContent = "";
    const icon = document.createElement("i");
    icon.className = "bi bi-terminal";
    title.append(icon, " ");
    // SPANS, not bare text nodes: they are what ellipsise when the bar is
    // narrow, the repo first (06-consoles.css `.session-repo`); a text node
    // inside an inline-flex box wraps instead.
    const parts = WBConsoleName.labelParts(win._deskConsoleName || "", win._deskAgent || "console");
    const nameSpan = document.createElement("span");
    nameSpan.className = "session-name";
    nameSpan.textContent = parts.name;
    const labelSpan = document.createElement("span");
    labelSpan.className = "session-label";
    labelSpan.textContent = parts.tag;
    title.append(nameSpan, " ", labelSpan);
    wireRename(win, nameSpan);
    if (switchable) appendCheckout(win, title, presentation);
    // The project name closes the title and is the first text cut; a console
    // with no repo has none, its default name already says `home`.
    const repo = win._deskRepo && win._deskRepo !== "~" ? projectNameOf(win._deskRepo) : "";
    if (repo) {
      const repoSpan = document.createElement("span");
      repoSpan.className = "session-repo";
      // The dot is inside the span, so a repo cut to nothing leaves no dot.
      repoSpan.textContent = `· ${repo}`;
      title.append(" ", repoSpan);
    }
  }

  // The checkout segment of the title: ` · <checkout> ▾`.
  function appendCheckout(win: any, title: any, presentation: any) {
    const sep = document.createElement("span");
    sep.className = "session-title-sep";
    sep.textContent = "·";
    title.append(" ", sep, " ");
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "session-checkout";
    btn.title = "Switch worktree";
    // Before `session-open` the announcement has not come; the record says
    // where the console was asked to run (#411), never the picker.
    btn.textContent = presentation.checkout ?? win._deskCheckout ?? "primary";
    const caret = document.createElement("i");
    caret.className = "bi bi-chevron-down";
    btn.append(caret);
    btn.addEventListener("pointerdown", (e: any) => e.stopPropagation());
    btn.addEventListener("click", (e: any) => {
      e.stopPropagation();
      openCheckoutMenu(win, btn);
    });
    title.append(btn);
  }

  // Rename a console from its title (ADR-0066 §3): the fence rename's rules.
  // At rest the name is plain text; a mouse double-click, or a finger held on
  // the name (`wireTitleTouch`), swaps in an input, and `endEdit` — the ONE
  // place an edit ends — draws the title again, which puts the text back.
  // Enter commits; Escape, a press anywhere else and a focus loss cancel. No
  // lock check: a locked or fenced console can be renamed. The detached popup
  // cannot: its sink stores nothing.
  function canRename() {
    return OPTS.canLaunch !== false;
  }
  function wireRename(win: any, span: any) {
    if (!canRename()) return;
    span.addEventListener("dblclick", (e: any) => {
      e.stopPropagation();
      // On touch a double tap maximizes, even on the name.
      if (win._lastPointerType !== "mouse") return;
      startRename(win, span);
    });
  }
  function startRename(win: any, span: any) {
    if (!span.isConnected) return;
    const input = document.createElement("input");
    input.className = "session-name-input";
    input.setAttribute("aria-label", "Console name");
    input.maxLength = WBConsoleName.NAME_MAX;
    input.value = win._deskConsoleName || "";
    input.style.width = `${Math.max(span.offsetWidth + 16, 96)}px`;
    let editing = true;
    // Capture phase, before the plane's pan handler swallows the press: the
    // pan calls `preventDefault()` on mousedown, so focus does not move.
    // Also ends an edit whose window left the page: not every browser fires
    // `blur` on a removed input.
    const stopOutside = (ev: any) => {
      if (ev.target !== input || !win.isConnected) endEdit(false);
    };
    const endEdit = (commit: any) => {
      if (!editing) return;
      editing = false; // first: removing the input fires `blur`, which re-enters
      document.removeEventListener("pointerdown", stopOutside, true);
      if (commit) {
        win._deskConsoleName = WBConsoleName.renameValue(
          input.value,
          consolePrefix(win._deskRepo),
          takenNames(win._deskId),
        );
        setWin(win, { consoleName: win._deskConsoleName });
      } else {
        // A name another page gave while this edit was open was skipped by
        // `converge`; take it now.
        const stored = desk.find((r: any) => r.id === win._deskId)?.consoleName;
        if (stored) win._deskConsoleName = stored;
      }
      input.remove();
      if (win._title && win._presentation) renderTitle(win, win._title, win._presentation);
    };
    input.addEventListener("pointerdown", (ev: any) => ev.stopPropagation());
    input.addEventListener("dblclick", (ev: any) => ev.stopPropagation());
    input.addEventListener("keydown", (ev: any) => {
      // Held here so an Escape meant for this edit never reaches the plane.
      ev.stopPropagation();
      // An Enter that confirms an IME candidate is not a commit.
      if (ev.isComposing) return;
      if (ev.key === "Enter" || ev.key === "Escape") endEdit(ev.key === "Enter");
    });
    input.addEventListener("blur", () => endEdit(false));
    span.replaceWith(input);
    input.focus();
    input.select();
    document.addEventListener("pointerdown", stopOutside, true);
  }

  // The checkout menu — ONE component, under the console's title segment and
  // under the Files bar's chip. `host` is where the element lands (a console
  // window, or the document for the shell) and what it is positioned against;
  // `onPick(row)` for a non-current row; `onRemove(row)` adds a trash action per
  // worktree row; `onCreate()` adds `+ new worktree…`. One menu at a time;
  // closes on a pick, a click elsewhere, or Escape.
  let openMenu: any = null;
  function closeCheckoutMenu() {
    if (!openMenu) return;
    openMenu.el.remove();
    document.removeEventListener("pointerdown", openMenu.away, true);
    document.removeEventListener("keydown", openMenu.key, true);
    openMenu = null;
  }
  function checkoutMenu({ anchor, host, rows, onPick, onRemove, onCreate }: any) {
    if (openMenu?.anchor === anchor) return closeCheckoutMenu();
    closeCheckoutMenu();
    const menu = document.createElement("div");
    menu.className = "session-checkout-menu";
    menu.addEventListener("pointerdown", (e: any) => e.stopPropagation());
    for (const row of rows) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "session-checkout-item" + (row.current ? " current" : "");
      const glyph = document.createElement("i");
      glyph.className = row.current ? "bi bi-check2" : row.primary ? "bi bi-house-door" : "bi bi-folder2";
      const name = document.createElement("span");
      name.className = "session-checkout-name";
      name.textContent = row.name;
      item.append(glyph, name);
      if (row.branch) {
        const branch = document.createElement("span");
        branch.className = "session-checkout-branch";
        branch.textContent = row.branch;
        item.append(branch);
      }
      // The agent in this tree (ADR-0059): the same words as the console's dot.
      if (row.state) {
        const state = document.createElement("span");
        state.className = `session-checkout-state ${row.state}`;
        state.title = agentStateTitle(row.state);
        item.append(state);
      }
      if (row.dirty) {
        const dot = document.createElement("span");
        dot.className = "session-checkout-dirty";
        dot.title = "Uncommitted changes";
        item.append(dot);
      }
      if (onRemove && !row.primary) {
        // A span, not a button: a button inside a button is not HTML, and the
        // browser would hoist it out of the row.
        const trash = document.createElement("span");
        trash.setAttribute("role", "button");
        trash.tabIndex = 0;
        trash.className = "session-checkout-remove";
        trash.title = "Delete this worktree";
        trash.innerHTML = '<i class="bi bi-trash3"></i>';
        // The trash must not also PICK the row it sits on.
        trash.addEventListener("click", (e: any) => {
          e.stopPropagation();
          closeCheckoutMenu();
          onRemove(row);
        });
        item.append(trash);
      }
      item.addEventListener("click", (e: any) => {
        e.stopPropagation();
        closeCheckoutMenu();
        if (!row.current) onPick(row);
      });
      menu.append(item);
    }
    if (onCreate) {
      const create = document.createElement("button");
      create.type = "button";
      create.className = "session-checkout-item create";
      create.innerHTML = '<i class="bi bi-folder-plus"></i><span class="session-checkout-name">New worktree…</span>';
      create.title = "Create a worktree and restart this console in it";
      create.addEventListener("click", (e: any) => {
        e.stopPropagation();
        closeCheckoutMenu();
        onCreate();
      });
      menu.append(create);
    }
    const r = anchor.getBoundingClientRect();
    if (host === document.body) {
      // The shell's chip: the menu floats over the page, at the anchor.
      menu.style.position = "fixed";
      menu.style.left = `${r.left}px`;
      menu.style.top = `${r.bottom + 2}px`;
    } else {
      const h = host.getBoundingClientRect();
      menu.style.left = `${Math.max(0, r.left - h.left)}px`;
      menu.style.top = `${r.bottom - h.top + 2}px`;
    }
    host.append(menu);
    const away = (e: any) => {
      if (!menu.contains(e.target) && !anchor.contains(e.target)) closeCheckoutMenu();
    };
    const key = (e: any) => {
      if (e.key === "Escape") closeCheckoutMenu();
    };
    document.addEventListener("pointerdown", away, true);
    document.addEventListener("keydown", key, true);
    openMenu = { anchor, el: menu, away, key };
    return menu;
  }
  function openCheckoutMenu(win: any, anchor: any) {
    const ref = win._deskRepo;
    checkoutMenu({
      anchor,
      host: win,
      rows: checkoutMenuRows(worktreeListings[ref], win._deskCheckout ?? null, sessionsOfRepo(ref)),
      onPick: (row: any) => switchCheckout(win, row.primary ? null : row.name),
      onCreate: () => createWorktreeFor(win),
    });
  }

  // Move a console to another checkout (#412): confirm (the session restarts
  // and its scrollback goes), then `moveTo`. The picker's per-repo selection is
  // never touched: that is what Files shows; this is where THIS console lives.
  async function switchCheckout(win: any, checkout: any) {
    if (typeof win._relaunchIn !== "function") return;
    const where = checkout ? `worktree ${checkout}` : "the primary tree";
    const ok = await askConfirm({
      title: `Restart in ${checkout ?? "primary"}?`,
      message: `Restarts the ${win._deskAgent} session in ${where}. You lose the text in this console.`,
      confirmLabel: "Restart",
    });
    if (!ok) return;
    moveTo(win, checkout);
  }
  // The record is written with the choice BEFORE anything is requested, so a
  // daemon that dies mid-launch still leaves the intent behind. A LIVE session
  // is ended on the daemon first (`/api/sessions/close`): `relaunchIn` was
  // built for an ended child, and moving a running one left the old session
  // alive with no window (measured 2026-09-16). A watcher holds no baton and
  // must not kill the child another operator drives; it just relaunches.
  function moveTo(win: any, checkout: any) {
    const from = win._deskCheckout ?? null;
    win._deskCheckout = checkout;
    setWin(win, { checkout: checkout ?? null });
    endLiveThen(win, () => {
      win._relaunchIn(checkout);
      WB.emit("console-switch-checkout", { repo: win._deskRepo, from, to: checkout });
    });
  }
  // End this window's session on the daemon if it is still running, then `go`.
  // A DORMANT console still holds its session and must still close it.
  function endLiveThen(win: any, go: any) {
    const id = sessionIdOf(win);
    const live = id != null && !win.classList.contains("ended") && !watchingOf(win);
    if (live && WBSessionRoute) {
      fetch(WBSessionRoute.closeUrl(id, win._deskRepo), { method: "POST" }).then(go, go);
    } else {
      go();
    }
  }
  // The titlebar's restart: offered on a live session too, so it always asks
  // first — one click beside maximize must not tree-kill a working agent.
  async function restartWin(win: any) {
    if (typeof win._relaunchIn !== "function") return;
    const ended = win.classList.contains("ended");
    const ok = await askConfirm({
      title: "Restart session?",
      message: ended
        ? `Starts a fresh ${win._deskAgent || "console"} session in this window. You lose the text in this console.`
        : `Ends the running ${win._deskAgent || "console"} session and starts a fresh one. You lose the text in this console.`,
      confirmLabel: "Restart",
      danger: !ended,
    });
    if (!ok) return;
    endLiveThen(win, () => win._relaunchIn(undefined));
  }

  // The "new worktree" prompt: a name (worktree AND branch, ADR-0063 §2) and
  // the base branch. The name gate is `WBProject.worktreeCreateRow`; a refusal
  // the daemon DID send (`error`) re-opens with the message under the field.
  // Resolves `{name, base}` or `null` on cancel.
  function askWorktree({ base, branches, listing, error = "", name = "" }: any): Promise<any> {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-confirm";
    const modal = document.createElement("div");
    modal.className = "modal confirm-modal wb-worktree";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", "New worktree");
    const head = document.createElement("div");
    head.className = "modal-head";
    head.innerHTML = '<i class="bi bi-folder-plus"></i>';
    const heading = document.createElement("span");
    heading.className = "modal-title";
    heading.textContent = "New worktree";
    head.append(heading);
    const form = document.createElement("div");
    form.className = "wb-worktree-form";
    const nameLabel = document.createElement("label");
    nameLabel.textContent = "Name";
    const nameInput = document.createElement("input");
    nameInput.className = "prompt-input";
    nameInput.placeholder = "Worktree and branch name";
    nameInput.value = name;
    const baseLabel = document.createElement("label");
    baseLabel.textContent = "From";
    const baseInput = document.createElement(branches?.length ? "select" : "input");
    baseInput.className = "prompt-input";
    if (branches?.length) {
      for (const b of branches) {
        const opt = document.createElement("option");
        opt.value = b;
        opt.textContent = b;
        baseInput.append(opt);
      }
      baseInput.value = branches.includes(base) ? base : branches[0];
    } else {
      baseInput.value = base || "";
      baseInput.placeholder = "Branch to start from";
    }
    const note = document.createElement("p");
    note.className = "wb-worktree-note";
    note.textContent = `${WBProject?.CARRY_OVER_NOTE || ""} The console restarts in the new worktree. You lose the text in this console.`;
    const err = document.createElement("p");
    err.className = "prompt-error";
    err.textContent = error;
    err.hidden = !error;
    form.append(nameLabel, nameInput, baseLabel, baseInput, note, err);
    const foot = document.createElement("div");
    foot.className = "modal-foot";
    const cancel = document.createElement("button");
    cancel.className = "btn";
    cancel.type = "button";
    cancel.textContent = "Cancel";
    const go = document.createElement("button");
    go.className = "btn accent";
    go.type = "button";
    go.textContent = "Create & restart";
    foot.append(cancel, go);
    modal.append(head, form, foot);
    scrim.append(modal);
    document.body.append(scrim);
    nameInput.focus();
    nameInput.select();

    return new Promise((resolve) => {
      let settled = false;
      const done = (value: any) => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        scrim.remove();
        resolve(value);
      };
      const problem = () =>
        WBProject?.worktreeNameProblem?.(listing || { worktrees: [] }, nameInput.value) || "";
      const submit = () => {
        const row = WBProject?.worktreeCreateRow?.(listing || { worktrees: [] }, baseInput.value, nameInput.value);
        if (!row) {
          err.textContent = problem() || "Choose a branch to start from.";
          err.hidden = false;
          nameInput.focus();
          return;
        }
        done({ name: row.name, base: row.base });
      };
      // The mask runs on every edit, typed or pasted: a refused character
      // never shows, and there is no message. The caret stays after the
      // last kept character. The create stays disabled, without a message,
      // while the name is still one the daemon would refuse.
      const mask = WBProject?.maskWorktreeName || ((s: any) => s);
      nameInput.addEventListener("input", () => {
        const raw = nameInput.value;
        const masked = mask(raw);
        if (masked !== raw) {
          const caret = mask(raw.slice(0, nameInput.selectionStart ?? raw.length)).length;
          nameInput.value = masked;
          nameInput.setSelectionRange(caret, caret);
        }
        err.textContent = "";
        err.hidden = true;
        go.disabled = !!problem();
      });
      go.disabled = !!problem();
      const onKey = (e: any) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          done(null);
        } else if (e.key === "Enter" && modal.contains(document.activeElement)) {
          e.stopPropagation();
          if (document.activeElement === cancel) done(null);
          else submit();
        }
      };
      document.addEventListener("keydown", onKey, true);
      cancel.addEventListener("click", () => done(null));
      go.addEventListener("click", submit);
    });
  }

  // `+ new worktree…` from a console's switcher: ask, `worktree.add`, tell the
  // shell, and move THIS console into it (the prompt already said it restarts).
  // Base list is `branch.list`, one read per prompt; unreadable → free text.
  async function createWorktreeFor(win: any) {
    const repo = win._deskRepo;
    if (!repo || repo === "~" || typeof win._relaunchIn !== "function") return;
    const daemon = window.WBDaemon;
    if (typeof daemon?.observe !== "function") return;
    let branches = [];
    let base = "";
    try {
      const reply = await daemon.observe("branch.list", { repo });
      const data = (reply && reply.status === "ok" && reply.branches) || {};
      if (Array.isArray(data.branches)) branches = data.branches;
      if (data.current && data.current !== "HEAD") base = data.current;
    } catch {}
    let error = "";
    let name = "";
    for (;;) {
      const ask = await askWorktree({ base, branches, listing: worktreeListings[repo], error, name });
      if (!ask) return;
      name = ask.name;
      base = ask.base;
      let reply;
      try {
        reply = await daemon.observe("worktree.add", { repo, name, base });
      } catch {
        error = "Could not reach the daemon. Check whether the worktree was created.";
        continue;
      }
      if (!reply || reply.status !== "ok") {
        error = WBFail.failed(reply, "Could not create the worktree: the daemon gave no reason.");
        continue;
      }
      ensureListing(repo, true);
      WB.emit("worktree-created", { project: repo, name, message: typeof reply.message === "string" ? reply.message : "" });
      moveTo(win, name);
      return;
    }
  }

  // Toggle a console between its floating rect and a full-VIEWPORT bleed. The
  // pre-maximize rect stays in the inline styles, so restoring drops the class.
  //
  // On a scrollable stage the bleed is pinned to what the operator is looking
  // at: `--max-left`/`--max-top` carry the viewport's scroll offsets, re-derived
  // by `syncMaxPin`. Re-asserted after the class flip because `maxlock`
  // (`overflow:hidden`) drops the scrollbars, which can clamp the offsets.
  //
  // `persist` writes `max`: the operator's own toggle, and the shell's columns
  // moving the maximize to another console (ADR-0051 §5). A restore
  // (`buildChrome`) and the torn-off fence window's columns write nothing: a
  // restore already reads the record, and that window's grid is never stored.
  function setMax(win: any, on: any, persist = false) {
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

  function toggleMax(win: any) {
    setMax(win, !win.classList.contains("maximized"), true);
  }

  // A column restores like a maximize, so it shows the same control.
  function paintMaxButton(win: any) {
    const btn = win._maxBtn;
    if (!btn) return;
    const on = win.classList.contains("maximized") || win.classList.contains("column");
    btn.title = on ? "Restore" : "Maximize";
    btn.innerHTML = on
      ? '<i class="bi bi-fullscreen-exit"></i>'
      : '<i class="bi bi-fullscreen"></i>';
  }

  // ---- columns (ADR-0051 §5) --------------------------------------------------
  // The shell (`app.ts`) owns the column list and folds it with `WBColumns`;
  // this module only paints the answer. It never reads `WBColumns`: the
  // detached-fence popup boots this file without it.
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

  function clearColumn(win: any) {
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
  function applyColumns(painted: any, opts: any) {
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
  function dropClosedElsewhere(id: any) {
    const win = findWindow(id);
    if (!win) return;
    tearDownMember(win, "window-closed");
    applyExtent();
  }

  function focusedId() {
    return stage()?.querySelector(".session-window.focused")?._deskId ?? null;
  }

  function focusColumn(id: any) {
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
    const out: any = {};
    for (const [id, entry] of popups.entries()) {
      out[id] = (entry.members || [])
        .filter((m: any) => m && m.id && m.kind !== "note")
        .map((m: any) => ({
          id: m.id,
          agent: m.agent,
          name: desk.find((r: any) => r.id === m.id)?.consoleName ?? m.consoleName ?? null,
          repo: m.repo === "~" ? null : (m.repo ?? null),
          kind: m.kind,
        }));
    }
    return {
      rows: list(),
      fences: fenceList().map(({ id, name }: any) => ({ id, name })),
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
  function isFull(win: any) {
    return document.fullscreenElement === win;
  }

  // ---- locked in place (ADR-0050 / ADR-0051 lock amendment) -----------------
  // A lock is a property of the DESK, honoured on every device: the gesture
  // handlers consult it and refuse; nothing else changes (maximize, fullscreen
  // and close do not rewrite the rect). A console is locked by its own record
  // OR by the fence holding its centre — the same `fenceOf` fold as membership.
  function fenceLocked(id: any) {
    return !!fences.find((f: any) => f.id === id)?.locked;
  }
  function isLocked(win: any) {
    if (win._deskLocked) return true;
    return heldByFence(win);
  }
  function heldByFence(el: any) {
    return fenceHolds(fences, restoreRect(el), OPTS.autoBoot === false);
  }
  // The one place a window's own lock is set: flag, class, glyph.
  function applyLock(win: any, locked: any) {
    win._deskLocked = !!locked;
    win.classList.toggle("locked", !!locked);
    paintLockGlyph(win);
  }
  // The glyph shows the EFFECTIVE lock: a console held by a locked fence reads
  // closed like its fence. Held-only, the button is disabled — its own toggle
  // would change nothing the operator can see; the fence's lock is the one to
  // open.
  function paintLockGlyph(win: any) {
    const btn = win.querySelector(".session-lock");
    if (!btn) return;
    const own = !!win._deskLocked;
    const held = !own && heldByFence(win);
    const locked = own || held;
    btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
    btn.title = held ? "Locked by its fence. Unlock the fence first." : own ? "Unlock" : "Lock in place";
    btn.setAttribute("aria-pressed", locked ? "true" : "false");
    btn.disabled = held;
  }
  function toggleLock(win: any) {
    applyLock(win, !win._deskLocked);
    setWin(win, { locked: !!win._deskLocked });
  }
  // Same for a fence: the class, the glyph, and the tile button, which is a
  // no-op on a locked fence and says so by being disabled.
  function paintFenceLock(el: any, locked: any) {
    el.classList.toggle("locked", !!locked);
    const btn = el.querySelector(".fence-lock");
    if (btn) {
      btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
      btn.title = locked ? "Unlock this fence" : "Lock this fence in place";
      btn.setAttribute("aria-pressed", locked ? "true" : "false");
    }
    const tile = el.querySelector(".fence-arrange");
    if (tile) tile.disabled = !!locked;
  }
  function setFenceLock(id: any, locked: any) {
    saveFences(fences.map((x: any) => (x.id === id ? { ...x, locked: !!locked } : x)));
    renderFences();
  }

  function toggleFull(win: any) {
    if (document.fullscreenElement === win) {
      // The promise rejects if we are already leaving; there is nothing to
      // recover, and `syncFullState` runs off the event either way.
      document.exitFullscreen().catch(() => {});
      return;
    }
    focusWin(win);
    // Requesting while ANOTHER element is fullscreen is a legal swap — browsers
    // move the top layer without a round trip through the exit.
    win.requestFullscreen().catch((err: any) => {
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
    for (const btn of document.querySelectorAll(".session-full")) {
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
    const at = (w: any, v: any) => parseInt(w.style.getPropertyValue(v), 10) || 0;
    const cols = [...st.querySelectorAll(".session-window.column")].sort(
      (a, b) => at(a, "--col-index") - at(b, "--col-index") || at(a, "--row-index") - at(b, "--row-index"),
    );
    if (cols.length) {
      for (const w of cols) focusWin(w);
      return;
    }
    // The LAST one, if a desk somehow carries two: it is the one whose record
    // was written most recently, and exactly one window can usefully be on top.
    const all = st.querySelectorAll(".session-window.maximized");
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
    for (const win of st.querySelectorAll(".session-window.maximized, .session-window.column")) {
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
  function applyExtent(opts?: any) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // The one place the freeze is kept in step with what is on screen.
    syncMaxLock();
    // Read the DOM, not `wins`: a window is on the stage from `buildChrome`'s
    // append (before `spawnWindow` registers it) to its removal. Fences count
    // too — ADR-0051 §2 sizes the plane to windows AND fences.
    const rects = [...st.querySelectorAll(".session-window, .fence, .note-card")].map(restoreRect);
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
      document.dispatchEvent(
        new CustomEvent("workbench:stage-extent", { detail: { width, height } }),
      );
    }
  }

  // ---- the per-client view (issue #339) ----------------------------------------
  // Where this browser profile was looking. `landed` latches the FIRST paint's
  // bbox landing so a later refit cannot re-centre a plane the operator has
  // panned — but a STORED offset is re-applied on every call: `.consoles-tab`
  // is `x-show`, and `display:none` destroys `#workspace`'s scroll position.
  // INVARIANT: never runs before `applyExtent()` on any path — the clamp needs
  // the extent the same frame's rects imply.
  let landed = false;
  // A reveal that arrived while `.consoles-tab` was `display:none` (viewport
  // measures 0, so `reveal` cannot centre): parked here, honoured by the first
  // `applyLanding` that CAN measure, AHEAD of the stored offset. Measured:
  // a reveal requested on the same synchronous stack as `activate` runs before
  // Alpine's `x-show` flip, which lands a microtask later.
  let pendingReveal: any = null;
  // Whether `restoreDesk` has finished on ANY of its exits (a failed fetch
  // included): distinguishes "empty because nothing restored
  // YET" from "empty because there is nothing to restore".
  let deskSettled = false;
  function applyLanding() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // A hidden tab measures a 0×0 viewport, where every landing centres on
    // nothing — and `saveOffset` would then persist that nothing.
    if (!ws.clientWidth || !ws.clientHeight) return;
    const rects = [...st.querySelectorAll(".session-window")].map(restoreRect);
    // A parked reveal outranks the stored offset: this is the frame it was
    // waiting for, and the operator's last act was asking for that window.
    if (pendingReveal != null) {
      const wanted = pendingReveal;
      pendingReveal = null;
      // Latch FIRST: `revealNow` stores the offset it scrolls to, and that
      // store is suppressed until the landing has happened.
      if (rects.length || deskSettled) landed = true;
      if (revealNow(wanted)) return;
    }
    const stored = viewStore?.read()?.off || null;
    if (landed && !stored) return;
    const at = viewLanding(
      stored,
      rects,
      { width: ws.clientWidth, height: ws.clientHeight },
      { width: st.offsetWidth, height: st.offsetHeight },
    );
    ws.scrollLeft = at.left;
    ws.scrollTop = at.top;
    // Latch only once the stage HOLDS something (or is known final):
    // `restoreView` reaches here after ONE round trip, `restoreDesk` needs two
    // plus the spawn; latching on the still-empty frame would make the real
    // landing return early at the guard above.
    if (rects.length || deskSettled) landed = true;
  }

  // The offset half of the store, debounced like the desk flush. SUPPRESSED
  // until the landing has been applied: `applyExtent` and the `x-show` flip both
  // fire `scroll` before the restore, so an unguarded listener would persist 0,0
  // over the operator's stored pan on every boot.
  let offsetFlush: any = null;
  let pendingOffset: any = null;
  function flushOffset() {
    // Writes the offset CAPTURED at schedule time, never a fresh read: a file
    // tab switched to inside the 250 ms hides `.consoles-tab` (`x-show`), and
    // `display:none` resets the offsets to 0 (measured: `off:{0,0}` stored over
    // a real 1500,850 pan). Bailing instead would drop the operator's last pan.
    if (!pendingOffset) return;
    viewStore?.patch({ off: pendingOffset });
    pendingOffset = null;
  }
  function saveOffset() {
    const ws = workspace();
    if (!landed || !ws || !ws.clientWidth || !ws.clientHeight) return;
    pendingOffset = { left: ws.scrollLeft, top: ws.scrollTop };
    clearTimeout(offsetFlush);
    offsetFlush = setTimeout(() => {
      offsetFlush = null;
      flushOffset();
    }, 250);
  }

  // Give a surface a place in the window tier WITHOUT focusing it. A restore
  // builds its consoles through `buildChrome`, which ends in `focusWin` and so
  // hands every window a z; a note card is built by `WBNotes.render` and had
  // none, which put it at `auto` — BELOW every console (z ≥ 61). MEASURED: a
  // card restored beside a console was visible where nothing overlapped and
  // deaf where something did, because the click landed on the terminal's
  // canvas and the keystrokes went to the shell. A surface on the plane is in
  // the tier or it is under it; there is no third state.
  function stackWin(win: any) {
    if (win.style.zIndex) return;
    // At the ceiling the counter stops and the newcomers tie: a tie among
    // cards is a stacking order, while a number past the ceiling would put a
    // card over the tab bar. The next `focusWin` renormalises the lot.
    if (z < Z_CEIL) z += 1;
    win.style.zIndex = z;
  }

  function focusWin(win: any) {
    z += 1;
    if (z > Z_CEIL) {
      // Renormalize: re-stack the existing windows by their current z, resetting
      // the counter so focus never pushes a console over the overlay/tabbar tier.
      const ordered = [...workspace().querySelectorAll(".session-window, .note-card")].sort(
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
    for (const w of workspace().querySelectorAll(".session-window.focused, .note-card.focused")) {
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
    return [...st.querySelectorAll(".session-window")].map((w) => ({
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
  function ingestSessions(sessions: any) {
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

  // The "one action" that reaches a window far from the current view (ADR-0051
  // §4): focus it and slide the viewport so it is centred. Returns the element,
  // or null when no window carries that desk id.
  function reveal(deskId: any) {
    const ws = workspace();
    const it = findWindow(deskId);
    if (!it) return null;
    if (ws && ws.clientWidth && ws.clientHeight) return revealNow(deskId);
    // A viewport measuring 0 is a tab still `display:none` (docs/TESTING-TRAPS.md
    // → The workbench page in a browser); centring against it clamps to 0,0. Focus now, park
    // the centring for the frame that can measure (`pendingReveal`).
    focusWin(it);
    pendingReveal = deskId;
    return it;
  }

  function findWindow(deskId: any) {
    const st = stage();
    if (!st) return null;
    return (
      [...st.querySelectorAll(".session-window")].find((w) => w._deskId === deskId) || null
    );
  }

  // The centring half, on a viewport that is known to measure.
  function revealNow(deskId: any) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return null;
    const it = findWindow(deskId);
    if (!it) return null;
    focusWin(it);
    // A maximized TARGET already fills the frame. Only the target is checked:
    // Go-to pans the plane while something else may be maximized, and `maxlock`
    // does NOT refuse a programmatic offset write — the resulting `scroll`
    // re-derives the pin (`syncMaxPin`, #338).
    if (it.classList.contains("maximized") || it.classList.contains("column")) return it;
    const to = bringIntoView(
      restoreRect(it),
      { width: ws.clientWidth, height: ws.clientHeight },
      { width: st.offsetWidth, height: st.offsetHeight },
    );
    ws.scrollLeft = to.left;
    ws.scrollTop = to.top;
    // The reveal IS the new view, stored NOW: `refitAll` runs `applyLanding` in
    // the same frame chain and re-applies the STORED offset, which for 250 ms
    // is still the pre-reveal one. A pending flush is dropped with it.
    if (landed) {
      pendingOffset = null;
      clearTimeout(offsetFlush);
      offsetFlush = null;
      viewStore?.patch({ off: { left: to.left, top: to.top } });
    }
    return it;
  }

  // ---- the fence floor ---------------------------------------------------------
  // The fence element, its two gestures, tiling and detaching are
  // `wb-console-fences.ts`.
  const FENCE_NAME_MAX = 60;

  // The refusal flash `createFence` schedules. Module scope so a gesture
  // starting inside its 600 ms window can CANCEL it: otherwise the timer strips
  // a `fence-invalid` the gesture put there, and with the cursor at rest no
  // move re-adds it.
  let fenceFlash: any = null;
  function clearFenceFlash() {
    if (fenceFlash == null) return;
    clearTimeout(fenceFlash.timer);
    fenceFlash.el.classList.remove("fence-invalid");
    fenceFlash = null;
  }

  function fenceEl(id: any) {
    const st = stage();
    if (!st) return null;
    for (const el of st.querySelectorAll(".fence")) {
      if (el.dataset.fenceId === id) return el;
    }
    return null;
  }

  // Re-derive every fence's count readout from the stage (#342). Membership is
  // never stored, so this folds the LIVE rects — from `renderFences`,
  // `setWin` and `forgetRecord`. NOT from `applyExtent`: it fires per
  // mousemove, and `offsetLeft` on a `.tiling` window is the INTERPOLATED
  // value mid-transition.
  function refreshFenceChrome() {
    const st = stage();
    // A hidden tab measures 0 and this fold reads MEASURED rects: refreshing
    // there writes `0 consoles` onto every fence. `refitAll` calls this again
    // on the first frame that can measure.
    if (!st || !st.offsetWidth || !st.offsetHeight) return;
    const els = new Map();
    for (const el of st.querySelectorAll(".fence")) els.set(el.dataset.fenceId, el);
    // A DETACHED fence's consoles are in a popup, so the membership fold
    // answers zero — but the fence is emptied, not empty (ADR-0051 §7a). Take
    // the count from the registry for those; the fold stays pure.
    const away = detachedMembers();
    for (const s of fenceSummaries(readFenceRects(st), readWindowRects(st))) {
      const el = els.get(s.id);
      if (!el) continue;
      const n = away[s.id] ? away[s.id].length : s.count;
      // Parenthesised: it trails the name field and reads as an aside to it.
      const count = el.querySelector(".fence-count");
      if (count) count.textContent = `(${n} console${n === 1 ? "" : "s"})`;
      const cols = el.querySelector(".fence-columns");
      if (cols) cols.disabled = s.count === 0;
    }
    paintFenceColumns();
    // A console HELD by a locked fence wears the fence's lock (the class drops
    // its bands and grab cursor). Derived here with membership, from live rects.
    for (const w of st.querySelectorAll(".session-window")) {
      w.classList.toggle("held", !w._deskLocked && heldByFence(w));
      paintLockGlyph(w);
    }
    // A card held by a locked fence is read-only for the same reason, and by
    // the same derivation (ADR-0064 §8) — the record is not rewritten. NOT in
    // the popup: `mountDetached` re-origins its members' rects into this
    // window while `fences` still holds the shell's stage coordinates, so the
    // derivation there would match a card to whatever fence happens to cover
    // the translated point.
    for (const el of OPTS.autoBoot === false ? [] : st.querySelectorAll(".note-card")) {
      const own = notes.find((n: any) => n.id === el.dataset.noteId);
      const held = !own?.locked && !!fenceOf(fences, restoreRect(el))?.locked;
      el.classList.toggle("held", held);
      window.WBNotes?.applyLock(el, !!own?.locked || held);
    }
  }

  // A detached fence's consoles are in the popup, and a phone paints one
  // console: no columns there (ADR-0051 §5, 2026-09-30). Also called from
  // `applyColumns`, which runs on every resize.
  function paintFenceColumns() {
    const narrow = columnMeasure().viewport <= PHONE_MAX_WIDTH;
    for (const el of stage()?.querySelectorAll(".fence") || []) {
      const btn = el.querySelector(".fence-columns");
      if (btn) btn.hidden = narrow || isDetached(el.dataset.fenceId);
    }
  }

  // The two DOM reads `refreshFenceChrome` and `fenceList` share: the stage is
  // where a fence and a window ARE, and membership is derived from those live
  // rects, never from `fences` or `wins`.
  function readFenceRects(st: any) {
    return [...st.querySelectorAll(".fence")].map((el) => ({
      id: el.dataset.fenceId,
      name: el.querySelector(".fence-name")?.value || "",
      rect: restoreRect(el),
    }));
  }

  function readWindowRects(st: any) {
    return [...st.querySelectorAll(".session-window")].map((w) => ({
      id: w._deskId,
      repo: w._deskRepo,
      rect: restoreRect(w),
    }));
  }

  // The fence list the toolbar picker shows (#343) — the same fold the fence
  // chrome reads. A SNAPSHOT at menu open, like `list()`.
  // The fence RECORDS (id, name, rect, locked), not the chrome summaries: the
  // note card derives its lock from the fence holding it and needs the rects.
  function fenceRecords() {
    return fences.map((f: any) => ({ ...f }));
  }

  // The cards, rendered by `wb-notes.ts`. Called wherever `renderFences` is —
  // the two collections go on the plane together or the extent is folded over
  // half of them.
  function renderNotes() {
    window.WBNotes?.render();
  }

  function fenceList() {
    const st = stage();
    if (!st) return [];
    return fenceSummaries(readFenceRects(st), readWindowRects(st));
  }

  // A PEER's liveness as a rule: (state, event, windowMs) -> { state, effects }.
  // Pure, so it decides both directions of the link (origin watching popup,
  // popup watching origin) and the node table drives the boundary. `windowMs`
  // is an ARGUMENT, never a global.
  //
  // Loss is TERMINAL: a beat after `lost` does not resurrect — the caller turns
  // the effect into a `window.close()` or a `reattachFence`, neither undoable.
  // Loss is STRICT (`> windowMs`), so the boundary tick is still alive.
  function peerFold(state: any, event: any, windowMs: any) {
    const seen = typeof state?.seen === "number" ? state.seen : null;
    const lost = !!state?.lost;
    const same = { seen, lost };
    if (lost) return { state: same, effects: [] };
    switch (event?.type) {
      case "beat":
        return { state: { seen: typeof event.at === "number" ? event.at : seen, lost: false }, effects: [] };
      case "tick":
        // `seen: null` — never heard from — expires nothing: the origin seeds
        // every entry with `Date.now()` at creation AND at boot-restore, so one
        // rule governs the post-reload adoption grace and steady state alike.
        if (seen == null || typeof event.at !== "number") return { state: same, effects: [] };
        if (event.at - seen > windowMs)
          return { state: { seen, lost: true }, effects: [{ type: "peer-lost" }] };
        return { state: same, effects: [] };
      case "gone":
        // An announced departure: the same effect, without waiting the window out.
        return { state: { seen, lost: true }, effects: [{ type: "peer-lost" }] };
      default:
        return { state: same, effects: [] };
    }
  }

  // ---- detaching a fence into its own window (issues #346, #347) ---------------
  // The popup registry, `detached` and `fencePopups`, is `popups`
  // (`wb-console-popups.ts`): every change to either goes through it.
  const PEER_WINDOW = WBDetachLink.PEER_WINDOW_MS;
  const HEARTBEAT = WBDetachLink.HEARTBEAT_MS;

  // The popup's member set, adopted as the truth. A console CLOSED inside the
  // popup ended a real daemon session; re-attaching it would wire a window to a
  // gone session, so its desk RECORD goes too. Both `popup-members` and
  // `popup-here` arrive here, so the two never prune differently.
  function adoptMembers(id: any, members: any) {
    const entry = popups.entry(id);
    if (!entry || !Array.isArray(members)) return;
    const alive = new Set(members.map((m) => m?.id).filter(Boolean));
    const dropped = [...(entry.members || []).map((m: any) => m?.id), ...(entry.memberIds || [])].filter(
      (wid) => wid && !alive.has(wid),
    );
    entry.members = members;
    entry.memberIds = members.map((m) => m?.id).filter(Boolean);
    entry.adopted = true;
    // A remove is a change the daemon applies to its own desk, so a record
    // this page has not read yet goes too.
    for (const wid of new Set(dropped)) forgetRecord(wid);
    commitDetached(popups.detachedIds());
  }

  // Is a quiet peer actually GONE? Silence is weak evidence: LIMIT — Chrome
  // throttles a hidden tab's timers to one tick per MINUTE after ~5 minutes, so
  // a workbench behind another tab stops beating while alive, and a six-second
  // window read a working popup as dead.
  // Two better witnesses, in order: the WINDOW HANDLE (answers `closed`
  // synchronously, when this document opened the popup), then a PROBE (message
  // delivery is not throttled; one unheard probe is death). A handle-less
  // entry (this tab reloaded) has only the second.
  function stillThere(id: any, entry: any) {
    if (entry.handle && !entry.handle.closed) {
      entry.peer = { seen: Date.now(), lost: false };
      entry.probed = false;
      return true;
    }
    if (entry.probed) return false;
    entry.probed = true;
    entry.peer = { seen: Date.now(), lost: false };
    link.post({ type: "origin-ping", tab: link.tab, fenceId: id });
    return true;
  }

  // The origin's heartbeat: one interval for ALL entries, so the cost does not
  // scale with the cap. It both announces this tab and ages every peer.
  let beat: any = null;
  function startBeat() {
    if (beat) return;
    beat = setInterval(() => {
      link.post({ type: "origin-beat", tab: link.tab });
      const at = Date.now();
      // A copy: `reattachFence` removes an entry inside this loop.
      for (const [id, entry] of popups.entries()) {
        if (!entry.peer) continue;
        const out = peerFold(entry.peer, { type: "tick", at }, PEER_WINDOW);
        entry.peer = out.state;
        // Consoles must never be nowhere: a popup that stopped answering brings
        // its members home. But SILENCE IS NOT DEATH (`stillThere`).
        if (out.effects.some((e) => e.type === "peer-lost") && !stillThere(id, entry)) {
          reattachFence(id);
        }
      }
    }, HEARTBEAT);
  }
  function stopBeat() {
    if (beat) clearInterval(beat);
    beat = null;
  }

  // The origin's half of the lifecycle channel. The channel is browser-WIDE:
  // the `tab` filter keeps a SECOND tab's popups out of this registry, the
  // `origin-` prefix drop keeps this tab from consuming its own broadcasts.
  link.onMessage((m: any) => {
    if (!m || typeof m.type !== "string") return;
    if (link.tab == null || m.tab !== link.tab) return;
    if (m.type.startsWith("origin-")) return;
    const id = m.fenceId;
    // An EARLIER popup of the same fence still talks while it unloads: its
    // `popup-gone` would re-attach the popup that replaced it, and its
    // `popup-members` would drop that popup's consoles (#476). Only the popup
    // this entry holds is heard. A ping is answered whoever sends it.
    if (m.type !== "popup-here" && m.type !== "popup-ping" && !popupMatches(popups.entry(id), m)) {
      return;
    }
    if (m.type === "popup-here") {
      // A popup that survived this tab's reload, announcing which fence it
      // holds. Adopted only when the RESTORED registry already says that fence
      // is detached — the payload alone must never be able to detach one.
      if (!isDetached(id)) return;
      if (popups.has(id) && !popupMatches(popups.entry(id), m)) return;
      // MUTATED IN PLACE, never replaced: `glyphClick`'s ping compares the entry
      // it captured with the one in the map.
      const entry = popups.entry(id) || newPopupEntry();
      // A restored entry learns which popup it holds from its first answer.
      if (entry.pid == null && typeof m.pid === "string") entry.pid = m.pid;
      const st = stage();
      entry.greeted = true;
      // The popup hands back the UNTRANSLATED snapshot it was given, so a
      // re-attach puts every console back where it was detached from. Adopted
      // whole, EMPTY included: an empty set is an answer, not a missing one.
      popups.put(id, entry);
      if (Array.isArray(m.members)) {
        adoptMembers(id, m.members);
        // A name report sent while this tab was reloading reached nobody. The
        // popup's members carry the name it gave, so it is recorded from
        // here, once the desk has loaded. `recordNoteName` refuses a record
        // that already has its path, so this is safe to repeat.
        deskReady.then(() => {
          for (const x of m.members) {
            if (x?.kind === "note" && typeof x.path === "string") {
              recordNoteName(id, { noteId: x.id, path: x.path });
            }
          }
        });
      }
      if (!entry.fence) {
        entry.fence = (st ? readFenceRects(st).find((f) => f.id === id) : null) || {
          id,
          name: "",
          rect: null,
        };
      }
      entry.peer = { seen: Date.now(), lost: false };
      entry.probed = false;
      popups.put(id, entry);
      // Re-persist: the snapshot the popup just handed back is a better member
      // list than the ids this tab restored, and the NEXT reload reads it.
      commitDetached(popups.detachedIds());
      showDetachGlyph(id, true);
    } else if (m.type === "popup-members") {
      // A console closed INSIDE the popup. Same registry gate as `popup-here`.
      if (isDetached(id)) adoptMembers(id, m.members);
    } else if (m.type === "popup-beat") {
      const entry = popups.entry(id);
      if (entry?.peer) entry.peer = peerFold(entry.peer, { type: "beat", at: Date.now() }, PEER_WINDOW).state;
      // Any word at all clears the probe: `stillThere` asks "has it answered
      // SINCE I asked", and a beat is an answer.
      if (entry) entry.probed = false;
    } else if (m.type === "popup-ping") {
      // The popup asking whether THIS document is still here. Answering from a
      // message handler is the point: a throttled tab still delivers messages.
      if (isDetached(id)) link.post({ type: "origin-here", tab: link.tab, fenceId: id });
    } else if (m.type === "popup-note-named") {
      recordNoteName(id, m);
    } else if (m.type === "popup-note-claimed") {
      recordNoteClaim(id, m);
    } else if (m.type === "popup-gone") {
      // The tab filter proved the sender is ours; `detachFold` makes a re-attach
      // of a fence this tab does not hold a no-op.
      reattachFence(id);
    }
  });

  // The popup's card gave a never-saved note its file. The popup cannot write
  // the desk (ADR-0051 §8), so it reports the name and this tab records it,
  // after checking the report. The report comes twice: over `postMessage`,
  // which arrives before the popup's own `wb-fence-reattach`, and over the
  // channel, which still reaches this tab after a reload. The second one is
  // refused because the record already has its path.
  function recordNoteName(id: any, m: any) {
    const entry = popups.entry(id);
    const record = notes.find((n: any) => n.id === m.noteId);
    if (!isDetached(id) || !noteNameOk(entry, record, m)) return;
    saveNotes(notes.map((n: any) => (n.id === m.noteId ? { ...n, path: m.path } : n)));
    entry.members = entry.members.map((x: any) => {
      if (x.kind !== "note" || x.id !== m.noteId) return x;
      const { draft, claim, ...rest } = x;
      return { ...rest, path: m.path };
    });
  }

  // The name the popup's card chose BEFORE its first write, kept on the member
  // and never on the desk (a path on the desk says a file is there). If the
  // popup closes with that write in flight, no name report follows, and a
  // re-attach reads or writes this name instead of choosing a second one.
  function recordNoteClaim(id: any, m: any) {
    const entry = popups.entry(id);
    const record = notes.find((n: any) => n.id === m.noteId);
    if (!isDetached(id) || !noteNameOk(entry, record, { noteId: m.noteId, path: m.claim })) return;
    entry.members = entry.members.map((x: any) =>
      x.kind === "note" && x.id === m.noteId ? { ...x, claim: m.claim } : x,
    );
  }

  // Unique in this browser: the clock separates this tab's documents, the
  // counter separates two detaches in one millisecond.
  let popupSeq = 0;
  function newPid() {
    popupSeq += 1;
    return `${Date.now().toString(36)}-${popupSeq}`;
  }

  // A refused fence verb, said ON the fence. Cleared on a timer so a stale
  // refusal cannot outlive the gesture that caused it.
  function fenceNotice(id: any, text: any) {
    const el = fenceEl(id)?.querySelector(".fence-notice");
    if (!el) return;
    el.textContent = text;
    clearTimeout(el._noticeTimer);
    el._noticeTimer = setTimeout(() => {
      el.textContent = "";
    }, 2600);
  }

  function showDetachGlyph(id: any, on: any) {
    const away = fenceEl(id)?.querySelector(".fence-detached");
    if (away) away.hidden = !on;
  }

  // What the popup is handed: one record per member in the shape `buildChrome`
  // restores from, plus the live session id. Rects are measured HERE,
  // untranslated — the popup translates for its own viewport and never sends
  // them back, so a re-attach returns every console to its original box.
  function fenceSnapshot(id: any) {
    const st = stage();
    if (!st) return [];
    const all = [...st.querySelectorAll(".session-window")];
    const byId = new Map(all.map((w) => [w._deskId, w]));
    const ids = fenceMembership(readFenceRects(st), readWindowRects(st))[id] || [];
    const windows = ids
      .map((wid: any) => byId.get(wid))
      .filter(Boolean)
      .map((win: any) => ({
        ...deskOf(win),
        session: sessionIdOf(win),
      }));
    // The cards the fence holds ride along (ADR-0064 §8), tagged so the popup
    // and the re-attach can tell them from a console. Their RECORDS travel,
    // not their DOM: a card is rebuilt in the popup from the same desk record
    // the stage built it from.
    // A note with no file yet carries its unsaved text (`draft`): the popup
    // must open in this click, before a first save could land. The draft
    // lives in this snapshot only, never in the desk (ADR-0064 §8, #475).
    const cards = notes
      .filter((n: any) => fenceOf(fences, n.rect || {})?.id === id)
      .map((n: any) => ({ ...n, ...window.WBNotes?.draftOf?.(n.id), kind: "note" }));
    return windows.concat(cards);
  }

  // Take a member off the plane WITHOUT forgetting its desk record (shared
  // state a second client still renders) and WITHOUT closing its daemon
  // session: `dispose()` closing the socket is the writer-slot release the
  // popup then re-acquires (ADR-0051 §9).
  function tearDownMember(win: any, reason: any) {
    win._term?.dispose(reason);
    win.remove();
    budget.untrackDormancy(win);
    wins.delete(win);
    changed();
  }

  function stopPoll(entry: any) {
    if (entry?.poll) clearInterval(entry.poll);
    if (entry?.rescue) clearTimeout(entry.rescue);
  }

  // `opts.force` is the GLYPH's call only. The automatic paths (`beforeunload`,
  // the closed-poll, the peer-loss tick) stay gated on the fold because their
  // signals arrive DOUBLED. A click is an instruction that must land even when
  // the state behind the glyph is wrong. Forcing is safe against the doubled
  // signal for the same reason the fold is: the entry is deleted here.
  function reattachFence(id: any, opts: any = {}) {
    const out = detachFold(popups.detachedIds(), { type: "reattach", fenceId: id });
    const held = out.effects.some((e) => e.type === "close");
    if (!held && !opts.force) return;
    const entry = popups.entry(id);
    stopPoll(entry);
    popups.remove(id);
    commitDetached(out.registry);
    try {
      if (entry?.handle && !entry.handle.closed) entry.handle.close();
    } catch {}
    // After a reload the handle is null, so only the channel can evict the
    // popup; otherwise it keeps driving the sessions re-spawned here.
    link.post({ type: "origin-close", tab: link.tab, fenceId: id, pid: entry?.pid ?? undefined });
    // The ORIGINAL records: the popup's own layout is discarded by never having
    // been read.
    for (const m of entry?.members || []) {
      // A card comes home by RE-RENDER: its record never left the desk, and
      // `renderNotes` puts back every card whose fence is no longer detached.
      // A draft whose popup closed before its first save comes home with it.
      if (m.kind === "note") {
        const record = notes.find((n: any) => n.id === m.id);
        if (typeof m.draft === "string" && record && !record.path) {
          window.WBNotes?.adoptDraft?.(m.id, m.draft, m.claim);
        }
        continue;
      }
      // A member already on the plane is not re-spawned: two windows over one
      // session is worse than a console left away.
      if (m.id && [...wins].some((w) => w._deskId === m.id)) continue;
      // The name comes from the desk, not the snapshot: a rename on another page
      // while the fence was away must not be undone (ADR-0066 §3).
      const rec = loadDesk().find((r: any) => r.id === m.id);
      const member = rec?.consoleName ? { ...m, consoleName: rec.consoleName } : m;
      if (member.session != null) {
        spawnWindow({ id: member.session, repo: member.repo }, member.agent || "console", member.repo, member);
      } else {
        // The snapshot keeps no session id, so a member relaunched in the
        // popup comes home as a placeholder. `_revive` attaches it to the
        // session that runs now and never launches one.
        spawnPlaceholder(member)._revive();
      }
    }
    showDetachGlyph(id, false);
    renderNotes();
    applyExtent();
    refreshFenceChrome();
    WB.emit("fence-reattach", { fence: id });
  }

  // The glyph is ONE verb: bring these consoles home, whatever the registry
  // believes (raising a buried popup is the head's detach button). Close the
  // window by handle or by channel — both is fine, `origin-close` is
  // idempotent — and put the members back.
  function glyphClick(id: any) {
    reattachFence(id, { force: true });
  }

  // The POPUP's side: render the members the opener handed over, translated by
  // the fence origin to sit near this window's top-left. The untranslated
  // snapshot stays in the OPENER — nothing measured here ever goes back.
  function mountDetached(fence: any, members: any) {
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
        window.WBNotes?.mountDetached(record);
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

  // The opener's half of the handshake, guarded as `app.ts` guards the detached
  // FILE viewer's: answered only from this origin AND from a window this tab
  // itself opened.
  window.addEventListener("message", (e: any) => {
    if (e.origin !== location.origin) return;
    let owner = null;
    for (const [id, entry] of popups.entries()) if (entry.handle === e.source) owner = id;
    if (owner == null) return;
    const m = e.data;
    if (!m) return;
    if (m.type === "wb-fence-ready") {
      const entry = popups.entry(owner);
      entry.greeted = true;
      if (entry.rescue) {
        clearTimeout(entry.rescue);
        entry.rescue = null;
      }
      // `tab` rides the handover, never the popup's own storage — `window.open` gave it a COPY of ours.
      e.source.postMessage(
        { type: "wb-fence-open", fence: entry.fence, members: entry.members, tab: link.tab, pid: entry.pid },
        location.origin,
      );
    } else if (m.type === "wb-emit") {
      WB.emit(m.action, m.detail);
    } else if (m.type === "wb-note-named") {
      recordNoteName(owner, m);
    } else if (m.type === "wb-note-claimed") {
      recordNoteClaim(owner, m);
    } else if (m.type === "wb-fence-reattach") {
      // `owner`, never the message's own field: the source lookup PROVED which
      // fence this window holds; the payload could name any.
      reattachFence(owner);
    }
  });

  // The verb the shortcut calls: walk one step and jump. Returns the id landed
  // on, or null when there is no fence (so the shell leaves the key unswallowed).
  function stepFence(step: any) {
    const st = stage();
    if (!st) return null;
    const id = fenceCycle(readFenceRects(st), focusedFence, step);
    if (id == null) return null;
    return jumpToFence(id) ? id : null;
  }

  // Upsert the DOM against `fences`. The rect is re-applied unless the fence is
  // under a gesture; the NAME is not written while the operator is typing in it
  // (an in-flight GET would yank the caret to a stale value).
  function renderFences() {
    const st = stage();
    if (!st) return;
    // Index the DOM by id rather than building an attribute SELECTOR: an id is
    // daemon data (a hand-edited `desk.toml` can carry any string), and one
    // quote in it would throw a SyntaxError out of the whole restore.
    const nodes = new Map();
    for (const el of st.querySelectorAll(".fence")) nodes.set(el.dataset.fenceId, el);
    const seen = new Set();
    for (const f of fences) {
      seen.add(f.id);
      const el = nodes.get(f.id) || buildFence(f);
      const r = f.rect || {};
      if (!inGesture(el)) {
        el.style.left = (r.left || 0) + "px";
        el.style.top = (r.top || 0) + "px";
        el.style.width = (r.width || 0) + "px";
        el.style.height = (r.height || 0) + "px";
      }
      const name = el.querySelector(".fence-name");
      if (name && name !== document.activeElement) name.value = f.name || "";
      paintFenceLock(el, !!f.locked);
    }
    for (const [id, el] of nodes) {
      if (!seen.has(id)) el.remove();
    }
    // A focused fence that is gone must not leave a dangling id: the birth path
    // resolves it, and a stale one would place the next console nowhere (#343).
    if (focusedFence && !seen.has(focusedFence)) clearFenceFocus();
    // The class rides the ELEMENT, and `buildFence` makes a fresh one for a
    // fence that arrived after the focus was taken.
    else if (focusedFence) focusFence(focusedFence);
    refreshFenceChrome();
  }

  // The next default name, from the numbers ALREADY on the plane, not the
  // count: `fences.length + 1` freezes at FENCE_MAX, so every fence past the
  // cap was born "Fence 13" (MEASURED), and the fence list IS the plane's map.
  // Only `Fence <n>` counts: a rename to "backend" must not move the next
  // default, and "Fence 99" is a number the operator chose.
  function atFenceCap() {
    return fences.length >= FENCE_MAX;
  }
  function createFence() {
    // AT THE CAP, REFUSE: the daemon refuses a fence past it too.
    // Refusing and SAYING SO are two jobs: this module knows no Alpine, so it
    // answers `false` and `newFence()` in app.ts does the talking; `atFenceCap`
    // is exported so the row can be disabled BEFORE the click.
    if (atFenceCap()) return false;
    const ws = workspace();
    const offset = { left: ws?.scrollLeft || 0, top: ws?.scrollTop || 0 };
    const viewport = { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 };
    const slot = nextFenceSlot(
      fences.map((f: any) => f.rect),
      offset,
      viewport,
    );
    // The whole scanned band is full. REFUSE — do not nudge the new fence into
    // a gap the operator never chose.
    if (slot < 0) {
      const blocked = fenceSpawnRect(offset, viewport, 0);
      const hit = fences.find((x: any) => rectsOverlap(blocked, x.rect || {}));
      const el = hit && fenceEl(hit.id);
      if (el) {
        clearFenceFlash();
        el.classList.add("fence-invalid");
        fenceFlash = { el, timer: setTimeout(clearFenceFlash, 600) };
      }
      return;
    }
    const spawn = fenceSpawnRect(offset, viewport, slot);
    const id = newFenceId();
    saveFences(
      fences.concat([
        {
          id,
          // Numbered from the names on the plane, not by the slot taken.
          name: nextFenceName(fences),
          rect: spawn,
          locked: false,
        },
      ]),
    );
    renderFences();
    applyExtent();
    // A slot below the fold is still a fence the operator asked for, so travel
    // to it; a creation the screen does not acknowledge reads as a no-op. One
    // already on screen is left alone.
    const onScreen =
      spawn.left >= offset.left &&
      spawn.top >= offset.top &&
      spawn.left + spawn.width <= offset.left + viewport.width &&
      spawn.top + spawn.height <= offset.top + viewport.height;
    if (!onScreen) jumpToFence(id);
    return true;
  }

  function renameFence(id: any, name: any) {
    saveFences(
      fences.map((f: any) =>
        f.id === id
          ? { ...f, name: String(name == null ? "" : name).slice(0, FENCE_NAME_MAX) }
          : f,
      ),
    );
    renderFences();
  }

  function removeFence(id: any) {
    // Removing a DETACHED fence would destroy the glyph that brings its consoles
    // home (ADR-0051 §7a) while the registry kept a `DETACH_MAX` slot. Refuse.
    if (isDetached(id)) {
      fenceNotice(id, "Return this fence's consoles to this window first");
      WB.emit("fence-remove-refused", { fence: id, reason: "detached" });
      return;
    }
    saveFences(fences.filter((f: any) => f.id !== id));
    renderFences();
    applyExtent();
  }

  // ---- navigating the plane ----------------------------------------------------
  // ---- the fence list is the map (issue #343, ADR-0051 §7) ---------------------
  // The focused fence is PER-CLIENT transient state: never written to the desk,
  // never to `WBView`. The desk is shared last-write-wins (ADR-0051 §8), so a
  // stored focus would move where the OTHER operator's next console is born.
  let focusedFence: any = null;

  function focusedFenceId() {
    return focusedFence;
  }

  function focusFence(id: any) {
    focusedFence = id;
    const st = stage();
    if (!st) return;
    for (const el of st.querySelectorAll(".fence")) {
      el.classList.toggle("is-focused", el.dataset.fenceId === id);
    }
  }

  function clearFenceFocus() {
    focusFence(null);
  }

  // ---- the slide itself --------------------------------------------------------
  // The jump ANIMATES so the operator keeps their bearings. Hand-rolled, not
  // `scrollTo({behavior:'smooth'})`: that one's duration is the browser's, it
  // cannot be cancelled, and Chrome ignores it while a `scroll` gesture is live.
  //
  // INVARIANT: the tween is a VIEW effect only — `slideTo` runs after the
  // destination is stored, so a dropped tween never loses the jump.
  const SLIDE_MS = 260;
  let slideRaf: any = null;

  function cancelSlide() {
    if (slideRaf == null) return;
    cancelAnimationFrame(slideRaf);
    slideRaf = null;
  }

  function reducedMotion() {
    try {
      return !!window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches;
    } catch {
      return false;
    }
  }

  function slideTo(ws: any, to: any) {
    cancelSlide();
    const from = { left: ws.scrollLeft, top: ws.scrollTop };
    const dx = to.left - from.left;
    const dy = to.top - from.top;
    // Nothing to travel, no rAF available (a harness), or the operator asked the
    // OS for less motion: land now. The end state is identical either way.
    if ((!dx && !dy) || typeof requestAnimationFrame !== "function" || reducedMotion()) {
      ws.scrollLeft = to.left;
      ws.scrollTop = to.top;
      return;
    }
    const t0 = performance.now();
    const step = (now: any) => {
      slideRaf = null;
      // The viewport was torn out mid-flight (tab swapped, page reloading).
      if (!ws.isConnected) return;
      const k = slideEase((now - t0) / SLIDE_MS);
      ws.scrollLeft = from.left + dx * k;
      ws.scrollTop = from.top + dy * k;
      if (k < 1) slideRaf = requestAnimationFrame(step);
    };
    slideRaf = requestAnimationFrame(step);
  }

  // One click on a fence's name slides the viewport to it — the map's anchor.
  // Returns the fence element, or null when no fence carries that id.
  function jumpToFence(id: any) {
    const el = fenceEl(id);
    if (!el) return null;
    focusFence(id);
    // Under a maximize (a column included: the leftmost is `.maximized`) the
    // view stays put. The console covers the plane, so a slide shows nothing,
    // and `syncMaxPin` chases every frame of it (ADR-0051 §7).
    const st = stage();
    if (st?.querySelector(".session-window.maximized")) return el;
    // A fence is a REGION: its corner is anchored (ADR-0051 §7 amended).
    return jumpToEl(el, anchorIntoView);
  }

  // The note card's jump (ADR-0064 §10): the same slide, but a card is a POINT
  // OF INTEREST like a window in the Go-to picker, so it is CENTRED — the ADR
  // draws that contrast with the fence explicitly.
  function jumpToNote(id: any) {
    const el = window.WBNotes?.cardEl(id);
    if (!el) return null;
    focusWin(el);
    return jumpToEl(el, bringIntoView);
  }

  // Put `el` in view with `fold` and keep it there: everything below the two
  // jumps' own focus rule and their own fold, shared because the second
  // surface (a card) must not re-derive the stored-offset invariant the first
  // one learned the hard way.
  function jumpToEl(el: any, fold: any) {
    const ws = workspace();
    const st = stage();
    // A viewport measuring 0 is a tab still `display:none`; centring would
    // clamp to 0,0. The focus above still holds.
    if (!ws || !st || !ws.clientWidth || !ws.clientHeight) return el;
    const view = { width: ws.clientWidth, height: ws.clientHeight };
    const ext = { width: st.offsetWidth, height: st.offsetHeight };
    const to = fold(restoreRect(el), view, ext);
    slideTo(ws, to);
    // A reveal parked on an unmeasurable viewport would slide the plane off the
    // fence just jumped to; the jump is the newer request.
    pendingReveal = null;
    // INVARIANT — this write is not optional (issue #337, `revealNow`): without
    // it `refitAll`'s `applyLanding` re-applies the PRE-jump stored offset in
    // the same frame chain and silently undoes the slide.
    if (landed) {
      pendingOffset = null;
      clearTimeout(offsetFlush);
      offsetFlush = null;
      viewStore?.patch({ off: { left: to.left, top: to.top } });
    }
    return el;
  }

  // One "line" of wheel delta in pixels, for a browser that reports
  // `deltaMode: DOM_DELTA_LINE` (Firefox) instead of pixels.
  const WHEEL_LINE = 16;


  // The auto-pan loop of a drag: while the pointer presses against a viewport
  // edge, the plane scrolls under it each frame, and `place(pointer)` in the tick
  // keeps the drop correct in stage coordinates. The gesture calls `follow`
  // after each of its own `place`, and `stop` when it ends.
  function autoPan(node: any, place: any) {
    let panRaf: any = null;
    let last: any = null;
    // INVARIANT: an uncancelled loop pans the plane forever after the button
    // is released, so `stop` is the FIRST statement of each gesture's `onUp`.
    const stop = () => {
      if (panRaf != null) cancelAnimationFrame(panRaf);
      panRaf = null;
    };
    const nudge = () => {
      const ws = workspace();
      if (!ws || !last) return { dx: 0, dy: 0 };
      return panNudge(last, ws.getBoundingClientRect(), PAN_BAND, PAN_STEP);
    };
    const tick = () => {
      panRaf = null;
      // The dragged node left the page mid-drag: `place` would write styles
      // onto a detached node forever.
      if (!node.isConnected) {
        stop();
        return;
      }
      const { dx, dy } = nudge();
      if (!dx && !dy) return; // leaving the band ENDS the loop
      const ws = workspace();
      ws.scrollLeft += dx;
      ws.scrollTop += dy;
      place(last);
      panRaf = requestAnimationFrame(tick);
    };
    const follow = (pointer: any) => {
      last = pointer;
      if (panRaf != null) return;
      const { dx, dy } = nudge();
      if (dx || dy) panRaf = requestAnimationFrame(tick);
    };
    return { follow, stop };
  }

  // `visibilitychange` and `online` both land on one iOS resume; without the
  // probe seam the popup would have no verdict at all.
  let staleProbe = OPTS.isStale || null;
  function setStaleProbe(fn: any) {
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
  // `wb-view.ts`). Absent — the popup reads nothing — means auto.
  function keyBarMode() {
    return viewStore?.read()?.keys ?? null;
  }

  function applyKeyBar(win: any) {
    win.classList.toggle("keys", keyBarVisible(keyBarMode(), hasTouchSurface()));
    win._applyInputMode?.();
  }

  function fontSize() {
    return viewStore?.read()?.font ?? FONT_DEFAULT;
  }

  // Every window at once. `fit` is required: the BOX does not change, so the
  // ResizeObserver never fires and the daemon would never learn the new size.
  function setFont(px: any) {
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
  function writeClipboard(text: any, term: any) {
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
  function readClipboard() {
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
  function spawnWindow(termOpts: any, label: any, repo: any, desk?: any) {
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
    let ctrlBtn: any = null;
    let shiftBtn: any = null;
    let selBtn: any = null;

    // Debounced nudge for a keystroke typed into a parked window (#335):
    // repeated typing EXTENDS the pulse rather than stacking timers.
    let nudgeTimer: any = null;
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
    const PEER_BUTTON: any = { wake: "Wake", retry: "Try again" };
    const showPeerDown = (group: any) => {
      let strip = win.querySelector(".session-peer-down");
      if (!strip) {
        strip = document.createElement("div");
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
        btn.addEventListener("click", (e: any) => {
          e.stopPropagation();
          if (btn.dataset.act === "wake") wakePeer?.(peerDaemon);
          else readFleet?.();
        });
        strip.append(text, detail, btn);
        win.insertBefore(strip, body);
      }
      const view = peerOfflineView(group, null, win._deskEnvironment);
      strip.querySelector(".session-peer-down-text").textContent = view.text;
      const detail = strip.querySelector(".session-detail");
      detail.querySelector("p").textContent = view.detail || "";
      detail.hidden = !view.detail;
      const btn = strip.querySelector(".session-reconnect");
      btn.dataset.act = view.action || "";
      btn.hidden = !PEER_BUTTON[view.action];
      if (PEER_BUTTON[view.action]) btn.textContent = PEER_BUTTON[view.action];
    };

    // NAMED, not inline: a dormant console rebuilds its terminal (`wakeWindow`)
    // and the rebuild must be wired to the same chrome. Everything closes over
    // `win`, never a particular terminal.
    const termWiring = {
      ...termOpts,
      onCtrlLatch: (on: any) => {
        if (ctrlBtn) ctrlBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      onShiftLatch: (on: any) => {
        if (shiftBtn) shiftBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      onSelecting: (on: any) => {
        if (selBtn) selBtn.setAttribute("aria-pressed", on ? "true" : "false");
      },
      // The daemon assigned/echoed this window's session id: record it.
      onSession: (_id: any, owner: any) => {
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
        btn.addEventListener("click", (e: any) => {
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
      onEnded: (announced: any, refusal: any) => {
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
      onPeerHold: (group: any) => showPeerDown(group),
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
      win._dormantSession = termOpts.id;
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
    const relaunchIn = (checkout: any) => {
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
    restartBtn.addEventListener("click", (e: any) => {
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
      const holdFocus = (e: any) => e.preventDefault();
      bar.addEventListener("pointerdown", holdFocus);
      bar.addEventListener("mousedown", holdFocus);

      // `icon` (a Bootstrap Icons class) draws the key as that glyph; without
      // it the key shows `text`.
      const key = (name: any, text: any, title: any, cls?: any, icon?: any) => {
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
      const toggleKeyboard = (field: any) => {
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
      win._rewire = (t: any) => {
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

      bar.addEventListener("click", (e: any) => {
        const btn = e.target.closest("button[data-key]");
        if (!btn) return;
        e.stopPropagation();
        const name = btn.dataset.key;
        // The clipboard READ goes first, before any focus move: Safari grants
        // it only to a call made synchronously inside the tap. `term.paste`
        // then rides `onData → sendInput`, so a watcher's paste is refused
        // like a keystroke. A refused or empty read is dropped silently.
        // Dormant: the terminal is off and every branch below speaks to one.
        if (!win._term) return;
        const read = name === "paste" ? readClipboard() : null;
        focusWin(win);
        if (read) {
          read
            .then(({ image, text }: any) => {
              if (image) win._term.pasteImage(image);
              else if (text) win._term.term.paste(text);
            })
            .catch(() => {})
            .finally(() => win._term.term.focus());
        } else if (name === "keyboard") {
          toggleKeyboard(win._term.term.textarea);
        } else if (name === "copy") {
          writeClipboard(win._term.term.getSelection(), win._term.term);
        } else if (name === "select") {
          win._term.setSelecting(!win._term.selecting);
        } else if (name === "font-up" || name === "font-down") {
          setFont(stepFont(fontSize(), name === "font-up" ? 1 : -1));
        } else {
          win._term.sendKey(name);
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
  function recordBirth(win: any, carry: any) {
    const r = desk.find((x: any) => x.id === win._deskId);
    if (r) {
      const fields: any = {};
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
  function adoptOrphan(win: any) {
    if (!win.isConnected || !win._deskUnrecorded) return;
    // A fresh read first: the record may have landed without a push here.
    reloadDesk().then(() => {
      if (!win.isConnected || !win._deskUnrecorded) return;
      if (desk.some((r: any) => r.id === win._deskId)) return;
      recordLeft(win);
    });
  }

  // The session the daemon announced, and the worktree with it, written only
  // when they differ from the record: a reconnect changes nothing else.
  function recordSession(win: any) {
    const r = desk.find((x: any) => x.id === win._deskId);
    if (!r) return;
    const fields: any = {};
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
  function deskOf(win: any) {
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
  // yet: the shell's columns wait for them instead of dropping them.
  const relaunching = new Set();
  function isRelaunching(deskId: any) {
    return relaunching.has(deskId);
  }
  // Marks `deskId`, then takes its window off the stage. `spawnOrMissing`
  // clears the mark; a take-down that throws clears it here.
  function markRelaunch(deskId: any, takeDown: any) {
    relaunching.add(deskId);
    try {
      takeDown();
    } catch (e) {
      relaunching.delete(deskId);
      throw e;
    }
  }

  async function spawnOrMissing(req: any, label: any, repo: any, carry: any) {
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
  async function liveSessionFor(win: any, record: any) {
    await reloadDesk();
    let sessions;
    try {
      const read = await readSessions();
      if (unheardRef(record.repo, read.unheard)) return undefined;
      sessions = read.sessions;
    } catch {
      return undefined;
    }
    const layout = loadDesk();
    // Deleted by another page: this window still stands for it.
    if (!layout.some((rec: any) => rec.id === win._deskId)) {
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
  let reviving: any = null;
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
  function spawnPlaceholder(record: any, missing?: any, refused?: any, held?: any) {
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
    const BUTTON: any = { wake: "Wake", retry: "Try again", relaunch: "Relaunch" };
    let peerAction = "relaunch";
    let wasOffline = false;
    let shown: any = null;
    let detail: any = null;
    let detailText: any = null;
    const show = (view: any) => {
      shown = view;
      text.textContent = view.text;
      peerAction = view.action;
      btn.hidden = !BUTTON[view.action];
      if (BUTTON[view.action]) btn.textContent = BUTTON[view.action];
      if (!detail) {
        detail = document.createElement("details");
        detail.className = "session-detail";
        const summary = document.createElement("summary");
        summary.textContent = "Details";
        detailText = document.createElement("p");
        detail.append(summary, detailText);
        note.append(detail);
      }
      detailText.textContent = view.detail || "";
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
        if (turn === "attach") attach(session);
        else if (turn === "relaunch") btn.click();
        else show({ text: `${peerHost(peerGroups.get(daemon), record.environment)} is back.`, detail: "", action: "relaunch" });
      } finally {
        asking = false;
      }
    };
    win._peerRefresh = showPeer;

    const markMissing = (name: any) => {
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
    const drop = (respawn?: any) => {
      win.remove();
      budget.untrackDormancy(win);
      wins.delete(win);
      applyExtent();
      if (!respawn) changed();
    };
    // The attach `restoreDesk` makes, into this record's id and rect. A session
    // another window drives parks this one as a watcher (`reconnectDecision`).
    const attach = (session: any) => {
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
    let checking: any = null;
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
    btn.addEventListener("click", async (e: any) => {
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
          show(shown);
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
  function open({ repo, agent, plain, checkout, command }: any) {
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

  // Set once the saved layout has been reconciled: `restoreDesk` has three
  // callers (boot, `afterLogin`, `startNewDesk`) and the retry below, and a
  // second reconcile would spawn every window twice.
  let deskReconciled = false;
  // A desk that did not load is NOT an empty desk: reconciled against `[]`,
  // every live session that names a record is adopted at the cascade under
  // that record's id. So a transport failure reads the desk again and
  // restores only once it lands. A refused (pre-login) read retries too, harmlessly;
  // an unreadable desk waits for the operator (`startNewDesk`).
  let deskRetryMs = 1000;
  function retryDeskLoad() {
    if (deskFailure) return;
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
    Promise.all([deskReady, readSessions()])
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
        const away = new Map(); // window id -> the detached fence holding it
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
        const pending = [];
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
            const entry = popups.entry(away.get(record.id));
            if (entry && !entry.members.some((m: any) => m.id === record.id)) {
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
          } else if (action === "relaunch" && peerHeld(record.repo, peerGroups)) {
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
        document.dispatchEvent(new CustomEvent("workbench:desk-restored"));
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
  // ---- the plane's own gestures ------------------------------------------------
  // Pan by dragging the BARE FLOOR. Calls neither `applyExtent` nor a desk
  // write nor `focusWin`: panning moves the view, not the rects.
  function onFloorDown(e: any) {
    // Primary button only — see makeDraggable.
    if (e.button !== 0) return;
    const ws = workspace();
    const st = stage();
    // Element IDENTITY is the floor-vs-window hit test: a press inside a
    // console targets that window. A fence is `pointer-events: none`, so a
    // press over one still targets the stage (#340).
    if (!ws || !st || e.target !== st) return;
    // The operator's hand outranks a jump in flight.
    cancelSlide();
    // A press on the bare floor OUTSIDE the focused fence leaves it (#343); a
    // press on its own floor does not. Same half-open `rectHolds` as membership.
    if (focusedFence) {
      const el = fenceEl(focusedFence);
      const box = st.getBoundingClientRect();
      const point = { x: e.clientX - box.left, y: e.clientY - box.top };
      if (!el || !rectHolds(restoreRect(el), point)) clearFenceFocus();
    }
    const startX = e.clientX;
    const startY = e.clientY;
    const startLeft = ws.scrollLeft;
    const startTop = ws.scrollTop;
    st.classList.add("panning");
    const onMove = (ev: any) => {
      // A swallowed mouseup (native context menu, alt-tab) would leave a sticky
      // pan.
      if (ev.buttons === 0) {
        onUp();
        return;
      }
      ws.scrollLeft = startLeft - (ev.clientX - startX);
      ws.scrollTop = startTop - (ev.clientY - startY);
    };
    // INVARIANT: every exit path drops EVERY listener and the class.
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      window.removeEventListener("blur", onUp);
      st.classList.remove("panning");
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
    window.addEventListener("blur", onUp);
    // No text selection starts, and the focused terminal keeps the keyboard.
    e.preventDefault();
  }

  // The wheel. The VERTICAL axis is native `overflow:auto`; this adds only the
  // horizontal reach where the platform does not provide it.
  function onWheel(e: any) {
    // The terminal owns its wheel. Its scrollback is reached by CSS
    // (`overscroll-behavior: contain`), never by `preventDefault`, which would
    // cancel the terminal's own scroll too.
    if (e.target?.closest?.(".session-window")) return;
    // Any wheel reaching the PLANE abandons a jump in flight — the vertical
    // one too, which is why this sits above the horizontal-only guard.
    cancelSlide();
    // A platform that converts shift-wheel itself delivers `deltaX`.
    if (!(e.shiftKey && e.deltaY !== 0 && e.deltaX === 0)) return;
    const ws = workspace();
    if (!ws) return;
    // `deltaY` is only pixels when `deltaMode` says so: Firefox reports LINE
    // (±3 per notch) and does not convert shift-wheel itself.
    const px =
      e.deltaMode === 1
        ? e.deltaY * WHEEL_LINE
        : e.deltaMode === 2
          ? e.deltaY * ws.clientHeight
          : e.deltaY;
    ws.scrollLeft += px;
    e.preventDefault();
  }

  // `passive: false` or `preventDefault` is a no-op: Chrome treats a wheel
  // listener on a scroll container as passive by default.
  function wireStage() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    st.addEventListener("mousedown", onFloorDown);
    ws.addEventListener("wheel", onWheel, { passive: false });
    // Gesture, wheel, scrollbar and `reveal`'s programmatic write all end in a
    // `scroll` on the viewport: the maximize pin is derived from it.
    ws.addEventListener("scroll", syncMaxPin);
    // A SECOND listener: the pin must stay exact, the offset is debounced.
    ws.addEventListener("scroll", saveOffset);
    // The ONLY writer of the fullscreen control's look (`syncFullState`). Each
    // surface (shell, popup) runs `wireStage` in its own document.
    document.addEventListener("fullscreenchange", syncFullState);
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
  function refitAll(attempt?: any) {
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
    document.dispatchEvent(new CustomEvent("workbench:columns-stale"));
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
