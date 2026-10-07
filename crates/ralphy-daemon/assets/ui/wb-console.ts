/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Consoles tab)

   Consoles are draggable, resizable windows on the STAGE, a plane the VIEWPORT
   (`#workspace`, `overflow:auto`) scrolls over. This module owns the window
   chrome (stage-relative drag/resize/tiling, the stage's extent); the body is a
   live xterm.js on a PTY over the daemon's `/ws/session` WebSocket.

   Opening/closing a console spawns/closes a daemon-owned session; on load the
   live sessions re-open as windows, so a reload reattaches with scrollback.

   Importing this module does nothing. Each entry module calls `createConsole`
   once per page, sets `window.WBConsole`, and calls `boot` after the page has
   started; each call starts with new state (ADR-0075 D7).
--------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
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

export function createConsole(window: any, document: any, location: any, opts: any) {
  // Plane geometry is `wb-geometry.ts` (ADR-0057): pure folds over rects.
  const {
    STAGE_MARGIN,
    stageExtent,
    FENCE_MIN,
    fenceSpawnRect,
    rectsOverlap,
    rectHolds,
    fenceMembership,
    fenceOf,
    fenceFits,
    fenceMoveDelta,
    tileIntoRect,
    RESIZE_MIN,
    resizeRect,
  } = WBGeometry;

  // What a console window IS and what it HOLDS (`wb-window-state.ts`).
  const { initWindow, sessionIdOf, watchingOf, windowCheckout } = WBWindowState;

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

  // ---- dormant consoles ----------------------------------------------------
  // Every console costs an xterm buffer, a ResizeObserver, a WebGL context
  // while it holds one (`rebalanceGpu`), and the parse+paint of every byte the
  // daemon sends, visible or not.
  //
  // So a window off the viewport long enough disposes its terminal and closes
  // its socket, and rebuilds on return. A window under columns, a maximize or
  // the physical screen counts as off the viewport (ADR-0051 §9, covered
  // amendment). The SESSION is untouched — child, PTY and
  // scrollback are the daemon's (session.rs) and the reattach replays them; same
  // "dispose the terminal, keep the record" as `tearDownMember`, releasing the
  // writer slot the same way. A dormant console wakes by the ORDINARY attach and
  // never sends `takeover`, so a session claimed meanwhile lands in the parked
  // state of ADR-0051 §9.
  //
  // Dormancy is runtime state of THIS client only: never persisted, never on the
  // desk record (ADR-0050), never told to the daemon.
  //
  // One-sided on purpose: slow to sleep, instant to wake, and the margin brings a
  // window back a screenful before it could be seen — panning stays free.
  const DORMANT_AFTER_MS = 15000;
  const DORMANT_MARGIN_PX = 300;
  // Built on first use: `#workspace` is not in the document when this module
  // evaluates. Without `IntersectionObserver` the feature is inert.
  let dormancyObserver: any = null;
  function dormancyWatch() {
    if (dormancyObserver) return dormancyObserver;
    if (typeof IntersectionObserver !== "function") return null;
    const root = workspace();
    if (!root) return null;
    dormancyObserver = new IntersectionObserver(
      (entries: any) => {
        for (const entry of entries) {
          entry.target._visible = entry.isIntersecting;
          applyDormancy(entry.target);
        }
      },
      { root, rootMargin: `${DORMANT_MARGIN_PX}px`, threshold: 0 },
    );
    return dormancyObserver;
  }
  function trackDormancy(win: any) {
    const watch = dormancyWatch();
    if (watch) watch.observe(win);
    else {
      // Nothing will ever report this window seen.
      win._visible = true;
      scheduleGpu();
    }
  }
  // Paired with every `wins.delete`: the observer holds its targets, so a window
  // taken off the plane without this stays reachable for the life of the page.
  function untrackDormancy(win: any) {
    if (win._dormantTimer) {
      clearTimeout(win._dormantTimer);
      win._dormantTimer = null;
    }
    dormancyObserver?.unobserve(win);
    // Its context, if it had one, goes to the next window in line.
    scheduleGpu();
  }

  // ---- the GPU budget ------------------------------------------------------
  // LIMIT: Chrome keeps 16 live WebGL contexts per renderer process and drops
  // the oldest past it. A desk restored as a cascade has every console seen
  // and uncovered at once (measured: 20 consoles, 4 contexts lost, Chrome on
  // Windows, 2026-10-04). So the page hands out at most GPU_BUDGET contexts,
  // to the windows on top; the others draw with the DOM renderer. The budget
  // is under 16 because the detached-fence popup has its own budget and can
  // share the renderer process.
  const GPU_BUDGET = 12;

  // The indexes of the windows that hold a context, pure and tabled. Each
  // window is {seen, covered, hasTerminal, z}: only a seen, uncovered window
  // with a terminal is a candidate, and the highest `z` win (focus raises a
  // window to the top). Ties keep the input order.
  function gpuHolders(windows: any, budget: any) {
    return windows
      .map((w: any, i: any) => ({ ...w, i }))
      .filter((w: any) => w.seen && !w.covered && w.hasTerminal)
      .sort((a: any, b: any) => b.z - a.z)
      .slice(0, budget)
      .map((w: any) => w.i);
  }

  // Coalesced: a restore asks once per window, and the drops must run before
  // the loads so the page never holds more than the budget.
  let gpuQueued = false;
  function scheduleGpu() {
    if (gpuQueued) return;
    gpuQueued = true;
    queueMicrotask(() => {
      gpuQueued = false;
      rebalanceGpu();
    });
  }
  function rebalanceGpu() {
    const list = [...wins];
    const keep = new Set<any>(
      gpuHolders(
        list.map((w) => ({
          // `=== true`, not the dormancy fold's reading: an unobserved window
          // is not yet seen.
          seen: w._visible === true,
          covered: isCovered(w),
          hasTerminal: !!w._term,
          z: parseInt(w.style.zIndex, 10) || 0,
        })),
        GPU_BUDGET,
      ).map((i: any) => list[i]),
    );
    for (const w of list) if (!keep.has(w)) w._term?.dropGpu();
    for (const w of keep) w._term.useGpu();
  }

  // Focus stacking. `z` climbs each time a window is raised; when it reaches the
  // ceiling the whole stack is renormalized back down (preserving order) so the
  // console z-index never overtakes the runs overlay (z 150) or the tabbar.
  const Z_BASE = 60;
  const Z_CEIL = 120;
  let z = Z_BASE;
  let cascade = 0;

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
  const DESK_MAX = 30;
  const sync = WBDeskSync.createSync();
  let desk: any = [];
  // Second record type (#340): named rectangles on the floor tier.
  const FENCE_MAX = 12;
  let fences: any = [];
  // Third record type (ADR-0064 §2): note cards, PLACEMENT only — the note's
  // text and colour live in its `.note` file. The CARD itself (DOM, editor,
  // autosave) is `wb-notes.ts`, which reaches this state through the exports
  // below.
  const NOTE_MAX = 32;
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

  // The elements under a gesture of the operator, from the press to the
  // release: windows, fences, cards, and every member a fence move carries.
  // A desk this page takes never moves one of them.
  const gestures = new Set();
  function inGesture(el: any) {
    return gestures.has(el);
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
    scheduleGpu();
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
    // NOT called here: this runs during module evaluation and `keyboardInset`
    // reads a `const` declared below — its temporal dead zone would throw out
    // of the whole IIFE. `var(--kb-inset, 0px)` already means "no keyboard".
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

  // The restore decision, a pure fold of the saved layout over the live session
  // list. Each live session is consumed by AT MOST ONE record (first in layout
  // order): a restarted daemon reuses ids, hence the full
  // `sessionId`+`repo`+`agent`+`kind` tuple.
  //
  // `relaunchAgents` is the operator's per-client opt-in (Settings → Consoles),
  // default OFF and passed in so the fold stays pure and the popup can never
  // turn it on.
  function reconcileDesk({ layout, sessions, relaunchAgents = false }: any) {
    const live = sessions || [];
    const used = new Set();
    const out: any = [];
    for (const record of layout || []) {
      // A session that names its record is that record's, whatever a stale
      // `sessionId` says, and never another record's (ADR-0050 amendment
      // 2026-10-04). The tuple below is for sessions from an older daemon.
      let i = live.findIndex((s: any, idx: any) => !used.has(idx) && s.record === record.id);
      if (i < 0) {
        i = live.findIndex(
          (s: any, idx: any) =>
            !used.has(idx) &&
            s.record == null &&
            s.id === record.sessionId &&
            s.repo === record.repo &&
            s.agent === record.agent &&
            s.kind === record.kind,
        );
      }
      if (i >= 0) {
        used.add(i);
        out.push({ record, session: live[i], action: "attach" });
      } else {
        // A shell is free and idempotent, so it comes back by itself; an agent
        // console waits for a click — loading a page must never spawn a vendor
        // CLI and spend quota nobody authorized. Only `relaunchAgents` lifts
        // this. The placeholder's button is a LAUNCH, not a reconnect: the old
        // PTY and its scrollback are gone.
        out.push({
          record,
          session: null,
          action: record.kind === "console" || relaunchAgents ? "relaunch" : "placeholder",
        });
      }
    }
    // A live session no record claims first looks for the record waiting for
    // it (a placeholder or would-be relaunch on the same repo, vendor, kind and
    // worktree): its `sessionId` was lost to a lost flush or reissued by a
    // restarted daemon, and attaching there keeps one console from coming back
    // as two — or, for a shell, from spawning a SECOND PTY. Only with no such
    // record is it adopted into a fresh one, so it stays visible and closable.
    // The ids an adopted window may take: one window per record id.
    const taken = new Set((layout || []).map((r: any) => r.id));
    live.forEach((s: any, idx: any) => {
      if (used.has(idx)) return;
      // A session that names a record this page has not read is adopted under
      // that id, so the page that launched it and this one write one record.
      if (s.record != null) {
        const id = taken.has(s.record) ? null : s.record;
        if (id) taken.add(id);
        out.push({ record: null, session: s, action: "adopt", id });
        return;
      }
      const waiting = out.find(
        ({ record, action }: any) =>
          action !== "attach" &&
          // An `adopt` entry pushed for an earlier session has no record.
          record != null &&
          record.repo === s.repo &&
          record.agent === s.agent &&
          record.kind === s.kind &&
          (record.checkout ?? null) === (s.checkout ?? null),
      );
      if (waiting) {
        waiting.session = s;
        waiting.action = "attach";
        return;
      }
      out.push({ record: null, session: s, action: "adopt", id: null });
    });
    return out;
  }

  // The live session a placeholder should attach to, or null: `reconcileDesk`'s
  // own verdict for `recordId`, so a session another record owns is never taken.
  // `layout` is a FRESH desk — another device may have relaunched this record
  // and written its new `sessionId` there. `held` lists the `{id, repo}` of the
  // sessions this page already shows: attaching one again would be a second
  // window on one session. Ids repeat across repos and peers, hence the pair.
  function placeholderSession({ layout, sessions, recordId, held = [] }: any) {
    const shown = (s: any) =>
      held.some(
        (h: any) =>
          h.id === s.id && (h.repo === "~" ? !s.repo || s.repo === "~" : s.repo === h.repo),
      );
    const verdict = reconcileDesk({
      layout,
      sessions: (sessions || []).filter((s: any) => s && !shown(s)),
    }).find(({ record }: any) => record?.id === recordId);
    return verdict?.action === "attach" ? verdict.session : null;
  }

  // The launch request a desk record relaunches with (#411). The daemon labels
  // a repo-less console "~"; sent back as a slug it hits `unknown repo`, so it
  // relaunches with no repo. An AGENT record asks for its vendor and worktree —
  // `{ console: true }` is the shell request. The checkout rides ONLY on the
  // agent request: the plain console stays on the primary (the `open` rule).
  function relaunchRequest(record: any) {
    const repo = record.repo === "~" ? undefined : record.repo;
    if (record.kind !== "agent") return { console: true, repo, command: consoleCommand(record.agent) };
    return { repo, agent: record.agent, checkout: record.checkout ?? null };
  }

  // The name a console box uses for the host of a peer project: the machine
  // name of a tunnel peer, else the environment (`WSL: Ubuntu`). `fallback` is
  // the environment the desk record kept, for a box drawn before the fleet list.
  function peerHost(group: any, fallback: any) {
    return WBFleet.peerName(group) || fallback || "The other computer";
  }

  // What a console box says about a project whose peer cannot serve it, from
  // that peer's fleet state. `group` is its fleet group (wb-fleet.ts), or null
  // before the fleet list arrived; `refusal` is the daemon's sentence from a
  // refused launch. `action` is one of
  //   "wake"  — a nudge can answer this state: wake the peer, then relaunch;
  //   "retry" — launch again;
  //   "wait"  — the daemon is already opening the tunnel again;
  //   null    — no click here fixes it (a token, a version, a descriptor).
  // The daemon's diagnosis is the detail: it names the cause and the remedy.
  function peerOfflineView(group: any, refusal: any, fallbackHost: any) {
    const host = peerHost(group, fallbackHost);
    const detail = (group && group.diagnosis) || (typeof refusal === "string" ? refusal.trim() : "");
    const view = (text: any, action: any) => ({ text, detail, action });
    const wakeOrRetry = WBFleet.wakeable(group) ? "wake" : "retry";
    switch (group && group.state) {
      case "asleep":
        return view(`${host} is asleep.`, wakeOrRetry);
      case "unreachable":
        return view(`Ralphy is not running on ${host}.`, wakeOrRetry);
      case "tunnel-closed":
        return view(`Reconnecting to ${host}…`, "wait");
      case "tunnel-silent":
        return view(`${host} does not answer.`, "retry");
      case "unauthorized":
      case "version-mismatch":
      case "refused":
      case "malformed":
        return view(`${host} cannot open this console.`, null);
      default:
        // Reachable or not known yet: the fleet has not seen what the launch saw.
        return view(`${host} did not start this console.`, "retry");
    }
  }

  // What a peer placeholder does on a fleet read. `available` and `offline`
  // are both false while the peer's state is unknown. Returns one of
  //   "relaunch" — the peer is back and this is a shell: open it again;
  //   "offer"    — the peer is back: say so and leave the click to the
  //                operator, because a resume never launches a vendor CLI;
  //   "stay"     — keep the box as it is.
  // Only a peer this box SAW offline counts as back. A refused launch on a
  // peer the fleet still calls reachable would otherwise relaunch, be refused,
  // and relaunch again.
  function peerReturnDecision({ kind, canLaunch, available, wasOffline }: any) {
    if (!available || !wasOffline) return "stay";
    return kind === "console" && canLaunch ? "relaunch" : "offer";
  }

  // What a box restored while the session list did not hear from its peer
  // does once that peer is available, from `liveSessionFor`'s answer:
  //   "attach"   — the console still runs there;
  //   "relaunch" — the list heard from the peer and it does not run: a shell
  //                opens again, as a restore would have opened it;
  //   "offer"    — the same for an agent console: the click is the operator's;
  //   "stay"     — still not known (`undefined`): ask again on the next read.
  function heldReturnDecision({ kind, canLaunch, session }: any) {
    if (session === undefined) return "stay";
    if (session) return "attach";
    return kind === "console" && canLaunch ? "relaunch" : "offer";
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
  function unheardRef(ref: any, unheard: any) {
    const daemon = WBFleet.refDaemon(ref);
    return !!daemon && !!unheard?.has(daemon);
  }

  // The fleet group of `ref`'s peer when that peer cannot serve it, else null:
  // a local ref, an unknown peer, and a reachable one all launch as usual.
  function peerHeld(ref: any, groups: any) {
    const daemon = WBFleet.refDaemon(ref);
    const group = daemon ? groups.get(daemon) : null;
    return group && !WBFleet.available(group) ? group : null;
  }

  // A console-kind session's `agent` label is its startup command, or the
  // literal `console` for the bare shell (the daemon labels it so on launch).
  // So the label alone says how to launch that console again.
  function consoleCommand(label: any) {
    return label && label !== "console" ? label : undefined;
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
  function isUnknownCheckout(reply: any) {
    return !!reply && reply.status === "error" && reply.message === "unknown checkout";
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

  // Pure. What one window is, given the painted consoles. `maximized: null`
  // means "not a column: leave its maximize alone". Two rows of one column
  // are columns too: what counts is how many consoles are painted.
  function columnClasses(painted: any, id: any) {
    const list = painted || [];
    const entry = list.find((p: any) => p.id === id);
    if (!entry) return { column: false, maximized: null };
    return { column: list.length >= 2, maximized: entry.index === 0 && !entry.row };
  }

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
    for (const [id, entry] of fencePopups) {
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
  // Is this console held by a LOCKED fence? Never in the popup (`autoBoot:
  // false`): `mountDetached` re-origins the members' rects into that window
  // while `fences` keeps the shell's stage coordinates, so the fold would
  // match a console to whatever fence covers the translated point. A detached
  // fence's members move freely there; the fence's lock holds again on the
  // stage when they come home. Note cards follow the same rule.
  function fenceHolds(records: any, rect: any, popup: any) {
    return !popup && !!fenceOf(records, rect)?.locked;
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
    scheduleGpu();
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

  // The session the window holds, on a `/api/sessions` listing: the daemon's
  // id AND the repo ref, because a restarted daemon hands out ids from 1
  // again and a peer's id 1 is not this daemon's (the ref carries the peer).
  function sessionRowFor(win: any, sessions: any) {
    const id = sessionIdOf(win);
    if (id == null) return null;
    const ref = win._deskRepo;
    return (
      (sessions || []).find(
        (s: any) => s && s.id === id && (ref === "~" ? !s.repo || s.repo === "~" : s.repo === ref),
      ) || null
    );
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

  // Drag by the titlebar, clamped to the STAGE (control buttons still click).
  // Coordinates are plane pixels: the stage's client rect carries the viewport's
  // scroll shift. The origin is pinned at 0, so no drag writes a negative
  // left/top. POINTER, not mouse: iOS synthesizes mouse events only AFTER a tap
  // resolves, never during a drag, so a `mousedown` titlebar fell through to
  // text selection. `touch-action: none` on the handle is required — without
  // it the browser claims the gesture as a scroll and fires `pointercancel`.
  // `opts` is the note card's seam (ADR-0064 §8): a card drags by the same
  // gesture — threshold, auto-pan, Escape, the lot — but it is locked by its
  // own record and persisted into `notes`, not into `desk`. Absent, the two
  // hooks are the window's, which is every existing caller.
  function makeDraggable(win: any, handle: any, opts?: any) {
    const heldFast = opts?.locked || (() => isLocked(win));
    // The rect at the end of the drag, computed at the act.
    const persist = opts?.onDrop || (() => setWin(win, { rect: restoreRect(win) }));
    handle.addEventListener("pointerdown", (e: any) => {
      if (e.target.closest("button, .session-name-input")) return;
      // Primary button only: a right/middle press is followed by `contextmenu`
      // (or no `pointerup`), stranding `onMove` on the document. `isPrimary` is
      // the touch half: a second finger opens its own stream.
      if (e.button !== 0 || !e.isPrimary) return;
      const pointerId = e.pointerId;
      focusWin(win);
      // No drag while maximized (double-click still restores) or fullscreen —
      // the top layer ignores the move while the drag REWRITES the inline rect.
      if (win.classList.contains("maximized") || win.classList.contains("column") || isFull(win)) return;
      // Locked in place — by its own record or by the fence holding it.
      if (heldFast()) return;
      gestures.add(win);
      const rect = win.getBoundingClientRect();
      const offX = e.clientX - rect.left;
      const offY = e.clientY - rect.top;
      // Armed only past `dragThreshold`: until then a press is a tap that
      // focuses and nothing more. `offX/offY` were taken above, so the first
      // placement after arming lands the full distance from the grab point.
      const threshold = dragThreshold(e.pointerType);
      const pressed = { x: e.clientX, y: e.clientY };
      let armed = false;
      // Put the window under `pointer` (a CLIENT point). Read the stage LIVE:
      // `applyExtent({grow:true})` widens it, and a wheel or auto-pan mid-drag
      // shifts its origin under a cached rect.
      const place = (pointer: any) => {
        const st = stage();
        const origin = st.getBoundingClientRect();
        const x = pointer.x - origin.left - offX;
        const y = pointer.y - origin.top - offY;
        win.style.left = Math.max(0, Math.min(x, st.offsetWidth - rect.width)) + "px";
        win.style.top = Math.max(0, Math.min(y, st.offsetHeight - rect.height)) + "px";
        applyExtent({ grow: true });
      };
      // Holding the window against the viewport edge scrolls the plane under it.
      // A window closed mid-drag ends the loop: nothing is left to carry.
      const pan = autoPan(win, place);
      const onMove = (ev: any) => {
        // Another pointer's stream (a second finger, the mouse during a touch drag).
        if (ev.pointerId !== pointerId) return;
        // `pointerup` is NOT guaranteed: a native context menu mid-drag or an
        // alt-tab with the button held swallows it, and the next pointerdown
        // installs its OWN closures, so nothing later could remove this pair.
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const last = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins(pressed, last, threshold)) return;
          armed = true;
        }
        place(last);
        pan.follow(last);
      };
      // Escape ends the drag where the window sits — no revert: a keyboard exit
      // from a loop whose mouseup may never arrive.
      const onKey = (ev: any) => {
        if (ev.key === "Escape") onUp();
      };
      const onUp = () => {
        pan.stop();
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch drag that the system takes over (an edge swipe, a call coming
        // in) ends in `pointercancel` and NEVER in `pointerup`.
        document.removeEventListener("pointercancel", onUp);
        document.removeEventListener("contextmenu", onUp);
        document.removeEventListener("keydown", onKey);
        window.removeEventListener("blur", onUp);
        applyExtent();
        // A tap persists NOTHING: it moved nothing.
        if (armed) persist();
        gestures.delete(win);
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      // `blur`: a native menu or another window took focus and the pointer
      // never comes back to deliver `buttons === 0`.
      window.addEventListener("blur", onUp);
      // `contextmenu`: a menu over the page ends the gesture whether or not the
      // browser also blurs; a double `onUp` is idempotent.
      document.addEventListener("contextmenu", onUp);
      document.addEventListener("keydown", onKey);
      e.preventDefault();
    });
  }

  // The titlebar's touch gestures. A mouse keeps `dblclick`; a finger or a pen
  // gets two taps → `onDoubleTap` (anywhere but a button, the name included),
  // and a hold on the name → the rename. `win._lastPointerType` tells both
  // `dblclick` listeners which input made the press, so a browser that also
  // turns two taps into a `dblclick` (Chrome on Android) does not act twice.
  // The rename opens on the RELEASE after the hold, not when the timer fires:
  // iOS raises the keyboard only for a `focus()` inside an input event.
  function wireTitleTouch(win: any, titlebar: any, onDoubleTap: any) {
    // Seen on an iPhone (2026-10-03): the double tap that maximized also
    // zoomed the page, although the bar has `touch-action: none`. Safari does
    // not zoom on a double tap whose `touchend` is cancelled. A button's
    // `touchend` is not cancelled: it is what makes the button's `click`.
    titlebar.addEventListener(
      "touchend",
      (e: any) => {
        if (!e.target.closest("button, .session-name-input")) e.preventDefault();
      },
      { passive: false },
    );
    titlebar.addEventListener("pointerdown", (e: any) => {
      win._lastPointerType = e.pointerType;
      if (e.pointerType === "mouse" || !e.isPrimary) return;
      if (e.target.closest("button, .session-name-input")) return;
      // No compatibility mouse events: a `mousedown` after the release would
      // take focus from the new name input, and its `blur` ends the edit.
      e.preventDefault();
      const pointerId = e.pointerId;
      const pressed = { x: e.clientX, y: e.clientY };
      const threshold = dragThreshold(e.pointerType);
      const span = canRename() ? e.target.closest(".session-name") : null;
      let moved = false;
      let held = false;
      let timer: any = span
        ? setTimeout(() => {
            timer = null;
            held = true;
          }, HOLD_MS)
        : null;
      const onMove = (ev: any) => {
        if (ev.pointerId !== pointerId) return;
        // `pointerup` is not guaranteed (see `makeDraggable`).
        if (ev.buttons === 0) {
          finish(null);
          return;
        }
        if (!moved && dragBegins(pressed, { x: ev.clientX, y: ev.clientY }, threshold)) {
          moved = true;
          held = false;
          clearTimeout(timer);
          timer = null;
        }
      };
      const onUp = (ev: any) => {
        if (ev.pointerId === pointerId) finish(ev);
      };
      const onCancel = (ev: any) => {
        if (ev.pointerId === pointerId) finish(null);
      };
      // Android fires `contextmenu` on a held finger; its menu is not ours.
      const onMenu = (ev: any) => ev.preventDefault();
      const finish = (up: any) => {
        clearTimeout(timer);
        timer = null;
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("pointercancel", onCancel);
        document.removeEventListener("contextmenu", onMenu, true);
        if (!up || moved) {
          win._lastTap = null;
          return;
        }
        if (held) {
          win._lastTap = null;
          startRename(win, span);
          return;
        }
        const tap = { t: up.timeStamp, x: up.clientX, y: up.clientY };
        if (isDoubleTap(win._lastTap, tap) && !isFull(win)) {
          win._lastTap = null;
          onDoubleTap();
        } else {
          win._lastTap = tap;
        }
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onCancel);
      document.addEventListener("contextmenu", onMenu, true);
    });
  }

  // ---- resize geometry ---------------------------------------------------------
  // One pure function for all eight directions (`resizeRect`, wb-geometry):
  // east/south move the far edge; west/north move `left`/`top` and derive the
  // size, so the OPPOSITE edge stays put.
  const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];


  // The repos a fence's members belong to, for the fence's chrome. Deduped,
  // sorted (DOM order is not stable), `"~"` rendered as `home` like `list()`.
  function fenceRepos(members: any) {
    const names = new Set((members || []).map((m: any) => (m?.repo === "~" ? "home" : m?.repo)));
    names.delete(undefined);
    names.delete(null);
    names.delete("");
    return [...names].sort().join(" · ");
  }

  // One fence readout for BOTH the fence's chrome and the toolbar list (#343):
  // `[{id, name, rect}]` + `[{id, repo, rect}]` in, one entry per fence IN ORDER
  // out. Folding membership here is what makes "the list and the fence never
  // disagree" a property of the code.
  function fenceSummaries(fences: any, windows: any) {
    const list = fences || [];
    const all = windows || [];
    const byId = new Map(all.map((w: any) => [w?.id, w]));
    const membership = fenceMembership(list, all);
    return list.map((f: any) => {
      const members = (membership[f.id] || []).map((wid: any) => byId.get(wid));
      return {
        id: f.id,
        name: f.name || "",
        count: members.length,
        repos: fenceRepos(members),
        locked: !!f.locked,
      };
    });
  }

  // Which grid slot a NEW fence takes: the first no existing fence occupies.
  // Indexing by `fences.length` would reuse a slot after a removal and land on
  // a survivor (the overlap ADR-0051 §6 does not enforce away). The scan runs
  // PAST the fence count: one big fence covering the viewport occupies the
  // first `taken.length + 1` slots while free plane sits below. `-1` means
  // nowhere, and the caller refuses rather than nudging into a gap.
  const FENCE_SLOT_SCAN = 64;
  function nextFenceSlot(rects: any, offset: any, viewport: any) {
    const taken = rects || [];
    const cap = Math.max(taken.length, FENCE_SLOT_SCAN);
    for (let i = 0; i <= cap; i++) {
      const candidate = fenceSpawnRect(offset, viewport, i);
      if (!taken.some((t: any) => rectsOverlap(candidate, t))) return i;
    }
    return -1;
  }

  // ---- the fence floor ---------------------------------------------------------
  // A fence is a stage child BELOW every window (`z-index: 1` against `Z_BASE`)
  // with `pointer-events: none`, so "never intercepts a window drag, resize or
  // focus click" holds by construction and `onFloorDown`'s `e.target !== st`
  // test keeps panning alive inside a fence. Only the name field, the two tool
  // buttons and the eight resize bands opt back in.
  const FENCE_NAME_MAX = 60;
  // Reuses `DIRS` so `resizeRect` answers all eight; its west/north legs clamp
  // at the pinned origin (no negative coordinate, ADR-0051 §2).
  const FENCE_DIRS = DIRS;

  function buildFence(f: any) {
    const el = document.createElement("div");
    el.className = "fence";
    el.dataset.fenceId = f.id;
    const head = document.createElement("div");
    head.className = "fence-head";
    // Two SMALL opt-in handles: a full-width interactive head would swallow the
    // floor's own pan (`onFloorDown` bails unless the press targets the stage).
    const grab = document.createElement("span");
    grab.className = "fence-grab";
    grab.title = "Move this fence";
    grab.textContent = "⠿";
    grab.addEventListener("pointerdown", startFenceMove(el, f));
    const name = document.createElement("input");
    name.className = "fence-name";
    name.setAttribute("aria-label", "Fence name");
    name.value = f.name || "";
    // READ-ONLY until double-clicked: the field lives in a bar the operator
    // also clicks to raise, focus and drag, and an always-live input turns a
    // slip into a rename. No `title`, no hover affordance (`.fence-name`): a
    // read-only field that lights up reads as editable.
    //
    // `pristine` is what a cancel returns to; `endEdit` is the ONE place an
    // edit ends. Committing on `change` (fires on blur) meant clicking away
    // SAVED with nothing to undo it. Now:
    //   Enter  → commit
    //   Escape → cancel, restoring the name
    //   a press anywhere else → cancel, restoring the name
    let pristine = name.value;
    let editing = false;
    // A document-level pointerdown, not just `blur`: the plane's pan handler
    // calls `preventDefault()` on mousedown, so pressing the stage does NOT move
    // focus (MEASURED). Capture phase, before the floor's handler swallows it.
    const stopOutside = (e: any) => {
      if (e.target !== name) endEdit(false);
    };
    const endEdit = (commit: any) => {
      if (!editing) return;
      editing = false; // first: the `blur()` below re-enters through the handler
      document.removeEventListener("pointerdown", stopOutside, true);
      name.readOnly = true;
      if (!commit) name.value = pristine;
      // Collapse the selection `select()` made. MEASURED: neither `blur()` nor
      // re-assigning `.value` (same string = no-op) clears it, so after Escape
      // the name stayed highlighted on a field nobody was editing.
      name.setSelectionRange(0, 0);
      name.blur();
      // Last, because it re-renders the fence and replaces this very input.
      if (commit) renameFence(f.id, name.value);
    };
    name.readOnly = true;
    // A single click leaves NO trace: prevented, it neither focuses the field
    // nor starts a selection. `dblclick` still arrives (cancelling a mousedown
    // default does not cancel the click pair). Once editing, the guard steps
    // aside.
    name.addEventListener("mousedown", (e: any) => {
      if (name.readOnly) e.preventDefault();
    });
    name.addEventListener("dblclick", () => {
      if (editing) return;
      pristine = name.value;
      editing = true;
      name.readOnly = false;
      name.focus();
      name.select();
      // Attached here, so the pointerdown that OPENED the edit is already past.
      document.addEventListener("pointerdown", stopOutside, true);
    });
    name.addEventListener("keydown", (e: any) => {
      if (e.key !== "Enter" && e.key !== "Escape") return;
      // Held at the field so an Escape meant for this edit never also reaches the
      // plane's own key handlers.
      e.stopPropagation();
      endEdit(e.key === "Enter");
    });
    // A real focus loss (Tab, the window losing focus) cancels too — same rule, and
    // `endEdit` is idempotent, so the `blur()` inside it lands here harmlessly.
    name.addEventListener("blur", () => endEdit(false));
    // Name · count · arrange (#342). Filled by `refreshFenceChrome`. No repo
    // list in the chrome: a fence is not bound to a project (ADR-0051 §6);
    // `fenceSummaries` still folds it.
    const count = document.createElement("span");
    count.className = "fence-count";
    // A fence verb's refusal belongs on the fence (`WB.emit` only reaches the
    // console; no toast surface). Filled and cleared by `fenceNotice`.
    const notice = document.createElement("span");
    notice.className = "fence-notice";
    head.append(grab, name, count, notice);
    // Arrange and close take the fence's TOP-RIGHT corner, like every other
    // closable surface here; trailing the head made their position a function
    // of the name's length.
    const tools = document.createElement("div");
    tools.className = "fence-tools";
    const tile = document.createElement("button");
    tile.className = "fence-arrange";
    tile.type = "button";
    tile.title = "Tile this fence's consoles";
    tile.textContent = "⊞";
    tile.addEventListener("click", async () => {
      // A detached fence has nothing here to tile; `arrangeFence` says so, so
      // the question is not put to the operator.
      if (detached.includes(f.id)) return arrangeFence(f.id);
      const ok = await askConfirm({
        title: "Tile this fence?",
        message: `Rearranges the consoles in ${f.name || "this fence"}. Sessions keep running.`,
        confirmLabel: "Tile",
      });
      if (ok) arrangeFence(f.id);
    });
    const drop = document.createElement("button");
    drop.className = "fence-drop";
    drop.type = "button";
    drop.title = "Remove this fence";
    drop.textContent = "×";
    drop.addEventListener("click", async () => {
      // A DETACHED fence refuses removal and says why; no question first.
      if (detached.includes(f.id)) return removeFence(f.id);
      const ok = await askConfirm({
        title: "Remove this fence?",
        message: `Removes ${f.name || "this fence"}. Consoles stay where they are.`,
        confirmLabel: "Remove",
        danger: true,
      });
      if (ok) removeFence(f.id);
    });
    const detach = document.createElement("button");
    detach.className = "fence-detach";
    detach.type = "button";
    detach.title = "Detach this fence into its own window";
    detach.textContent = "⧉";
    detach.addEventListener("click", () => detachFence(f.id));
    // Lock in place: the fence and every console it holds refuse a drag. Glyph
    // painted by `paintFenceLock` from `renderFences`.
    const lock = document.createElement("button");
    lock.className = "fence-lock";
    lock.type = "button";
    lock.addEventListener("click", () => setFenceLock(f.id, !fenceLocked(f.id)));
    // Every member as columns, in a grid that follows the stage (ADR-0051 §5,
    // 2026-09-30). The shell owns the columns, so this only names the members
    // and their stage rects. `refreshFenceChrome` paints disabled and hidden.
    const columns = document.createElement("button");
    columns.className = "fence-columns";
    columns.type = "button";
    columns.title = "Open this fence's consoles as columns";
    columns.innerHTML = '<i class="bi bi-layout-three-columns"></i>';
    columns.addEventListener("click", () => {
      const st = stage();
      if (!st || detached.includes(f.id)) return;
      const all = readWindowRects(st);
      const ids = new Set(fenceMembership(readFenceRects(st), all)[f.id] || []);
      const items = all.filter((w) => ids.has(w.id)).map((w) => ({ id: w.id, rect: w.rect }));
      if (!items.length) return;
      document.dispatchEvent(new CustomEvent("workbench:fence-columns", { detail: { items } }));
    });
    // BETWEEN arrange and close: close stays the OUTERMOST control
    // (wb_fence_342.py asserts exactly that).
    tools.append(tile, columns, lock, detach, drop);
    // What tells an EMPTIED fence from an empty one (ADR-0051 §7a): a detached
    // fence keeps its name, rect and list entry and carries this glyph; clicking
    // it brings the consoles home. `hidden` here AND in the stylesheet: an
    // author `display` beats the UA's `[hidden]` rule.
    const away = document.createElement("div");
    away.className = "fence-detached";
    away.title = "Return this fence's consoles to this window";
    away.textContent = "⧉";
    away.hidden = true;
    away.addEventListener("click", () => glyphClick(f.id));
    // Every edge and corner resizes. Only the SE handle (`.fence-grip`) is
    // visible; the other seven announce themselves through the cursor.
    const handles = FENCE_DIRS.map((dir) => {
      const h = document.createElement("div");
      h.className = dir === "se" ? "fence-edge fence-grip" : "fence-edge";
      h.dataset.dir = dir;
      h.title = "Resize this fence";
      h.addEventListener("pointerdown", startFenceResize(el, f, dir));
      return h;
    });
    // ORDER IS THE HIT TEST: the bands overlap the head and the tools and all
    // take pointer events; later siblings win, so the interactive clusters go
    // last or the north band eats the name field and the close button.
    el.append(...handles, head, tools, away);
    stage()?.append(el);
    return el;
  }

  // ---- the fence gestures (issue #341) -----------------------------------------
  // Both gestures read the fence's rect from the DOM, never from the captured
  // `f`: `f.rect` goes stale the first time the fence moves. Only `f.id` is
  // taken from the closure.
  //
  // INVARIANT, on EVERY exit path (mouseup, the `ev.buttons === 0` recovery,
  // `window` blur): the document listeners are removed and the gesture is
  // finalized EXACTLY ONCE, accepted or refused. `done` makes a doubled exit
  // a no-op.
  //
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

  function startFenceMove(el: any, f: any) {
    return (e: any) => {
      if (e.button !== 0 || !e.isPrimary) return; // primary button only — see makeDraggable
      if (fenceLocked(f.id)) return;
      const st = stage();
      if (!st) return;
      const pointerId = e.pointerId;
      const start = restoreRect(el);
      // Membership is computed ONCE, at mousedown, and frozen for the gesture:
      // recomputing per move makes windows join and leave under the cursor as
      // the fence sweeps the plane, and the drop would carry a set nobody chose.
      // Windows AND cards: a fence carries every surface whose centre it holds
      // (ADR-0064 §8). The ids are NAMESPACED because the two collections are
      // keyed independently and `fenceMembership` sees one flat list.
      const all = [...st.querySelectorAll(".session-window")]
        .map((w) => ({ el: w, id: "w:" + w._deskId, kind: "window", rect: restoreRect(w) }))
        .concat(
          [...st.querySelectorAll(".note-card")].map((el) => ({
            el,
            id: "n:" + el.dataset.noteId,
            kind: "note",
            rect: restoreRect(el),
          })),
        );
      // The FULL fence list, not a singleton: the fold's `break` decides an
      // overlapping pair (reachable via a hand-edited `desk.toml`), and a
      // singleton bypasses it.
      const live = fences.map((x: any) => (x.id === f.id ? { id: x.id, rect: start } : x));
      const ids = new Set(fenceMembership(live, all)[f.id] || []);
      const carried = all.filter((m) => ids.has(m.id));
      gestures.add(el);
      for (const m of carried) gestures.add(m.el);
      const startX = e.clientX;
      const startY = e.clientY;
      // The plane's origin AT MOUSEDOWN: auto-pan scrolls the viewport
      // mid-gesture and slides the stage under a stationary cursor, so every
      // delta is taken against the LIVE origin (as `makeDraggable`'s `place`).
      const origin0 = st.getBoundingClientRect();
      let delta = { dx: 0, dy: 0 };
      let fits = true;
      let done = false;
      // A press with no movement is a CLICK, not a drop: it changes nothing,
      // so it writes nothing. `armed` is the same rule with a width
      // (`dragThreshold`).
      let moved = false;
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      clearFenceFlash();
      const place = (pointer: any) => {
        const origin = st.getBoundingClientRect();
        const d = fenceMoveDelta(
          {
            dx: pointer.x - startX + (origin0.left - origin.left),
            dy: pointer.y - startY + (origin0.top - origin.top),
          },
          start,
          carried.map((m) => m.rect),
        );
        const rect = { ...start, left: start.left + d.dx, top: start.top + d.dy };
        fits = fenceFits(fences, { id: f.id, rect });
        el.classList.toggle("fence-invalid", !fits);
        el.style.left = rect.left + "px";
        el.style.top = rect.top + "px";
        // A refused position previews the FENCE (that is the feedback) but never
        // the members: dragging over a neighbour must not shuffle its windows.
        if (d.dx || d.dy) moved = true;
        if (fits) {
          delta = d;
          for (const m of carried) {
            m.el.style.left = m.rect.left + d.dx + "px";
            m.el.style.top = m.rect.top + d.dy + "px";
            // A card on top floats elsewhere; its place is the shadow.
            if (m.el._noteShadow) {
              m.el._noteShadow.style.left = m.el.style.left;
              m.el._noteShadow.style.top = m.el.style.top;
            }
          }
        }
        applyExtent({ grow: true });
      };
      // Holding the fence against a viewport edge scrolls the plane under it. A
      // fence re-rendered or removed mid-drag ends the loop.
      const pan = autoPan(el, place);
      const onMove = (ev: any) => {
        if (ev.pointerId !== pointerId) return; // a second finger is not this gesture
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const last = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins({ x: startX, y: startY }, last, threshold)) return;
          armed = true;
        }
        place(last);
        pan.follow(last);
      };
      const onUp = () => {
        if (done) return;
        done = true;
        pan.stop();
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch gesture the system takes over ends in `pointercancel`.
        document.removeEventListener("pointercancel", onUp);
        window.removeEventListener("blur", onUp);
        el.classList.remove("fence-invalid");
        gestures.delete(el);
        for (const m of carried) gestures.delete(m.el);
        // Refuse, do NOT snap: the fence and everything it carries go back to
        // where the gesture began and nothing is persisted.
        if (!fits || !moved) {
          el.style.left = start.left + "px";
          el.style.top = start.top + "px";
          for (const m of carried) {
            m.el.style.left = m.rect.left + "px";
            m.el.style.top = m.rect.top + "px";
            if (m.el._noteShadow) {
              m.el._noteShadow.style.left = m.el.style.left;
              m.el._noteShadow.style.top = m.el.style.top;
            }
          }
          applyExtent();
          return;
        }
        saveFences(
          fences.map((x: any) =>
            x.id === f.id
              ? { ...x, rect: { ...start, left: start.left + delta.dx, top: start.top + delta.dy } }
              : x,
          ),
        );
        renderFences();
        // Each member is written EXACTLY ONCE, here, at its computed place —
        // a write per mousemove would send N changes per frame for a gesture
        // with one outcome. The cards go in ONE call for the same reason.
        for (const m of carried) {
          if (m.kind === "note") continue;
          setWin(m.el, { rect: { ...m.rect, left: m.rect.left + delta.dx, top: m.rect.top + delta.dy } });
        }
        const cards = carried.filter((m) => m.kind === "note").map((m) => m.el);
        if (cards.length) window.WBNotes?.persistCards(cards);
        applyExtent();
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
  }

  // Resize moves the FENCE only — never a member: a window whose centre falls
  // outside the new rect stops being reported by `fenceMembership`. That holds
  // for WEST and NORTH too — the opposite edge is anchored, so it is a resize,
  // not the §6 move that carries members.
  function startFenceResize(el: any, f: any, dir: any) {
    const way = FENCE_DIRS.includes(dir) ? dir : "se";
    return (e: any) => {
      if (e.button !== 0 || !e.isPrimary) return;
      if (fenceLocked(f.id)) return;
      const st = stage();
      if (!st) return;
      const pointerId = e.pointerId;
      const start = restoreRect(el);
      // Captured ONCE (see `startResize`): a live re-read feeds the extent this
      // gesture grows back in as its own bound.
      const bounds = { width: st.offsetWidth, height: st.offsetHeight };
      const startX = e.clientX;
      const startY = e.clientY;
      let out = start;
      let fits = true;
      let done = false;
      let sized = false; // a click is not a resize — see `moved` in startFenceMove
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      clearFenceFlash();
      gestures.add(el);
      const onMove = (ev: any) => {
        if (ev.pointerId !== pointerId) return; // a second finger is not this gesture
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        if (!armed) {
          if (!dragBegins({ x: startX, y: startY }, { x: ev.clientX, y: ev.clientY }, threshold))
            return;
          armed = true;
        }
        const next = resizeRect(
          way,
          start,
          { dx: ev.clientX - startX, dy: ev.clientY - startY },
          FENCE_MIN,
          bounds,
        );
        fits = fenceFits(fences, { id: f.id, rect: next });
        if (next.width !== start.width || next.height !== start.height) sized = true;
        if (fits) out = next;
        el.classList.toggle("fence-invalid", !fits);
        el.style.left = next.left + "px";
        el.style.top = next.top + "px";
        el.style.width = next.width + "px";
        el.style.height = next.height + "px";
        applyExtent({ grow: true });
      };
      const onUp = () => {
        if (done) return;
        done = true;
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch gesture the system takes over ends in `pointercancel`.
        document.removeEventListener("pointercancel", onUp);
        window.removeEventListener("blur", onUp);
        el.classList.remove("fence-invalid");
        gestures.delete(el);
        const rect = fits && sized ? out : start;
        el.style.left = rect.left + "px";
        el.style.top = rect.top + "px";
        el.style.width = rect.width + "px";
        el.style.height = rect.height + "px";
        if (!fits || !sized) {
          applyExtent();
          return;
        }
        saveFences(fences.map((x: any) => (x.id === f.id ? { ...x, rect } : x)));
        renderFences();
        applyExtent();
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
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
      if (btn) btn.hidden = narrow || detached.includes(el.dataset.fenceId);
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

  // ---- walking the fences from the keyboard ------------------------------------
  // Pure. `[{id, rect}]` + the id in hand + a step (+1/-1) yields the next id in
  // READING ORDER (top band first, left to right) — the desk array is creation
  // order, which would teleport across the stage. The band is what keeps a row
  // a row: side-by-side fences are never pixel-aligned on `top`.
  const FENCE_BAND = 120;

  function fenceOrder(fences: any) {
    return [...(fences || [])].sort((a, b) => {
      const at = Math.floor((a?.rect?.top || 0) / FENCE_BAND);
      const bt = Math.floor((b?.rect?.top || 0) / FENCE_BAND);
      if (at !== bt) return at - bt;
      const al = a?.rect?.left || 0;
      const bl = b?.rect?.left || 0;
      if (al !== bl) return al - bl;
      // Total order, so the walk is the same on every client: two fences at the
      // very same point would otherwise cycle in whatever order `sort` picked.
      return String(a?.id).localeCompare(String(b?.id));
    });
  }

  // `null` when there is nothing to walk. With no fence in hand the step decides
  // which end to enter from, so the first Alt+Shift+→ lands on the top-left
  // fence and the first Alt+Shift+← on the bottom-right one.
  function fenceCycle(fences: any, currentId: any, step: any) {
    const order = fenceOrder(fences);
    if (!order.length) return null;
    const d = step < 0 ? -1 : 1;
    const at = order.findIndex((f) => f.id === currentId);
    if (at < 0) return (d > 0 ? order[0] : order[order.length - 1]).id;
    return order[(at + d + order.length) % order.length].id;
  }

  // How many fences may be detached at once: a popup is a real OS window with
  // its own sockets and renderers; the cap stops a stuck key opening forty.
  const DETACH_MAX = 4;

  // The detach registry's fold: (registry, event) -> { registry, effects }.
  // Pure — no DOM, no storage, no `window`. `registry` (fence ids) is never
  // mutated: a new array comes back, so the caller commits only once the
  // effects have run (a popup the browser blocked leaves the registry as was).
  // Three events: detach, reattach, focus. A new event is a new case here.
  function detachFold(registry: any, event: any) {
    const reg = Array.isArray(registry) ? registry : [];
    const id = event?.fenceId;
    const held = reg.includes(id);
    switch (event?.type) {
      case "detach":
        // Already detached: one popup per fence, so this is a request to SEE
        // the window that already exists, not to open a second one.
        if (held) return { registry: reg.slice(), effects: [{ type: "focus", fenceId: id }] };
        if (reg.length >= DETACH_MAX)
          return { registry: reg.slice(), effects: [{ type: "refuse", fenceId: id, reason: "cap" }] };
        return { registry: reg.concat([id]), effects: [{ type: "open", fenceId: id }] };
      case "reattach":
        // Idempotent on purpose: BOTH the popup's `beforeunload` and the
        // opener's `closed` poll report a re-attach, so the second one must be
        // a no-op rather than a second spawn of the same consoles.
        if (!held) return { registry: reg.slice(), effects: [] };
        return { registry: reg.filter((f) => f !== id), effects: [{ type: "close", fenceId: id }] };
      case "focus":
        return { registry: reg.slice(), effects: held ? [{ type: "focus", fenceId: id }] : [] };
      default:
        return { registry: reg.slice(), effects: [] };
    }
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
  // `detached` is the in-memory mirror; its DURABLE copy is this tab's
  // session-scoped storage behind `link` (ADR-0051 §8), which carries a detach
  // across an F5 and kills it with the tab. Every transition goes through
  // `commitDetached` so the two never disagree.
  let detached: any = [];
  const fencePopups = new Map(); // fenceId -> { handle, members, fence, poll, peer }
  const PEER_WINDOW = WBDetachLink.PEER_WINDOW_MS;
  const HEARTBEAT = WBDetachLink.HEARTBEAT_MS;

  function isDetached(id: any) {
    return detached.includes(id);
  }

  // A popup entry with no popup behind it yet — the shape both the boot restore
  // and an unheralded `popup-here` start from.
  function newPopupEntry(memberIds?: any): any {
    return {
      handle: null,
      members: [],
      memberIds: memberIds || [],
      fence: null,
      poll: null,
      greeted: false,
      rescue: null,
      // Whether the POPUP has told us its member set. Until it has, an empty
      // `members` means "not asked yet" and the restored ids stand in; after it
      // has, an empty one means EMPTY — the operator closed them all in there.
      adopted: false,
      peer: { seen: Date.now(), lost: false },
      // Whether a silent window has already been probed: one unanswered probe
      // is death, not one quiet window (`stillThere`).
      probed: false,
    };
  }

  // The popup's member set, adopted as the truth. A console CLOSED inside the
  // popup ended a real daemon session; re-attaching it would wire a window to a
  // gone session, so its desk RECORD goes too. Both `popup-members` and
  // `popup-here` arrive here, so the two never prune differently.
  function adoptMembers(id: any, members: any) {
    const entry = fencePopups.get(id);
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
    commitDetached(detached);
  }

  // The member ids each detached fence holds, for the registry: the live
  // snapshot, else the ids restored from the last write.
  function detachedMembers() {
    const out: any = {};
    for (const id of detached) {
      const entry = fencePopups.get(id);
      const live = (entry?.members || []).map((m: any) => m.id).filter(Boolean);
      // `adopted` is what lets an EMPTY live list mean empty: the popup answered
      // and holds nothing. Without it the fallback below would re-persist the
      // very consoles the operator just closed in there.
      out[id] = entry?.adopted || live.length ? live : (entry?.memberIds || []).slice();
    }
    return out;
  }

  // THE ONE PLACE `detached` CHANGES. INVARIANT on every return path: the
  // mirror and the stored registry hold the same ids, and no heartbeat timer
  // runs while nothing is detached.
  function commitDetached(next: any) {
    detached = next;
    link.writeRegistry(detached, detachedMembers());
    if (detached.length) startBeat();
    else stopBeat();
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
      // A snapshot: `reattachFence` mutates `fencePopups` inside this loop.
      for (const [id, entry] of [...fencePopups]) {
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
    if (m.type !== "popup-here" && m.type !== "popup-ping" && !popupMatches(fencePopups.get(id), m)) {
      return;
    }
    if (m.type === "popup-here") {
      // A popup that survived this tab's reload, announcing which fence it
      // holds. Adopted only when the RESTORED registry already says that fence
      // is detached — the payload alone must never be able to detach one.
      if (!isDetached(id)) return;
      if (fencePopups.has(id) && !popupMatches(fencePopups.get(id), m)) return;
      // MUTATED IN PLACE, never replaced: `glyphClick`'s ping compares the entry
      // it captured with the one in the map.
      const entry = fencePopups.get(id) || newPopupEntry();
      // A restored entry learns which popup it holds from its first answer.
      if (entry.pid == null && typeof m.pid === "string") entry.pid = m.pid;
      const st = stage();
      entry.greeted = true;
      // The popup hands back the UNTRANSLATED snapshot it was given, so a
      // re-attach puts every console back where it was detached from. Adopted
      // whole, EMPTY included: an empty set is an answer, not a missing one.
      fencePopups.set(id, entry);
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
      fencePopups.set(id, entry);
      // Re-persist: the snapshot the popup just handed back is a better member
      // list than the ids this tab restored, and the NEXT reload reads it.
      commitDetached(detached);
      showDetachGlyph(id, true);
    } else if (m.type === "popup-members") {
      // A console closed INSIDE the popup. Same registry gate as `popup-here`.
      if (isDetached(id)) adoptMembers(id, m.members);
    } else if (m.type === "popup-beat") {
      const entry = fencePopups.get(id);
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
    const entry = fencePopups.get(id);
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
    const entry = fencePopups.get(id);
    const record = notes.find((n: any) => n.id === m.noteId);
    if (!isDetached(id) || !noteNameOk(entry, record, { noteId: m.noteId, path: m.claim })) return;
    entry.members = entry.members.map((x: any) =>
      x.kind === "note" && x.id === m.noteId ? { ...x, claim: m.claim } : x,
    );
  }

  // Is a popup's note-name report one this tab may record? Pure. The
  // note must be a card the popup holds, its desk record must have no path
  // yet (a name is given once, ADR-0064 §4), and the path must name a note.
  function noteNameOk(entry: any, record: any, msg: any) {
    const held = (entry?.members || []).some((x: any) => x?.kind === "note" && x.id === msg?.noteId);
    if (!held || !record || record.id !== msg.noteId || record.path) return false;
    const path = msg.path;
    // Relative to the repo: no parent step, no root, no drive letter.
    return (
      typeof path === "string" &&
      path.endsWith(".note") &&
      !path.includes("..") &&
      !/^[\\/]/.test(path) &&
      !/^[a-zA-Z]:/.test(path)
    );
  }

  // Unique in this browser: the clock separates this tab's documents, the
  // counter separates two detaches in one millisecond.
  let popupSeq = 0;
  function newPid() {
    popupSeq += 1;
    return `${Date.now().toString(36)}-${popupSeq}`;
  }

  // Does a lifecycle message come from the popup this entry holds? Pure. A
  // popup's `pid` is given at detach and rides every message it sends. An
  // entry restored after a reload has no `pid` until the popup's first
  // `popup-here`, and until then it hears any popup of its fence.
  function popupMatches(entry: any, m: any) {
    if (!entry) return false;
    if (entry.pid == null) return true;
    return m?.pid === entry.pid;
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
    untrackDormancy(win);
    wins.delete(win);
    changed();
  }

  function stopPoll(entry: any) {
    if (entry?.poll) clearInterval(entry.poll);
    if (entry?.rescue) clearTimeout(entry.rescue);
  }

  function detachFence(id: any) {
    const out: any = detachFold(detached, { type: "detach", fenceId: id });
    for (const effect of out.effects) {
      if (effect.type === "focus") {
        // Raising a popup that already holds this fence — the ONLY way to raise
        // one. The handle is the direct route; after a reload it died with the
        // document and the channel is the only one left.
        const live = fencePopups.get(id);
        if (live?.handle && !live.handle.closed) live.handle.focus();
        else link.post({ type: "origin-focus", tab: link.tab, fenceId: id, pid: live?.pid ?? undefined });
        WB.emit("fence-focus", { fence: id });
        return;
      }
      if (effect.type === "refuse") {
        fenceNotice(id, `Maximum of ${DETACH_MAX} detached fences`);
        WB.emit("fence-detach-refused", { fence: id, reason: effect.reason });
        return; // the registry is NOT committed
      }
    }
    if (!out.effects.some((e: any) => e.type === "open")) return;

    const st = stage();
    const fence = st ? readFenceRects(st).find((f) => f.id === id) : null;
    // INVARIANT: either the popup exists AND the members are torn down, or
    // neither. `window.open` therefore runs BEFORE a single window is touched —
    // a blocked popup must leave the fence exactly as it was.
    // `fence` is the daemon's route for the page (`Shell` in `assets.rs`).
    const handle = window.open("fence", "", "popup,width=900,height=700");
    if (!handle) {
      fenceNotice(id, "Could not detach: pop-up blocked");
      WB.emit("fence-detach-blocked", { fence: id });
      return; // the registry is NOT committed, nothing was torn down
    }
    // The fence's consoles leave the columns BEFORE the snapshot, so a leftmost
    // that left travels with `max: false` and no terminal shows in two places.
    // Synchronous: the popup document loads later.
    const leaving = st ? fenceMembership(readFenceRects(st), readWindowRects(st))[id] || [] : [];
    document.dispatchEvent(new CustomEvent("workbench:columns-leave", { detail: { ids: leaving } }));
    const members = fenceSnapshot(id);

    const entry: any = {
      handle,
      members,
      memberIds: members.map((m: any) => m.id).filter(Boolean),
      fence: fence || { id, name: "", rect: null },
      // This popup's identity, on every lifecycle message both ways (#476).
      pid: newPid(),
      poll: null,
      greeted: false,
      rescue: null,
      // Seeded NOW, not at the first `popup-beat`: the popup needs a page load
      // and a handshake before it can beat; `PEER_WINDOW_MS` is the grace.
      peer: { seen: Date.now(), lost: false },
    };
    // The entry lands BEFORE the commit, so `detachedMembers()` has the ids to
    // persist. Still nothing is torn down yet — the invariant above holds.
    fencePopups.set(id, entry);
    commitDetached(out.registry);
    // Armed BEFORE the teardown: a throw there would leave the entry with no
    // watcher, and a force-closed popup fires no `beforeunload`. The fold is
    // idempotent on a re-attach, so the doubled signal costs nothing.
    entry.poll = setInterval(() => {
      if (entry.handle.closed) reattachFence(id);
    }, 500);
    // A page that never completes the handshake (load failure, navigation, an
    // auth interstitial) is NOT closed, so the poll never fires. Bring the
    // consoles home instead.
    entry.rescue = setTimeout(() => {
      if (!entry.greeted && fencePopups.get(id) === entry) reattachFence(id);
    }, 5000);
    for (const m of members) {
      if (m.kind === "note") continue;
      const win = [...wins].find((w) => w._deskId === m.id);
      // The popup attaches each member again.
      if (win) tearDownMember(win, "reconnect");
    }
    // The cards leave the stage the same way: `renderNotes` drops every card
    // whose fence is now detached, and the records stay exactly where they are.
    renderNotes();
    showDetachGlyph(id, true);
    applyExtent();
    refreshFenceChrome();
    WB.emit("fence-detach", { fence: id });
  }

  // `opts.force` is the GLYPH's call only. The automatic paths (`beforeunload`,
  // the closed-poll, the peer-loss tick) stay gated on the fold because their
  // signals arrive DOUBLED. A click is an instruction that must land even when
  // the state behind the glyph is wrong. Forcing is safe against the doubled
  // signal for the same reason the fold is: the entry is deleted here.
  function reattachFence(id: any, opts: any = {}) {
    const out = detachFold(detached, { type: "reattach", fenceId: id });
    const held = out.effects.some((e) => e.type === "close");
    if (!held && !opts.force) return;
    const entry = fencePopups.get(id);
    stopPoll(entry);
    fencePopups.delete(id);
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
    for (const [id, entry] of fencePopups) if (entry.handle === e.source) owner = id;
    if (owner == null) return;
    const m = e.data;
    if (!m) return;
    if (m.type === "wb-fence-ready") {
      const entry = fencePopups.get(owner);
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
  const FENCE_NAME_RE = /^Fence (\d+)$/;
  function nextFenceName(existing: any) {
    const highest = (existing || []).reduce((max: any, f: any) => {
      const m = FENCE_NAME_RE.exec(String(f?.name ?? ""));
      return m ? Math.max(max, Number(m[1])) : max;
    }, 0);
    return `Fence ${highest + 1}`;
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
    if (detached.includes(id)) {
      fenceNotice(id, "Return this fence's consoles to this window first");
      WB.emit("fence-remove-refused", { fence: id, reason: "detached" });
      return;
    }
    saveFences(fences.filter((f: any) => f.id !== id));
    renderFences();
    applyExtent();
  }

  // ---- navigating the plane ----------------------------------------------------
  // The scroll offsets that bring `target` (a STAGE-relative rect) into the
  // viewport, pure: centre it, then clamp to `[0, extent - viewport]`. ALWAYS
  // centres. Callers: `reveal` (the Go-to picker, #337) and ADR-0051 §7's fence
  // jump.
  // ONE clamp per axis: the final `Math.max(0, …)` stops a viewport bigger than
  // the extent asking for a negative offset. Flooring the ceiling too would
  // make that floor unfalsifiable by the table's negative control.
  function clampOffset(offset: any, viewport: any, extent: any) {
    const maxLeft = (extent?.width || 0) - (viewport?.width || 0);
    const maxTop = (extent?.height || 0) - (viewport?.height || 0);
    return {
      left: Math.max(0, Math.min(offset?.left || 0, maxLeft)),
      top: Math.max(0, Math.min(offset?.top || 0, maxTop)),
    };
  }

  function bringIntoView(target: any, viewport: any, extent: any) {
    const vw = viewport?.width || 0;
    const vh = viewport?.height || 0;
    const left = (target?.left || 0) + (target?.width || 0) / 2 - vw / 2;
    const top = (target?.top || 0) + (target?.height || 0) / 2 - vh / 2;
    return clampOffset({ left, top }, viewport, extent);
  }

  // The other anchoring: the target's TOP-LEFT corner, one inset in from the
  // viewport's. A fence is a region the operator works inside, not a point of
  // interest — centring it wastes the screen above and left. `bringIntoView`
  // keeps CENTRING for the Go-to picker (#337 pins it).
  const VIEW_INSET = 24;

  function anchorIntoView(target: any, viewport: any, extent: any, inset: any) {
    const pad = inset == null ? VIEW_INSET : inset;
    return clampOffset(
      { left: (target?.left || 0) - pad, top: (target?.top || 0) - pad },
      viewport,
      extent,
    );
  }

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

  // Ease-out cubic on a 0..1 clock: fast off the mark, settling into the target.
  function slideEase(t: any) {
    const x = Math.min(1, Math.max(0, t));
    return 1 - Math.pow(1 - x, 3);
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

  // The bounding box of a set of stage-relative rects; all zeros for none, so an
  // empty desk centres on the pinned origin rather than on nothing.
  function bboxOf(rects: any) {
    const list = rects || [];
    if (!list.length) return { left: 0, top: 0, width: 0, height: 0 };
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    for (const r of list) {
      left = Math.min(left, r.left || 0);
      top = Math.min(top, r.top || 0);
      right = Math.max(right, (r.left || 0) + (r.width || 0));
      bottom = Math.max(bottom, (r.top || 0) + (r.height || 0));
    }
    return { left, top, width: right - left, height: bottom - top };
  }

  // Where the viewport lands on load (#339), pure. The stored per-client offset
  // wins only while it still SHOWS work (some window intersects the viewport
  // placed there); otherwise the bbox landing. The clamp comes BEFORE the test:
  // an offset saved on a bigger screen is a legitimate view pulled into this
  // extent.
  function viewLanding(stored: any, rects: any, viewport: any, extent: any) {
    const num = (v: any) => (typeof v === "number" && Number.isFinite(v) ? v : null);
    const left = num(stored?.left);
    const top = num(stored?.top);
    if (left !== null && top !== null) {
      const at = clampOffset({ left, top }, viewport, extent);
      const vw = viewport?.width || 0;
      const vh = viewport?.height || 0;
      const shows = (rects || []).some(
        (r: any) =>
          (r.left || 0) < at.left + vw &&
          (r.left || 0) + (r.width || 0) > at.left &&
          (r.top || 0) < at.top + vh &&
          (r.top || 0) + (r.height || 0) > at.top,
      );
      if (shows) return at;
    }
    return bringIntoView(bboxOf(rects), viewport, extent);
  }

  // How far the plane scrolls per frame while a window is dragged against the
  // viewport edge, and how wide the pressure band at each edge is.
  const PAN_BAND = 48;
  const PAN_STEP = 24;
  // One "line" of wheel delta in pixels, for a browser that reports
  // `deltaMode: DOM_DELTA_LINE` (Firefox) instead of pixels.
  const WHEEL_LINE = 16;

  // The auto-pan rule, pure. `viewport` is a CLIENT rect; `pointer` is a client
  // point. Each edge contributes a pressure in `[0, band]` and the axis takes
  // their DIFFERENCE — deliberately, so a viewport narrower than two bands
  // cancels instead of oscillating between its own two edges.
  function panNudge(pointer: any, viewport: any, band: any, step: any) {
    const b = band == null ? PAN_BAND : band;
    const s = step == null ? PAN_STEP : step;
    const press = (v: any) => Math.max(0, Math.min(v, b));
    const axis = (near: any, far: any) => Math.round((s * (far - near)) / b);
    return {
      dx: axis(
        press(b - ((pointer?.x || 0) - (viewport?.left || 0))),
        press(b - ((viewport?.right || 0) - (pointer?.x || 0))),
      ),
      dy: axis(
        press(b - ((pointer?.y || 0) - (viewport?.top || 0))),
        press(b - ((viewport?.bottom || 0) - (pointer?.y || 0))),
      ),
    };
  }

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

  // Wire one handle: drag it and the window's rect follows `resizeRect`. Every
  // exit path (mouseup anywhere on the document) drops BOTH listeners and
  // persists exactly once.
  // Same seam as `makeDraggable`'s, for the same second caller.
  function startResize(win: any, dir: any, opts?: any) {
    const heldFast = opts?.locked || (() => isLocked(win));
    const persist = opts?.onDrop || (() => setWin(win, { rect: restoreRect(win) }));
    const min = opts?.min || RESIZE_MIN;
    return (e: any) => {
      if (e.button !== 0 || !e.isPrimary) return; // see makeDraggable
      const pointerId = e.pointerId;
      focusWin(win);
      if (win.classList.contains("maximized") || win.classList.contains("column") || isFull(win)) return;
      if (heldFast()) return; // the JS guard is the truth; the CSS only hides the bands
      gestures.add(win);
      const rect = {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
      // The STAGE is the bound (ADR-0051 §5). Captured ONCE: a live re-read
      // feeds back on itself — the extent this gesture grows becomes the bound
      // of its next move, inflating the window ~one margin per mousemove.
      const st = stage();
      const bounds = { width: st.offsetWidth, height: st.offsetHeight };
      const startX = e.clientX;
      const startY = e.clientY;
      // See makeDraggable: a press under the threshold is a tap on the band.
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      const onMove = (ev: any) => {
        // A second finger opens its own stream and is not this gesture.
        if (ev.pointerId !== pointerId) return;
        if (!armed) {
          if (!dragBegins({ x: startX, y: startY }, { x: ev.clientX, y: ev.clientY }, threshold))
            return;
          armed = true;
        }
        const out = resizeRect(
          dir,
          rect,
          { dx: ev.clientX - startX, dy: ev.clientY - startY },
          min,
          bounds,
        );
        win.style.left = out.left + "px";
        win.style.top = out.top + "px";
        win.style.width = out.width + "px";
        win.style.height = out.height + "px";
      };
      const onUp = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch resize the system takes over ends here and nowhere else.
        document.removeEventListener("pointercancel", onUp);
        applyExtent();
        if (armed) persist(); // a tap on a band changed nothing
        gestures.delete(win);
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
  }

  // The workbench session codec, mirrored from src/protocol.rs. A terminal frame
  // is [0x01][session u64 BE][raw bytes]; a resize rides a command frame [0x02]
  // [JSON {id, verb:"resize", payload:{rows, cols}}]. One session per socket in
  // this slice, so the session id is always 1.
  const TAG_TERMINAL = 0x01;
  const TAG_COMMAND = 0x02;
  const SESSION_ID = 1;

  function encodeTerminal(str: any) {
    const data = new TextEncoder().encode(str);
    const out = new Uint8Array(1 + 8 + data.length);
    out[0] = TAG_TERMINAL;
    out[8] = SESSION_ID;
    out.set(data, 9);
    return out;
  }

  function encodeCommand(verb: any, payload: any) {
    const body = new TextEncoder().encode(JSON.stringify({ id: 0, verb, payload }));
    const out = new Uint8Array(1 + body.length);
    out[0] = TAG_COMMAND;
    out.set(body, 1);
    return out;
  }

  function encodeResize(rows: any, cols: any) {
    return encodeCommand("resize", { rows, cols });
  }

  // Why this page closes a console socket, sent as DATA just before the close:
  // close metadata does not survive the trip (#334), and the peer relay
  // forwards data frames unchanged. The daemon only logs it. One of
  // `dormant`, `reconnect`, `window-closed`.
  function encodeDetach(reason: any) {
    return encodeCommand("detach", { reason });
  }

  // Announce `reason` on an open socket. A socket still connecting cannot send.
  function announceDetach(ws: any, reason: any) {
    if (reason && ws && ws.readyState === 1) ws.send(encodeDetach(reason));
  }

  // Failed re-opens before a socket is given up on, and how many a never-opened
  // would-be writer spends before settling for watching. Module scope so
  // `reconnectDecision` can be tabled without an `attachTerminal` instance.
  const MAX_FAILED_REOPENS = 10;
  const WATCH_AFTER = 3;

  // RESUME — the tablet case. A suspended tab runs no JS while the link is torn
  // down, so it comes back holding sockets that report OPEN and never deliver
  // another byte. Named `resume`, never `wake`: waking is for a peer daemon
  // (CONTEXT.md).
  //
  // `stale` is the caller's verdict: the shell feeds it from the presence
  // heartbeat (`setStaleProbe`); the popup, with none, falls back to how long
  // it was hidden — so an ordinary desktop tab switch churns nothing.
  const RESUME_HIDDEN_MS = 60000;
  const RESUME_DEBOUNCE_MS = 1500;

  // A handshake gets this long to open (`WBDaemon`'s twin). Without a deadline
  // a reattach opened onto a link that is not up yet (an iPhone back from a
  // call) sits in CONNECTING until the OS abandons TCP/TLS, under a
  // "[connection lost — reconnecting…]" that no resume will touch.
  const CONNECT_TIMEOUT_MS = 8000;

  // `visibilitychange` and `online` both land on one iOS resume; without the
  // probe seam the popup would have no verdict at all.
  let staleProbe = OPTS.isStale || null;
  function setStaleProbe(fn: any) {
    staleProbe = typeof fn === "function" ? fn : null;
  }

  // Retire a socket so its pending events cannot reach us. `onmessage` matters
  // as much as `onclose`: a frame still queued lands AFTER this returns, when
  // `ws` names the replacement. Local, not `WBDaemon`'s: this module loads on
  // its own in the node harness and the popup.
  function detachSocket(ws: any, reason: any) {
    if (!ws) return;
    ws.onclose = null;
    ws.onmessage = null;
    ws.onopen = null;
    ws.onerror = null;
    try {
      announceDetach(ws, reason);
      if (ws.readyState <= 1) ws.close();
    } catch {}
  }

  // The engine, not the brand: WebKit answers "Apple Computer, Inc." in every
  // browser on iPadOS; Chromium "Google Inc."; Firefox "". Pure so the string
  // table is the contract.
  function isWebKit(vendor: any) {
    return typeof vendor === "string" && vendor.startsWith("Apple");
  }

  // Whether this engine must render in the DOM instead of on the GPU: the WebGL
  // addon draws scrolled rows twice on WebKit.
  function prefersDomRenderer(vendor: any) {
    return isWebKit(vendor);
  }

  // Whether to BUILD the fullscreen button. `fullscreenEnabled` is false in a
  // sandboxed frame and a standalone PWA. On WebKit it is true but iOS drops
  // out of fullscreen the moment a text field takes focus, so on an iPad the
  // first keystroke would cancel it; maximize is the honest control there.
  function fullscreenOffered(enabled: any, vendor: any) {
    return enabled === true && !isWebKit(vendor);
  }

  // TOUCH SCROLLING is ours. MEASURED: the touch lands on `.xterm-screen`, and
  // `.xterm-viewport` (the scroller) is its SIBLING, so the browser walks up to
  // `#workspace` and pans the workbench; `overscroll-behavior: contain` on the
  // viewport is inert for the same reason. A wheel works only because xterm
  // forwards `wheel` in JS. Upstream: xterm.js #3613, #594, #5377.
  // Pure: pixels dragged → lines, at the cell height, sign flipped. A
  // zero/absent cell height yields 0, not Infinity.
  function touchScrollLines(dyPx: any, cellHeight: any) {
    if (!Number.isFinite(dyPx) || !Number.isFinite(cellHeight) || cellHeight <= 0) return 0;
    return -dyPx / cellHeight;
  }

  // WHO the gesture belongs to. xterm hands a wheel to the APPLICATION when it
  // asked for mouse events (Claude Code and every full-screen TUI), turns it
  // into arrow keys in the alternate buffer, and moves its own viewport only in
  // the plain case — under a TUI the viewport's history is stale frames
  // ("ghost" text). `mode` is `term.modes.mouseTrackingMode`; `bufferType` is
  // `term.buffer.active.type`.
  function touchScrollTarget(mode: any, bufferType: any) {
    if (typeof mode === "string" && mode !== "none") return "app";
    if (bufferType === "alternate") return "app";
    return "viewport";
  }

  // How many fingers, whose gesture. One is the terminal's. Two are the
  // CANVAS's: `touch-action: none` on the body took every browser gesture, so
  // the pan is given back here through the same `scrollLeft/Top` writes the
  // mouse pan makes. Under `maxlock` there is nowhere to pan. Three are the
  // system's.
  function touchGesture(fingers: any, maxlock: any) {
    if (fingers === 1) return "terminal";
    if (fingers === 2 && !maxlock) return "canvas";
    return "none";
  }

  // The point between the fingers, which is what a two-finger pan tracks: the
  // fingers can drift apart or together without the plane jumping.
  function touchCentroid(touches: any) {
    const list: any[] = Array.from(touches ?? []);
    if (!list.length) return { x: 0, y: 0 };
    let x = 0;
    let y = 0;
    for (const t of list) {
      x += t.clientX;
      y += t.clientY;
    }
    return { x: x / list.length, y: y / list.length };
  }

  // How far a press travels before it is a DRAG. A finger never holds still,
  // and with `touch-action: none` the browser no longer tells a tap from a
  // scroll for us: below the threshold the press is a click. The threshold
  // DELAYS the start and never swallows the delta — placement runs from the
  // grab offset taken at pointerdown. An unknown pointer type gets the
  // finger's number.
  const DRAG_THRESHOLD = { mouse: 4, touch: 10 };
  function dragThreshold(pointerType: any) {
    return pointerType === "mouse" ? DRAG_THRESHOLD.mouse : DRAG_THRESHOLD.touch;
  }
  function dragBegins(start: any, pointer: any, threshold: any) {
    const dx = (pointer?.x || 0) - (start?.x || 0);
    const dy = (pointer?.y || 0) - (start?.y || 0);
    return Math.hypot(dx, dy) >= threshold;
  }

  // Two taps on a console's titlebar make a double tap when the second comes
  // within DOUBLE_TAP_MS of the first and lands within a finger's drag
  // threshold of it. Our own rule, not `dblclick`: WebKit on iOS does not
  // turn two taps on a `touch-action: none` bar into one. `prev` and `tap`
  // are `{ t, x, y }` (ms, client px); `prev` is null after a double tap.
  const DOUBLE_TAP_MS = 300;
  function isDoubleTap(prev: any, tap: any) {
    if (!prev || !tap) return false;
    const gap = tap.t - prev.t;
    return gap >= 0 && gap <= DOUBLE_TAP_MS && !dragBegins(prev, tap, DRAG_THRESHOLD.touch);
  }
  // A finger held this long on the console name, without moving past the
  // drag threshold, renames the console.
  const HOLD_MS = 500;

  // Inertia. Terminals hold thousands of lines and a strict 1:1 drag makes the
  // scrollback unreachable by hand, which is the substance of xterm #594.
  // `FLING_DECAY` is per 16ms frame; below `FLING_MIN` the glide has stopped
  // being motion and starts being drift, so it is cut rather than eased.
  const FLING_DECAY = 0.94;
  const FLING_MIN = 0.02; // px/ms
  function flingStep(velocity: any, ms: any) {
    if (!Number.isFinite(velocity) || !Number.isFinite(ms) || ms <= 0) {
      return { dy: 0, velocity: 0 };
    }
    const next = velocity * Math.pow(FLING_DECAY, ms / 16);
    return { dy: velocity * ms, velocity: Math.abs(next) < FLING_MIN ? 0 : next };
  }

  // THE KEY BAR (the tablet's missing row): a virtual keyboard has no Esc, no
  // Ctrl and — on iOS — no arrows.
  //
  // The bytes each button sends. An arrow is NOT one sequence: in application
  // cursor mode a full-screen program expects `ESC O A`, and `ESC [ A` there
  // scrolls nothing. `appCursor` is read live off `term.modes`.
  // Null-prototype: a plain literal answers `"toString"` with a function, and
  // the name comes off a `data-key` attribute — a string from the DOM.
  const KEY_BYTES = Object.assign(Object.create(null), {
    esc: "\x1b",
    tab: "\t",
    // CR, what a real Return key sends. Lets a menu be answered with the bar
    // alone, without opening the virtual keyboard.
    enter: "\r",
    // Opens an agent's command menu without the virtual keyboard.
    slash: "/",
    "ctrl-c": "\x03",
  });
  const ARROW_FINAL = Object.assign(Object.create(null), {
    up: "A",
    down: "B",
    right: "C",
    left: "D",
  });
  // `shift` is the bar's Shift latch. xterm's encodings: Tab becomes back-tab
  // (CBT, which Claude Code cycles its modes on), and an arrow takes the
  // modifier parameter 2 in both cursor modes. Esc, Enter, / and ^C have no
  // Shift form here, so they are sent unchanged.
  function keySequence(name: any, appCursor: any, shift: any) {
    if (shift && name === "tab") return "\x1b[Z";
    const literal = KEY_BYTES[name];
    if (typeof literal === "string") return literal;
    const final = ARROW_FINAL[name];
    if (typeof final !== "string") return "";
    if (shift) return "\x1b[1;2" + final;
    return (appCursor ? "\x1bO" : "\x1b[") + final;
  }

  // One key-bar tap under the Shift latch. `shift` toggles the latch and sends
  // nothing. A key that sends bytes uses the latch once and clears it; a key
  // that sends nothing leaves it set.
  function barKey(name: any, appCursor: any, latched: any) {
    if (name === "shift") return { seq: "", latched: !latched };
    const seq = keySequence(name, appCursor, latched);
    return { seq, latched: seq ? false : latched };
  }

  // The latching Ctrl: a finger presses one key at a time, so `Ctrl` arms and
  // the NEXT character is folded. Only a single printable character folds — `d`
  // can be a whole paste or a bracketed-paste burst, and masking its first byte
  // would corrupt it. Anything else passes through WITH the latch still set.
  function applyCtrlLatch(latched: any, d: any) {
    if (!latched || typeof d !== "string" || d.length !== 1) return { out: d, latched };
    const code = d.toUpperCase().charCodeAt(0);
    if (code < 0x40 || code > 0x5f) return { out: d, latched };
    return { out: String.fromCharCode(code & 0x1f), latched: false };
  }

  // Whether `d` is one of the answers xterm writes back for a terminal
  // QUERY: cursor position and status (`CSI…R`, `CSI…n`), device attributes
  // (`CSI…c`), mode and keyboard reports (`CSI…$y`, `CSI?…u`), window reports
  // (`CSI…t`), and the DCS and OSC replies. xterm sends each answer as one
  // `onData` call. A replayed backlog asks its old questions again (ConPTY's
  // startup `ESC[6n` is always there), so these answers are dropped while it
  // replays. A modified F3 (`CSI 1;5R`) has the same bytes as a cursor answer;
  // the replay lasts a moment, so that collision is accepted.
  const TERMINAL_REPLY =
    /^\x1b(?:\[(?:[?>]?[\d;]*[Rnc]|\??[\d;]+\$y|[\d;]+t|\?\d*u)|P[\s\S]*\x1b\\|\]\d+;[\s\S]*(?:\x07|\x1b\\))$/;
  function isTerminalReply(d: any) {
    return typeof d === "string" && TERMINAL_REPLY.test(d);
  }

  // Whether a window shows the bar. `mode` is "on", "off", or absent for auto
  // (has a touch surface). `any-pointer` rather than `pointer`: an iPad with a
  // Magic Keyboard reports a FINE primary pointer and is still a tablet.
  function keyBarVisible(mode: any, coarse: any) {
    if (mode === "on") return true;
    if (mode === "off") return false;
    return !!coarse;
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

  // The `inputmode` of a terminal's input field. With the key bar shown, the
  // virtual keyboard opens only from the bar's keyboard key: `none` keeps the
  // field focused, so the bar keys, a paste and a hardware keyboard still
  // type, with no virtual keyboard on screen. Without the bar the attribute
  // is absent and the browser decides. Pure.
  function terminalInputMode(barShown: any, keyboardOpen: any) {
    return barShown && !keyboardOpen ? "none" : null;
  }

  // Whether the key bar offers a PASTE button. `readText` exists only in a
  // secure context, and unlike the write there is no `execCommand` fallback
  // for a read. Pure: takes the clipboard object (or `undefined`).
  function pasteOffered(clipboard: any) {
    return !!clipboard && typeof clipboard.readText === "function";
  }

  // THE RIGHT BUTTON is copy or paste, and the browser menu never opens over a
  // console. It is never reported to the child either: a child that asked for
  // mouse events gets each press as a report, and xterm clears the selection
  // on every report (`SelectionService` on `onUserInput`), so the press meant
  // to copy erased the text first. With a selection it copies; without one it
  // pastes; where the clipboard cannot be read (an insecure origin) it does
  // nothing, and Ctrl+V still pastes. Pure.
  function rightClickAction(hasSelection: any, canPaste: any) {
    if (hasSelection) return "copy";
    return canPaste ? "paste" : "none";
  }

  // THE LEFT BUTTON UNDER A TUI. xterm gives every press to a child that asked
  // for mouse events and selects only with Shift (Option on macOS), a key no
  // operator reaches for. A plain left press is held instead ("hold"): moved
  // past the drag threshold it becomes a terminal selection, released in place
  // it reaches the child as the click it was. A press with any modifier keeps
  // xterm's own routing, so Alt+drag still gives the drag to the child. Pure:
  // `mode` is `term.modes.mouseTrackingMode`.
  function pressRoute(mode: any, button: any, modified: any) {
    if (typeof mode !== "string" || mode === "none") return "pass";
    return button === 0 && !modified ? "hold" : "pass";
  }

  // The modifier that makes xterm select while a child owns the mouse
  // (`shouldForceSelection`): Option on macOS, which needs
  // `macOptionClickForcesSelection`, and Shift elsewhere. Alt is NOT set
  // outside macOS: there it asks for a column selection. Pure.
  function forceSelectionKeys(platform: any) {
    return /Mac|iPhone|iPad/.test(platform || "") ? { altKey: true } : { shiftKey: true };
  }

  // A move with no button pressed is a report too (mode "any", DECSET 1003)
  // and clears the selection the same way: moving the pointer to the right
  // button would erase it. Held back while a selection exists. Pure.
  function holdMoveReport(mode: any, hasSelection: any, buttons: any) {
    return typeof mode === "string" && mode !== "none" && !!hasSelection && buttons === 0;
  }

  // THE PHONE BLEED. Fullscreen is withheld on WebKit (`fullscreenOffered`), so
  // on a phone maximize is the ceiling and the chrome folds away below this
  // width: `syncMaxLock` writes `body.console-max`, 01-base.css gates on the
  // same number. Width, not pointer: an iPad keeps its chrome. 560px is the
  // workbench's phone breakpoint (04-canvas.css, 11-appended.css).
  const PHONE_MAX_WIDTH = 560;
  function phoneBleed(maxed: any, viewportWidth: any) {
    return !!maxed && Number.isFinite(viewportWidth) && viewportWidth <= PHONE_MAX_WIDTH;
  }

  // The buffer row under a finger, for the line-selection mode. `selectLines`
  // takes buffer-absolute rows, hence `viewportY`. Clamped to the screen so a
  // finger that slid off the bottom selects to the last row; a zero/NaN cell
  // height answers the top row, not NaN.
  function selectionRow(clientY: any, screenTop: any, cellHeight: any, rows: any, viewportY: any) {
    const base = Number.isFinite(viewportY) ? viewportY : 0;
    if (!Number.isFinite(cellHeight) || cellHeight <= 0 || !Number.isFinite(clientY)) return base;
    const last = Math.max(0, (Number.isFinite(rows) ? rows : 1) - 1);
    const row = Math.floor((clientY - (screenTop || 0)) / cellHeight);
    return base + Math.min(last, Math.max(0, row));
  }

  // TERMINAL FONT SIZE, per browser profile: an iPad and a desktop sharing this
  // desk disagree about glyph size. FONT_DEFAULT is xterm's own default.
  const FONT_MIN = 10;
  const FONT_MAX = 28;
  const FONT_DEFAULT = 15;

  function stepFont(current: any, delta: any) {
    const from = Number.isFinite(current) ? current : FONT_DEFAULT;
    return Math.min(FONT_MAX, Math.max(FONT_MIN, Math.round(from) + delta));
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

  // THE VIRTUAL KEYBOARD'S BITE out of the viewport, in px, published as
  // `--kb-inset` (styles.css reads it on `.maximized` and `:fullscreen`). Pure:
  // layout viewport minus what is visible.
  //   iOS      PANS the visual viewport: `height` shrinks, `offsetTop` grows.
  //   Android  with `interactive-widget=resizes-content` shrinks the layout
  //            viewport itself, so this reads ~0 and the CSS var path is inert.
  // A pinch is not a keyboard: `scale` gates it off.
  const ZOOM_EPSILON = 0.01;
  function keyboardInset({ innerHeight, height, offsetTop, scale }: any) {
    if (typeof scale === "number" && Math.abs(scale - 1) > ZOOM_EPSILON) return 0;
    const inset = (innerHeight || 0) - (height || 0) - (offsetTop || 0);
    if (!Number.isFinite(inset) || inset <= 0) return 0;
    return Math.round(inset);
  }

  // Pure, tabled like `reconnectDecision`. CONNECTING is already the reconnect —
  // closing it only restarts the handshake a round-trip later — until it
  // outlives the handshake deadline, whose timer froze along with the tab.
  function resumeDecision({ readyState, stale, connectingMs }: any) {
    if (readyState == null) return "reconnect";
    if (readyState === 0) return connectingMs >= CONNECT_TIMEOUT_MS ? "reconnect" : "none";
    if (readyState === 1) return stale ? "reconnect" : "none";
    return "reconnect";
  }

  // Whether a new window attaches at once or starts asleep. A window that
  // reattaches to a known session starts asleep, and the observer's first
  // report (it always sends one for a new target) wakes it if it is visible.
  // Otherwise a restored desk replays the text of every console, then puts the
  // ones off the viewport to sleep 15 s later: measured on three devices
  // (2026-10-05), 42% of the console bytes went to those replays.
  //   "dormant" — build the chrome only;
  //   "attach"  — build the terminal and open the socket now.
  // A launch has no id to wake to (`dormancyDecision` D5), and without an
  // observer nothing would ever wake the window.
  function birthDecision({ id, observed }: any) {
    return id != null && observed ? "dormant" : "attach";
  }

  // The dormancy rule, pure and tabled. The observer supplies `intersecting`,
  // `applyDormancy` owns the grace period. Returns exactly one of
  //   "sleep" — dispose this window's terminal and release its socket;
  //   "wake"  — rebuild the terminal and reattach;
  //   "hold"  — leave it exactly as it is.
  function dormancyDecision({
    intersecting,
    covered,
    dormant,
    maximized,
    fullscreen,
    focused,
    hasTerminal,
    ended,
    sessionId,
  }: any) {
    // Visible outranks everything. A window under a full bleed is inside the
    // viewport but nobody sees it: the observer reports geometry, not paint.
    if (intersecting && !covered) return dormant ? "wake" : "hold";
    if (dormant) return "hold";
    // D1: maximized/fullscreen fills the viewport; "outside" is a lie the
    // observer can tell in the frame between the class and the layout.
    if (maximized || fullscreen) return "hold";
    // D2: the focused window is being typed into — and every drag/resize begins
    // with a `pointerdown` that focuses, so this covers a window mid-drag too.
    if (focused) return "hold";
    // D3: a placeholder has no terminal to dispose.
    if (!hasTerminal) return "hold";
    // D4: an ENDED session has no daemon to replay it; sleeping would throw its
    // scrollback away for good.
    if (ended) return "hold";
    // D5: no id is nothing to reattach TO — waking would compose a LAUNCH url
    // and spawn a second vendor CLI (`reconnectDecision` R1).
    if (sessionId == null) return "hold";
    return "sleep";
  }

  // The largest image a paste will send (ADR-0055 §4): the daemon's
  // `MAX_IMAGE_BYTES`, mirrored so an oversized screenshot is refused before
  // base64. The daemon remains the authority.
  const IMAGE_PASTE_MAX = 4 * 1024 * 1024;

  // The image-paste rule (ADR-0055 §5), pure and tabled. `types` are the
  // clipboard items' MIME types, `size` the image item's byte length. One of
  //   "passthrough" — no image on the clipboard: xterm's own text paste runs;
  //   "watched"     — an image, but this window only watches: refuse visibly;
  //   "too-large"   — an image past the cap: refuse without sending;
  //   "drop"        — an image to hand to `image.write`.
  function pasteDecision({ types, size, watching }: any) {
    const hasImage = (types || []).some(
      (t: any) => typeof t === "string" && t.startsWith("image/"),
    );
    if (!hasImage) return "passthrough";
    if (watching) return "watched";
    if (!(size >= 0) || size > IMAGE_PASTE_MAX) return "too-large";
    return "drop";
  }

  // The last line a console prints when it gives up. A launch the daemon
  // refused names the reason; the browser cannot read it anywhere else,
  // because a refused launch never had a session to show.
  function endNotice(announced: any, message: any) {
    if (announced !== "refused") return "[session closed]";
    const why = typeof message === "string" ? message.trim() : "";
    return why ? `[could not start: ${why}]` : "[could not start]";
  }

  // The reconnect rule (#334), pure and tabled. Returns one of "reconnect" /
  // "park-as-watcher" / "give-up".
  //
  // `announced` is the daemon's reason from a data frame BEFORE the close
  // ("taken-over" / "child-exited" / "daemon-shutdown" / "refused"), else
  // null. It is the only trustworthy signal of a deliberate end: the browser
  // reports 1005/wasClean=false even for a served Close frame, so an
  // unannounced dirty close is read as a flaky link.
  function reconnectDecision({
    everOpened,
    announced,
    idKnown,
    failedReopens,
  }: any) {
    // R1: no id is nothing to reattach TO; reconnecting would spawn a SECOND
    // session.
    if (!idKnown) return "give-up";
    // R2/R3: the daemon said why. Taken over → park and watch; else gone.
    if (announced === "taken-over") return "park-as-watcher";
    if (announced != null) return "give-up";
    if (failedReopens > MAX_FAILED_REOPENS) return "give-up";
    // No rule reads the close code or `wasClean` (ADR-0051 §9). A proxy in
    // the path can close the page side cleanly, with 1000, for a socket the
    // daemon dropped, so a clean close is not a deliberate end. Every
    // deliberate end is announced (R2/R3).
    // R6: held the session before, so a drop is a flaky link.
    if (everOpened) return "reconnect";
    // R7/R8: never opened. Retry a bounded number of times (an F5 racing the
    // old bridge's teardown), then settle for watching.
    if (failedReopens < WATCH_AFTER) return "reconnect";
    return "park-as-watcher";
  }

  // Whether a reattach may open a socket at all (ADR-0070 D2, event 7). A
  // socket to a peer the fleet calls down fails and retries, so the window
  // holds instead, and the fleet read that calls the peer back releases it.
  // `decision` is "connect" for a window's first socket, else
  // `reconnectDecision`'s answer. Returns it unchanged, or "hold".
  // No group (a local project, the popup, a fleet not read yet) changes
  // nothing, and a launch (no id) is held by its placeholder (`peerHeld`).
  function peerGate({ decision, group, id }: any) {
    if (id == null || !group || decision === "give-up") return decision;
    return WBFleet.available(group) ? decision : "hold";
  }

  // The terminal's surface, ADR-0035's palette. xterm.js takes no CSS variables
  // (WebGL paints the glyphs), so these mirror :root in styles.css and must
  // move with it — the lockstep `wb-monaco.ts` keeps.
  // Base colours ONLY: the 16 ANSI slots stay xterm's defaults, the palette
  // every vendor TUI picked its colours against. The background is pure black,
  // not `--log-bg` — the same exception `wb-monaco.ts` makes.
  const TERMINAL_THEME = {
    background: "#000000",
    foreground: "#d4ccc0", // --text
    cursor: "#e8d9a8", // --console-text
    cursorAccent: "#000000",
    selectionBackground: "#423a31", // --surface-hi
  };

  // The largest OSC 52 payload accepted, measured on the BASE64 so an oversized
  // string is never materialised. xterm's own OSC limit is 10MB.
  const OSC52_MAX_B64 = 128 * 1024;

  // What an agent put on the clipboard is pasted into a shell: a TRAILING
  // NEWLINE turns a mis-paste into an execution (`curl … | sh\n`), and an
  // escape sequence reaches the terminal it is pasted into. A code-point test
  // so the source carries no control-character escapes of its own.
  function scrubClipboard(text: any) {
    let out = "";
    for (const ch of text.replace(/\r\n/g, "\n")) {
      const c = ch.codePointAt(0);
      // Keep tab (9) and newline (10); drop the rest of C0 and DEL (127).
      if (c === 9 || c === 10 || (c >= 32 && c !== 127)) out += ch;
    }
    return out.replace(/\n+$/, "");
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

  // Pure over `ClipboardItem`s: the first image wins over text, as in the
  // keyboard `paste` event (ADR-0055).
  async function clipboardContent(items: any) {
    const list: any[] = Array.from(items || []);
    for (const item of list) {
      const type = (item.types || []).find((t: any) => t.startsWith("image/"));
      if (type) return { image: await item.getType(type) };
    }
    for (const item of list) {
      if ((item.types || []).includes("text/plain")) {
        return { text: await (await item.getType("text/plain")).text() };
      }
    }
    return { text: "" };
  }

  // Attach a real xterm.js terminal into `body`, wired to a PTY over `/ws/session`.
  // `opts` is one of: {repo, agent} (a NEW agent launch), {console:true[, repo]}
  // (a NEW free-console launch — home dir when `repo` absent), or
  // {id[, takeover][, watch]} (a REATTACH; `watch` is read-only). Returns a
  // handle so the window chrome can refit, take the baton, and close it.
  function attachTerminal(body: any, opts: any) {
    const term = new Terminal({ convertEol: false, theme: TERMINAL_THEME });
    // Set rather than passed: the constructor literal is pinned in lib.rs as
    // the theme contract; the size is a per-profile preference.
    term.options.fontSize = fontSize();
    // Option+drag selects on macOS while a TUI owns the mouse; Shift+drag is
    // xterm's default elsewhere (`shouldForceSelection`).
    term.options.macOptionClickForcesSelection = true;
    const fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(body);
    // GPU glyph rendering with a DOM fallback: on a lost context the addon is
    // disposed and xterm falls back to DOM without dropping the session.
    // NOT on WebKit: the addon renders scrolled rows twice there (xterm.js
    // #3357, #5816; reproduced with the scrollbar, so the renderer, not our
    // gesture). Every browser on iPadOS is WebKit.
    // The terminal starts on the DOM renderer; the page decides which windows
    // hold a context (`rebalanceGpu`) and calls `useGpu`/`dropGpu`.
    let webgl: any = null;
    // The canvases the addon added, so `dropGpu` asks only them for a context:
    // `getContext` on a canvas that has none would create one.
    let gpuCanvases: any = [];
    // A browser that cannot give a context is not asked again.
    let gpuBroken = false;
    function useGpu() {
      if (webgl || gpuBroken || prefersDomRenderer(navigator.vendor)) return;
      try {
        const addon = new WebglAddon.WebglAddon();
        // Lost to the browser: back to the DOM renderer without dropping the
        // session. The slot goes back to the page at its next rebalance.
        addon.onContextLoss(() => {
          if (webgl === addon) dropGpu();
        });
        const before = new Set(term.element.querySelectorAll("canvas"));
        term.loadAddon(addon);
        webgl = addon;
        gpuCanvases = [...term.element.querySelectorAll("canvas")].filter((c) => !before.has(c));
      } catch {
        gpuBroken = true;
      }
    }
    // MEASURED: disposing the addon does not free the browser's slot until the
    // context is collected; an explicit `loseContext` does (14 contexts, 8
    // disposed, 8 new: 7 "Too many active WebGL contexts" warnings without it,
    // 0 with it; Chromium headless, @xterm/addon-webgl 0.19.0, 2026-10-04).
    function dropGpu() {
      const addon = webgl;
      if (!addon) return;
      const canvases = gpuCanvases;
      webgl = null;
      gpuCanvases = [];
      addon.dispose();
      for (const c of canvases) c.getContext("webgl2")?.getExtension("WEBGL_lose_context")?.loseContext();
    }
    term.loadAddon(new WebLinksAddon.WebLinksAddon());

    // The touch gesture this terminal owns (`touchScrollLines`). Single finger
    // only; the stylesheet's `touch-action: none` already told the browser the
    // console is not a pan surface, and A+/A− is a terminal's zoom.
    let touchY: any = null;
    let touchX = 0;
    let touchLastY = 0;
    let touchAccum = 0;
    let touchLastAt = 0;
    let touchVelocity = 0;
    let fling = 0;
    // THE LINE-SELECTION MODE: xterm selects only through mouse events and the
    // touch handlers below spend the finger on scrolling. Armed by the key
    // bar's `sel` button for ONE gesture: a single-finger drag selects whole
    // buffer lines (`selectLines` is the public API; cell-precise is not), and
    // lifting the finger disarms it, leaving the selection for the copy button.
    let selecting = false;
    let selStart: any = null;
    const setSelecting = (on: any) => {
      selecting = !!on;
      selStart = null;
      if (typeof opts.onSelecting === "function") opts.onSelecting(selecting);
    };
    const rowAt = (clientY: any) => {
      const screen = term.element?.querySelector(".xterm-screen");
      const top = screen ? screen.getBoundingClientRect().top : 0;
      return selectionRow(clientY, top, cellHeight(), term.rows, term.buffer.active.viewportY);
    };
    const cellHeight = () => {
      const el = term.element;
      const rows = term.rows;
      return el && rows > 0 ? el.clientHeight / rows : 0;
    };
    // Scroll by a fractional number of lines, carrying the remainder (a slow
    // drag moves less than one row per event). Coalesced to ONE scroll per
    // frame: `touchmove` fires faster than the display refreshes, and several
    // paints in one frame show up as a half-updated screen on a slow renderer.
    let scrollRaf = 0;
    // The app's share goes in through xterm's OWN wheel listener as line-mode
    // wheel events — one per line, so `consumeWheelEvent` neither dampens nor
    // batches them — at the finger's coordinates (a mouse report carries the
    // cell). xterm then does what it does for the trackpad.
    const wheelToApp = (lines: any) => {
      const el = term.element;
      if (!el) return;
      const deltaY = Math.sign(lines);
      for (let n = Math.abs(lines); n > 0; n--) {
        el.dispatchEvent(
          new WheelEvent("wheel", {
            deltaY,
            deltaMode: WheelEvent.DOM_DELTA_LINE,
            clientX: touchX,
            clientY: touchY ?? touchLastY,
            bubbles: true,
            cancelable: true,
          }),
        );
      }
    };
    const flushScroll = () => {
      scrollRaf = 0;
      const whole = Math.trunc(touchAccum);
      if (whole === 0) return;
      touchAccum -= whole;
      // Decided per flush, not per gesture: an app can take the mouse or drop
      // into the alternate buffer while a finger is still down.
      if (touchScrollTarget(term.modes.mouseTrackingMode, term.buffer.active.type) === "app") wheelToApp(whole);
      else term.scrollLines(whole);
    };
    const scrollByPixels = (dy: any) => {
      touchAccum += touchScrollLines(dy, cellHeight());
      if (!scrollRaf) scrollRaf = requestAnimationFrame(flushScroll);
    };
    const stopScroll = () => {
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
      scrollRaf = 0;
    };
    const stopFling = () => {
      if (fling) cancelAnimationFrame(fling);
      fling = 0;
    };
    // Repaint every row: a renderer that left a row half-drawn mid-gesture is
    // corrected once at the end rather than every frame.
    const refreshScreen = () => {
      try {
        term.refresh(0, term.rows - 1);
      } catch {}
    };
    // A two-finger pan of the plane, live until either finger lifts. The
    // remaining finger does NOT resume a scroll: it never had a `touchstart`.
    let pan: any = null;
    const stopPan = () => {
      if (!pan) return;
      pan = null;
      stage()?.classList.remove("panning");
    };
    body.addEventListener(
      "touchstart",
      (e: any) => {
        stopFling();
        // Armed selection. Prevented so the synthesized click never reaches
        // xterm's mousedown, which would clear the selection; the textarea
        // keeps its focus, so the keyboard stays up.
        if (selecting && e.touches.length === 1) {
          e.preventDefault();
          touchY = null;
          selStart = rowAt(e.touches[0].clientY);
          try {
            term.selectLines(selStart, selStart);
          } catch {}
          return;
        }
        const ws = workspace();
        const gesture = touchGesture(e.touches.length, !!ws?.classList.contains("maxlock"));
        if (gesture === "canvas") {
          // A second finger ends the terminal's gesture: the plane owns the touch.
          touchY = null;
          stopScroll();
          touchAccum = 0;
          // The operator's hand outranks a jump in flight (as `onFloorDown`).
          cancelSlide();
          const c = touchCentroid(e.touches);
          pan = { x: c.x, y: c.y, left: ws.scrollLeft, top: ws.scrollTop };
          stage()?.classList.add("panning");
          return;
        }
        stopPan();
        if (gesture !== "terminal") {
          touchY = null;
          return;
        }
        touchY = e.touches[0].clientY;
        touchX = e.touches[0].clientX;
        touchAccum = 0;
        touchVelocity = 0;
        touchLastAt = e.timeStamp;
        // NOT prevented: the tap must reach xterm, or the terminal never takes
        // focus and the on-screen keyboard never opens.
      },
      // Non-passive ONLY for the armed-selection branch above.
      { passive: false },
    );
    body.addEventListener(
      "touchmove",
      (e: any) => {
        if (selecting && selStart != null) {
          if (e.touches.length === 1) {
            const row = rowAt(e.touches[0].clientY);
            try {
              term.selectLines(Math.min(selStart, row), Math.max(selStart, row));
            } catch {}
          }
          e.preventDefault();
          return;
        }
        if (pan) {
          if (e.touches.length !== 2) return;
          const ws = workspace();
          const c = touchCentroid(e.touches);
          if (ws) {
            ws.scrollLeft = pan.left - (c.x - pan.x);
            ws.scrollTop = pan.top - (c.y - pan.y);
          }
          e.preventDefault();
          return;
        }
        if (touchY == null || e.touches.length !== 1) return;
        const y = e.touches[0].clientY;
        touchX = e.touches[0].clientX;
        const dy = y - touchY;
        touchY = y;
        const dt = e.timeStamp - touchLastAt;
        touchLastAt = e.timeStamp;
        if (dt > 0) touchVelocity = dy / dt;
        scrollByPixels(dy);
        // Without this the canvas underneath pans instead.
        e.preventDefault();
      },
      { passive: false },
    );
    const endTouch = (e: any) => {
      // The selection stays (the copy button reads it); the arming does not.
      if (selecting) {
        if (e.touches.length === 0) setSelecting(false);
        return;
      }
      if (pan) {
        if (e.touches.length < 2) stopPan();
        return;
      }
      if (touchY == null) return;
      touchLastY = touchY;
      touchY = null;
      // A finger lifted long after it stopped moving is a hold, not a flick.
      if (e.timeStamp - touchLastAt > 80 || Math.abs(touchVelocity) < FLING_MIN) {
        refreshScreen();
        return;
      }
      let v = touchVelocity;
      let last = performance.now();
      const glide = (now: any) => {
        const step = flingStep(v, now - last);
        last = now;
        v = step.velocity;
        // Already inside a frame: flush HERE, not through `scrollByPixels`.
        touchAccum += touchScrollLines(step.dy, cellHeight());
        stopScroll();
        flushScroll();
        fling = v ? requestAnimationFrame(glide) : 0;
        // The glide has stopped: correct a dropped partial paint once.
        if (!fling) refreshScreen();
      };
      fling = requestAnimationFrame(glide);
    };
    body.addEventListener("touchend", endTouch, { passive: true });
    body.addEventListener("touchcancel", endTouch, { passive: true });
    fit.fit();

    // A pasted IMAGE is not text (ADR-0055): xterm forwards only `text/plain`.
    // The bytes become a clipboard drop (`image.write`) and the drop's PATH is
    // pasted through `term.paste` — bracketed when the child asked, NO trailing
    // newline either way. Text falls through untouched. The gate mirrors
    // `onData`: a watcher SEES the refusal, and nothing leaves its window.
    // Returns whether the paste was taken (false = let the text through).
    const dropImage = (types: any, file: any) => {
      const decision = pasteDecision({ types, size: file ? file.size : -1, watching });
      if (decision === "passthrough") return false;
      if (decision === "watched") {
        if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
        return true;
      }
      if (decision === "too-large") {
        term.write("\r\n[paste refused — too large]\r\n");
        return true;
      }
      const daemon = window.WBDaemon;
      if (!daemon) {
        // The popup forgot its bridge: say so rather than swallow the paste.
        term.write("\r\n[paste refused — daemon not connected]\r\n");
        return true;
      }
      const reader = new FileReader();
      reader.onerror = () => term.write("\r\n[paste refused — unreadable]\r\n");
      reader.onload = () => {
        const base64 = String(reader.result).replace(/^data:[^,]*,/, "");
        daemon
          .write("image.write", { repo: currentRepo, base64 })
          .then((reply: any) => {
            if (WBFail.isError(reply) || !reply.path) {
              const why = WBFail.why(reply, "the daemon refused it");
              term.write(`\r\n[paste refused — ${why}]\r\n`);
              return;
            }
            term.paste(reply.path);
            term.focus();
          })
          // The socket closed with no reply: say what the browser saw.
          .catch((err: any) => {
            const why = (err && err.message) || "connection unavailable";
            term.write(`\r\n[paste refused — ${why}]\r\n`);
          });
      };
      reader.readAsDataURL(file);
      return true;
    };
    term.textarea.addEventListener("paste", (e: any) => {
      const items: any[] = Array.from(e.clipboardData?.items ?? []);
      const image = items.find((i) => i.type.startsWith("image/"));
      const file = image ? image.getAsFile() : null;
      if (dropImage(items.map((i) => i.type), file)) e.preventDefault();
    });

    // OSC 52 — "put this on the clipboard". xterm's core does not implement
    // it, and OSC 52 has no reply, so an agent would announce a copy it never
    // got. WRITE ONLY:
    // - The READ form (`52;c;?`) is never answered: it would let an agent read
    //   the operator's clipboard.
    // - Refused while `replaying`: the scrollback replays as raw bytes, and a
    //   copy from an hour ago would rewrite the clipboard on every reattach.
    // - Refused in a watcher: the same bytes reach EVERY attached window, and
    //   the window whose operator asked owns the clipboard. (Copying BY HAND
    //   in a watcher stays allowed — the #335 gate is about writing to the
    //   child.)
    term.parser.registerOscHandler(52, (data: any) => {
      // NEVER return the clipboard promise: `OscHandler.end` PAUSES the parser
      // on a promise, and a rejected write (unfocused document) would stall
      // the terminal.
      if (replaying || watching) return true;
      const semi = data.indexOf(";");
      if (semi < 0) return true;
      const payload = data.slice(semi + 1);
      // `?` reads and `!` clears; both are no-ops.
      if (payload === "?" || payload === "!") return true;
      if (payload.length > OSC52_MAX_B64) return true;
      let text;
      try {
        const bin = atob(payload);
        text = new TextDecoder().decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
      } catch {
        // base64url or bad padding: `atob` THROWS into xterm's parser.
        return true;
      }
      writeClipboard(scrubClipboard(text), term);
      return true;
    });

    // Ctrl+Insert copies the selection. NOT Ctrl+Shift+C: on Chrome/Edge that
    // is the DevTools accelerator and a page cannot take it back. Ctrl+C
    // belongs to the child.
    term.attachCustomKeyEventHandler((e: any) => {
      // Alt+Shift+arrows in a column walk the columns and their rows (ADR-0051
      // §5): xterm must not send them to the child, and the shell's document
      // listener takes them.
      if (
        e.altKey &&
        e.shiftKey &&
        !e.ctrlKey &&
        !e.metaKey &&
        /^Arrow(Left|Right|Up|Down)$/.test(e.code) &&
        body.closest(".session-window")?.classList.contains("column")
      ) {
        return false;
      }
      // Alt+Shift+R and Alt+Shift+<digit> open a console from inside a
      // terminal too. Only where the shell's document listener exists: a
      // detached popup has none, so its terminal keeps the key.
      if (
        e.altKey &&
        e.shiftKey &&
        !e.ctrlKey &&
        !e.metaKey &&
        /^(?:Digit\d|KeyR)$/.test(e.code) &&
        typeof window.getShell === "function"
      ) {
        return false;
      }
      if (e.type !== "keydown" || !e.ctrlKey || e.shiftKey || e.altKey) return true;
      if (e.key !== "Insert" || !term.hasSelection()) return true;
      writeClipboard(term.getSelection(), term);
      return false;
    });
    // `rightClickAction` and `holdMoveReport`, applied. CAPTURE phase on
    // `body`: xterm binds its listeners on `term.element`, a child, so a stop
    // here means neither xterm nor the child gets the event. The decision is
    // made on `mousedown`, while the selection still exists. Both clipboard
    // calls run inside the press, a user gesture.
    body.addEventListener(
      "mousedown",
      (e: any) => {
        if (e.button !== 2) return;
        const rightTaken = rightClickAction(term.hasSelection(), pasteOffered(navigator.clipboard));
        e.stopPropagation();
        // xterm's own mousedown focused the terminal; it no longer runs.
        e.preventDefault();
        term.focus();
        if (rightTaken === "copy") {
          writeClipboard(term.getSelection(), term);
          term.clearSelection();
        } else if (rightTaken === "paste") {
          readClipboard()
            .then(({ image, text }: any) => {
              if (image) dropImage([image.type], image);
              else if (text) term.paste(text);
            })
            .catch(() => {});
        }
      },
      true,
    );
    body.addEventListener(
      "contextmenu",
      (e: any) => {
        e.preventDefault();
        e.stopPropagation();
      },
      true,
    );
    // `pressRoute`, applied. The held press is REPLAYED to xterm as a
    // synthetic event: with the force-selection key once it turns into a
    // drag, or as itself (then the release) once it ends in place. The
    // replays are marked so this listener lets them through. The real release
    // is stopped: xterm adds its `mouseup` listener to the document during the
    // replayed press, and the real release would report a second time.
    const replayed = new WeakSet();
    const replay = (target: any, type: any, from: any, keys: any) => {
      const ev = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        view: from.view,
        clientX: from.clientX,
        clientY: from.clientY,
        screenX: from.screenX,
        screenY: from.screenY,
        button: 0,
        buttons: type === "mousedown" ? 1 : 0,
        // xterm's selection starts only on `detail === 1` (a single click).
        detail: 1,
        ...keys,
      });
      replayed.add(ev);
      target.dispatchEvent(ev);
    };
    body.addEventListener(
      "mousedown",
      (e: any) => {
        if (replayed.has(e) || !term.element?.contains(e.target)) return;
        const modified = e.shiftKey || e.altKey || e.ctrlKey || e.metaKey;
        if (pressRoute(term.modes.mouseTrackingMode, e.button, modified) !== "hold") return;
        e.stopPropagation();
        e.preventDefault();
        term.focus();
        const doc = body.ownerDocument;
        const start = { x: e.clientX, y: e.clientY };
        const end = () => {
          doc.removeEventListener("mousemove", onMove, true);
          doc.removeEventListener("mouseup", onUp, true);
        };
        const onMove = (m: any) => {
          if (!dragBegins(start, { x: m.clientX, y: m.clientY }, dragThreshold("mouse"))) return;
          end();
          // The selection service is disabled under tracking, so it does not
          // extend an old selection; a new drag replaces it.
          term.clearSelection();
          replay(e.target, "mousedown", e, forceSelectionKeys(navigator.platform || navigator.userAgent));
        };
        const onUp = (u: any) => {
          end();
          u.stopPropagation();
          replay(e.target, "mousedown", e, {});
          replay(e.target, "mouseup", u, {});
        };
        doc.addEventListener("mousemove", onMove, true);
        doc.addEventListener("mouseup", onUp, true);
      },
      true,
    );
    body.addEventListener(
      "mousemove",
      (e: any) => {
        if (holdMoveReport(term.modes.mouseTrackingMode, term.hasSelection(), e.buttons)) {
          e.stopPropagation();
        }
      },
      true,
    );
    // Refit whenever THIS window's body changes size. The only ResizeObserver
    // in the file; it resizes a TERMINAL, never a window rect (#336).
    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {}
    });
    ro.observe(body);

    let currentSessionId = opts.id ?? null;
    let currentRepo = opts.repo ?? null;
    let currentDaemonId: any = null;
    let currentEnvironment: any = null;
    let leaving = false;

    // A dropped socket does NOT end the session: the daemon keeps the child
    // alive (session_ws's teardown invariant), so a close is recovered by
    // reattaching to the SAME id. The daemon replays scrollback on reattach,
    // so the terminal is reset on a reconnecting open. Backoff is exponential
    // with jitter, capped.
    //
    // NO RECONNECT EVER CARRIES `takeover` (#334): two open workbenches
    // reclaiming the writer slot on a timer evicted each other ~1.1s per flip,
    // indefinitely. The baton moves only on `takeOver()`. `reconnectDecision`
    // owns the choice; this carries it out.
    const RECONNECT_BASE = 1000;
    const RECONNECT_MAX = 15000;
    let ws: any = null;
    let opened = false; // has the CURRENT socket opened
    let everOpened = false; // has ANY socket of this window opened
    // True on EVERY path into the watcher role (the park below, or `{id,
    // watch}` from the start). The `term.onData` gate reads this flag.
    let watching = !!opts.watch;
    let announced: any = null; // the daemon's reason, when it named one before closing
    let refusal: any = null; // the daemon's words when that reason is "refused"
    let switching = false; // an intentional close on the way to a takeover
    let firstConnect = true;
    // True while the scrollback replay is being parsed. The replay is RAW BYTES
    // (one terminal frame), so every escape sequence in the backlog runs again
    // (OSC 52 is refused meanwhile). Cleared by the write callback, not after
    // `term.write` returns: xterm parses ASYNCHRONOUSLY.
    let replaying = false;
    let retryDelay = 0;
    let retryTimer: any = null;
    let failedReopens = 0;
    let connectingSince = 0;
    // This window is done. Without the latch a resume would reconnect a dead
    // id and print a second "[session closed]".
    let ended = false;
    let lastResumeAt = 0;
    // True while the fleet calls this window's peer down: no socket, no timer.
    // The session lives on the peer, so a hold never gives up.
    let held = false;
    const peerGroup = () => (typeof opts.peerGroup === "function" ? opts.peerGroup() : null);

    function hold(group: any) {
      held = true;
      if (retryTimer) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (typeof opts.onPeerHold === "function") opts.onPeerHold(group);
    }

    function release() {
      held = false;
      retryDelay = 0;
      failedReopens = 0;
      if (typeof opts.onPeerBack === "function") opts.onPeerBack();
      connect({ id: currentSessionId, repo: currentRepo, watch: watching });
    }

    function giveUp() {
      ended = true;
      // Stop observing so a dead-ws terminal doesn't keep firing fit() until the
      // window is closed.
      ro.disconnect();
      term.write("\r\n" + endNotice(announced, refusal) + "\r\n");
      if (typeof opts.onEnded === "function") opts.onEnded(announced, refusal);
    }

    function scheduleReconnect() {
      retryDelay = Math.min(
        retryDelay ? retryDelay * 2 : RECONNECT_BASE,
        RECONNECT_MAX,
      );
      const wait = retryDelay + Math.random() * 0.3 * retryDelay; // jitter
      retryTimer = setTimeout(() => {
        retryTimer = null;
        connect({ id: currentSessionId, repo: currentRepo, watch: watching });
      }, wait);
    }

    function connect(connOpts: any) {
      opened = false;
      announced = null;
      refusal = null;
      // A reattach (`id`) gets the backlog replayed; a fresh launch has none.
      // An empty scrollback sends no replay frame, so the flag rides until the
      // first LIVE frame clears it.
      replaying = connOpts.id != null;
      ws = new WebSocket(
        WBSessionRoute.url(WS_ORIGIN, {
          ...connOpts,
          holder: WBSessionRoute.tabHolder(),
        }),
      );
      ws.binaryType = "arraybuffer";
      connectingSince = Date.now();
      // `close()` on a CONNECTING socket fires `onclose` with `opened` false:
      // the ordinary failed-reopen path, backoff and give-up count included.
      const handshake = ws;
      setTimeout(() => {
        if (handshake.readyState !== 0) return;
        try {
          handshake.close();
        } catch {}
      }, CONNECT_TIMEOUT_MS);
      ws.onopen = () => {
        opened = true;
        everOpened = true;
        retryDelay = 0;
        failedReopens = 0;
        // A reconnect reattaches and the daemon replays the whole backlog; clear
        // what's on screen first so the replay repaints instead of duplicating.
        if (!firstConnect) term.reset();
        firstConnect = false;
        // HERE, not in `onPark`: the reset above would wipe a line written
        // before the socket opened.
        if (watching) {
          term.write("\r\n[read-only: another window has control]\r\n");
        }
        fit.fit();
        ws.send(encodeResize(term.rows, term.cols));
      };
      ws.onmessage = (ev: any) => {
        const a = new Uint8Array(ev.data);
        if (a[0] === TAG_TERMINAL) {
          if (currentSessionId == null) {
            currentSessionId = Number(
              new DataView(a.buffer, a.byteOffset + 1, 8).getBigUint64(0),
            );
            // A fresh launch's id is only known now; the chrome records under it.
            if (typeof opts.onSession === "function") opts.onSession(currentSessionId);
          }
          // The callback tells the OSC 52 handler the replay is behind us.
          term.write(
            a.subarray(9),
            replaying
              ? () => {
                  replaying = false;
                }
              : undefined,
          );
        } else if (a[0] === TAG_COMMAND) {
          // The daemon's deliberate-end announcement, sent as DATA before the
          // Close frame: the close metadata does not survive the trip (#334).
          let c = null;
          try {
            c = JSON.parse(new TextDecoder().decode(a.subarray(1)));
          } catch {}
          if (c && c.verb === "session-open") {
            const owner = WBSessionRoute.announcement(
              {
                sessionId: currentSessionId,
                daemonId: currentDaemonId,
                environment: currentEnvironment,
              },
              c.payload,
            );
            currentSessionId = owner.sessionId;
            currentDaemonId = owner.daemonId;
            currentEnvironment = owner.environment;
            if (typeof opts.onSession === "function")
              opts.onSession(currentSessionId, c.payload);
            // A launch that joined a session another page drives is read-only
            // from the start; the daemon says so here.
            if (c.payload?.watch === true && !watching) {
              watching = true;
              term.write("\r\n[read-only: another window has control]\r\n");
              if (typeof opts.onPark === "function") opts.onPark("joined");
            }
          } else if (c && c.verb === "session-end") {
            announced = c.payload?.reason ?? "child-exited";
            refusal = typeof c.payload?.message === "string" ? c.payload.message : null;
          }
        }
      };
      // Swallow the error event; onclose drives recovery in every case.
      ws.onerror = () => {};
      ws.onclose = (event: any) => {
        if (leaving || switching) return;
        if (!opened) failedReopens += 1;
        const decision = reconnectDecision({
          everOpened,
          announced,
          idKnown: currentSessionId != null,
          failedReopens,
        });
        switch (peerGate({ decision, group: peerGroup(), id: currentSessionId })) {
          case "hold":
            if (!opened) failedReopens -= 1;
            hold(peerGroup());
            return;
          case "give-up":
            giveUp();
            return;
          case "park-as-watcher":
            // Park ONCE, immediately. A watch socket that itself drops falls
            // back to the backoff, so a refused watch cannot busy-loop.
            if (!watching) {
              watching = true;
              if (typeof opts.onPark === "function") opts.onPark(announced);
              connect({ id: currentSessionId, repo: currentRepo, watch: true });
            } else {
              scheduleReconnect();
            }
            return;
          default:
            if (retryDelay === 0) {
              term.write("\r\n[connection lost — reconnecting…]\r\n");
              // The fleet may already know why; its answer can hold this window.
              if (typeof opts.readFleet === "function") opts.readFleet();
            }
            scheduleReconnect();
        }
      };
    }

    // Every byte this window sends to the child goes through here (keyboard,
    // key bar, paste), so the watched gate and the Ctrl latch apply to all.
    let ctrlLatched = false;
    let shiftLatched = false;
    function sendInput(raw: any) {
      const folded = applyCtrlLatch(ctrlLatched, raw);
      ctrlLatched = folded.latched;
      if (typeof opts.onCtrlLatch === "function") opts.onCtrlLatch(ctrlLatched);
      const d = folded.out;
      // The daemon-side drop in `Attachment::write` (session.rs) stays as
      // defence in depth; this gate makes the refusal VISIBLE (#335).
      if (watching) {
        if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
        return false;
      }
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(encodeTerminal(d));
        return true;
      }
      return false;
    }

    // The backlog's old queries are answered again during a replay; those
    // answers would reach the child as typed input.
    term.onData((d: any) => {
      if (replaying && isTerminalReply(d)) return;
      sendInput(d);
    });
    term.onResize(({ rows, cols }: any) => {
      if (ws && ws.readyState === WebSocket.OPEN)
        ws.send(encodeResize(rows, cols));
    });

    if (peerGate({ decision: "connect", group: peerGroup(), id: opts.id }) === "hold") hold(peerGroup());
    else connect(opts);

    return {
      term,
      fit,
      useGpu,
      dropGpu,
      get ws() {
        return ws;
      },
      get sessionId() {
        return currentSessionId;
      },
      get daemonId() {
        return currentDaemonId;
      },
      get environment() {
        return currentEnvironment;
      },
      get watching() {
        return watching;
      },
      // A key-bar tap, through `sendInput`: refused for a watcher like a
      // keystroke, and `Ctrl` then `c` folds through the same latch.
      // The Shift latch lives here, not in `sendInput`: the virtual keyboard
      // has its own Shift, so only the next BAR key consumes it.
      sendKey(name: any) {
        if (name === "ctrl") {
          ctrlLatched = !ctrlLatched;
          if (typeof opts.onCtrlLatch === "function") opts.onCtrlLatch(ctrlLatched);
          return ctrlLatched;
        }
        const step = barKey(name, !!term.modes?.applicationCursorKeysMode, shiftLatched);
        if (step.latched !== shiftLatched) {
          shiftLatched = step.latched;
          if (typeof opts.onShiftLatch === "function") opts.onShiftLatch(shiftLatched);
        }
        if (name === "shift") return shiftLatched;
        return step.seq ? sendInput(step.seq) : false;
      },
      get ctrlLatched() {
        return ctrlLatched;
      },
      get shiftLatched() {
        return shiftLatched;
      },
      // Arm (or disarm) the line-selection gesture. NOT gated on `watching`:
      // a selection is a read, and a watcher may copy what it sees.
      setSelecting,
      // The paste key's image: the same drop as a keyboard paste, same gate.
      pasteImage(blob: any) {
        return dropImage([blob.type], blob);
      },
      get selecting() {
        return selecting;
      },
      // The page came back from a suspend (or the network did). Returns whether
      // it acted. The `currentSessionId == null` bail is load-bearing: a window
      // not yet told its id would compose a LAUNCH url and spawn a second
      // vendor CLI (`reconnectDecision` R1, `takeOver`).
      resume(stale: any) {
        if (leaving || ended || currentSessionId == null) return false;
        // Held: the fleet decides, so ask it rather than dial a peer it calls
        // down. Its answer reaches `peerRefresh`.
        if (held) {
          if (typeof opts.readFleet === "function") opts.readFleet();
          return false;
        }
        const now = Date.now();
        if (now - lastResumeAt < RESUME_DEBOUNCE_MS) return false;
        // A pending backoff is brought forward. `retryDelay` is kept: it stops
        // "[connection lost]" printing twice for one drop.
        if (retryTimer) {
          lastResumeAt = now;
          clearTimeout(retryTimer);
          retryTimer = null;
          connect({ id: currentSessionId, repo: currentRepo, watch: watching });
          return true;
        }
        const readyState = ws ? ws.readyState : null;
        const connectingMs = now - connectingSince;
        if (resumeDecision({ readyState, stale, connectingMs }) === "none") return false;
        lastResumeAt = now;
        detachSocket(ws, "reconnect");
        connect({ id: currentSessionId, repo: currentRepo, watch: watching });
        return true;
      },
      // The ONLY place in this file that sets `takeover` — from the parked
      // banner's button. `switching` makes the current socket's onclose a no-op.
      takeOver() {
        if (currentSessionId == null) return;
        switching = true;
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        // Detach EVERY handler before closing: the events land AFTER this
        // returns, when `switching` is false again and `ws` names the new
        // socket. A queued `session-end` would otherwise attach a stale reason
        // to the NEW connection and turn its next drop into a give-up.
        detachSocket(ws, "reconnect");
        watching = false;
        announced = null;
        refusal = null;
        failedReopens = 0;
        retryDelay = 0;
        switching = false;
        if (typeof opts.onResume === "function") opts.onResume();
        connect({ id: currentSessionId, repo: currentRepo, takeover: true });
      },
      // A fleet read arrived. A held window goes back when its peer is
      // available, or when the fleet no longer lists it (the ordinary retry
      // decides then). A window in its backoff holds when the peer is down.
      peerRefresh() {
        if (leaving || ended) return;
        const group = peerGroup();
        const gate = peerGate({ decision: "reconnect", group, id: currentSessionId });
        if (held) {
          if (gate === "hold") opts.onPeerHold?.(group);
          else release();
          return;
        }
        if (retryTimer && gate === "hold") hold(group);
      },
      // `reason` (see `encodeDetach`) tells the daemon why the socket closes.
      dispose(reason: any) {
        leaving = true;
        if (held) {
          held = false;
          if (typeof opts.onPeerBack === "function") opts.onPeerBack();
        }
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        ro.disconnect();
        // A glide or a pending scroll would call `scrollLines` on a disposed
        // terminal.
        stopFling();
        stopScroll();
        if (ws && ws.readyState <= 1) {
          try {
            announceDetach(ws, reason);
          } catch {}
          ws.close();
        }
        dropGpu();
        term.dispose();
      },
    };
  }

  // `.session-window`'s CSS floor (`styles.css`, pinned by
  // `shell_arranges_into_the_fence`). It OUTRANKS an inline width (MEASURED: a
  // 176x116 cell rendered 240x150, 52 px past its fence). Declared HERE, above
  // `buildChrome`: a `const` below would be in its temporal dead zone for a
  // spawn on the boot stack.
  const WIN_MIN_W = 240;
  const WIN_MIN_H = 150;

  // Where a console born into a focused fence lands (#343), pure: fence rect,
  // cascade index and head-band height in, one box out. The box may shrink
  // BELOW the CSS floor for a small fence; the caller relaxes
  // `minWidth`/`minHeight` for exactly those axes. `roomX`/`roomY` cap the
  // cascade offset: a bare `k * step` walks out of a small fence.
  const SPAWN_PAD = 12;
  const SPAWN_STEP = 24;

  function spawnRectIn(fence: any, index: any, headH: any) {
    const f = fence || {};
    const fl = f.left || 0;
    const ft = f.top || 0;
    const fw = f.width || 0;
    const fh = f.height || 0;
    const head = headH || 0;
    const width = Math.max(1, Math.min(560, fw - SPAWN_PAD * 2));
    const height = Math.max(1, Math.min(340, fh - head - SPAWN_PAD * 2));
    const k = (index || 0) % 8;
    const roomX = Math.max(0, fw - SPAWN_PAD * 2 - width);
    const roomY = Math.max(0, fh - head - SPAWN_PAD * 2 - height);
    // The outer `Math.min` is for the DEGENERATE fence narrower than the pad
    // pair: the pad alone would push the box past the far edge, and the
    // centre-based fold would report the newborn console in NO fence.
    const offX = Math.min(SPAWN_PAD + Math.min(k * SPAWN_STEP, roomX), Math.max(0, fw - width));
    const offY = Math.min(
      head + SPAWN_PAD + Math.min(k * SPAWN_STEP, roomY),
      Math.max(0, fh - height),
    );
    return { left: fl + offX, top: ft + offY, width, height };
  }

  // Where a console born OUTSIDE a fence lands, pure: viewport (offset and
  // size), cascade index and the fence records in, one box out. A console is
  // never born held by a LOCKED fence: it would wear that lock at once, and
  // the operator could not drag it out. The cascade steps past a slot whose
  // centre a locked fence holds (the `fenceHolds` fold); when every slot is
  // held, the box moves right of the fence that holds it until one is free.
  // `moved` says the box left the viewport's cascade, so the caller reveals it.
  function freeSpawnRect(view: any, index: any, fences: any) {
    const v = view || {};
    // An unmeasurable viewport is a tab still `display:none`: plain caps.
    const width = v.width ? Math.max(WIN_MIN_W, Math.min(560, Math.round(v.width * 0.62))) : 560;
    const height = v.height ? Math.max(WIN_MIN_H, Math.min(340, Math.round(v.height * 0.6))) : 340;
    const at = (k: any) => ({
      left: Math.max(0, v.left || 0) + 30 + (k % 8) * SPAWN_STEP,
      top: Math.max(0, v.top || 0) + 20 + (k % 8) * SPAWN_STEP,
      width,
      height,
    });
    const start = index || 0;
    for (let i = 0; i < 8; i++) {
      const rect = at(start + i);
      if (!fenceHolds(fences, rect, false)) return { rect, moved: false };
    }
    const rect = at(start);
    // Each step leaves one fence behind for good, so the walk ends within one
    // step per fence.
    for (let i = 0; i <= (fences || []).length; i++) {
      const held = fenceOf(fences, rect);
      if (!held?.locked) break;
      rect.left = (held.rect?.left || 0) + (held.rect?.width || 0) + SPAWN_PAD;
    }
    return { rect, moved: true };
  }

  // The floating-window chrome, shared by a live console and a placeholder:
  // rect (from a desk record, else cascaded), titlebar, body, eight resize
  // handles. `desk` is a record (or a partial carrying at least `kind`);
  // everything the record needs later is hung off the element.
  function buildChrome(label: any, repo: any, desk: any, kind: any) {
    const win = document.createElement("div");
    win.className = "session-window";
    // Every field this element will carry is written HERE, by the module that
    // declares them (wb-window-state.ts): a field nothing seeds reads as its
    // declared default, not `undefined`.
    initWindow(win, {
      _deskId: desk?.id || newDeskId(),
      _deskRepo: repo || "~",
      _deskAgent: label,
      _deskKind: desk?.kind || kind,
      _deskDaemonId: desk?.daemonId ?? null,
      _deskEnvironment: desk?.environment ?? null,
      // The worktree (#411), seeded from the record; the launch request and
      // then the daemon's `session-open` overwrite it.
      _deskCheckout: desk?.checkout ?? null,
      // Locked in place (ADR-0050 lock amendment), seeded from the record.
      _deskLocked: !!desk?.locked,
      // The console name (ADR-0066 §2): a carried record keeps its own; a new
      // console takes the lowest free number of its prefix.
      _deskConsoleName:
        desk?.consoleName || WBConsoleName.defaultName(consolePrefix(repo), takenNames(null)),
    });
    const rect = desk?.rect;
    // Set when the free cascade had to leave the viewport (`freeSpawnRect`).
    let spawnMoved = false;
    if (rect) {
      win.style.left = rect.left + "px";
      win.style.top = rect.top + "px";
      win.style.width = rect.width + "px";
      win.style.height = rect.height + "px";
    } else {
      cascade = (cascade + 1) % 8;
      // Born INTO the focused fence when there is one (#343). The rect is
      // written at CONSTRUCTION so a record never snapshots mid-transition
      // (#342). MEASURABLE, not merely present: `openConsoleItem` calls
      // `activate` then `open` on the SAME synchronous stack, so a spawn can
      // land while the tab is still `display:none` and `restoreRect` reads all
      // zeros — a 1x1 window persisted to the shared desk. Fall back to the
      // free cascade; the focus survives for the next spawn.
      // A LOCKED focused fence is not a host: the console would be born held
      // by its lock. It takes the free cascade instead.
      const el = focusedFence && !fenceLocked(focusedFence) && fenceEl(focusedFence);
      const host = el && el.offsetWidth && el.offsetHeight ? el : null;
      if (host) {
        const headH = host.querySelector(".fence-head")?.offsetHeight || 28;
        const box = spawnRectIn(restoreRect(host), cascade, headH);
        win.style.left = box.left + "px";
        win.style.top = box.top + "px";
        win.style.width = box.width + "px";
        win.style.height = box.height + "px";
        // The CSS floor outranks the inline size (#342); relax it for exactly
        // the axes below it.
        if (box.width < WIN_MIN_W) win.style.minWidth = box.width + "px";
        if (box.height < WIN_MIN_H) win.style.minHeight = box.height + "px";
      } else {
        // The free cascade is anchored at the VIEWPORT's current offset, not
        // the plane's origin, and sized from the viewport, not the stage
        // (which `applyExtent` grows well past it).
        const ws = workspace();
        const view = {
          left: ws?.scrollLeft || 0,
          top: ws?.scrollTop || 0,
          width: ws?.clientWidth || 0,
          height: ws?.clientHeight || 0,
        };
        // The popup gets no fences: its members' rects are re-origined, so the
        // fold would match the wrong fence (`fenceHolds`).
        const spawn = freeSpawnRect(view, cascade, OPTS.autoBoot === false ? [] : fences);
        win.style.left = spawn.rect.left + "px";
        win.style.top = spawn.rect.top + "px";
        win.style.width = spawn.rect.width + "px";
        win.style.height = spawn.rect.height + "px";
        spawnMoved = spawn.moved;
      }
    }

    const titlebar = document.createElement("div");
    titlebar.className = "session-titlebar";
    // The agent's state as a dot before the label (ADR-0059). Hidden until a
    // session row says something; a shell console never does.
    const stateDot = document.createElement("span");
    stateDot.className = "session-state";
    stateDot.hidden = true;
    win._stateDot = stateDot;
    const title = document.createElement("span");
    title.className = "session-title";
    const presentation = sessionPresentation(label, repo, desk, null);
    win._title = title;
    renderTitle(win, title, presentation);
    title.title = presentation.tooltip;
    const actions = document.createElement("span");
    actions.className = "session-actions";
    // Open another console beside this maximized one (ADR-0051 §5). Shown and
    // enabled by `applyColumns`. The torn-off fence window calls it too, and
    // never shows the button: it boots without `autoBoot`.
    const colBtn = document.createElement("button");
    colBtn.className = "session-column";
    colBtn.title = "Slice";
    colBtn.innerHTML = '<i class="bi bi-arrows-expand-vertical"></i>';
    colBtn.hidden = true;
    win._colBtn = colBtn;
    // Restart is offered on a live session too, behind a confirm
    // (`restartWin`); hidden only where nothing can launch (the popup).
    const restartBtn = document.createElement("button");
    restartBtn.className = "session-restart";
    restartBtn.title = "Restart session";
    restartBtn.innerHTML = '<i class="bi bi-arrow-clockwise"></i>';
    restartBtn.hidden = OPTS.canLaunch === false;
    const maxBtn = document.createElement("button");
    maxBtn.className = "session-max";
    maxBtn.title = "Maximize";
    maxBtn.innerHTML = '<i class="bi bi-fullscreen"></i>';
    win._maxBtn = maxBtn;
    // Fullscreen is orthogonal to maximize (viewport vs physical screen). Built
    // only where the browser can HOLD it (`fullscreenOffered`).
    const fullBtn = document.createElement("button");
    fullBtn.className = "session-full";
    fullBtn.title = "Full screen";
    fullBtn.innerHTML = '<i class="bi bi-arrows-fullscreen"></i>';
    fullBtn.hidden = !fullscreenOffered(document.fullscreenEnabled, navigator.vendor);
    const closeBtn = document.createElement("button");
    closeBtn.className = "session-close";
    closeBtn.title = "Close";
    closeBtn.innerHTML = '<i class="bi bi-x-lg"></i>';
    // Lock in place. Glyph and title painted by `applyLock`.
    const lockBtn = document.createElement("button");
    lockBtn.className = "session-lock";
    actions.append(colBtn, fullBtn, maxBtn, restartBtn, lockBtn, closeBtn);
    // The dot sits WITH the title: the bar is space-between.
    const head = document.createElement("span");
    head.className = "session-head";
    head.append(stateDot, title);
    titlebar.append(head, actions);

    const body = document.createElement("div");
    body.className = "session-body";
    const grip = document.createElement("div");
    grip.className = "session-resize";
    win.append(titlebar, body, grip);
    // Eight handles; the corner grip above is decoration only.
    for (const dir of DIRS) {
      const h = document.createElement("div");
      h.className = `session-handle h-${dir}`;
      h.addEventListener("pointerdown", startResize(win, dir));
      win.append(h);
    }
    stage().append(win);
    applyExtent();

    // Pointer: a touch raises the window on contact, not after the tap resolves.
    win.addEventListener("pointerdown", () => focusWin(win));
    makeDraggable(win, titlebar);
    // Maximize/restore: the button, a double-click on the titlebar, or a
    // double tap on it. The shell owns the columns, so a column's restore is
    // its decision.
    const maxOrRestore = () => {
      if (win.classList.contains("column")) {
        document.dispatchEvent(
          new CustomEvent("workbench:column-restore", { detail: { id: win._deskId } }),
        );
        return;
      }
      toggleMax(win);
      document.dispatchEvent(new CustomEvent("workbench:columns-stale"));
    };
    maxBtn.addEventListener("click", (e: any) => {
      e.stopPropagation();
      maxOrRestore();
    });
    titlebar.addEventListener("dblclick", (e: any) => {
      // Fullscreen hides the maximize control; a double-click must not toggle
      // it unseen underneath.
      // Nor may a double-click on the name, which renames (ADR-0066 §3).
      if (e.target.closest("button, .session-name, .session-name-input") || isFull(win)) return;
      // A finger's double tap is `wireTitleTouch`'s.
      if (win._lastPointerType !== "mouse") return;
      maxOrRestore();
    });
    wireTitleTouch(win, titlebar, maxOrRestore);
    colBtn.addEventListener("click", (e: any) => {
      e.stopPropagation();
      document.dispatchEvent(
        new CustomEvent("workbench:column-open", {
          detail: { id: win._deskId, rect: colBtn.getBoundingClientRect() },
        }),
      );
    });
    lockBtn.addEventListener("click", (e: any) => {
      e.stopPropagation();
      toggleLock(win);
    });
    applyLock(win, !!desk?.locked);
    fullBtn.addEventListener("click", (e: any) => {
      e.stopPropagation();
      toggleFull(win);
    });
    // Re-apply a persisted maximized state (the inline rect above is the box it
    // restores to).
    if (rect && desk.max) setMax(win, true);
    // A console pushed past a locked fence may be out of view: slide to it.
    if (spawnMoved) reveal(win._deskId);
    else focusWin(win);
    return { win, body, title, restartBtn, fullBtn, lockBtn, maxBtn, closeBtn };
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
    if (birthDecision({ id: termOpts.id, observed: !!dormancyWatch() }) === "dormant") {
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
      untrackDormancy(win);
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
        untrackDormancy(win);
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
    trackDormancy(win);
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
      untrackDormancy(win);
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
    trackDormancy(win);
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
        for (const id of detached) {
          const entry = fencePopups.get(id);
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
            const entry = fencePopups.get(away.get(record.id));
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
        if (detached.length) commitDetached(detached);
        renderFences();
        renderNotes();
        // The glyph is DOM the fences own: lit only once they are on the stage.
        for (const id of detached) showDetachGlyph(id, true);
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
        for (const id of detached) showDetachGlyph(id, true);
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
  // BEFORE `restoreDesk` is issued, so `detached` is already true when that
  // fetch decides which records to put on the plane.
  function restoreDetached() {
    const saved = link.readRegistry();
    if (!saved.length) return;
    const savedMembers = link.readMembers();
    for (const id of saved) {
      // No handle (it died with the document), only member ids. `popup-here`
      // hands the snapshot back; `restoreDesk` seeds a fallback from the
      // records it skips.
      fencePopups.set(id, newPopupEntry(savedMembers[id] || []));
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

  // Tile ONE fence's members into its own rect (#342); windows animate via
  // the `.tiling` CSS transition. The grid is inset by the fence's OWN chrome:
  // the head band and the SE `.fence-grip` sit BELOW every window, so a member
  // parked on either makes the fence's controls unhittable.
  const FENCE_GRIP = 14;
  function arrangeFence(id: any) {
    // Detached: tiling the empty box would rewrite the rects the popup will
    // restore from (ADR-0051 §7a).
    if (detached.includes(id)) return;
    // A locked fence keeps its layout: tiling would rewrite every member's rect.
    if (fenceLocked(id)) return;
    const st = stage();
    const el = fenceEl(id);
    if (!st || !el) return;
    const rect = restoreRect(el);
    const all = [...st.querySelectorAll(".session-window")].map((w) => ({
      el: w,
      id: w._deskId,
      rect: restoreRect(w),
    }));
    // The FULL fence list with this fence's LIVE rect: the fold's `break`
    // decides an overlapping pair, and a singleton bypasses it.
    const live = fences.map((x: any) => (x.id === id ? { id: x.id, rect } : x));
    const ids = new Set(fenceMembership(live, all)[id] || []);
    // A maximized console is NOT tiled: a tile rect written onto it is
    // invisible while it REPLACES the pre-maximize rect. Filtered before the
    // grid so it stays hole-free (#338). A LOCKED console is skipped too.
    const members = all
      .filter(
        (m) =>
          ids.has(m.id) &&
          !m.el.classList.contains("maximized") &&
          !m.el.classList.contains("column") &&
          !m.el._deskLocked,
      )
      .map((m) => m.el);
    // An empty fence is a NO-OP, not an error.
    if (!members.length) return;
    const headH = el.querySelector(".fence-head")?.offsetHeight || 28;
    const tiles = tileIntoRect(
      {
        left: rect.left,
        top: rect.top + headH,
        width: rect.width,
        height: Math.max(0, rect.height - headH - FENCE_GRIP),
      },
      members,
    );
    members.forEach((win, i) => {
      const t = tiles[i];
      // The computed tile, written now: a read after the transition would
      // race a hidden tab and a convergence.
      setWin(win, { rect: { left: t.left, top: t.top, width: t.width, height: t.height } });
      win.classList.add("tiling");
      win.style.left = t.left + "px";
      win.style.top = t.top + "px";
      win.style.width = t.width + "px";
      win.style.height = t.height + "px";
      // Relaxed to the cell for tiles below the floor, CLEARED otherwise.
      win.style.minWidth = t.width < WIN_MIN_W ? t.width + "px" : "";
      win.style.minHeight = t.height < WIN_MIN_H ? t.height + "px" : "";
      focusWin(win);
      setTimeout(() => win.classList.remove("tiling"), 260);
    });
    // A maximized console must not be BURIED by the tiles: `maxlock` leaves no
    // way to scroll away from a full bleed whose titlebar is covered.
    for (const win of wins) {
      if (win.classList.contains("maximized") || win.classList.contains("column")) focusWin(win);
    }
    // AFTER the 0.24s tiling transition: an immediate fold would measure the
    // pre-arrange boxes.
    setTimeout(() => {
      for (const win of members) {
        try {
          win._term?.fit.fit();
        } catch {}
      }
      refreshFenceChrome();
      applyExtent();
    }, 300);
  }

  function count() {
    return wins.size;
  }

  // Re-read the daemon's desk and restore it: the pre-login `/api/desk`
  // answered 401 under `Session`. Called from `rehydrateAfterAuth` (#327).
  function afterLogin() {
    return reloadDesk().then(() => {
      // `fencePopups.size` counts as "windows already up": detaching every
      // fence drives `wins.size` to 0, and a `restoreDesk` would respawn the
      // popups' members.
      if (deskLoaded && wins.size === 0 && fencePopups.size === 0) {
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
