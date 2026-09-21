/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Consoles tab)

   Consoles are draggable, resizable windows on the STAGE, a plane the VIEWPORT
   (`#workspace`, `overflow:auto`) scrolls over. This module owns the window
   chrome (stage-relative drag/resize/tiling, the stage's extent); the body is a
   live xterm.js on a PTY over the daemon's `/ws/session` WebSocket.

   Opening/closing a console spawns/closes a daemon-owned session; on load the
   live sessions re-open as windows, so a reload reattaches with scrollback.
--------------------------------------------------------------------------- */
window.WBConsole = (function () {
  // Plane geometry is `wb-geometry.js` (ADR-0057): pure folds over rects.
  // Destructured, and with no `{}` fallback: a missing `<script>` tag throws
  // here naming the module, not `stageExtent is not a function` at first drag.
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
  } = window.WBGeometry;

  // What a console window IS and what it HOLDS (`wb-window-state.js`). No
  // fallback, for the same reason as above.
  const { initWindow, sessionIdOf, watchingOf, windowCheckout } = window.WBWindowState;

  // The viewport (the scrolling box) and the stage (the sized plane inside it).
  const workspace = () => document.getElementById("workspace");
  const stage = () => document.getElementById("stage");
  // Scheme-match the session socket to the page (see wb-daemon.js WS_ORIGIN):
  // `wss://` over a TLS dev-tunnel/proxy, `ws://` for a plain-http localhost bind.
  const WS_ORIGIN =
    (location.protocol === "https:" ? "wss://" : "ws://") + location.host;
  // Injection point, read ONCE at load. `index.html` sets nothing (every default
  // below is the shell's behaviour); `detached-fence.html` overrides all five,
  // which is what makes the popup unable to author the desk or the viewport.
  const OPTS = window.WBConsoleOpts || {};
  const deskSink = OPTS.deskSink || window.WBDeskSink.daemon();
  const viewStore = OPTS.viewStore || window.WBView;
  // Detach registry + lifecycle channel (#347). Denied in the popup: `window.open`
  // hands it a COPY of the opener's session-scoped store, so a read there drifts.
  // This module names no browser store of its own — pinned in lib.rs.
  const link = OPTS.detachLink || window.WBDetachLink.link();
  const wins = new Set();

  // ---- dormant consoles ----------------------------------------------------
  // Every console costs an xterm buffer, a ResizeObserver, a WebGL context and
  // the parse+paint of every byte the daemon sends, visible or not. LIMIT: Chrome
  // caps a document at ~16 live WebGL contexts; past it the addon loses its
  // context (`onContextLoss` in `attachTerminal`) and EVERY terminal falls to
  // the DOM renderer.
  //
  // So a window off the viewport long enough disposes its terminal and closes
  // its socket, and rebuilds on return. The SESSION is untouched — child, PTY and
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
  let dormancyObserver = null;
  function dormancyWatch() {
    if (dormancyObserver) return dormancyObserver;
    if (typeof IntersectionObserver !== "function") return null;
    const root = workspace();
    if (!root) return null;
    dormancyObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          entry.target._visible = entry.isIntersecting;
          applyDormancy(entry.target);
        }
      },
      { root, rootMargin: `${DORMANT_MARGIN_PX}px`, threshold: 0 },
    );
    return dormancyObserver;
  }
  function trackDormancy(win) {
    dormancyWatch()?.observe(win);
  }
  // Paired with every `wins.delete`: the observer holds its targets, so a window
  // taken off the plane without this stays reachable for the life of the page.
  function untrackDormancy(win) {
    if (win._dormantTimer) {
      clearTimeout(win._dormantTimer);
      win._dormantTimer = null;
    }
    dormancyObserver?.unobserve(win);
  }

  // Focus stacking. `z` climbs each time a window is raised; when it reaches the
  // ceiling the whole stack is renormalized back down (preserving order) so the
  // console z-index never overtakes the runs overlay (z 150) or the tabbar.
  const Z_BASE = 60;
  const Z_CEIL = 120;
  let z = Z_BASE;
  let cascade = 0;

  function changed() {
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
  function askConfirm({ title, message, confirmLabel = "Confirm", danger = false, notice = false }) {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-confirm";
    const modal = document.createElement("div");
    modal.className = "modal confirm-modal";
    modal.setAttribute("role", "alertdialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", title);
    const head = document.createElement("div");
    head.className = "modal-head";
    head.innerHTML =
      `<i class="bi ${danger ? "bi-exclamation-triangle" : "bi-question-circle"}"` +
      `${danger ? ' style="color: var(--danger)"' : ""}></i>`;
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
      const done = (ok) => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        scrim.remove();
        resolve(ok);
      };
      const onKey = (e) => {
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
      scrim.addEventListener("mousedown", (e) => {
        if (e.target === scrim) done(false);
      });
      cancel.addEventListener("click", () => done(false));
      go.addEventListener("click", () => done(true));
    });
  }

  // A message with an OK and nothing else (a verb's refusal, verbatim).
  function askNotice({ title, message, danger = true }) {
    return askConfirm({ title, message, confirmLabel: "OK", danger, notice: true });
  }

  // ---- the desk layout ---------------------------------------------------------
  // What was open, not merely where a session sat: one record per window keyed
  // by a STABLE client-side id (repo, agent, session kind, rect, maximized).
  // The daemon's session id is a volatile ATTRIBUTE — a restarted daemon hands
  // out ids from 1 again. The desk lives in the DAEMON (`GET`/`PUT /api/desk`,
  // ADR-0050); `desk` is the in-memory mirror and the SYNCHRONOUS source of
  // truth, which keeps `persistWin`/`forgetRecord`/`deskOf` callable from a
  // mousemove. Capped so it cannot grow without bound.
  const DESK_MAX = 24;
  let desk = [];
  // The upload PERMIT: `PUT /api/desk` replaces the desk wholesale, so nothing
  // flushes until the daemon's own desk has landed. Under the `Session` policy
  // the pre-login GET answers 401; treating that as "empty" and flushing would
  // destroy the layout on the first drag, so a refused load leaves this false
  // until `reloadDesk()` succeeds after login.
  let deskLoaded = false;
  // Mutated since the load was issued: a record created or deleted here must
  // survive a later-arriving GET.
  let deskDirty = false;

  // Ids this page deleted; they must not come back on a later-arriving GET.
  const deskRemoved = new Set();

  // Second record type (#340): named rectangles on the floor tier. Same store,
  // route and upload permit as `desk`.
  const FENCE_MAX = 12;
  let fences = [];
  let fencesDirty = false;
  // Fence ids this page deleted; same role as `deskRemoved`.
  const fencesRemoved = new Set();

  // Third record type (#406, ADR-0063 §4): the selected checkout per repo ref,
  // `{ <ref>: <worktree name> }`. Same store, route and permit. The reactive copy
  // the chip and the tree render lives in `app.js` (a closure variable here is
  // invisible to Alpine); this is persistence.
  let checkouts = {};
  let checkoutsDirty = false;
  // Refs this page cleared; they must not come back on a later-arriving GET.
  const checkoutsRemoved = new Set();

  // Per id, NOT a wholesale replace: a fence drawn before this page's own GET
  // lands (the toolbar is live before `deskReady` resolves) would otherwise be
  // discarded, and the next flush would write the loss through. `deskLoaded`
  // does not cover this — it lifts AFTER the discard.
  function ingestFences(fetched) {
    if (!fencesDirty) {
      fences = fetched;
      return;
    }
    const mine = new Set(fences.map((f) => f.id));
    fences = fetched
      .filter((f) => !mine.has(f.id) && !fencesRemoved.has(f.id))
      .concat(fences);
  }

  // The `ingestFences` rule per ref: a selection made here wins, a ref cleared
  // here stays cleared, the daemon's other refs come in. An old daemon sends no
  // `checkouts` at all.
  function ingestCheckouts(fetched) {
    if (!checkoutsDirty) {
      checkouts = { ...fetched };
      return;
    }
    const merged = {};
    for (const [ref, name] of Object.entries(fetched)) {
      if (!(ref in checkouts) && !checkoutsRemoved.has(ref)) merged[ref] = name;
    }
    checkouts = { ...merged, ...checkouts };
  }

  function ingestDesk(payload) {
    const fetched = Array.isArray(payload?.windows) ? payload.windows : [];
    ingestFences(Array.isArray(payload?.fences) ? payload.fences : []);
    const fetchedCheckouts = payload?.checkouts;
    ingestCheckouts(
      fetchedCheckouts && typeof fetchedCheckouts === "object" && !Array.isArray(fetchedCheckouts)
        ? fetchedCheckouts
        : {},
    );
    if (!deskDirty) {
      desk = fetched;
    } else {
      desk = mergeDesk(desk, fetched, deskRemoved);
    }
    deskLoaded = true;
    applyLocksFromMirror();
  }

  // The lock is the ONE record field applied from the mirror onto a live window
  // without a reload: otherwise this page's next drag uploads `locked:false`
  // with a newer `ts` and wins the fold over another page's lock. Rects are NOT
  // applied here — a window mid-gesture must not be yanked by a flush's
  // read-before-write.
  function applyLocksFromMirror() {
    // The mirror is exercised without a document (the node table), and a page
    // ingests its first GET before the stage exists.
    if (typeof document?.getElementById !== "function") return;
    const st = stage();
    if (!st) return;
    const byId = new Map(desk.map((r) => [r.id, r]));
    for (const w of st.querySelectorAll(".session-window")) {
      const r = byId.get(w._deskId);
      if (r && !!r.locked !== !!w._deskLocked) applyLock(w, !!r.locked);
    }
    for (const el of st.querySelectorAll(".fence")) {
      const f = fences.find((x) => x.id === el.dataset.fenceId);
      if (f) paintFenceLock(el, !!f.locked);
    }
    refreshFenceChrome(); // the `held` class on a locked fence's members
  }

  // Per id, newest `ts` wins. "Local wins per id" wrote a stale mirror back over
  // another page's `sessionId`, and the next load adopted the orphaned session
  // into a fresh record — one console twice (ADR-0050 §2 assumed one page). A
  // record this page deleted stays deleted; the daemon's other records come in,
  // in its order, with this page's own after them.
  function mergeDesk(local, fetched, removed) {
    const mine = new Map(local.map((r) => [r.id, r]));
    const out = [];
    for (const theirs of fetched) {
      if (removed.has(theirs.id)) continue;
      const ours = mine.get(theirs.id);
      if (!ours) {
        out.push(theirs);
        continue;
      }
      out.push((theirs.ts || 0) > (ours.ts || 0) ? theirs : ours);
      mine.delete(theirs.id);
    }
    for (const r of local) if (mine.has(r.id)) out.push(r);
    return out;
  }

  // Load (or re-load, after a login) the daemon's desk. Never rejects: an
  // unreachable daemon leaves `deskLoaded` false, which keeps this page from
  // uploading over a desk it never read.
  function reloadDesk() {
    if (!window.WBMode?.isDaemon()) {
      deskLoaded = true;
      return Promise.resolve();
    }
    return fetch("/api/desk")
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error("desk unavailable"))))
      .then(ingestDesk)
      .catch(() => {});
  }
  // `restoreDesk` awaits this before reconciling, so the layout is never
  // reconciled against a desk that has not landed.
  const deskReady = reloadDesk();

  function loadDesk() {
    return desk.slice();
  }
  // Keep the `max` newest records by `ts`, preserving layout order (the order
  // decides which record wins a contended session in `reconcileDesk`). `live`
  // names ids that must NEVER be evicted — a window still on screen losing its
  // record would strand it, unrestorable, on the next load.
  function pruneDesk(records, max, live) {
    if (records.length <= max) return records.slice();
    const pinned = live || new Set();
    const keep = new Set(
      [...records]
        .sort((a, b) => (b.ts || 0) - (a.ts || 0))
        .sort((a, b) => (pinned.has(b.id) ? 1 : 0) - (pinned.has(a.id) ? 1 : 0))
        .slice(0, max),
    );
    return records.filter((r) => keep.has(r));
  }
  function saveDesk(records) {
    // A DETACHED member is not in `wins` — it lives in a popup — and its `ts` is
    // stale, so at `DESK_MAX` it would sort first for eviction and be stranded,
    // unrestorable, the moment it came home. Pin it like any window on screen.
    const live = new Set([...wins].map((w) => w._deskId));
    for (const entry of fencePopups.values()) {
      for (const m of entry.members) live.add(m.id);
    }
    const before = new Set(desk.map((r) => r.id));
    desk = pruneDesk(records, DESK_MAX, live);
    const after = new Set(desk.map((r) => r.id));
    for (const id of before) if (!after.has(id)) deskRemoved.add(id);
    deskDirty = true;
    scheduleDeskFlush();
  }
  // Capped HERE as well as in the daemon: the flush discards the PUT response,
  // so an uncapped client would show 13 fences while the store held 12.
  function saveFences(next) {
    const before = new Set(fences.map((f) => f.id));
    fences = pruneDesk(next, FENCE_MAX);
    const after = new Set(fences.map((f) => f.id));
    for (const id of before) if (!after.has(id)) fencesRemoved.add(id);
    fencesDirty = true;
    scheduleDeskFlush();
  }
  // The selected checkout for one repo ref, or `null` — the primary tree.
  function checkoutOf(ref) {
    return checkouts[ref] || null;
  }
  // Select (`name`) or clear (`null`) a project's checkout and flush. The map
  // is REPLACED, not mutated, so a copy handed out earlier stays what it was.
  function setCheckout(ref, name) {
    if (name) {
      checkouts = { ...checkouts, [ref]: String(name) };
      checkoutsRemoved.delete(ref);
    } else {
      const next = { ...checkouts };
      delete next[ref];
      checkouts = next;
      checkoutsRemoved.add(ref);
    }
    checkoutsDirty = true;
    scheduleDeskFlush();
  }
  function allCheckouts() {
    return { ...checkouts };
  }
  // Resolves once the boot desk load has settled (landed OR refused) — what
  // `app.js` awaits before copying the mirror into its reactive map.
  function whenDeskLoaded() {
    return deskReady;
  }
  // ONE spelling for both flush paths. `checkouts` is always sent (`{}` when
  // empty); the daemon omits it from what it serves when empty.
  function deskBody() {
    // `removed` turns the daemon's PUT from a wholesale replace into a fold
    // (ADR-0050 amendment 2026-09-20): a record ABSENT from the body may be one
    // this page never read, so deletion has to be said. Never pruned.
    return {
      windows: desk,
      fences,
      checkouts,
      removed: { windows: [...deskRemoved], fences: [...fencesRemoved], checkouts: [...checkoutsRemoved] },
    };
  }
  // The upload, debounced and fire-and-forget. WHERE it goes is `deskSink`'s
  // business (wb-desk-sink.js), which also owns the chaining that keeps two
  // mutations 250 ms apart from landing out of order.
  let deskFlush = null;
  function scheduleDeskFlush() {
    if (!window.WBMode?.isDaemon()) return;
    clearTimeout(deskFlush);
    // Cleared when it FIRES too: a spent timer id is still truthy, and `pagehide`
    // would read it as "a write is pending" and re-upload a stale mirror.
    deskFlush = setTimeout(() => {
      deskFlush = null;
      flushDesk();
    }, 250);
  }
  // The flushes of this page, in order — see `flushDesk`.
  let deskWrite = Promise.resolve();
  function flushDesk() {
    // Never upload over a desk this page failed to read (offline, or pre-login
    // under `Session`): the PUT is a wholesale replace.
    if (!deskLoaded) return;
    // Read before write: the mirror is only as fresh as its last GET, and a
    // second page persisting in between would be overwritten. The re-read folds
    // through `mergeDesk`; a failed read uploads the mirror as is. The body is
    // serialised AFTER the fold from `desk` only — persisted rects, never a live
    // measurement (#339). Chained on the previous flush HERE, before the sink's
    // chain: the sink only orders what it is handed, and a slower first read
    // would hand it two flushes out of order.
    deskWrite = deskWrite
      .catch(() => {})
      .then(() => fetch("/api/desk"))
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null)
      .then((payload) => {
        if (payload) ingestDesk(payload);
        const body = JSON.stringify(deskBody());
        return deskSink.put(body);
      });
  }
  // A mutation in the last 250 ms before the tab closes would otherwise be
  // dropped. The sink's `putSync` rides `keepalive`, which outlives the document.
  window.addEventListener("pagehide", () => {
    // The per-client view first, and NOT behind the desk guard: `WBView`'s store
    // is synchronous and `deskLoaded` says nothing about it — gating it would
    // drop the last pan of every pre-login or demo page.
    if (offsetFlush) {
      clearTimeout(offsetFlush);
      offsetFlush = null;
      flushOffset();
    }
    // NOTHING closes a detached popup here: `pagehide` fires on a RELOAD exactly
    // as on a close, with no reliable discriminator (#347). The popup declares
    // its peer lost after `PEER_WINDOW_MS` without a beat and closes itself,
    // which covers a clean close and a force-kill alike (ADR-0051 §8).
    if (!deskLoaded || !deskFlush) return;
    clearTimeout(deskFlush);
    deskFlush = null;
    deskSink.putSync(JSON.stringify(deskBody()));
  });

  // Coming back from a suspend. Registered in EVERY document that runs this
  // module (shell and each popup): each owns the sockets of the windows it
  // paints, and nothing else would revive them.
  let hiddenAt = 0;

  // The verdict the probe gives, or — with no probe, which is the popup — how
  // long this document was hidden. `Infinity` for the network events: `online`
  // fires precisely because the link the sockets ran over is a different link now.
  function isStale(hiddenMs) {
    if (staleProbe) {
      try {
        return staleProbe() === true;
      } catch {
        return true;
      }
    }
    return hiddenMs > RESUME_HIDDEN_MS;
  }

  function resumeAll(stale) {
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
  function sleepWindow(win) {
    const t = win._term;
    if (!t) return false;
    // Carried across the gap, because the handle that knows them is about to go.
    win._dormantSession = t.sessionId;
    win._dormantWatch = t.watching;
    // `reach` finds a window by `_term.sessionId` OR `_wantsSession`. Without
    // this a "go to session" on a sleeping console would miss its own window
    // and spawn a SECOND one against the same id, which the daemon would park.
    if (t.sessionId != null) win._wantsSession = t.sessionId;
    t.dispose();
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
  function wakeWindow(win) {
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
  function applyDormancy(win) {
    const verdict = dormancyDecision(dormancyInputs(win));
    if (win._dormantTimer) {
      clearTimeout(win._dormantTimer);
      win._dormantTimer = null;
    }
    if (verdict === "wake") wakeWindow(win);
    else if (verdict === "sleep") {
      win._dormantTimer = setTimeout(() => {
        win._dormantTimer = null;
        if (dormancyDecision(dormancyInputs(win)) === "sleep") sleepWindow(win);
      }, DORMANT_AFTER_MS);
    }
    return verdict;
  }

  // The live reading of one window, handed to the pure fold.
  function dormancyInputs(win) {
    return {
      // Unobserved windows have never been told; treat them as visible, which
      // is the reading that changes nothing.
      intersecting: win._visible !== false,
      dormant: !!win._dormant,
      maximized: win.classList.contains("maximized"),
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
  });
  // No `pageshow`: a document holding an open WebSocket is not bfcache-eligible,
  // so the restore this would catch cannot happen here.
  window.addEventListener("online", () => resumeAll(true));

  // The key-bar setting changed. The shell re-emits every save on
  // `workbench:action`. A detached popup never receives it (its `WB.emit` posts
  // to the opener) and reads no store, so its bar stays on auto.
  document.addEventListener("workbench:action", (e) => {
    if (e.detail?.action !== "setting-change" || e.detail.key !== "consoles.key_bar") return;
    for (const w of wins) applyKeyBar(w);
  });

  // Publish the keyboard inset. The popup and the node harness load this module
  // without `visualViewport`; absent, `--kb-inset` stays unset, which every
  // `var(--kb-inset, 0px)` already assumes.
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


  function newId(prefix) {
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
  function measurable(win) {
    return !!(win.offsetWidth || win.offsetHeight);
  }
  // Whether all four inline offsets are numbers — the shape `buildChrome`
  // leaves. `parseInt` is deliberate: a `"0px"` is a real 0, not an absence.
  function hasInlineRect(win) {
    return ["left", "top", "width", "height"].every((prop) =>
      Number.isFinite(parseInt(win.style?.[prop], 10)),
    );
  }
  function restoreRect(win) {
    const inline = (prop, fallback) => parseInt(win.style[prop], 10) || fallback;
    const fromInline = () => ({
      left: inline("left", win.offsetLeft),
      top: inline("top", win.offsetTop),
      width: inline("width", win.offsetWidth),
      height: inline("height", win.offsetHeight),
    });
    if (!measurable(win)) return fromInline();
    if (!win.classList.contains("maximized") && !isFull(win)) {
      return {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
    }
    return fromInline();
  }

  // Snapshot a window's placement. A maximized window stores its *pre-maximize*
  // rect (the class drives the full-bleed via CSS), so `max` restores the
  // full-screen state while the stored rect still restores the underlying box.
  function persistWin(win) {
    // A detached window measures 0×0 at 0,0 — and because this upserts by id, a
    // late mouseup after the window was removed would RESURRECT a record that
    // `forgetRecord` just deleted.
    if (!win._deskId || !win.isConnected) return;
    // Unmeasurable AND without an inline rect: there is no honest box to write
    // (`restoreRect` would answer all zeros). The record already on the desk
    // stays as it is, and the next layout act persists a measured one.
    if (!measurable(win) && !hasInlineRect(win)) return;
    const rec = {
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
      ts: Date.now(),
    };
    const records = loadDesk();
    const i = records.findIndex((r) => r.id === rec.id);
    if (i >= 0) records[i] = rec;
    else records.push(rec);
    saveDesk(records);
    // A moved window may have joined or left a region; membership is derived.
    // (No lowercase noun here on purpose: #341's pin greps this body for it.)
    refreshFenceChrome();
  }
  function forgetRecord(deskId) {
    if (!deskId) return;
    saveDesk(loadDesk().filter((r) => r.id !== deskId));
    refreshFenceChrome();
  }

  // The restore decision, a pure fold of the saved layout over the live session
  // list. Each live session is consumed by AT MOST ONE record (first in layout
  // order): a restarted daemon reuses ids, hence the full
  // `sessionId`+`repo`+`agent`+`kind` tuple.
  //
  // `relaunchAgents` is the operator's per-client opt-in (Settings → Consoles),
  // default OFF and passed in so the fold stays pure and the popup can never
  // turn it on.
  function reconcileDesk({ layout, sessions, relaunchAgents = false }) {
    const live = sessions || [];
    const used = new Set();
    const out = [];
    for (const record of layout || []) {
      const i = live.findIndex(
        (s, idx) =>
          !used.has(idx) &&
          s.id === record.sessionId &&
          s.repo === record.repo &&
          s.agent === record.agent &&
          s.kind === record.kind,
      );
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
    live.forEach((s, idx) => {
      if (used.has(idx)) return;
      const waiting = out.find(
        ({ record, action }) =>
          action !== "attach" &&
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
      out.push({ record: null, session: s, action: "adopt" });
    });
    return out;
  }

  // The launch request a desk record relaunches with (#411). The daemon labels
  // a repo-less console "~"; sent back as a slug it hits `unknown repo`, so it
  // relaunches with no repo. An AGENT record asks for its vendor and worktree —
  // `{ console: true }` is the shell request. The checkout rides ONLY on the
  // agent request: the plain console stays on the primary (the `open` rule).
  function relaunchRequest(record) {
    const repo = record.repo === "~" ? undefined : record.repo;
    if (record.kind !== "agent") return { console: true, repo };
    return { repo, agent: record.agent, checkout: record.checkout ?? null };
  }

  // Whether the worktree a relaunch asks for is still there (#411). The daemon
  // refuses a launch into a missing worktree with a `400` BEFORE the socket
  // upgrades, and a browser cannot read that status. So ask first, through the
  // cheapest Observe read that takes a checkout (no spawn): the ONE reply that
  // means the tree is gone is `unknown checkout`. Anything else — an
  // unreachable daemon included — lets the launch decide.
  async function checkoutStillThere(repo, checkout) {
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
  function isUnknownCheckout(reply) {
    return !!reply && reply.status === "error" && reply.message === "unknown checkout";
  }

  // The title says `agent · repo · environment`. The repo is the SLUG, never the
  // ref: a peer ref carries a `<daemon_id>/` routing head (ADR-0052 §5) and the
  // environment segment already says that. The full ref rides `tooltip`.
  function sessionPresentation(label, repo, prior, owner) {
    const daemonId = owner?.daemon_id ?? prior?.daemonId ?? null;
    const environment = owner?.environment ?? prior?.environment ?? null;
    const slug = window.WBFleet ? window.WBFleet.refSlug(repo) : repo;
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
      tooltip: [repo || "", name].filter(Boolean).join("\n"),
      title: [label, checkout, slug || "home", environment].filter(Boolean).join(" · "),
    };
  }

  // ---- the title's worktree segment as a switcher (#412) --------------------
  //
  // Listing per repo ref. Only a repo with at least one worktree gets a
  // switcher. Fed by the shell (`ingestWorktrees`, from every `worktree.list` it
  // reads for the picker) and, for a repo the picker never opened, by ONE read
  // of our own per ref at the first agent window — `worktree.list` is a git
  // spawn, so never per render and never periodic.
  const worktreeListings = {};
  const listingReads = new Map();
  function ingestWorktrees(ref, listing) {
    if (!ref) return;
    worktreeListings[ref] = listing || null;
    for (const win of wins) {
      if (win._deskRepo === ref && win._title && win._presentation) {
        renderTitle(win, win._title, win._presentation);
      }
    }
  }
  function ensureListing(ref, force = false) {
    if (!ref || ref === "~" || (ref in worktreeListings && !force) || listingReads.has(ref)) return;
    const daemon = window.WBDaemon;
    if (typeof daemon?.observe !== "function" || OPTS.canLaunch === false) return;
    const read = daemon
      .observe("worktree.list", { repo: ref })
      .then((reply) => {
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
  function checkoutMenuRows(listing, current, sessions, primaryBranch = "", primaryDirty = false) {
    const rows = [{ name: "primary", branch: String(primaryBranch || ""), dirty: primaryDirty === true, primary: true }];
    for (const w of listing?.worktrees || []) {
      rows.push({
        name: String(w.name || ""),
        branch: String(w.branch || ""),
        dirty: w.dirty === true,
        primary: false,
      });
    }
    const states = sessions && window.WBProject?.worktreeStates ? window.WBProject.worktreeStates(rows, sessions) : {};
    return rows.map((r) => ({ ...r, current: (current ?? "primary") === r.name, state: states[r.name] || null }));
  }
  // The shell's last `/api/sessions` poll, kept for the menus' state dots.
  let lastSessions = [];
  function sessionsOfRepo(ref) {
    const route = window.WBSessionRoute;
    return (lastSessions || []).filter((s) => s && (route ? route.matchesRepo(s, ref) : s.repo === ref));
  }

  // The title: `agent · <checkout> · slug · environment`. On an agentic console
  // the checkout segment is ALWAYS a button (`primary` on the primary tree): it
  // is where the first worktree is born via `+ new worktree…`, so it cannot
  // wait for one to exist (ADR-0063, amendment 2026-09-16 b). Never on a plain
  // shell (stays on the primary, #408), a placeholder (no `_relaunchIn`) or the
  // detached popup (`canLaunch === false`).
  function renderTitle(win, title, presentation) {
    win._presentation = presentation;
    const switchable =
      win._deskKind !== "console" &&
      typeof win._relaunchIn === "function" &&
      OPTS.canLaunch !== false;
    title.textContent = "";
    const icon = document.createElement("i");
    icon.className = "bi bi-terminal";
    title.append(icon, " ");
    // A SPAN, not a bare text node: it is what ellipsises when the bar is
    // narrow (06-consoles.css `.session-title-rest`); a text node inside an
    // inline-flex box wraps instead. The tooltip carries the full form.
    const rest = document.createElement("span");
    rest.className = "session-title-rest";
    if (!switchable) {
      rest.textContent = presentation.title;
      title.append(rest);
      return;
    }
    title.append(`${win._deskAgent} · `);
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "session-checkout";
    btn.title = "switch worktree";
    // Before `session-open` the announcement has not come; the record says
    // where the console was asked to run (#411), never the picker.
    btn.textContent = presentation.checkout ?? win._deskCheckout ?? "primary";
    const caret = document.createElement("i");
    caret.className = "bi bi-chevron-down";
    btn.append(caret);
    btn.addEventListener("pointerdown", (e) => e.stopPropagation());
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openCheckoutMenu(win, btn);
    });
    title.append(btn);
    const slug = (window.WBFleet ? window.WBFleet.refSlug(win._deskRepo) : win._deskRepo) || "home";
    const tail = [slug, presentation.environment].filter(Boolean).join(" · ");
    if (tail) {
      rest.textContent = ` · ${tail}`;
      title.append(rest);
    }
  }

  // The checkout menu — ONE component, under the console's title segment and
  // under the Files bar's chip. `host` is where the element lands (a console
  // window, or the document for the shell) and what it is positioned against;
  // `onPick(row)` for a non-current row; `onRemove(row)` adds a trash action per
  // worktree row; `onCreate()` adds `+ new worktree…`. One menu at a time;
  // closes on a pick, a click elsewhere, or Escape.
  let openMenu = null;
  function closeCheckoutMenu() {
    if (!openMenu) return;
    openMenu.el.remove();
    document.removeEventListener("pointerdown", openMenu.away, true);
    document.removeEventListener("keydown", openMenu.key, true);
    openMenu = null;
  }
  function checkoutMenu({ anchor, host, rows, onPick, onRemove, onCreate }) {
    if (openMenu?.anchor === anchor) return closeCheckoutMenu();
    closeCheckoutMenu();
    const menu = document.createElement("div");
    menu.className = "session-checkout-menu";
    menu.addEventListener("pointerdown", (e) => e.stopPropagation());
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
        state.title = `agent ${row.state}`;
        item.append(state);
      }
      if (row.dirty) {
        const dot = document.createElement("span");
        dot.className = "session-checkout-dirty";
        dot.title = "uncommitted changes";
        item.append(dot);
      }
      if (onRemove && !row.primary) {
        // A span, not a button: a button inside a button is not HTML, and the
        // browser would hoist it out of the row.
        const trash = document.createElement("span");
        trash.setAttribute("role", "button");
        trash.tabIndex = 0;
        trash.className = "session-checkout-remove";
        trash.title = "remove this worktree";
        trash.innerHTML = '<i class="bi bi-trash3"></i>';
        // The trash must not also PICK the row it sits on.
        trash.addEventListener("click", (e) => {
          e.stopPropagation();
          closeCheckoutMenu();
          onRemove(row);
        });
        item.append(trash);
      }
      item.addEventListener("click", (e) => {
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
      create.innerHTML = '<i class="bi bi-folder-plus"></i><span class="session-checkout-name">new worktree…</span>';
      create.title = "cut a new worktree and restart this console in it";
      create.addEventListener("click", (e) => {
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
    const away = (e) => {
      if (!menu.contains(e.target) && !anchor.contains(e.target)) closeCheckoutMenu();
    };
    const key = (e) => {
      if (e.key === "Escape") closeCheckoutMenu();
    };
    document.addEventListener("pointerdown", away, true);
    document.addEventListener("keydown", key, true);
    openMenu = { anchor, el: menu, away, key };
    return menu;
  }
  function openCheckoutMenu(win, anchor) {
    const ref = win._deskRepo;
    checkoutMenu({
      anchor,
      host: win,
      rows: checkoutMenuRows(worktreeListings[ref], win._deskCheckout ?? null, sessionsOfRepo(ref)),
      onPick: (row) => switchCheckout(win, row.primary ? null : row.name),
      onCreate: () => createWorktreeFor(win),
    });
  }

  // Move a console to another checkout (#412): confirm (the session restarts
  // and its scrollback goes), then `moveTo`. The picker's per-repo selection is
  // never touched: that is what Files shows; this is where THIS console lives.
  async function switchCheckout(win, checkout) {
    if (typeof win._relaunchIn !== "function") return;
    const where = checkout ? `worktree ${checkout}` : "the primary tree";
    const ok = await askConfirm({
      title: `Restart in ${checkout ?? "primary"}?`,
      message: `Restarts the ${win._deskAgent} session in ${where}. Scrollback is lost.`,
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
  function moveTo(win, checkout) {
    const from = win._deskCheckout ?? null;
    win._deskCheckout = checkout;
    persistWin(win);
    const go = () => {
      win._relaunchIn(checkout);
      WB.emit("console-switch-checkout", { repo: win._deskRepo, from, to: checkout });
    };
    // A DORMANT console still holds its session and must still close it.
    const id = sessionIdOf(win);
    const live = id != null && !win.classList.contains("ended") && !watchingOf(win);
    if (live && window.WBSessionRoute) {
      fetch(window.WBSessionRoute.closeUrl(id, win._deskRepo), { method: "POST" }).then(go, go);
    } else {
      go();
    }
  }

  // The "new worktree" prompt: a name (worktree AND branch, ADR-0063 §2) and
  // the base branch. The name gate is `WBProject.worktreeCreateRow`; a refusal
  // the daemon DID send (`error`) re-opens with the message under the field.
  // Resolves `{name, base}` or `null` on cancel.
  function askWorktree({ base, branches, listing, error = "", name = "" }) {
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
    nameInput.placeholder = "worktree and branch name";
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
      baseInput.placeholder = "branch to cut from";
    }
    const note = document.createElement("p");
    note.className = "wb-worktree-note";
    note.textContent = `${window.WBProject?.CARRY_OVER_NOTE || ""} The console restarts in the new worktree; its scrollback is lost.`;
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
      const done = (value) => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        scrim.remove();
        resolve(value);
      };
      const submit = () => {
        const row = window.WBProject?.worktreeCreateRow?.(listing || { worktrees: [] }, baseInput.value, nameInput.value);
        if (!row) {
          err.textContent = nameInput.value.trim()
            ? "not a name the daemon takes — one path segment, not a flag, not an existing worktree"
            : "a name is needed";
          err.hidden = false;
          nameInput.focus();
          return;
        }
        done({ name: row.name, base: row.base });
      };
      const onKey = (e) => {
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
      scrim.addEventListener("mousedown", (e) => {
        if (e.target === scrim) done(null);
      });
      cancel.addEventListener("click", () => done(null));
      go.addEventListener("click", submit);
    });
  }

  // `+ new worktree…` from a console's switcher: ask, `worktree.add`, tell the
  // shell, and move THIS console into it (the prompt already said it restarts).
  // Base list is `branch.list`, one read per prompt; unreadable → free text.
  async function createWorktreeFor(win) {
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
        error = (reply && reply.message) || "worktree create refused";
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
  function toggleMax(win, btn) {
    const ws = workspace();
    const offsets = ws ? { left: ws.scrollLeft, top: ws.scrollTop } : null;
    const maxed = win.classList.toggle("maximized");
    if (!maxed) {
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
    btn.title = maxed ? "restore" : "maximize";
    btn.innerHTML = maxed
      ? '<i class="bi bi-fullscreen-exit"></i>'
      : '<i class="bi bi-fullscreen"></i>';
    focusWin(win);
    try {
      win._term?.fit.fit();
    } catch {}
    applyExtent();
    persistWin(win);
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
  function isFull(win) {
    return document.fullscreenElement === win;
  }

  // ---- locked in place (ADR-0050 / ADR-0051 lock amendment) -----------------
  // A lock is a property of the DESK, honoured on every device: the gesture
  // handlers consult it and refuse; nothing else changes (maximize, fullscreen
  // and close do not rewrite the rect). A console is locked by its own record
  // OR by the fence holding its centre — the same `fenceOf` fold as membership.
  function fenceLocked(id) {
    return !!fences.find((f) => f.id === id)?.locked;
  }
  function isLocked(win) {
    if (win._deskLocked) return true;
    const holder = fenceOf(fences, restoreRect(win));
    return !!holder?.locked;
  }
  // The one place a window's lock state is painted: flag, class, glyph.
  function applyLock(win, locked) {
    win._deskLocked = !!locked;
    win.classList.toggle("locked", !!locked);
    const btn = win.querySelector(".session-lock");
    if (btn) {
      btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
      btn.title = locked ? "unlock" : "lock in place";
      btn.setAttribute("aria-pressed", locked ? "true" : "false");
    }
  }
  function toggleLock(win) {
    applyLock(win, !win._deskLocked);
    persistWin(win); // `ts: Date.now()` is the bump the fold arbitrates on
  }
  // Same for a fence: the class, the glyph, and the tile button, which is a
  // no-op on a locked fence and says so by being disabled.
  function paintFenceLock(el, locked) {
    el.classList.toggle("locked", !!locked);
    const btn = el.querySelector(".fence-lock");
    if (btn) {
      btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
      btn.title = locked ? "unlock this fence" : "lock this fence in place";
      btn.setAttribute("aria-pressed", locked ? "true" : "false");
    }
    const tile = el.querySelector(".fence-arrange");
    if (tile) tile.disabled = !!locked;
  }
  function setFenceLock(id, locked) {
    saveFences(fences.map((x) => (x.id === id ? { ...x, locked: !!locked, ts: Date.now() } : x)));
    renderFences();
  }

  function toggleFull(win) {
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
    for (const btn of document.querySelectorAll(".session-full")) {
      const win = btn.closest(".session-window");
      const on = !!win && el === win;
      btn.title = on ? "exit fullscreen" : "fullscreen";
      btn.innerHTML = on
        ? '<i class="bi bi-fullscreen-exit"></i>'
        : '<i class="bi bi-arrows-fullscreen"></i>';
      // The touch-target grow lives on the WINDOW, not the button: the whole
      // titlebar is the operator's exit affordance on a tablet.
      win?.classList.toggle("fullscreen", on);
    }
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
  }

  // The maximize pin, DERIVED the same way: `--max-left`/`--max-top` are only
  // honest while they equal the viewport's CURRENT offsets.
  // INVARIANT: every path that changes `#workspace`'s scroll offsets ends here.
  // The viewport's `scroll` event (`wireStage`) covers gesture, wheel, scrollbar
  // AND `reveal`'s programmatic write; `toggleMax` calls it after the flip. Only
  // ever writes to `.maximized` windows — un-maximize REMOVES both properties.
  function syncMaxPin() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    for (const win of st.querySelectorAll(".session-window.maximized")) {
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
  function applyExtent(opts) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // The one place the freeze is kept in step with what is on screen.
    syncMaxLock();
    // Read the DOM, not `wins`: a window is on the stage from `buildChrome`'s
    // append (before `spawnWindow` registers it) to its removal. Fences count
    // too — ADR-0051 §2 sizes the plane to windows AND fences.
    const rects = [...st.querySelectorAll(".session-window, .fence")].map(restoreRect);
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
  // `openConsoleItem` calls `reach` on the same synchronous stack as `activate`,
  // and Alpine's `x-show` flip lands a microtask later.
  let pendingReveal = null;
  // Whether `restoreDesk` has finished on ANY of its exits (demo early return
  // and failed fetch included): distinguishes "empty because nothing restored
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
  let offsetFlush = null;
  let pendingOffset = null;
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

  function focusWin(win) {
    z += 1;
    if (z > Z_CEIL) {
      // Renormalize: re-stack the existing windows by their current z, resetting
      // the counter so focus never pushes a console over the overlay/tabbar tier.
      const ordered = [...workspace().querySelectorAll(".session-window")].sort(
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
    for (const w of workspace().querySelectorAll(".session-window.focused")) {
      if (w !== win) w.classList.remove("focused");
    }
    win.classList.add("focused");
  }

  // Every window on the plane, for the Go-to picker. Reads the DOM, not `wins`:
  // a snapshot at menu open, not reactive state.
  function list() {
    const st = stage();
    if (!st) return [];
    return [...st.querySelectorAll(".session-window")].map((w) => ({
      id: w._deskId,
      agent: w._deskAgent,
      // `"~"` is the desk's "no repo"; the picker says `home`, like the titlebar.
      repo: w._deskRepo === "~" ? null : w._deskRepo,
      kind: w._deskKind,
      running: !w.classList.contains("placeholder"),
      state: w._agentState ?? null,
    }));
  }

  // The session the window holds, on a `/api/sessions` listing: the daemon's
  // id AND the repo ref, because a restarted daemon hands out ids from 1
  // again and a peer's id 1 is not this daemon's (the ref carries the peer).
  function sessionRowFor(win, sessions) {
    const id = sessionIdOf(win);
    if (id == null) return null;
    const ref = win._deskRepo;
    return (
      (sessions || []).find(
        (s) => s && s.id === id && (ref === "~" ? !s.repo || s.repo === "~" : s.repo === ref),
      ) || null
    );
  }

  // The agent-state dot per window, from the shell's `/api/sessions` poll
  // (ADR-0059 §5): the state word as a class, hidden when the row says
  // nothing. A placeholder has no session and keeps no dot.
  function ingestSessions(sessions) {
    lastSessions = Array.isArray(sessions) ? sessions : [];
    for (const win of wins) {
      const dot = win._stateDot;
      if (!dot) continue;
      const row = sessionRowFor(win, sessions);
      const state = row?.agent_state?.state ?? null;
      win._agentState = state;
      dot.className = "session-state" + (state ? ` ${state}` : "");
      dot.title = state ? `agent ${state}${row.agent_state.detail ? ": " + row.agent_state.detail : ""}` : "";
      dot.hidden = !state;
    }
  }

  // The "one action" that reaches a window far from the current view (ADR-0051
  // §4): focus it and slide the viewport so it is centred. Returns the element,
  // or null when no window carries that desk id.
  function reveal(deskId) {
    const ws = workspace();
    const it = findWindow(deskId);
    if (!it) return null;
    if (ws && ws.clientWidth && ws.clientHeight) return revealNow(deskId);
    // A viewport measuring 0 is a tab still `display:none` (CONTEXT.md →
    // Testing conventions); centring against it clamps to 0,0. Focus now, park
    // the centring for the frame that can measure (`pendingReveal`).
    focusWin(it);
    pendingReveal = deskId;
    return it;
  }

  function findWindow(deskId) {
    const st = stage();
    if (!st) return null;
    return (
      [...st.querySelectorAll(".session-window")].find((w) => w._deskId === deskId) || null
    );
  }

  // The centring half, on a viewport that is known to measure.
  function revealNow(deskId) {
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
    if (it.classList.contains("maximized")) return it;
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
  function makeDraggable(win, handle) {
    handle.addEventListener("pointerdown", (e) => {
      if (e.target.closest("button")) return;
      // Primary button only: a right/middle press is followed by `contextmenu`
      // (or no `pointerup`), stranding `onMove` on the document. `isPrimary` is
      // the touch half: a second finger opens its own stream.
      if (e.button !== 0 || !e.isPrimary) return;
      const pointerId = e.pointerId;
      focusWin(win);
      // No drag while maximized (double-click still restores) or fullscreen —
      // the top layer ignores the move while the drag REWRITES the inline rect.
      if (win.classList.contains("maximized") || isFull(win)) return;
      // Locked in place — by its own record or by the fence holding it.
      if (isLocked(win)) return;
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
      const place = (pointer) => {
        const st = stage();
        const origin = st.getBoundingClientRect();
        const x = pointer.x - origin.left - offX;
        const y = pointer.y - origin.top - offY;
        win.style.left = Math.max(0, Math.min(x, st.offsetWidth - rect.width)) + "px";
        win.style.top = Math.max(0, Math.min(y, st.offsetHeight - rect.height)) + "px";
        applyExtent({ grow: true });
      };
      // Auto-pan: holding the window against the viewport edge scrolls the plane
      // under it. `place(last)` in the tick keeps the DROP position correct in
      // stage coordinates.
      let panRaf = null;
      let last = null;
      // INVARIANT: an uncancelled loop pans the plane forever after the button
      // is released, so this runs as the FIRST statement of `onUp`.
      const stopPan = () => {
        if (panRaf != null) cancelAnimationFrame(panRaf);
        panRaf = null;
      };
      const nudge = () => {
        const ws = workspace();
        if (!ws || !last) return { dx: 0, dy: 0 };
        return panNudge(last, ws.getBoundingClientRect(), PAN_BAND, PAN_STEP);
      };
      const tickPan = () => {
        panRaf = null;
        // A window closed mid-drag: nothing left to carry, and `place` would
        // write styles onto a detached node forever.
        if (!win.isConnected) {
          stopPan();
          return;
        }
        const { dx, dy } = nudge();
        if (!dx && !dy) return; // leaving the band ENDS the loop
        const ws = workspace();
        ws.scrollLeft += dx;
        ws.scrollTop += dy;
        place(last);
        panRaf = requestAnimationFrame(tickPan);
      };
      const onMove = (ev) => {
        // Another pointer's stream (a second finger, the mouse during a touch drag).
        if (ev.pointerId !== pointerId) return;
        // `pointerup` is NOT guaranteed: a native context menu mid-drag or an
        // alt-tab with the button held swallows it, and the next pointerdown
        // installs its OWN closures, so nothing later could remove this pair.
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        last = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins(pressed, last, threshold)) return;
          armed = true;
        }
        place(last);
        if (panRaf != null) return;
        const { dx, dy } = nudge();
        if (dx || dy) panRaf = requestAnimationFrame(tickPan);
      };
      // Escape ends the drag where the window sits — no revert: a keyboard exit
      // from a loop whose mouseup may never arrive.
      const onKey = (ev) => {
        if (ev.key === "Escape") onUp();
      };
      const onUp = () => {
        stopPan();
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch drag that the system takes over (an edge swipe, a call coming
        // in) ends in `pointercancel` and NEVER in `pointerup`.
        document.removeEventListener("pointercancel", onUp);
        document.removeEventListener("contextmenu", onUp);
        document.removeEventListener("keydown", onKey);
        window.removeEventListener("blur", onUp);
        applyExtent();
        // A tap persists NOTHING: a fresh `ts` on an identical record would
        // overrule a real move made on another device under the desk fold.
        if (armed) persistWin(win);
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

  // ---- resize geometry ---------------------------------------------------------
  // One pure function for all eight directions (`resizeRect`, wb-geometry):
  // east/south move the far edge; west/north move `left`/`top` and derive the
  // size, so the OPPOSITE edge stays put.
  const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];


  // The repos a fence's members belong to, for the fence's chrome. Deduped,
  // sorted (DOM order is not stable), `"~"` rendered as `home` like `list()`.
  function fenceRepos(members) {
    const names = new Set((members || []).map((m) => (m?.repo === "~" ? "home" : m?.repo)));
    names.delete(undefined);
    names.delete(null);
    names.delete("");
    return [...names].sort().join(" · ");
  }

  // One fence readout for BOTH the fence's chrome and the toolbar list (#343):
  // `[{id, name, rect}]` + `[{id, repo, rect}]` in, one entry per fence IN ORDER
  // out. Folding membership here is what makes "the list and the fence never
  // disagree" a property of the code.
  function fenceSummaries(fences, windows) {
    const list = fences || [];
    const all = windows || [];
    const byId = new Map(all.map((w) => [w?.id, w]));
    const membership = fenceMembership(list, all);
    return list.map((f) => {
      const members = (membership[f.id] || []).map((wid) => byId.get(wid));
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
  function nextFenceSlot(rects, offset, viewport) {
    const taken = rects || [];
    const cap = Math.max(taken.length, FENCE_SLOT_SCAN);
    for (let i = 0; i <= cap; i++) {
      const candidate = fenceSpawnRect(offset, viewport, i);
      if (!taken.some((t) => rectsOverlap(candidate, t))) return i;
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

  function buildFence(f) {
    const el = document.createElement("div");
    el.className = "fence";
    el.dataset.fenceId = f.id;
    const head = document.createElement("div");
    head.className = "fence-head";
    // Two SMALL opt-in handles: a full-width interactive head would swallow the
    // floor's own pan (`onFloorDown` bails unless the press targets the stage).
    const grab = document.createElement("span");
    grab.className = "fence-grab";
    grab.title = "move this fence";
    grab.textContent = "⠿";
    grab.addEventListener("pointerdown", startFenceMove(el, f));
    const name = document.createElement("input");
    name.className = "fence-name";
    name.setAttribute("aria-label", "fence name");
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
    const stopOutside = (e) => {
      if (e.target !== name) endEdit(false);
    };
    const endEdit = (commit) => {
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
    name.addEventListener("mousedown", (e) => {
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
    name.addEventListener("keydown", (e) => {
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
    tile.title = "tile this fence's consoles";
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
    drop.title = "remove this fence";
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
    detach.title = "detach this fence into its own window";
    detach.textContent = "⧉";
    detach.addEventListener("click", () => detachFence(f.id));
    // Lock in place: the fence and every console it holds refuse a drag. Glyph
    // painted by `paintFenceLock` from `renderFences`.
    const lock = document.createElement("button");
    lock.className = "fence-lock";
    lock.type = "button";
    lock.addEventListener("click", () => setFenceLock(f.id, !fenceLocked(f.id)));
    // BETWEEN arrange and close: close stays the OUTERMOST control
    // (wb_fence_342.py asserts exactly that).
    tools.append(tile, lock, detach, drop);
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
      h.title = "resize this fence";
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
  let fenceFlash = null;
  function clearFenceFlash() {
    if (fenceFlash == null) return;
    clearTimeout(fenceFlash.timer);
    fenceFlash.el.classList.remove("fence-invalid");
    fenceFlash = null;
  }

  function fenceEl(id) {
    const st = stage();
    if (!st) return null;
    for (const el of st.querySelectorAll(".fence")) {
      if (el.dataset.fenceId === id) return el;
    }
    return null;
  }

  function startFenceMove(el, f) {
    return (e) => {
      if (e.button !== 0 || !e.isPrimary) return; // primary button only — see makeDraggable
      if (fenceLocked(f.id)) return;
      const st = stage();
      if (!st) return;
      const pointerId = e.pointerId;
      const start = restoreRect(el);
      // Membership is computed ONCE, at mousedown, and frozen for the gesture:
      // recomputing per move makes windows join and leave under the cursor as
      // the fence sweeps the plane, and the drop would carry a set nobody chose.
      const all = [...st.querySelectorAll(".session-window")].map((w) => ({
        el: w,
        id: w._deskId,
        rect: restoreRect(w),
      }));
      // The FULL fence list, not a singleton: the fold's `break` decides an
      // overlapping pair (reachable via a hand-edited `desk.toml`), and a
      // singleton bypasses it.
      const live = fences.map((x) => (x.id === f.id ? { id: x.id, rect: start } : x));
      const ids = new Set(fenceMembership(live, all)[f.id] || []);
      const carried = all.filter((m) => ids.has(m.id));
      const startX = e.clientX;
      const startY = e.clientY;
      // The plane's origin AT MOUSEDOWN: auto-pan scrolls the viewport
      // mid-gesture and slides the stage under a stationary cursor, so every
      // delta is taken against the LIVE origin (as `makeDraggable`'s `place`).
      const origin0 = st.getBoundingClientRect();
      let delta = { dx: 0, dy: 0 };
      let fits = true;
      let done = false;
      // A press with no movement is a CLICK, not a drop: persisting it would
      // upload the fence and every member with a fresh `ts`, reordering
      // `pruneDesk`'s eviction for a gesture that changed nothing. `armed` is
      // the same rule with a width (`dragThreshold`).
      let moved = false;
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      clearFenceFlash();
      const place = (pointer) => {
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
          }
        }
        applyExtent({ grow: true });
      };
      // Auto-pan, as `makeDraggable`: holding the fence against a viewport edge
      // scrolls the plane under it. `place(last)` in the tick keeps the drop
      // correct in stage coordinates.
      let panRaf = null;
      let last = null;
      // INVARIANT: an uncancelled loop pans the plane forever after the button is
      // released, so this runs as the FIRST statement of `onUp`.
      const stopPan = () => {
        if (panRaf != null) cancelAnimationFrame(panRaf);
        panRaf = null;
      };
      const nudge = () => {
        const ws = workspace();
        if (!ws || !last) return { dx: 0, dy: 0 };
        return panNudge(last, ws.getBoundingClientRect(), PAN_BAND, PAN_STEP);
      };
      const tickPan = () => {
        panRaf = null;
        // The fence was re-rendered or removed mid-drag: `place` would write
        // styles onto a detached node forever.
        if (!el.isConnected) {
          stopPan();
          return;
        }
        const { dx, dy } = nudge();
        if (!dx && !dy) return; // leaving the band ENDS the loop
        const ws = workspace();
        ws.scrollLeft += dx;
        ws.scrollTop += dy;
        place(last);
        panRaf = requestAnimationFrame(tickPan);
      };
      const onMove = (ev) => {
        if (ev.pointerId !== pointerId) return; // a second finger is not this gesture
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        last = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins({ x: startX, y: startY }, last, threshold)) return;
          armed = true;
        }
        place(last);
        if (panRaf != null) return;
        const { dx, dy } = nudge();
        if (dx || dy) panRaf = requestAnimationFrame(tickPan);
      };
      const onUp = () => {
        if (done) return;
        done = true;
        stopPan();
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch gesture the system takes over ends in `pointercancel`.
        document.removeEventListener("pointercancel", onUp);
        window.removeEventListener("blur", onUp);
        el.classList.remove("fence-invalid");
        // Refuse, do NOT snap: the fence and everything it carries go back to
        // where the gesture began and nothing is persisted.
        if (!fits || !moved) {
          el.style.left = start.left + "px";
          el.style.top = start.top + "px";
          for (const m of carried) {
            m.el.style.left = m.rect.left + "px";
            m.el.style.top = m.rect.top + "px";
          }
          applyExtent();
          return;
        }
        saveFences(
          fences.map((x) =>
            x.id === f.id
              ? {
                  ...x,
                  rect: { ...start, left: start.left + delta.dx, top: start.top + delta.dy },
                  ts: Date.now(),
                }
              : x,
          ),
        );
        renderFences();
        // Each member persists EXACTLY ONCE, here — a `persistWin` per mousemove
        // would upload N records per frame for a gesture with one outcome.
        for (const m of carried) persistWin(m.el);
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
  function startFenceResize(el, f, dir) {
    const way = FENCE_DIRS.includes(dir) ? dir : "se";
    return (e) => {
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
      const onMove = (ev) => {
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
        const rect = fits && sized ? out : start;
        el.style.left = rect.left + "px";
        el.style.top = rect.top + "px";
        el.style.width = rect.width + "px";
        el.style.height = rect.height + "px";
        if (!fits || !sized) {
          applyExtent();
          return;
        }
        saveFences(fences.map((x) => (x.id === f.id ? { ...x, rect, ts: Date.now() } : x)));
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
  // `persistWin` and `forgetRecord`. NOT from `applyExtent`: it fires per
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
    }
    // A console HELD by a locked fence wears the fence's lock (the class drops
    // its bands and grab cursor). Derived here with membership, from live rects.
    for (const w of st.querySelectorAll(".session-window")) {
      w.classList.toggle("held", !w._deskLocked && !!fenceOf(fences, restoreRect(w))?.locked);
    }
  }

  // The two DOM reads `refreshFenceChrome` and `fenceList` share: the stage is
  // where a fence and a window ARE, and membership is derived from those live
  // rects, never from `fences` or `wins`.
  function readFenceRects(st) {
    return [...st.querySelectorAll(".fence")].map((el) => ({
      id: el.dataset.fenceId,
      name: el.querySelector(".fence-name")?.value || "",
      rect: restoreRect(el),
    }));
  }

  function readWindowRects(st) {
    return [...st.querySelectorAll(".session-window")].map((w) => ({
      id: w._deskId,
      repo: w._deskRepo,
      rect: restoreRect(w),
    }));
  }

  // The fence list the toolbar picker shows (#343) — the same fold the fence
  // chrome reads. A SNAPSHOT at menu open, like `list()`.
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

  function fenceOrder(fences) {
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
  function fenceCycle(fences, currentId, step) {
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
  function detachFold(registry, event) {
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
  function peerFold(state, event, windowMs) {
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
  let detached = [];
  const fencePopups = new Map(); // fenceId -> { handle, members, fence, poll, peer }
  const PEER_WINDOW = window.WBDetachLink.PEER_WINDOW_MS;
  const HEARTBEAT = window.WBDetachLink.HEARTBEAT_MS;

  function isDetached(id) {
    return detached.includes(id);
  }

  // A popup entry with no popup behind it yet — the shape both the boot restore
  // and an unheralded `popup-here` start from.
  function newPopupEntry(memberIds) {
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
  function adoptMembers(id, members) {
    const entry = fencePopups.get(id);
    if (!entry || !Array.isArray(members)) return;
    const alive = new Set(members.map((m) => m?.id).filter(Boolean));
    const dropped = [...(entry.members || []).map((m) => m?.id), ...(entry.memberIds || [])].filter(
      (wid) => wid && !alive.has(wid),
    );
    entry.members = members;
    entry.memberIds = members.map((m) => m?.id).filter(Boolean);
    entry.adopted = true;
    for (const wid of new Set(dropped)) {
      // Marked removed as well as deleted: the record may not have LANDED yet
      // (a desk GET still in flight on this reload), and `forgetRecord` can only
      // filter what it can see. Without this the arriving GET would put it back.
      deskRemoved.add(wid);
      forgetRecord(wid);
    }
    commitDetached(detached);
  }

  // The member ids each detached fence holds, for the registry: the live
  // snapshot, else the ids restored from the last write.
  function detachedMembers() {
    const out = {};
    for (const id of detached) {
      const entry = fencePopups.get(id);
      const live = (entry?.members || []).map((m) => m.id).filter(Boolean);
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
  function commitDetached(next) {
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
  function stillThere(id, entry) {
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
  let beat = null;
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
  link.onMessage((m) => {
    if (!m || typeof m.type !== "string") return;
    if (link.tab == null || m.tab !== link.tab) return;
    if (m.type.startsWith("origin-")) return;
    const id = m.fenceId;
    if (m.type === "popup-here") {
      // A popup that survived this tab's reload, announcing which fence it
      // holds. Adopted only when the RESTORED registry already says that fence
      // is detached — the payload alone must never be able to detach one.
      if (!isDetached(id)) return;
      // MUTATED IN PLACE, never replaced: `glyphClick`'s ping compares the entry
      // it captured with the one in the map.
      const entry = fencePopups.get(id) || newPopupEntry();
      const st = stage();
      entry.greeted = true;
      // The popup hands back the UNTRANSLATED snapshot it was given, so a
      // re-attach puts every console back where it was detached from. Adopted
      // whole, EMPTY included: an empty set is an answer, not a missing one.
      fencePopups.set(id, entry);
      if (Array.isArray(m.members)) adoptMembers(id, m.members);
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
    } else if (m.type === "popup-gone") {
      // The tab filter proved the sender is ours; `detachFold` makes a re-attach
      // of a fence this tab does not hold a no-op.
      reattachFence(id);
    }
  });

  // A refused fence verb, said ON the fence. Cleared on a timer so a stale
  // refusal cannot outlive the gesture that caused it.
  function fenceNotice(id, text) {
    const el = fenceEl(id)?.querySelector(".fence-notice");
    if (!el) return;
    el.textContent = text;
    clearTimeout(el._noticeTimer);
    el._noticeTimer = setTimeout(() => {
      el.textContent = "";
    }, 2600);
  }

  function showDetachGlyph(id, on) {
    const away = fenceEl(id)?.querySelector(".fence-detached");
    if (away) away.hidden = !on;
  }

  // What the popup is handed: one record per member in the shape `buildChrome`
  // restores from, plus the live session id. Rects are measured HERE,
  // untranslated — the popup translates for its own viewport and never sends
  // them back, so a re-attach returns every console to its original box.
  function fenceSnapshot(id) {
    const st = stage();
    if (!st) return [];
    const all = [...st.querySelectorAll(".session-window")];
    const byId = new Map(all.map((w) => [w._deskId, w]));
    const ids = fenceMembership(readFenceRects(st), readWindowRects(st))[id] || [];
    return ids
      .map((wid) => byId.get(wid))
      .filter(Boolean)
      .map((win) => ({
        ...deskOf(win),
        session: sessionIdOf(win),
      }));
  }

  // Take a member off the plane WITHOUT forgetting its desk record (shared
  // state a second client still renders) and WITHOUT closing its daemon
  // session: `dispose()` closing the socket is the writer-slot release the
  // popup then re-acquires (ADR-0051 §9).
  function tearDownMember(win) {
    win._term?.dispose();
    win.remove();
    untrackDormancy(win);
    wins.delete(win);
    changed();
  }

  function stopPoll(entry) {
    if (entry?.poll) clearInterval(entry.poll);
    if (entry?.rescue) clearTimeout(entry.rescue);
  }

  function detachFence(id) {
    const out = detachFold(detached, { type: "detach", fenceId: id });
    for (const effect of out.effects) {
      if (effect.type === "focus") {
        // Raising a popup that already holds this fence — the ONLY way to raise
        // one. The handle is the direct route; after a reload it died with the
        // document and the channel is the only one left.
        const live = fencePopups.get(id);
        if (live?.handle && !live.handle.closed) live.handle.focus();
        else link.post({ type: "origin-focus", tab: link.tab, fenceId: id });
        WB.emit("fence-focus", { fence: id });
        return;
      }
      if (effect.type === "refuse") {
        fenceNotice(id, `at most ${DETACH_MAX} detached fences`);
        WB.emit("fence-detach-refused", { fence: id, reason: effect.reason });
        return; // the registry is NOT committed
      }
    }
    if (!out.effects.some((e) => e.type === "open")) return;

    const st = stage();
    const members = fenceSnapshot(id);
    const fence = st ? readFenceRects(st).find((f) => f.id === id) : null;
    // INVARIANT: either the popup exists AND the members are torn down, or
    // neither. `window.open` therefore runs BEFORE a single window is touched —
    // a blocked popup must leave the fence exactly as it was.
    const handle = window.open("detached-fence.html", "", "popup,width=900,height=700");
    if (!handle) {
      fenceNotice(id, "the browser blocked the popup");
      WB.emit("fence-detach-blocked", { fence: id });
      return; // the registry is NOT committed, nothing was torn down
    }

    const entry = {
      handle,
      members,
      memberIds: members.map((m) => m.id).filter(Boolean),
      fence: fence || { id, name: "", rect: null },
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
      const win = [...wins].find((w) => w._deskId === m.id);
      if (win) tearDownMember(win);
    }
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
  function reattachFence(id, opts = {}) {
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
    link.post({ type: "origin-close", tab: link.tab, fenceId: id });
    // The ORIGINAL records: the popup's own layout is discarded by never having
    // been read.
    for (const m of entry?.members || []) {
      // A member already on the plane is not re-spawned: two windows over one
      // session is worse than a console left away.
      if (m.id && [...wins].some((w) => w._deskId === m.id)) continue;
      if (m.session != null) {
        spawnWindow({ id: m.session, repo: m.repo }, m.agent || "console", m.repo, m);
      } else {
        spawnPlaceholder(m);
      }
    }
    showDetachGlyph(id, false);
    applyExtent();
    refreshFenceChrome();
    WB.emit("fence-reattach", { fence: id });
  }

  // The glyph is ONE verb: bring these consoles home, whatever the registry
  // believes (raising a buried popup is the head's detach button). Close the
  // window by handle or by channel — both is fine, `origin-close` is
  // idempotent — and put the members back.
  function glyphClick(id) {
    reattachFence(id, { force: true });
  }

  // The POPUP's side: render the members the opener handed over, translated by
  // the fence origin to sit near this window's top-left. The untranslated
  // snapshot stays in the OPENER — nothing measured here ever goes back.
  function mountDetached(fence, members) {
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
      if (m.session != null) {
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

  // The opener's half of the handshake, guarded as `app.js` guards the detached
  // FILE viewer's: answered only from this origin AND from a window this tab
  // itself opened.
  window.addEventListener("message", (e) => {
    if (!window.WBMode?.isDemo() && e.origin !== location.origin) return;
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
      // Demo-aware (the popup's `PEER` mirrors it): under `file://` the popup's
      // origin is OPAQUE, and an unconditional `location.origin` is dropped
      // (Chrome) or throws (Firefox). `tab` rides the handover, never the
      // popup's own storage — `window.open` gave it a COPY of ours.
      e.source.postMessage(
        { type: "wb-fence-open", fence: entry.fence, members: entry.members, tab: link.tab },
        window.WBMode?.isDemo() ? "*" : location.origin,
      );
    } else if (m.type === "wb-emit") {
      WB.emit(m.action, m.detail);
    } else if (m.type === "wb-fence-reattach") {
      // `owner`, never the message's own field: the source lookup PROVED which
      // fence this window holds; the payload could name any.
      reattachFence(owner);
    }
  });

  // The verb the shortcut calls: walk one step and jump. Returns the id landed
  // on, or null when there is no fence (so the shell leaves the key unswallowed).
  function stepFence(step) {
    const st = stage();
    if (!st) return null;
    const id = fenceCycle(readFenceRects(st), focusedFence, step);
    if (id == null) return null;
    return jumpToFence(id) ? id : null;
  }

  // Upsert the DOM against `fences`. The rect is always re-applied; the NAME is
  // not written while the operator is typing in it (an in-flight GET would yank
  // the caret to a stale value).
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
      el.style.left = (r.left || 0) + "px";
      el.style.top = (r.top || 0) + "px";
      el.style.width = (r.width || 0) + "px";
      el.style.height = (r.height || 0) + "px";
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
  function nextFenceName(existing) {
    const highest = (existing || []).reduce((max, f) => {
      const m = FENCE_NAME_RE.exec(String(f?.name ?? ""));
      return m ? Math.max(max, Number(m[1])) : max;
    }, 0);
    return `Fence ${highest + 1}`;
  }

  function createFence() {
    // AT THE CAP, REFUSE: `saveFences` prunes by oldest `ts`, which for a
    // creation would silently drop a DIFFERENT, named fence. The prune stays as
    // the backstop for a desk arriving over the cap from another client.
    // Refusing and SAYING SO are two jobs: this module knows no Alpine, so it
    // answers `false` and `newFence()` in app.js does the talking; `atFenceCap`
    // is exported so the row can be disabled BEFORE the click.
    if (atFenceCap()) return false;
    const ws = workspace();
    const offset = { left: ws?.scrollLeft || 0, top: ws?.scrollTop || 0 };
    const viewport = { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 };
    const slot = nextFenceSlot(
      fences.map((f) => f.rect),
      offset,
      viewport,
    );
    // The whole scanned band is full. REFUSE — do not nudge the new fence into
    // a gap the operator never chose.
    if (slot < 0) {
      const blocked = fenceSpawnRect(offset, viewport, 0);
      const hit = fences.find((x) => rectsOverlap(blocked, x.rect || {}));
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
          ts: Date.now(),
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

  function renameFence(id, name) {
    saveFences(
      fences.map((f) =>
        f.id === id
          ? { ...f, name: String(name == null ? "" : name).slice(0, FENCE_NAME_MAX), ts: Date.now() }
          : f,
      ),
    );
    renderFences();
  }

  function removeFence(id) {
    // Removing a DETACHED fence would destroy the glyph that brings its consoles
    // home (ADR-0051 §7a) while the registry kept a `DETACH_MAX` slot. Refuse.
    if (detached.includes(id)) {
      fenceNotice(id, "Return this fence's consoles to this window first");
      WB.emit("fence-remove-refused", { fence: id, reason: "detached" });
      return;
    }
    saveFences(fences.filter((f) => f.id !== id));
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
  function clampOffset(offset, viewport, extent) {
    const maxLeft = (extent?.width || 0) - (viewport?.width || 0);
    const maxTop = (extent?.height || 0) - (viewport?.height || 0);
    return {
      left: Math.max(0, Math.min(offset?.left || 0, maxLeft)),
      top: Math.max(0, Math.min(offset?.top || 0, maxTop)),
    };
  }

  function bringIntoView(target, viewport, extent) {
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

  function anchorIntoView(target, viewport, extent, inset) {
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
  let focusedFence = null;

  function focusedFenceId() {
    return focusedFence;
  }

  function focusFence(id) {
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
  let slideRaf = null;

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
  function slideEase(t) {
    const x = Math.min(1, Math.max(0, t));
    return 1 - Math.pow(1 - x, 3);
  }

  function slideTo(ws, to) {
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
    const step = (now) => {
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
  function jumpToFence(id) {
    const el = fenceEl(id);
    if (!el) return null;
    focusFence(id);
    const ws = workspace();
    const st = stage();
    // A viewport measuring 0 is a tab still `display:none`; centring would
    // clamp to 0,0. The focus above still holds.
    if (!ws || !st || !ws.clientWidth || !ws.clientHeight) return el;
    const view = { width: ws.clientWidth, height: ws.clientHeight };
    const ext = { width: st.offsetWidth, height: st.offsetHeight };
    const to = anchorIntoView(restoreRect(el), view, ext);
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
  function bboxOf(rects) {
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
  function viewLanding(stored, rects, viewport, extent) {
    const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : null);
    const left = num(stored?.left);
    const top = num(stored?.top);
    if (left !== null && top !== null) {
      const at = clampOffset({ left, top }, viewport, extent);
      const vw = viewport?.width || 0;
      const vh = viewport?.height || 0;
      const shows = (rects || []).some(
        (r) =>
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
  function panNudge(pointer, viewport, band, step) {
    const b = band == null ? PAN_BAND : band;
    const s = step == null ? PAN_STEP : step;
    const press = (v) => Math.max(0, Math.min(v, b));
    const axis = (near, far) => Math.round((s * (far - near)) / b);
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


  // Wire one handle: drag it and the window's rect follows `resizeRect`. Every
  // exit path (mouseup anywhere on the document) drops BOTH listeners and
  // persists exactly once.
  function startResize(win, dir) {
    return (e) => {
      if (e.button !== 0 || !e.isPrimary) return; // see makeDraggable
      const pointerId = e.pointerId;
      focusWin(win);
      if (win.classList.contains("maximized") || isFull(win)) return;
      if (isLocked(win)) return; // the JS guard is the truth; the CSS only hides the bands
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
      const onMove = (ev) => {
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
          RESIZE_MIN,
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
        if (armed) persistWin(win); // a tap on a band changed nothing
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

  function encodeTerminal(str) {
    const data = new TextEncoder().encode(str);
    const out = new Uint8Array(1 + 8 + data.length);
    out[0] = TAG_TERMINAL;
    out[8] = SESSION_ID;
    out.set(data, 9);
    return out;
  }

  function encodeResize(rows, cols) {
    const json = JSON.stringify({ id: 0, verb: "resize", payload: { rows, cols } });
    const body = new TextEncoder().encode(json);
    const out = new Uint8Array(1 + body.length);
    out[0] = TAG_COMMAND;
    out.set(body, 1);
    return out;
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

  // `visibilitychange` and `online` both land on one iOS resume; without the
  // probe seam the popup would have no verdict at all.
  let staleProbe = OPTS.isStale || null;
  function setStaleProbe(fn) {
    staleProbe = typeof fn === "function" ? fn : null;
  }

  // Retire a socket so its pending events cannot reach us. `onmessage` matters
  // as much as `onclose`: a frame still queued lands AFTER this returns, when
  // `ws` names the replacement. Local, not `WBDaemon`'s: this module loads on
  // its own in the node harness and the popup.
  function detachSocket(ws) {
    if (!ws) return;
    ws.onclose = null;
    ws.onmessage = null;
    ws.onopen = null;
    ws.onerror = null;
    try {
      if (ws.readyState <= 1) ws.close();
    } catch {}
  }

  // The engine, not the brand: WebKit answers "Apple Computer, Inc." in every
  // browser on iPadOS; Chromium "Google Inc."; Firefox "". Pure so the string
  // table is the contract.
  function isWebKit(vendor) {
    return typeof vendor === "string" && vendor.startsWith("Apple");
  }

  // Whether this engine must render in the DOM instead of on the GPU: the WebGL
  // addon draws scrolled rows twice on WebKit.
  function prefersDomRenderer(vendor) {
    return isWebKit(vendor);
  }

  // Whether to BUILD the fullscreen button. `fullscreenEnabled` is false in a
  // sandboxed frame and a standalone PWA. On WebKit it is true but iOS drops
  // out of fullscreen the moment a text field takes focus, so on an iPad the
  // first keystroke would cancel it; maximize is the honest control there.
  function fullscreenOffered(enabled, vendor) {
    return enabled === true && !isWebKit(vendor);
  }

  // TOUCH SCROLLING is ours. MEASURED: the touch lands on `.xterm-screen`, and
  // `.xterm-viewport` (the scroller) is its SIBLING, so the browser walks up to
  // `#workspace` and pans the workbench; `overscroll-behavior: contain` on the
  // viewport is inert for the same reason. A wheel works only because xterm
  // forwards `wheel` in JS. Upstream: xterm.js #3613, #594, #5377.
  // Pure: pixels dragged → lines, at the cell height, sign flipped. A
  // zero/absent cell height yields 0, not Infinity.
  function touchScrollLines(dyPx, cellHeight) {
    if (!Number.isFinite(dyPx) || !Number.isFinite(cellHeight) || cellHeight <= 0) return 0;
    return -dyPx / cellHeight;
  }

  // WHO the gesture belongs to. xterm hands a wheel to the APPLICATION when it
  // asked for mouse events (Claude Code and every full-screen TUI), turns it
  // into arrow keys in the alternate buffer, and moves its own viewport only in
  // the plain case — under a TUI the viewport's history is stale frames
  // ("ghost" text). `mode` is `term.modes.mouseTrackingMode`; `bufferType` is
  // `term.buffer.active.type`.
  function touchScrollTarget(mode, bufferType) {
    if (typeof mode === "string" && mode !== "none") return "app";
    if (bufferType === "alternate") return "app";
    return "viewport";
  }

  // How many fingers, whose gesture. One is the terminal's. Two are the
  // CANVAS's: `touch-action: none` on the body took every browser gesture, so
  // the pan is given back here through the same `scrollLeft/Top` writes the
  // mouse pan makes. Under `maxlock` there is nowhere to pan. Three are the
  // system's.
  function touchGesture(fingers, maxlock) {
    if (fingers === 1) return "terminal";
    if (fingers === 2 && !maxlock) return "canvas";
    return "none";
  }

  // The point between the fingers, which is what a two-finger pan tracks: the
  // fingers can drift apart or together without the plane jumping.
  function touchCentroid(touches) {
    const list = Array.from(touches ?? []);
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
  function dragThreshold(pointerType) {
    return pointerType === "mouse" ? DRAG_THRESHOLD.mouse : DRAG_THRESHOLD.touch;
  }
  function dragBegins(start, pointer, threshold) {
    const dx = (pointer?.x || 0) - (start?.x || 0);
    const dy = (pointer?.y || 0) - (start?.y || 0);
    return Math.hypot(dx, dy) >= threshold;
  }

  // Inertia. Terminals hold thousands of lines and a strict 1:1 drag makes the
  // scrollback unreachable by hand, which is the substance of xterm #594.
  // `FLING_DECAY` is per 16ms frame; below `FLING_MIN` the glide has stopped
  // being motion and starts being drift, so it is cut rather than eased.
  const FLING_DECAY = 0.94;
  const FLING_MIN = 0.02; // px/ms
  function flingStep(velocity, ms) {
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
    "ctrl-c": "\x03",
  });
  const ARROW_FINAL = Object.assign(Object.create(null), {
    up: "A",
    down: "B",
    right: "C",
    left: "D",
  });
  function keySequence(name, appCursor) {
    const literal = KEY_BYTES[name];
    if (typeof literal === "string") return literal;
    const final = ARROW_FINAL[name];
    if (typeof final !== "string") return "";
    return (appCursor ? "\x1bO" : "\x1b[") + final;
  }

  // The latching Ctrl: a finger presses one key at a time, so `Ctrl` arms and
  // the NEXT character is folded. Only a single printable character folds — `d`
  // can be a whole paste or a bracketed-paste burst, and masking its first byte
  // would corrupt it. Anything else passes through WITH the latch still set.
  function applyCtrlLatch(latched, d) {
    if (!latched || typeof d !== "string" || d.length !== 1) return { out: d, latched };
    const code = d.toUpperCase().charCodeAt(0);
    if (code < 0x40 || code > 0x5f) return { out: d, latched };
    return { out: String.fromCharCode(code & 0x1f), latched: false };
  }

  // Whether a window shows the bar. `mode` is "on", "off", or absent for auto
  // (has a touch surface). `any-pointer` rather than `pointer`: an iPad with a
  // Magic Keyboard reports a FINE primary pointer and is still a tablet.
  function keyBarVisible(mode, coarse) {
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

  // Per browser profile (wb-settings.js `scope: client`, stored by
  // `wb-view.js`). Absent — the popup reads nothing — means auto.
  function keyBarMode() {
    return viewStore?.read()?.keys ?? null;
  }

  function applyKeyBar(win) {
    win.classList.toggle("keys", keyBarVisible(keyBarMode(), hasTouchSurface()));
  }

  // Whether the key bar offers a PASTE button. `readText` exists only in a
  // secure context, and unlike the write there is no `execCommand` fallback
  // for a read. Pure: takes the clipboard object (or `undefined`).
  function pasteOffered(clipboard) {
    return !!clipboard && typeof clipboard.readText === "function";
  }

  // THE PHONE BLEED. Fullscreen is withheld on WebKit (`fullscreenOffered`), so
  // on a phone maximize is the ceiling and the chrome folds away below this
  // width: `syncMaxLock` writes `body.console-max`, 01-base.css gates on the
  // same number. Width, not pointer: an iPad keeps its chrome. 560px is the
  // workbench's phone breakpoint (04-canvas.css, 11-appended.css).
  const PHONE_MAX_WIDTH = 560;
  function phoneBleed(maxed, viewportWidth) {
    return !!maxed && Number.isFinite(viewportWidth) && viewportWidth <= PHONE_MAX_WIDTH;
  }

  // The buffer row under a finger, for the line-selection mode. `selectLines`
  // takes buffer-absolute rows, hence `viewportY`. Clamped to the screen so a
  // finger that slid off the bottom selects to the last row; a zero/NaN cell
  // height answers the top row, not NaN.
  function selectionRow(clientY, screenTop, cellHeight, rows, viewportY) {
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

  function stepFont(current, delta) {
    const from = Number.isFinite(current) ? current : FONT_DEFAULT;
    return Math.min(FONT_MAX, Math.max(FONT_MIN, Math.round(from) + delta));
  }

  function fontSize() {
    return viewStore?.read()?.font ?? FONT_DEFAULT;
  }

  // Every window at once. `fit` is required: the BOX does not change, so the
  // ResizeObserver never fires and the daemon would never learn the new size.
  function setFont(px) {
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
  function keyboardInset({ innerHeight, height, offsetTop, scale }) {
    if (typeof scale === "number" && Math.abs(scale - 1) > ZOOM_EPSILON) return 0;
    const inset = (innerHeight || 0) - (height || 0) - (offsetTop || 0);
    if (!Number.isFinite(inset) || inset <= 0) return 0;
    return Math.round(inset);
  }

  // Pure, tabled like `reconnectDecision`. CONNECTING is already the reconnect —
  // closing it only restarts the handshake a round-trip later.
  function resumeDecision({ readyState, stale }) {
    if (readyState == null) return "reconnect";
    if (readyState === 0) return "none";
    if (readyState === 1) return stale ? "reconnect" : "none";
    return "reconnect";
  }

  // The dormancy rule, pure and tabled. The observer supplies `intersecting`,
  // `applyDormancy` owns the grace period. Returns exactly one of
  //   "sleep" — dispose this window's terminal and release its socket;
  //   "wake"  — rebuild the terminal and reattach;
  //   "hold"  — leave it exactly as it is.
  function dormancyDecision({
    intersecting,
    dormant,
    maximized,
    fullscreen,
    focused,
    hasTerminal,
    ended,
    sessionId,
  }) {
    // Visible outranks everything.
    if (intersecting) return dormant ? "wake" : "hold";
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
  function pasteDecision({ types, size, watching }) {
    const hasImage = (types || []).some(
      (t) => typeof t === "string" && t.startsWith("image/"),
    );
    if (!hasImage) return "passthrough";
    if (watching) return "watched";
    if (!(size >= 0) || size > IMAGE_PASTE_MAX) return "too-large";
    return "drop";
  }

  // The reconnect rule (#334), pure and tabled. Returns one of "reconnect" /
  // "park-as-watcher" / "give-up".
  //
  // `announced` is the daemon's eviction reason from a data frame BEFORE the
  // close ("taken-over" / "child-exited" / "daemon-shutdown"), else null. It is
  // the only trustworthy signal of a deliberate end: the browser reports
  // 1005/wasClean=false even for a served Close frame, so an unannounced dirty
  // close is read as a flaky link.
  function reconnectDecision({
    code,
    wasClean,
    opened,
    everOpened,
    announced,
    idKnown,
    failedReopens,
  }) {
    // R1: no id is nothing to reattach TO; reconnecting would spawn a SECOND
    // session.
    if (!idKnown) return "give-up";
    // R2/R3: the daemon said why. Taken over → park and watch; else gone.
    if (announced === "taken-over") return "park-as-watcher";
    if (announced != null) return "give-up";
    if (failedReopens > MAX_FAILED_REOPENS) return "give-up";
    // R5: a clean close of a socket that DID open is a deliberate server end
    // (an older daemon, a proxy closing).
    if (opened && (wasClean || code === 1000 || code === 1001)) return "give-up";
    // R6: held the session before, so a drop is a flaky link.
    if (everOpened) return "reconnect";
    // R7/R8: never opened. Retry a bounded number of times (an F5 racing the
    // old bridge's teardown), then settle for watching.
    if (failedReopens < WATCH_AFTER) return "reconnect";
    return "park-as-watcher";
  }

  // The terminal's surface, ADR-0035's palette. xterm.js takes no CSS variables
  // (WebGL paints the glyphs), so these mirror :root in styles.css and must
  // move with it — the lockstep `wb-monaco.js` keeps.
  // Base colours ONLY: the 16 ANSI slots stay xterm's defaults, the palette
  // every vendor TUI picked its colours against. The background is pure black,
  // not `--log-bg` — the same exception `wb-monaco.js` makes.
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
  function scrubClipboard(text) {
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
  function writeClipboard(text, term) {
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

  // The read half. Always a promise: an insecure origin has no
  // `navigator.clipboard` and the API throws synchronously.
  function readClipboard() {
    try {
      return Promise.resolve(navigator.clipboard.readText());
    } catch {
      return Promise.resolve("");
    }
  }

  // Attach a real xterm.js terminal into `body`, wired to a PTY over `/ws/session`.
  // `opts` is one of: {repo, agent} (a NEW agent launch), {console:true[, repo]}
  // (a NEW free-console launch — home dir when `repo` absent), or
  // {id[, takeover][, watch]} (a REATTACH; `watch` is read-only). Returns a
  // handle so the window chrome can refit, take the baton, and close it.
  function attachTerminal(body, opts) {
    const term = new Terminal({ convertEol: false, theme: TERMINAL_THEME });
    // Set rather than passed: the constructor literal is pinned in lib.rs as
    // the theme contract; the size is a per-profile preference.
    term.options.fontSize = fontSize();
    const fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(body);
    // GPU glyph rendering with a DOM fallback: on a lost context the addon is
    // disposed and xterm falls back to DOM without dropping the session.
    // NOT on WebKit: the addon renders scrolled rows twice there (xterm.js
    // #3357, #5816; reproduced with the scrollbar, so the renderer, not our
    // gesture). Every browser on iPadOS is WebKit.
    if (!prefersDomRenderer(navigator.vendor)) {
      try {
        const webgl = new WebglAddon.WebglAddon();
        webgl.onContextLoss(() => webgl.dispose());
        term.loadAddon(webgl);
      } catch {}
    }
    term.loadAddon(new WebLinksAddon.WebLinksAddon());

    // The touch gesture this terminal owns (`touchScrollLines`). Single finger
    // only; the stylesheet's `touch-action: none` already told the browser the
    // console is not a pan surface, and A+/A− is a terminal's zoom.
    let touchY = null;
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
    let selStart = null;
    const setSelecting = (on) => {
      selecting = !!on;
      selStart = null;
      if (typeof opts.onSelecting === "function") opts.onSelecting(selecting);
    };
    const rowAt = (clientY) => {
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
    const wheelToApp = (lines) => {
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
    const scrollByPixels = (dy) => {
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
    let pan = null;
    const stopPan = () => {
      if (!pan) return;
      pan = null;
      stage()?.classList.remove("panning");
    };
    body.addEventListener(
      "touchstart",
      (e) => {
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
      (e) => {
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
    const endTouch = (e) => {
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
      const glide = (now) => {
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
    term.textarea.addEventListener("paste", (e) => {
      const items = Array.from(e.clipboardData?.items ?? []);
      const image = items.find((i) => i.type.startsWith("image/"));
      const file = image ? image.getAsFile() : null;
      const decision = pasteDecision({
        types: items.map((i) => i.type),
        size: file ? file.size : -1,
        watching,
      });
      if (decision === "passthrough") return;
      e.preventDefault();
      if (decision === "watched") {
        if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
        return;
      }
      if (decision === "too-large") {
        term.write("\r\n[paste refused — too large]\r\n");
        return;
      }
      const daemon = window.WBDaemon;
      if (!daemon) {
        // The popup forgot its bridge: say so rather than swallow the paste.
        term.write("\r\n[paste refused: daemon not connected]\r\n");
        return;
      }
      const reader = new FileReader();
      reader.onerror = () => term.write("\r\n[paste refused — unreadable]\r\n");
      reader.onload = () => {
        const base64 = String(reader.result).replace(/^data:[^,]*,/, "");
        daemon
          .write("image.write", { repo: currentRepo, base64 })
          .then((reply) => {
            if (window.WBFail.isError(reply) || !reply.path) {
              const why = window.WBFail.message(reply, "refused");
              term.write(`\r\n[paste refused — ${why}]\r\n`);
              return;
            }
            term.paste(reply.path);
            term.focus();
          })
          // The socket closed with no reply: say what the browser saw.
          .catch((err) => {
            const why = (err && err.message) || "connection unavailable";
            term.write(`\r\n[paste refused — ${why}]\r\n`);
          });
      };
      reader.readAsDataURL(file);
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
    term.parser.registerOscHandler(52, (data) => {
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
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown" || !e.ctrlKey || e.shiftKey || e.altKey) return true;
      if (e.key !== "Insert" || !term.hasSelection()) return true;
      writeClipboard(term.getSelection(), term);
      return false;
    });
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
    let currentDaemonId = null;
    let currentEnvironment = null;
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
    let ws = null;
    let opened = false; // has the CURRENT socket opened
    let everOpened = false; // has ANY socket of this window opened
    // True on EVERY path into the watcher role (the park below, or `{id,
    // watch}` from the start). The `term.onData` gate reads this flag.
    let watching = !!opts.watch;
    let announced = null; // the daemon's reason, when it named one before closing
    let switching = false; // an intentional close on the way to a takeover
    let firstConnect = true;
    // True while the scrollback replay is being parsed. The replay is RAW BYTES
    // (one terminal frame), so every escape sequence in the backlog runs again
    // (OSC 52 is refused meanwhile). Cleared by the write callback, not after
    // `term.write` returns: xterm parses ASYNCHRONOUSLY.
    let replaying = false;
    let retryDelay = 0;
    let retryTimer = null;
    let failedReopens = 0;
    // This window is done. Without the latch a resume would reconnect a dead
    // id and print a second "[session closed]".
    let ended = false;
    let lastResumeAt = 0;

    function giveUp() {
      ended = true;
      // Stop observing so a dead-ws terminal doesn't keep firing fit() until the
      // window is closed.
      ro.disconnect();
      term.write("\r\n[session closed]\r\n");
      if (typeof opts.onEnded === "function") opts.onEnded();
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

    function connect(connOpts) {
      opened = false;
      announced = null;
      // A reattach (`id`) gets the backlog replayed; a fresh launch has none.
      // An empty scrollback sends no replay frame, so the flag rides until the
      // first LIVE frame clears it.
      replaying = connOpts.id != null;
      ws = new WebSocket(window.WBSessionRoute.url(WS_ORIGIN, connOpts));
      ws.binaryType = "arraybuffer";
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
          term.write("\r\n[read-only: another window controls this session]\r\n");
        }
        fit.fit();
        ws.send(encodeResize(term.rows, term.cols));
      };
      ws.onmessage = (ev) => {
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
            const owner = window.WBSessionRoute.announcement(
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
          } else if (c && c.verb === "session-end") {
            announced = c.payload?.reason ?? "child-exited";
          }
        }
      };
      // Swallow the error event; onclose drives recovery in every case.
      ws.onerror = () => {};
      ws.onclose = (event) => {
        if (leaving || switching) return;
        if (!opened) failedReopens += 1;
        switch (
          reconnectDecision({
            code: event?.code,
            wasClean: !!event?.wasClean,
            opened,
            everOpened,
            announced,
            idKnown: currentSessionId != null,
            failedReopens,
          })
        ) {
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
            }
            scheduleReconnect();
        }
      };
    }

    // Every byte this window sends to the child goes through here (keyboard,
    // key bar, paste), so the watched gate and the Ctrl latch apply to all.
    let ctrlLatched = false;
    function sendInput(raw) {
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

    term.onData(sendInput);
    term.onResize(({ rows, cols }) => {
      if (ws && ws.readyState === WebSocket.OPEN)
        ws.send(encodeResize(rows, cols));
    });

    connect(opts);

    return {
      term,
      fit,
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
      sendKey(name) {
        if (name === "ctrl") {
          ctrlLatched = !ctrlLatched;
          if (typeof opts.onCtrlLatch === "function") opts.onCtrlLatch(ctrlLatched);
          return ctrlLatched;
        }
        const seq = keySequence(name, !!term.modes?.applicationCursorKeysMode);
        if (!seq) return false;
        return sendInput(seq);
      },
      get ctrlLatched() {
        return ctrlLatched;
      },
      // Arm (or disarm) the line-selection gesture. NOT gated on `watching`:
      // a selection is a read, and a watcher may copy what it sees.
      setSelecting,
      get selecting() {
        return selecting;
      },
      // The page came back from a suspend (or the network did). Returns whether
      // it acted. The `currentSessionId == null` bail is load-bearing: a window
      // not yet told its id would compose a LAUNCH url and spawn a second
      // vendor CLI (`reconnectDecision` R1, `takeOver`).
      resume(stale) {
        if (leaving || ended || currentSessionId == null) return false;
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
        if (resumeDecision({ readyState: ws ? ws.readyState : null, stale }) === "none")
          return false;
        lastResumeAt = now;
        detachSocket(ws);
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
        detachSocket(ws);
        watching = false;
        announced = null;
        failedReopens = 0;
        retryDelay = 0;
        switching = false;
        if (typeof opts.onResume === "function") opts.onResume();
        connect({ id: currentSessionId, repo: currentRepo, takeover: true });
      },
      dispose() {
        leaving = true;
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        ro.disconnect();
        // A glide or a pending scroll would call `scrollLines` on a disposed
        // terminal.
        stopFling();
        stopScroll();
        if (ws && ws.readyState <= 1) ws.close();
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

  function spawnRectIn(fence, index, headH) {
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

  // The floating-window chrome, shared by a live console and a placeholder:
  // rect (from a desk record, else cascaded), titlebar, body, eight resize
  // handles. `desk` is a record (or a partial carrying at least `kind`);
  // everything the record needs later is hung off the element.
  function buildChrome(label, repo, desk, kind) {
    const win = document.createElement("div");
    win.className = "session-window";
    // Every field this element will carry is written HERE, by the module that
    // declares them (wb-window-state.js): a field nothing seeds reads as its
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
    });
    const rect = desk?.rect;
    if (rect) {
      win.style.left = rect.left + "px";
      win.style.top = rect.top + "px";
      win.style.width = rect.width + "px";
      win.style.height = rect.height + "px";
    } else {
      cascade = (cascade + 1) % 8;
      // Born INTO the focused fence when there is one (#343). The rect is
      // written at CONSTRUCTION so `persistWin` never snapshots mid-transition
      // (#342). MEASURABLE, not merely present: `openConsoleItem` calls
      // `activate` then `open` on the SAME synchronous stack, so a spawn can
      // land while the tab is still `display:none` and `restoreRect` reads all
      // zeros — a 1x1 window persisted to the shared desk. Fall back to the
      // free cascade; the focus survives for the next spawn.
      const el = focusedFence && fenceEl(focusedFence);
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
        const vw = ws?.clientWidth || 0;
        const vh = ws?.clientHeight || 0;
        win.style.left = Math.max(0, ws?.scrollLeft || 0) + 30 + cascade * 24 + "px";
        win.style.top = Math.max(0, ws?.scrollTop || 0) + 20 + cascade * 24 + "px";
        // An unmeasurable viewport is a tab still `display:none`: plain caps.
        win.style.width = (vw ? Math.max(WIN_MIN_W, Math.min(560, Math.round(vw * 0.62))) : 560) + "px";
        win.style.height =
          (vh ? Math.max(WIN_MIN_H, Math.min(340, Math.round(vh * 0.6))) : 340) + "px";
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
    // Restart is chrome for a session that ENDED: hidden while alive (one click
    // from maximize would tree-kill the vendor CLI), revealed by `onEnded`,
    // never built where nothing can launch (the popup).
    const restartBtn = document.createElement("button");
    restartBtn.className = "session-restart";
    restartBtn.title = "restart session";
    restartBtn.innerHTML = '<i class="bi bi-arrow-clockwise"></i>';
    restartBtn.hidden = true;
    const maxBtn = document.createElement("button");
    maxBtn.className = "session-max";
    maxBtn.title = "maximize";
    maxBtn.innerHTML = '<i class="bi bi-fullscreen"></i>';
    // Fullscreen is orthogonal to maximize (viewport vs physical screen). Built
    // only where the browser can HOLD it (`fullscreenOffered`).
    const fullBtn = document.createElement("button");
    fullBtn.className = "session-full";
    fullBtn.title = "fullscreen";
    fullBtn.innerHTML = '<i class="bi bi-arrows-fullscreen"></i>';
    fullBtn.hidden = !fullscreenOffered(document.fullscreenEnabled, navigator.vendor);
    const closeBtn = document.createElement("button");
    closeBtn.className = "session-close";
    closeBtn.title = "close";
    closeBtn.innerHTML = '<i class="bi bi-x-lg"></i>';
    // Lock in place. Glyph and title painted by `applyLock`.
    const lockBtn = document.createElement("button");
    lockBtn.className = "session-lock";
    actions.append(restartBtn, fullBtn, lockBtn, maxBtn, closeBtn);
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
    // Maximize/restore: the button, or a double-click on the titlebar.
    maxBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleMax(win, maxBtn);
    });
    titlebar.addEventListener("dblclick", (e) => {
      if (e.target.closest("button")) return;
      toggleMax(win, maxBtn);
    });
    lockBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleLock(win);
    });
    applyLock(win, !!desk?.locked);
    fullBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleFull(win);
    });
    // Re-apply a persisted maximized state (the inline rect above is the box it
    // restores to).
    if (rect && desk.max) toggleMax(win, maxBtn);
    focusWin(win);
    return { win, body, title, restartBtn, fullBtn, lockBtn, maxBtn, closeBtn };
  }

  // Build the chrome and attach a live terminal. Shared by `open()` and the
  // load-time restore; `termOpts` is the `attachTerminal` opts, `desk` the
  // record this window continues (absent for a fresh launch).
  function spawnWindow(termOpts, label, repo, desk) {
    const kind = termOpts.console ? "console" : "agent";
    const { win, body, title, restartBtn, closeBtn } = buildChrome(label, repo, desk, kind);
    // A launch that names a worktree records the intent NOW, so a daemon that
    // dies mid-launch still leaves it behind.
    if (termOpts.checkout !== undefined) win._deskCheckout = termOpts.checkout ?? null;

    // The terminal owns the Ctrl latch and the selection arming; the key-bar
    // buttons (assigned below) only REFLECT them.
    let ctrlBtn = null;
    let selBtn = null;

    // Debounced nudge for a keystroke typed into a parked window (#335):
    // repeated typing EXTENDS the pulse rather than stacking timers.
    let nudgeTimer = null;
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

    // NAMED, not inline: a dormant console rebuilds its terminal (`wakeWindow`)
    // and the rebuild must be wired to the same chrome. Everything closes over
    // `win`, never a particular terminal.
    const termWiring = {
      ...termOpts,
      onCtrlLatch: (on) => {
        if (ctrlBtn) ctrlBtn.setAttribute("aria-pressed", on ? "true" : "false");
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
        persistWin(win);
      },
      // Parked: watching a session another window drives. It KEEPS its window
      // and output; the strip's button is the only way `takeover` is ever sent
      // (#334, ADR-0051 §9).
      onPark: () => {
        if (win.querySelector(".session-parked")) return;
        const strip = document.createElement("div");
        strip.className = "session-parked";
        const text = document.createElement("span");
        const parkedRepo = window.WBFleet ? window.WBFleet.refSlug(repo) : repo;
        text.textContent = `read-only: ${label} · ${parkedRepo || "home"} is controlled by another window`;
        const hint = document.createElement("span");
        hint.className = "session-parked-hint";
        const btn = document.createElement("button");
        btn.className = "session-reconnect";
        btn.dataset.act = "take-over";
        btn.textContent = "take over";
        btn.addEventListener("click", (e) => {
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
        if (hintEl) hintEl.textContent = "input is read-only — take over to type";
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
      // agent said) and the restart control appears.
      onEnded: () => {
        clearNudge();
        win.querySelector(".session-parked")?.remove();
        win.classList.add("ended");
        if (OPTS.canLaunch !== false) restartBtn.hidden = false;
      },
    };
    win._termWiring = termWiring;
    // On the window, not in a local: after a sleep/wake cycle a captured local
    // would name a disposed terminal.
    win._term = attachTerminal(body, termWiring);
    // The placeholder's relaunch path: carry this window's record, drop the
    // dead window, spawn a FRESH session — never the old `id`/`watch` opts,
    // which would reattach to a torn-down session. `checkout` CHOSEN by the
    // title's switcher (#412); `undefined` means "the recorded one".
    const relaunchIn = (checkout) => {
      const carry = deskOf(win);
      clearNudge();
      closeCheckoutMenu();
      win._term?.dispose();
      win.remove();
      untrackDormancy(win);
      wins.delete(win);
      applyExtent();
      // `win._deskKind`, not the local `kind`: a window reattached at load was
      // spawned with `{id, repo}` only. `~` is the daemon's repo-less label.
      const plain = win._deskKind === "console";
      const at = repo === "~" ? undefined : repo;
      // A window reattached at load has no `termOpts.checkout`; the recorded
      // announcement, else the desk record, keeps the restart in its tree (#411).
      const fresh = plain
        ? { console: true, repo: at }
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
    restartBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      relaunchIn(undefined);
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
      const holdFocus = (e) => e.preventDefault();
      bar.addEventListener("pointerdown", holdFocus);
      bar.addEventListener("mousedown", holdFocus);

      const key = (name, text, title, cls) => {
        const b = document.createElement("button");
        b.type = "button";
        // Not in the tab order: a keyboard user already has these keys.
        b.tabIndex = -1;
        b.className = "session-key" + (cls ? " " + cls : "");
        b.dataset.key = name;
        b.title = title;
        b.innerHTML = text;
        bar.append(b);
        return b;
      };

      key("esc", "esc", "Escape");
      key("tab", "tab", "Tab");
      ctrlBtn = key("ctrl", "ctrl", "Ctrl: applies to the next key");
      ctrlBtn.setAttribute("aria-pressed", "false");
      key("left", '<i class="bi bi-arrow-left"></i>', "Left");
      key("down", '<i class="bi bi-arrow-down"></i>', "Down");
      key("up", '<i class="bi bi-arrow-up"></i>', "Up");
      key("right", '<i class="bi bi-arrow-right"></i>', "Right");
      key("ctrl-c", "^C", "Ctrl-C — interrupt");
      // Arms ONE drag to select whole lines; the gesture's end disarms it.
      selBtn = key("select", "sel", "Select lines: drag across the screen");
      selBtn.setAttribute("aria-pressed", "false");

      const gap = document.createElement("span");
      gap.className = "session-keys-gap";
      bar.append(gap);

      // `writeClipboard`'s textarea fallback runs inside this click (a user
      // gesture), which is what makes it work on an insecure LAN origin.
      // `bi-copy`, not `bi-clipboard`: the clipboard glyph is the PASTE icon.
      const copyBtn = key("copy", '<i class="bi bi-copy"></i>', "Copy selection");
      copyBtn.disabled = true;
      const syncCopy = () => {
        copyBtn.disabled = !win._term?.term.hasSelection();
      };
      // The one piece of chrome bound to a PARTICULAR terminal:
      // `onSelectionChange` is on the xterm instance, so a woken console's new
      // instance needs it again.
      win._rewire = (t) => {
        t.term.onSelectionChange(syncCopy);
        syncCopy();
      };
      win._rewire(win._term);

      // Paste. The read has no `execCommand` fallback, so on an insecure origin
      // the button is disabled (`pasteOffered`). Text only: an image rides the
      // keyboard's `paste` event (ADR-0055), not this.
      const pasteBtn = key("paste", '<i class="bi bi-clipboard"></i>', "Paste");
      pasteBtn.disabled = !pasteOffered(navigator.clipboard);

      key("font-down", "A−", "Smaller text");
      key("font-up", "A+", "Larger text");

      bar.addEventListener("click", (e) => {
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
            .then((text) => {
              if (text) win._term.term.paste(text);
            })
            .catch(() => {})
            .finally(() => win._term.term.focus());
        } else if (name === "copy") {
          writeClipboard(win._term.term.getSelection(), win._term.term);
        } else if (name === "select") {
          win._term.setSelecting(!win._term.selecting);
        } else if (name === "font-up" || name === "font-down") {
          setFont(stepFont(fontSize(), name === "font-up" ? 1 : -1));
        } else {
          win._term.sendKey(name);
        }
        // Back to the terminal, inside the gesture, so the keyboard stays up.
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
          : `Ends the ${label} session. Scrollback is lost.`,
        confirmLabel: "Close",
        danger: true,
      });
      if (!ok) return;
      const finish = () => {
        // A window closed mid-pulse must not leave `nudgeTimer` pending.
        clearNudge();
        win._term?.dispose();
        forgetRecord(win._deskId);
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
        fetch(window.WBSessionRoute.closeUrl(id, win._deskRepo), {
          method: "POST",
        }).then(
          (response) => {
            if (window.WBSessionRoute.closeSucceeded(response.status)) finish();
            else
              win._term?.term.write(
                `\r\n[close failed — HTTP ${response.status}]\r\n`,
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
    persistWin(win);
    return win;
  }

  // The window's placement as a desk record, to carry identity and box across
  // a rebuild (takeover, placeholder → live console).
  function deskOf(win) {
    return {
      id: win._deskId,
      repo: win._deskRepo,
      agent: win._deskAgent,
      kind: win._deskKind,
      daemonId: win._deskDaemonId,
      environment: win._deskEnvironment,
      checkout: win._deskCheckout ?? null,
      locked: !!win._deskLocked,
      rect: restoreRect(win),
      max: win.classList.contains("maximized"),
    };
  }

  // Spawn an agent console — unless the worktree it asks for is gone, in which
  // case a placeholder SAYS so (#411). A console must never silently land on
  // the primary tree because its own vanished (the #409 gates).
  async function spawnOrMissing(req, label, repo, carry) {
    if (req.checkout && !(await checkoutStillThere(repo, req.checkout))) {
      return spawnPlaceholder({ ...carry, checkout: req.checkout }, req.checkout);
    }
    return spawnWindow(req, label, repo, carry);
  }

  // An agent console the daemon no longer runs: same chrome, same box, no
  // session — one click relaunches into this very record. `missing` names a
  // worktree that no longer exists (#411): the button relaunches on the
  // PRIMARY tree, explicitly by its label.
  function spawnPlaceholder(record, missing) {
    const { win, body, closeBtn } = buildChrome(record.agent, record.repo, record, record.kind);
    win.classList.add("placeholder");

    const note = document.createElement("div");
    note.className = "session-offline";
    const text = document.createElement("p");
    text.textContent = "agent console — not running";
    const btn = document.createElement("button");
    btn.className = "session-reconnect";
    btn.textContent = "relaunch";
    // Relaunching spawns a vendor CLI: the popup offers no way to start anything.
    note.append(text, ...(OPTS.canLaunch === false ? [] : [btn]));
    body.append(note);

    const markMissing = (name) => {
      missing = name;
      win.classList.add("missing-checkout");
      text.textContent = `worktree ${name} no longer exists`;
      btn.textContent = "relaunch in primary";
    };
    if (missing) markMissing(missing);
    // A placeholder restored for a recorded worktree asks whether that tree is
    // still there (no spawn), so the box says "gone" on load, not on the click.
    else if (record.checkout && OPTS.canLaunch !== false) {
      checkoutStillThere(record.repo, record.checkout).then((there) => {
        if (!there && win.isConnected) markMissing(record.checkout);
      });
    }

    const drop = () => {
      win.remove();
      untrackDormancy(win);
      wins.delete(win);
      applyExtent();
      changed();
    };
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const carry = deskOf(win);
      drop();
      // The agent menu's launch path, reusing this record's id, rect and
      // maximized state — in the recorded worktree unless that is the one that
      // is gone, in which case the button said "primary".
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
    persistWin(win);
    return win;
  }

  // `agent` names an adapter (claude/codex/opencode); when `plain` is set there
  // is no agent — a normal shell in the repo dir, labelled "console".
  function open({ repo, agent, plain, checkout }) {
    const label = agent || "console";
    // The plain console ignores the checkout: it rides the repo path (on a
    // peer, `wsl.exe --cd`) and stays on the primary.
    spawnWindow(plain ? { console: true, repo } : { repo, agent, checkout }, label, repo);
    WB.emit("console-open", { repo: repo || null, agent: agent || null, plain: !!plain });
  }

  // Reach an ALREADY LIVE session by id: focus the window holding it, else
  // attach one. INVARIANT — no path here composes a `?repo=&agent=` launch: a
  // busy id parks as a watcher of the SAME id, so "reach" can never become a
  // second session (#304, #334).
  function reach({ id, agent, repo }) {
    // No id asks for a NEW console: `sessionIdOf` answers `null` for a
    // placeholder, so a null id would match the first one it meets.
    if (id == null) return spawnWindow({ repo }, agent || "console", repo);
    for (const win of wins) {
      if (sessionIdOf(win) === id) {
        // Reveal, not merely focus: the session may be off the viewport.
        reveal(win._deskId) || focusWin(win);
        return win;
      }
    }
    return spawnWindow({ id, repo }, agent || "console", repo);
  }

  // Restore the desk: reconcile the saved layout against the daemon's live
  // sessions and dispatch one window per verdict. A REJECTED fetch leaves the
  // desk untouched — no relaunch, no phantom placeholders.
  function restoreDesk() {
    // The static demo restores nothing but must still LAND, or the latch never
    // arms and the offset is never stored.
    if (!window.WBMode?.isDaemon()) {
      deskSettled = true;
      applyLanding();
      return;
    }
    Promise.all([
      deskReady,
      fetch("/api/sessions").then((r) =>
        r.ok ? r.json() : Promise.reject(new Error("sessions unavailable")),
      ),
    ])
      .then(async ([, sessions]) => {
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
        for (const { record, session, action } of reconcileDesk({
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
          } else if (action === "relaunch") {
            // `relaunchRequest` carries the worktree the record was in (#411).
            pending.push(
              spawnOrMissing(relaunchRequest(record), record.agent, record.repo, record),
            );
          } else if (action === "placeholder") {
            spawnPlaceholder(record);
          } else {
            // `adopt`: a cascaded window with a fresh record, keeping the live
            // session's own kind so the desk relaunches it correctly next time.
            spawnWindow(
              { id: session.id, repo: session.repo },
              session.agent || "console",
              session.repo,
              {
                kind: session.kind,
                daemonId: session.daemon_id,
                environment: session.environment,
                checkout: session.checkout ?? null,
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
        // The glyph is DOM the fences own: lit only once they are on the stage.
        for (const id of detached) showDetachGlyph(id, true);
        applyExtent();
        raiseMaximized();
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
  // Pan by dragging the BARE FLOOR. Calls neither `applyExtent` nor
  // `persistWin` nor `focusWin`: panning moves the view, not the rects.
  function onFloorDown(e) {
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
    const onMove = (ev) => {
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
  function onWheel(e) {
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

  // Wired in the static demo too (`restoreDesk` returns early): an empty plane
  // still pans. `autoBoot: false` is the popup, which renders only the members
  // its opener hands over.
  function boot() {
    wireStage();
    restoreDetached();
    if (OPTS.autoBoot !== false) restoreDesk();
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }

  // Refit every open console. Called when the Consoles tab returns to view: a
  // terminal opened/reattached while the tab was display:none measured 0×0.
  function refitAll(attempt) {
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
  }

  // Tile ONE fence's members into its own rect (#342); windows animate via
  // the `.tiling` CSS transition. The grid is inset by the fence's OWN chrome:
  // the head band and the SE `.fence-grip` sit BELOW every window, so a member
  // parked on either makes the fence's controls unhittable.
  const FENCE_GRIP = 14;
  function arrangeFence(id) {
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
    const live = fences.map((x) => (x.id === id ? { id: x.id, rect } : x));
    const ids = new Set(fenceMembership(live, all)[id] || []);
    // A maximized console is NOT tiled: a tile rect written onto it is
    // invisible while it REPLACES the pre-maximize rect. Filtered before the
    // grid so it stays hole-free (#338). A LOCKED console is skipped too.
    const members = all
      .filter((m) => ids.has(m.id) && !m.el.classList.contains("maximized") && !m.el._deskLocked)
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
      if (win.classList.contains("maximized")) focusWin(win);
    }
    // AFTER the 0.24s tiling transition: an immediate fold would measure the
    // pre-arrange boxes. The PERSIST is in here for the same reason:
    // `persistWin` reads `offsetLeft`/`offsetWidth`, which still hold the
    // PRE-arrange box while the transition runs (MEASURED: a reload replayed
    // the old layout).
    setTimeout(() => {
      for (const win of members) {
        try {
          win._term?.fit.fit();
        } catch {}
        // The tab may have been hidden meanwhile (`x-show`): a hidden window
        // measures 0x0 at 0,0. Skip; the inline rect survives.
        if (!win.offsetWidth || !win.offsetHeight) continue;
        persistWin(win);
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
      applyExtent();
    });
  }

  return {
    open,
    relaunchRequest,
    checkoutMenuRows,
    ingestWorktrees,
    ingestSessions,
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
    reconnectDecision,
    resumeDecision,
    resumeAll,
    dormancyDecision,
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
    prefersDomRenderer,
    isWebKit,
    fullscreenOffered,
    flingStep,
    keySequence,
    applyCtrlLatch,
    keyBarVisible,
    pasteOffered,
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
    pasteDecision,
    reconcileDesk,
    mergeDesk,
    restoreRect,
    sessionPresentation,
    pruneDesk,
    reach,
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
    detachFold,
    peerFold,
    DETACH_MAX,
    detachFence,
    reattachFence,
    isDetached,
    mountDetached,
    stepFence,
    jumpToFence,
    focusedFence: focusedFenceId,
    spawnRectIn,
    createFence,
    atFenceCap,
    nextFenceName,
    FENCE_MAX,
    renameFence,
    removeFence,
  };
})();

