/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Consoles tab)

   Consoles live as draggable, resizable windows on the STAGE — a plane over the
   dotted floor that the VIEWPORT (`#workspace`, `overflow:auto`) scrolls over.
   This module contributes the window chrome (stage-relative drag/resize/tiling
   and the stage's own extent); the terminal body is the REAL thing, a live
   xterm.js attached to a PTY over the daemon's `/ws/session` WebSocket —
   transplanted verbatim from crates/ralphy-daemon/assets/ui/index.html
   (index.html contributes the truth, this module contributes the chrome).

   Opening/closing a console spawns/closes a daemon-owned session; on page load
   the live sessions are re-opened as windows so a reload reattaches with
   scrollback.
--------------------------------------------------------------------------- */
window.WBConsole = (function () {
  // The viewport (the scrolling box) and the stage (the sized plane inside it).
  const workspace = () => document.getElementById("workspace");
  const stage = () => document.getElementById("stage");
  // Scheme-match the session socket to the page (see wb-daemon.js WS_ORIGIN):
  // `wss://` over a TLS dev-tunnel/proxy, `ws://` for a plain-http localhost bind.
  const WS_ORIGIN =
    (location.protocol === "https:" ? "wss://" : "ws://") + location.host;
  const wins = new Set();
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

  // ---- the desk layout ---------------------------------------------------------
  // What was open, not merely where a session sat: each window contributes a
  // record keyed by a STABLE client-side id, carrying repo, agent, session kind,
  // rect and maximized flag. The daemon's session id is a volatile ATTRIBUTE —
  // a restarted daemon hands out ids from 1 again, so keying on it (as the
  // retired geometry store did) leaves records pointing at sessions that no
  // longer exist. The array is capped so it cannot grow without bound.
  // The desk lives in the DAEMON (`GET`/`PUT /api/desk`, ADR-0050), not the
  // browser: a workbench session survives the browser, so its window must too.
  // `desk` is the in-memory mirror and the SYNCHRONOUS source of truth, which is
  // what keeps `persistWin`/`forgetRecord`/`deskOf` callable from a mousemove.
  const DESK_MAX = 24;
  let desk = [];
  // `deskLoaded` is the upload PERMIT: until the daemon's own desk has landed we
  // do not know what we would be replacing, and `PUT /api/desk` replaces the desk
  // wholesale. Under the `Session` policy the pre-login `/api/desk` answers 401 —
  // treating that as "an empty desk" and then flushing would destroy the
  // operator's layout on their first drag, so a refused load leaves this false
  // and every flush is suppressed until `reloadDesk()` succeeds after login.
  let deskLoaded = false;
  // Whether this page has mutated `desk` since the load was issued. A record the
  // operator just created or deleted must survive a later-arriving GET.
  let deskDirty = false;

  // Ids this page deleted; they must not come back on a later-arriving GET.
  const deskRemoved = new Set();

  // The desk's second record type (issue #340): named rectangles on the floor
  // tier. Same store, same route, same upload permit as `desk` — a fence is
  // daemon state, so it comes back on any browser.
  const FENCE_MAX = 12;
  let fences = [];
  let fencesDirty = false;
  // Fence ids this page deleted; same role as `deskRemoved`.
  const fencesRemoved = new Set();

  // The fence half of the fold, per id — NOT a wholesale replace. A page that
  // draws a fence before its own GET lands (the boot race: `deskReady` is issued
  // at module load and the toolbar is live before it resolves) would otherwise
  // discard every persisted fence, and the very next flush would write that loss
  // through. The `deskLoaded` permit does not cover this: it lifts on the line
  // below, AFTER the discard already happened.
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

  function ingestDesk(payload) {
    const fetched = Array.isArray(payload?.windows) ? payload.windows : [];
    ingestFences(Array.isArray(payload?.fences) ? payload.fences : []);
    if (!deskDirty) {
      desk = fetched;
    } else {
      // Local wins per id (it is newer by construction), and a record deleted
      // here stays deleted — merging the daemon's copy back in would resurrect
      // exactly what `forgetRecord` removed.
      const mine = new Set(desk.map((r) => r.id));
      const removed = new Set(deskRemoved);
      desk = fetched
        .filter((r) => !mine.has(r.id) && !removed.has(r.id))
        .concat(desk);
    }
    deskLoaded = true;
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
    const live = new Set([...wins].map((w) => w._deskId));
    const before = new Set(desk.map((r) => r.id));
    desk = pruneDesk(records, DESK_MAX, live);
    const after = new Set(desk.map((r) => r.id));
    for (const id of before) if (!after.has(id)) deskRemoved.add(id);
    deskDirty = true;
    scheduleDeskFlush();
  }
  // Capped HERE as well as in the daemon: the flush discards the PUT response,
  // so a client that ignored the cap would show 13 fences while the store held
  // 12 — and the one the daemon dropped is the oldest by `ts`, not the one the
  // operator just drew.
  function saveFences(next) {
    const before = new Set(fences.map((f) => f.id));
    fences = pruneDesk(next, FENCE_MAX);
    const after = new Set(fences.map((f) => f.id));
    for (const id of before) if (!after.has(id)) fencesRemoved.add(id);
    fencesDirty = true;
    scheduleDeskFlush();
  }
  // The upload body. ONE spelling for both flush paths, so a record type can
  // never be uploaded by one and dropped by the other.
  function deskBody() {
    return { windows: desk, fences };
  }
  // The upload, debounced and fire-and-forget. INVARIANT: no drag, resize, close
  // or `persistWin` path may await or throw on this — a refused PUT costs a stale
  // position and the next mutation supersedes it (last write wins, no ETag).
  // Chained on the previous flush so two mutations 250 ms apart cannot land out
  // of order over a LAN or a dev tunnel.
  let deskFlush = null;
  let deskInFlight = Promise.resolve();
  function scheduleDeskFlush() {
    if (!window.WBMode?.isDaemon()) return;
    clearTimeout(deskFlush);
    // Cleared when it FIRES, not only when it is replaced: a spent timer id is
    // still truthy, and `pagehide` reads it as "a write is pending" — which
    // re-uploaded a stale mirror over a newer desk on every reload.
    deskFlush = setTimeout(() => {
      deskFlush = null;
      flushDesk();
    }, 250);
  }
  function flushDesk() {
    // Never upload over a desk this page failed to read (offline, or pre-login
    // under the `Session` policy) — that is a wholesale replace of the
    // operator's real layout with whatever this page happens to hold.
    if (!deskLoaded) return;
    const body = JSON.stringify(deskBody());
    deskInFlight = deskInFlight
      .catch(() => {})
      .then(() =>
        fetch("/api/desk", {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body,
        }).catch(() => {}),
      );
  }
  // A mutation inside the last 250 ms before the tab closes would otherwise be
  // dropped — the window would come back "open" on the next load. `keepalive`
  // lets the request outlive the document.
  window.addEventListener("pagehide", () => {
    // The per-client view first, and BEFORE the desk guard below: `WBView`'s
    // store is synchronous (no `keepalive` dance needed) and `deskLoaded` has
    // nothing to say about it — gating the offset on the desk's upload permit
    // would drop the last pan of every pre-login or demo page.
    if (offsetFlush) {
      clearTimeout(offsetFlush);
      offsetFlush = null;
      flushOffset();
    }
    if (!deskLoaded || !deskFlush) return;
    clearTimeout(deskFlush);
    deskFlush = null;
    try {
      fetch("/api/desk", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(deskBody()),
        keepalive: true,
      }).catch(() => {});
    } catch {}
  });
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
  // A window's RESTORE box. While maximized the `.maximized` class pins all four
  // offsets via `!important` (left/top to 0, width/height to the full bleed), so
  // every component must be read from the inline styles — those still hold the
  // pre-maximize rect. Reading `offsetLeft`/`offsetTop` here would persist 0,0 and
  // the window would restore to the workspace corner instead of where it was.
  function restoreRect(win) {
    if (!win.classList.contains("maximized")) {
      return {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
    }
    const inline = (prop, fallback) => parseInt(win.style[prop], 10) || fallback;
    return {
      left: inline("left", win.offsetLeft),
      top: inline("top", win.offsetTop),
      width: inline("width", win.offsetWidth),
      height: inline("height", win.offsetHeight),
    };
  }

  // Snapshot a window's placement. A maximized window stores its *pre-maximize*
  // rect (the class drives the full-bleed via CSS), so `max` restores the
  // full-screen state while the stored rect still restores the underlying box.
  function persistWin(win) {
    // A detached window measures 0×0 at 0,0 — and because this upserts by id, a
    // late mouseup after the window was removed would RESURRECT a record that
    // `forgetRecord` just deleted.
    if (!win._deskId || !win.isConnected) return;
    const rec = {
      id: win._deskId,
      repo: win._deskRepo,
      agent: win._deskAgent,
      kind: win._deskKind,
      rect: restoreRect(win),
      max: win.classList.contains("maximized"),
      sessionId: win._term?.sessionId ?? null,
      ts: Date.now(),
    };
    const records = loadDesk();
    const i = records.findIndex((r) => r.id === rec.id);
    if (i >= 0) records[i] = rec;
    else records.push(rec);
    saveDesk(records);
    // A window that moved may have joined or left a region — membership is
    // derived, so the readouts only change when something re-derives them.
    // (Written without the lowercase noun on purpose: #341's pin greps this
    // whole body for it, and the pin is worth more than the word.)
    refreshFenceChrome();
  }
  function forgetRecord(deskId) {
    if (!deskId) return;
    saveDesk(loadDesk().filter((r) => r.id !== deskId));
    refreshFenceChrome();
  }

  // The restore decision, as a pure fold of the saved layout over the live
  // session list. Each live session is consumed by AT MOST ONE record (the first
  // in layout order), because a restarted daemon reuses ids and two stale records
  // could otherwise both claim id 1 — hence the full `sessionId`+`repo`+`agent`+
  // `kind` tuple rather than a bare id match.
  function reconcileDesk({ layout, sessions }) {
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
        // console waits for one deliberate click (loading a page must never spawn
        // a vendor CLI and spend quota nobody authorized).
        out.push({
          record,
          session: null,
          action: record.kind === "console" ? "relaunch" : "placeholder",
        });
      }
    }
    // A live session no record claims (opened in another tab, or by an older
    // build) is adopted, so it stays visible and closable.
    live.forEach((s, idx) => {
      if (!used.has(idx)) out.push({ record: null, session: s, action: "adopt" });
    });
    return out;
  }

  // Toggle a console between its floating rect and a full-VIEWPORT bleed. The
  // pre-maximize rect stays in the inline styles (drag/resize are inert while
  // maximized), so restoring is just dropping the class.
  //
  // On a scrollable stage the bleed must be pinned to what the operator is
  // looking at, not to the plane's origin: `--max-left`/`--max-top` carry the
  // viewport's scroll offsets, and `syncMaxPin` re-derives them from the LIVE
  // offsets on every path that can change them. The offsets are re-asserted
  // after the class flip because `maxlock` (`overflow:hidden`) drops the
  // scrollbars, which can clamp `scrollLeft`/`scrollTop` on the way.
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
    // AFTER the restore, never from `offsets`: the pin must be derived from the
    // offsets that actually SURVIVED the `maxlock` flip, not from the pair read
    // before it.
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

  // Size the stage to hold every window. NOTHING here moves or resizes a window:
  // the plane grows under them instead, and a viewport too small to show it all
  // scrolls (issue #336 deleted the clamp-and-refit that used to deform the
  // layout on every chrome-panel toggle). `grow` floors each axis at the current
  // pixels — shrinking mid-drag would clamp `scrollLeft` under the operator's
  // cursor and make the view jump; the exact recompute runs on mouseup.
  // INVARIANT: every path that creates, moves, resizes, closes or restores a
  // window ends here.
  // The scroll freeze that keeps a maximized window's viewport pin honest.
  // DERIVED from the DOM at every layout mutation, never toggled by hand:
  // closing a maximized console removes the window without ever passing through
  // `toggleMax`, and a hand-held lock then stranded `overflow:hidden` on the
  // viewport for the rest of the page's life — the plane could not be scrolled
  // again, which is exactly the unreachable-window state ADR-0051 §4 exists to
  // eliminate.
  function syncMaxLock() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    ws.classList.toggle("maxlock", !!st.querySelector(".session-window.maximized"));
  }

  // The maximize pin itself, DERIVED the same way. `--max-left`/`--max-top`
  // place the full bleed over what the operator is looking at, so they are only
  // honest while they equal the viewport's CURRENT offsets — a pin written once
  // at maximize time silently desyncs the moment anything pans the plane.
  // INVARIANT: every path that changes `#workspace`'s scroll offsets ends here.
  // The viewport's own `scroll` event (`wireStage`) covers the gesture, the
  // wheel, the scrollbar AND `reveal`'s programmatic write; `toggleMax` calls it
  // after the class flip, and the un-maximize branch there still REMOVES both
  // properties, which is why this only ever writes to `.maximized` windows.
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

  function applyExtent(opts) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // Every create/move/resize/close/restore path reaches here, so this is the
    // one place the freeze can be kept in step with what is actually on screen.
    syncMaxLock();
    // Read the DOM, not `wins`: a window is on the stage from the moment
    // `buildChrome` appends it (before `spawnWindow` registers it) and gone the
    // moment it is removed, so this can never count a phantom or miss a new one.
    // Fences count too — ADR-0051 §2 sizes the plane to windows AND fences.
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
    // Publish the extent to the frame's footer pill (issue #338). ONLY on a real
    // change: a drag folds the extent on every mousemove, and an unconditional
    // dispatch would re-render Alpine per frame for an unchanged pair.
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
  // since panned — but a STORED offset is re-applied on every call, because
  // `.consoles-tab` is `x-show` and `display:none` destroys `#workspace`'s
  // scroll position: without that, one tab switch silently loses the pan.
  // INVARIANT: this never runs before `applyExtent()` on any path — the clamp
  // needs the extent the same frame's rects imply, or a restored offset would be
  // clamped against a stage that has not grown yet.
  let landed = false;
  // A reveal that arrived while `.consoles-tab` was still `display:none`. The
  // viewport measures 0 there, so `reveal` cannot centre against it and parks
  // the desk id here instead; the first `applyLanding` that CAN measure honours
  // it AHEAD of the stored offset. Both halves matter: without the park the
  // reveal is dropped (measured — `openConsoleItem` calls `reach` on the same
  // synchronous stack as `activate`, and Alpine's `x-show` flip is a microtask
  // later), and without the precedence the landing re-applies the stored view
  // and slides the plane straight back off the window that was asked for.
  let pendingReveal = null;
  // Whether `restoreDesk` has finished — on ANY of its three exits, including
  // the demo early return and a failed fetch. It is what lets the latch below
  // distinguish "the stage is empty because nothing was restored YET" from "the
  // stage is empty because there is nothing to restore".
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
    const stored = window.WBView?.read()?.off || null;
    if (landed && !stored) return;
    const at = viewLanding(
      stored,
      rects,
      { width: ws.clientWidth, height: ws.clientHeight },
      { width: st.offsetWidth, height: st.offsetHeight },
    );
    ws.scrollLeft = at.left;
    ws.scrollTop = at.top;
    // Latch only once the stage HOLDS something (or is known to be final):
    // `restoreView` reaches `refitAll` — and so this — after ONE round trip,
    // while `restoreDesk` needs two plus the window spawn. Latching on that
    // still-empty frame would make the later, real landing return early at the
    // guard above, and the operator would boot pinned at 0,0 with every
    // restored console off-frame.
    if (rects.length || deskSettled) landed = true;
  }

  // The offset half of the store, debounced like the desk flush. SUPPRESSED
  // until the landing has been applied: `applyExtent` and the `x-show` flip both
  // fire `scroll` before the restore, so an unguarded listener would persist 0,0
  // over the operator's stored pan on every boot.
  let offsetFlush = null;
  let pendingOffset = null;
  function flushOffset() {
    // Writes the offset CAPTURED at schedule time, never a fresh read. The
    // debounce outlives its own guard: switching to a file tab inside the
    // 250 ms window hides `.consoles-tab` (`x-show`), and `display:none` resets
    // the viewport's offsets to 0 — a flush that re-measured would persist that
    // reset as the operator's chosen view (measured: `off:{0,0}` stored over a
    // real 1500,850 pan, ~470 ms after boot), and one that merely bailed would
    // silently drop the operator's last pan instead.
    if (!pendingOffset) return;
    window.WBView?.patch({ off: pendingOffset });
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
  // the stage is where a window IS, and this is a snapshot taken when the menu
  // opens rather than reactive state to keep in step.
  function list() {
    const st = stage();
    if (!st) return [];
    return [...st.querySelectorAll(".session-window")].map((w) => ({
      id: w._deskId,
      agent: w._deskAgent,
      // `"~"` is the desk's spelling of "no repo"; the picker says `home`, the
      // same word the titlebar uses, rather than leaking the storage token.
      repo: w._deskRepo === "~" ? null : w._deskRepo,
      kind: w._deskKind,
      running: !w.classList.contains("placeholder"),
    }));
  }

  // The "one action" that reaches a window far from the current view (ADR-0051
  // §4): focus it and slide the viewport so it is centred. Returns the element,
  // or null when no window carries that desk id.
  function reveal(deskId) {
    const ws = workspace();
    const it = findWindow(deskId);
    if (!it) return null;
    if (ws && ws.clientWidth && ws.clientHeight) return revealNow(deskId);
    // A viewport that measures 0 is a tab still `display:none` — this repo has
    // measured that trap (CONTEXT.md → Testing conventions). Centring against
    // it would clamp to 0,0 and slide the plane somewhere the operator never
    // asked for. Focus now, and park the centring for the frame that can
    // measure it (see `pendingReveal`) rather than dropping it.
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
    // A maximized console already fills the frame, so there is nothing to
    // centre. Only the TARGET is checked: Go-to is the one product path that
    // pans the plane while something else is maximized, and `maxlock`
    // (`overflow:hidden`) does NOT refuse a programmatic offset write — the
    // resulting `scroll` re-derives the pin (`syncMaxPin`), so the full bleed
    // follows the frame instead of desyncing from it (issue #338).
    if (it.classList.contains("maximized")) return it;
    const to = bringIntoView(
      restoreRect(it),
      { width: ws.clientWidth, height: ws.clientHeight },
      { width: st.offsetWidth, height: st.offsetHeight },
    );
    ws.scrollLeft = to.left;
    ws.scrollTop = to.top;
    // The reveal IS the operator's new view, stored NOW rather than by the
    // debounced `scroll` flush: `refitAll` runs its own `applyLanding` in the
    // same frame chain, and that call re-applies the STORED offset — which, for
    // the 250 ms until the flush, is still the pre-reveal one. Writing it here
    // is what stops the landing from undoing the centring. A pending flush is
    // dropped with it: it carries the offset captured before this scroll.
    if (landed) {
      pendingOffset = null;
      clearTimeout(offsetFlush);
      offsetFlush = null;
      window.WBView?.patch({ off: { left: to.left, top: to.top } });
    }
    return it;
  }

  // Drag by the titlebar, clamped to the STAGE (control buttons still click).
  // Coordinates are plane pixels: the stage's client rect already carries the
  // viewport's scroll shift, so a drag reads the same at any scroll offset. The
  // origin stays pinned at 0, so no drag can ever write a negative left/top.
  function makeDraggable(win, handle) {
    handle.addEventListener("mousedown", (e) => {
      if (e.target.closest("button")) return;
      // Primary button only: a right/middle press is followed by a `contextmenu`
      // (or no `mouseup` at all), which would strand `onMove` on the document and
      // leave the window tracking a cursor with no button held.
      if (e.button !== 0) return;
      focusWin(win);
      // Maximized windows don't drag — the titlebar double-click still restores.
      if (win.classList.contains("maximized")) return;
      const rect = win.getBoundingClientRect();
      const offX = e.clientX - rect.left;
      const offY = e.clientY - rect.top;
      // Put the window under `pointer` (a CLIENT point). Read the stage LIVE,
      // both for its size — `applyExtent({grow:true})` widens it as the window
      // nears the far edge — and for its ORIGIN: the viewport scrolls, and a
      // wheel or an auto-pan mid-drag would otherwise shift the plane under a
      // cached rect and drop the window off the cursor.
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
      // under it, so moving a console somewhere off-screen is one gesture. The
      // `place(last)` inside the tick is what keeps the DROP position correct in
      // stage coordinates — the window keeps following the cursor while the
      // plane slides beneath it.
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
        // The mouseup is NOT guaranteed to arrive: a right-press opening the
        // native context menu mid-drag, or an alt-tab with the button held,
        // swallows it. Without this recovery the pan loop re-arms forever and
        // no later gesture can remove this pair, because the next mousedown
        // installs its OWN closures.
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        last = { x: ev.clientX, y: ev.clientY };
        place(last);
        if (panRaf != null) return;
        const { dx, dy } = nudge();
        if (dx || dy) panRaf = requestAnimationFrame(tickPan);
      };
      // Escape ends the drag where the window currently sits — it does not
      // revert it. There is no "cancel" in this gesture's vocabulary: the
      // window has been following the cursor and its rect is already the
      // operator's; what Escape buys is a keyboard exit from a loop whose
      // mouseup may never arrive.
      const onKey = (ev) => {
        if (ev.key === "Escape") onUp();
      };
      const onUp = () => {
        stopPan();
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.removeEventListener("contextmenu", onUp);
        document.removeEventListener("keydown", onKey);
        window.removeEventListener("blur", onUp);
        applyExtent();
        persistWin(win);
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
      // The other half of the lost-mouseup recovery: `blur` fires when a native
      // menu or another window takes focus, which is the case where the pointer
      // never comes back to deliver the `buttons === 0` move above.
      window.addEventListener("blur", onUp);
      // …and `contextmenu` covers the case that recovery ASSUMES: a native menu
      // opened mid-drag. Whether the browser also blurs the window there is not
      // something this code should have to be right about — a menu over the
      // page is the end of the gesture either way, and a double `onUp` is
      // idempotent (`stopPan` is null-safe and the removals are no-ops).
      document.addEventListener("contextmenu", onUp);
      document.addEventListener("keydown", onKey);
      e.preventDefault();
    });
  }

  // ---- resize geometry ---------------------------------------------------------
  // The eight directions differ only in which rectangle components move, so the
  // whole resize is one pure function: `dir` (`n`/`s`/`e`/`w` and the four
  // corners), the stage-relative start `rect`, the pointer `delta`, the
  // minimum size and the stage `bounds` yield a new rect. East/south move the
  // far edge; west/north move `left`/`top` and derive the size, so the OPPOSITE
  // edge stays put and the window does not slide under the cursor.
  const RESIZE_MIN = { width: 240, height: 150 }; // matches .session-window's CSS minimums
  const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

  // ---- the stage extent --------------------------------------------------------
  // How big the plane under the windows must be, as a pure function of the rects
  // and the viewport: the bbox of the windows plus a margin of drag room past
  // their own edges, unioned per axis with the viewport. The origin is pinned at
  // 0,0 and the plane grows right and down only — a negative coordinate would
  // mean re-anchoring the origin and rewriting every rect (issue #336).
  // The viewport leg is what keeps an empty stage exactly viewport-sized, so a
  // scrollbar only ever measures something real.
  const STAGE_MARGIN = 200;

  function stageExtent(rects, viewport, margin) {
    const m = margin == null ? STAGE_MARGIN : margin;
    let right = 0;
    let bottom = 0;
    for (const r of rects || []) {
      right = Math.max(right, (r.left || 0) + (r.width || 0));
      bottom = Math.max(bottom, (r.top || 0) + (r.height || 0));
    }
    return {
      width: Math.max(viewport?.width || 0, right + m),
      height: Math.max(viewport?.height || 0, bottom + m),
    };
  }

  // ---- where a new fence lands (issue #340) ------------------------------------
  // Pure. A deterministic 2-column grid anchored at the viewport's CURRENT
  // offset, so a fence is born where the operator is looking rather than at the
  // pinned origin. DISJOINT BY CONSTRUCTION for every index: ADR-0051 §6's
  // non-overlap enforcement is the next slice's, and a spawn rule that stacked
  // fences would ship the overlap before the invariant exists to fix it.
  const FENCE_SIZE = { width: 720, height: 460 };
  const FENCE_MIN = { width: 240, height: 150 };
  const FENCE_INSET = 40;
  const FENCE_GAP = 24;
  const FENCE_COLS = 2;

  function fenceSpawnRect(offset, viewport, index) {
    const width = Math.max(
      FENCE_MIN.width,
      Math.min(FENCE_SIZE.width, (viewport?.width || 0) - 2 * FENCE_INSET),
    );
    const height = Math.max(
      FENCE_MIN.height,
      Math.min(FENCE_SIZE.height, (viewport?.height || 0) - 2 * FENCE_INSET),
    );
    const i = index || 0;
    const col = i % FENCE_COLS;
    const row = Math.floor(i / FENCE_COLS);
    return {
      left: Math.max(0, offset?.left || 0) + FENCE_INSET + col * (width + FENCE_GAP),
      top: Math.max(0, offset?.top || 0) + FENCE_INSET + row * (height + FENCE_GAP),
      width,
      height,
    };
  }

  function rectsOverlap(a, b) {
    return (
      (a?.left || 0) < (b?.left || 0) + (b?.width || 0) &&
      (a?.left || 0) + (a?.width || 0) > (b?.left || 0) &&
      (a?.top || 0) < (b?.top || 0) + (b?.height || 0) &&
      (a?.top || 0) + (a?.height || 0) > (b?.top || 0)
    );
  }

  // ---- a fence is a group (issue #341) -----------------------------------------
  // Membership is DERIVED, never stored: a window belongs to the fence that holds
  // its CENTRE point, so nothing has to be kept in step and a record can never
  // disagree with the geometry. Containment is HALF-OPEN (`left <= cx < left +
  // width`) — fences may abut, and a closed test would put a centre sitting on a
  // shared border inside both of them, breaking "exactly one fence".
  function rectCentre(rect) {
    return {
      x: (rect?.left || 0) + (rect?.width || 0) / 2,
      y: (rect?.top || 0) + (rect?.height || 0) / 2,
    };
  }

  // Does `rect` hold `point`? The half-open test above, extracted once (issue
  // #343) so membership and the floor's focus-clearing hit test can never drift
  // into two spellings of the same containment.
  function rectHolds(rect, point) {
    const r = rect || {};
    const left = r.left || 0;
    const top = r.top || 0;
    return (
      (point?.x || 0) >= left &&
      (point?.x || 0) < left + (r.width || 0) &&
      (point?.y || 0) >= top &&
      (point?.y || 0) < top + (r.height || 0)
    );
  }

  // `fences` is `[{ id, rect }]`, `windows` is `[{ id, rect }]`; the answer maps
  // EVERY fence id (an empty one to `[]`) and omits a window in no fence. The
  // `break` is the second half of "exactly one": half-open containment makes the
  // fences disjoint as point sets, and this makes the fold's own answer so even
  // if a stored rect pair ever overlaps.
  function fenceMembership(fences, windows) {
    const list = fences || [];
    const out = {};
    for (const f of list) out[f.id] = [];
    for (const w of windows || []) {
      const c = rectCentre(w?.rect);
      for (const f of list) {
        if (rectHolds(f.rect, c)) {
          out[f.id].push(w.id);
          break;
        }
      }
    }
    return out;
  }

  // May `candidate` (`{ id, rect }`) take the plane? Pure, and the SAME strict
  // `rectsOverlap` the spawn rule uses, so abutting fences stay buildable and
  // one predicate answers for create, move and resize alike. A candidate is
  // never compared with itself — a move must not refuse its own start rect.
  function fenceFits(fences, candidate) {
    const rect = candidate?.rect || {};
    return !(fences || []).some(
      (f) => f.id !== candidate?.id && rectsOverlap(rect, f.rect || {}),
    );
  }

  // The move delta a fence and the members it carries may actually take: the
  // request, clamped so no left/top lands below 0. The plane's origin is pinned
  // (issue #336) and grows right and down only, so a negative coordinate is not
  // a position — it is a lost window. The MEMBERS are in the fold too: one can
  // sit further left than the fence that carries it.
  function fenceMoveDelta(delta, fenceRect, memberRects) {
    const members = memberRects || [];
    const minLeft = Math.min(fenceRect?.left || 0, ...members.map((r) => r?.left || 0));
    const minTop = Math.min(fenceRect?.top || 0, ...members.map((r) => r?.top || 0));
    return {
      dx: Math.max(delta?.dx || 0, -minLeft),
      dy: Math.max(delta?.dy || 0, -minTop),
    };
  }

  // Tiling, as a pure fold (issue #342): target rect plus member list in, one
  // rect per member out, in order. The grid is the global Arrange's — `cols =
  // ceil(sqrt(n))` — kept aspect-independent so this stays a MOVE of the act,
  // not a redesign of it.
  //
  // Pad and gap degrade PER AXIS: a rect too small for its member count yields
  // a cell below `TILE_MIN` (a NEGATIVE height, for a short fence), and that
  // axis falls back to a bare `extent / k`. Collapsing both axes together would
  // deform an axis that still fits. Containment holds by construction in either
  // branch — the far edge lands at exactly `pad + k*size + (k-1)*gap = extent -
  // pad` — which is what makes "no member escapes the fence" a property of the
  // function rather than of its caller.
  const TILE_PAD = 12;
  const TILE_GAP = 10;
  const TILE_MIN = 24;

  function tileIntoRect(rect, members) {
    const n = (members || []).length;
    if (!n) return [];
    const cols = Math.ceil(Math.sqrt(n));
    const rows = Math.ceil(n / cols);
    const axis = (extent, k) => {
      const size = (extent - TILE_PAD * 2 - TILE_GAP * (k - 1)) / k;
      if (size >= TILE_MIN) return { pad: TILE_PAD, gap: TILE_GAP, size };
      return { pad: 0, gap: 0, size: extent / k };
    };
    const x = axis(rect?.width || 0, cols);
    const y = axis(rect?.height || 0, rows);
    return (members || []).map((_, i) => ({
      left: (rect?.left || 0) + x.pad + (i % cols) * (x.size + x.gap),
      top: (rect?.top || 0) + y.pad + Math.floor(i / cols) * (y.size + y.gap),
      width: x.size,
      height: y.size,
    }));
  }

  // The repos a fence's members belong to, for the fence's own chrome — what
  // lets the operator read a fence without visiting it. Deduped, sorted (DOM
  // order is not stable) and `"~"` rendered as `home`, the same rule `list()`
  // applies so the desk's storage token never leaks to the screen.
  function fenceRepos(members) {
    const names = new Set((members || []).map((m) => (m?.repo === "~" ? "home" : m?.repo)));
    names.delete(undefined);
    names.delete(null);
    names.delete("");
    return [...names].sort().join(" · ");
  }

  // One fence readout, for BOTH the fence's own chrome and the toolbar list
  // (issue #343): `[{id, name, rect}]` + `[{id, repo, rect}]` in, one entry per
  // fence IN ORDER out. Folding membership and repos here — rather than at each
  // caller — is what makes "the list and the fence can never disagree" a
  // property of the code instead of a test that happens to pass.
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
      };
    });
  }

  // Which grid slot a NEW fence takes, pure: the first one no existing fence
  // occupies. Indexing by `fences.length` instead would reuse a slot after a
  // removal — drop the middle of three and the next fence lands exactly on the
  // survivor, shipping the overlap ADR-0051 §6 does not yet enforce away.
  // Bounded by the slots a free one can hide in; the fallback is the old rule.
  function nextFenceSlot(rects, offset, viewport) {
    const taken = rects || [];
    for (let i = 0; i <= taken.length; i++) {
      const candidate = fenceSpawnRect(offset, viewport, i);
      if (!taken.some((t) => rectsOverlap(candidate, t))) return i;
    }
    return taken.length;
  }

  // ---- the fence floor ---------------------------------------------------------
  // A fence is a stage child on a tier BELOW every window (`z-index: 1` against
  // `Z_BASE`), and `pointer-events: none` on the box makes "never intercepts a
  // window drag, resize or focus click" true by construction — a press over a
  // fence reaches the window above it, or the stage below it, so `onFloorDown`'s
  // `e.target !== st` hit test keeps working unchanged and panning survives
  // inside a fence. Only the name field and the drop button opt back in.
  const FENCE_NAME_MAX = 60;

  function buildFence(f) {
    const el = document.createElement("div");
    el.className = "fence";
    el.dataset.fenceId = f.id;
    const head = document.createElement("div");
    head.className = "fence-head";
    // Two SMALL opt-in handles rather than an interactive head or box: the head
    // is the only pointer-taking band, and a full-width one would swallow the
    // floor's own pan (`onFloorDown` bails unless the press targets the stage).
    const grab = document.createElement("span");
    grab.className = "fence-grab";
    grab.title = "move this fence";
    grab.textContent = "⠿";
    grab.addEventListener("mousedown", startFenceMove(el, f));
    const name = document.createElement("input");
    name.className = "fence-name";
    name.setAttribute("aria-label", "fence name");
    name.value = f.name || "";
    // Always-live input, no click-to-swap editor: zero mode state, one code
    // path. `change` commits (Enter blurs, which fires it).
    name.addEventListener("change", () => renameFence(f.id, name.value));
    name.addEventListener("keydown", (e) => {
      if (e.key === "Enter") name.blur();
    });
    // Name · count · repos · arrange (issue #342): the fence is where tiling
    // means something, and the two readouts are what let the operator read a
    // fence without visiting it. Both are filled by `refreshFenceChrome`.
    const count = document.createElement("span");
    count.className = "fence-count";
    const repos = document.createElement("span");
    repos.className = "fence-repos";
    const tile = document.createElement("button");
    tile.className = "fence-arrange";
    tile.type = "button";
    tile.title = "tile this fence's consoles";
    tile.textContent = "⊞";
    tile.addEventListener("click", () => arrangeFence(f.id));
    const drop = document.createElement("button");
    drop.className = "fence-drop";
    drop.type = "button";
    drop.title = "remove this fence";
    drop.textContent = "×";
    drop.addEventListener("click", () => removeFence(f.id));
    head.append(grab, name, count, repos, tile, drop);
    const grip = document.createElement("div");
    grip.className = "fence-grip";
    grip.title = "resize this fence";
    grip.addEventListener("mousedown", startFenceResize(el, f));
    el.append(head, grip);
    stage()?.append(el);
    return el;
  }

  // ---- the fence gestures (issue #341) -----------------------------------------
  // Both gestures read the fence's rect from the DOM, never from the captured
  // `f`: `buildFence` runs once and `f.rect` goes stale the first time the fence
  // moves. Only `f.id` is taken from the closure.
  //
  // INVARIANT, honoured on EVERY exit path (mouseup, the `ev.buttons === 0`
  // lost-mouseup recovery, and `window` blur): the document listeners are
  // removed and the gesture is finalized EXACTLY ONCE, whether the drop is
  // accepted or refused. `done` is what makes a doubled exit — blur then
  // mouseup — a no-op instead of a second persist or a second revert.
  // The refusal flash `createFence` schedules below. Owned at module scope so a
  // gesture starting inside its 600 ms window can CANCEL it: otherwise the timer
  // strips a `fence-invalid` the gesture put there, and with the cursor at rest
  // no further move re-adds it — the fence would look valid while its drop is
  // about to be reverted.
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
      if (e.button !== 0) return; // primary button only — see makeDraggable
      const st = stage();
      if (!st) return;
      const start = restoreRect(el);
      // Membership is computed ONCE, at mousedown, and frozen for the gesture:
      // recomputing per move makes windows join and leave under the cursor as
      // the fence sweeps the plane, and the drop would carry a set nobody chose.
      const all = [...st.querySelectorAll(".session-window")].map((w) => ({
        el: w,
        id: w._deskId,
        rect: restoreRect(w),
      }));
      // The FULL fence list, not a one-element one: the fold's `break` is what
      // decides an overlapping pair (reachable through a hand-edited
      // `desk.toml`), and a singleton list bypasses it — the window would be
      // reported under one fence and carried by the other.
      const live = fences.map((x) => (x.id === f.id ? { id: x.id, rect: start } : x));
      const ids = new Set(fenceMembership(live, all)[f.id] || []);
      const carried = all.filter((m) => ids.has(m.id));
      const startX = e.clientX;
      const startY = e.clientY;
      let delta = { dx: 0, dy: 0 };
      let fits = true;
      let done = false;
      // A press with no movement is a CLICK, not a drop. Persisting it would
      // upload the fence and every member it carries with a fresh `ts` — which
      // reorders `pruneDesk`'s eviction and makes an unrelated record the
      // victim at the cap, for a gesture that changed nothing.
      let moved = false;
      clearFenceFlash();
      const onMove = (ev) => {
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const d = fenceMoveDelta(
          { dx: ev.clientX - startX, dy: ev.clientY - startY },
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
      const onUp = () => {
        if (done) return;
        done = true;
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
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
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
  }

  // Resize moves the FENCE only — never a member. A window whose centre falls
  // outside the new rect simply stops being reported by `fenceMembership`, which
  // is the whole point of deriving membership instead of storing it.
  function startFenceResize(el, f) {
    return (e) => {
      if (e.button !== 0) return;
      const st = stage();
      if (!st) return;
      const start = restoreRect(el);
      // Captured ONCE, for `startResize`'s reason (line ~1027): a live re-read
      // feeds the extent this gesture grows back in as its own bound.
      const bounds = { width: st.offsetWidth, height: st.offsetHeight };
      const startX = e.clientX;
      const startY = e.clientY;
      let out = start;
      let fits = true;
      let done = false;
      let sized = false; // a click is not a resize — see `moved` in startFenceMove
      clearFenceFlash();
      const onMove = (ev) => {
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const next = resizeRect(
          "se",
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
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
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
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
  }

  // Re-derive every fence's count/repos readout from the stage (issue #342).
  // Membership is never stored, so the chrome is a fold of the LIVE rects —
  // called from `renderFences`, `persistWin` and `forgetRecord`, the three
  // points every layout mutation already passes through. NOT from
  // `applyExtent`: that fires per mousemove during a drag, and `offsetLeft` on
  // a `.tiling` window returns the INTERPOLATED value mid-transition, so a
  // refresh there would both thrash and read a membership still in flight.
  function refreshFenceChrome() {
    const st = stage();
    if (!st) return;
    const els = new Map();
    for (const el of st.querySelectorAll(".fence")) els.set(el.dataset.fenceId, el);
    for (const s of fenceSummaries(readFenceRects(st), readWindowRects(st))) {
      const el = els.get(s.id);
      if (!el) continue;
      const count = el.querySelector(".fence-count");
      if (count) count.textContent = `${s.count} console${s.count === 1 ? "" : "s"}`;
      const repos = el.querySelector(".fence-repos");
      if (repos) repos.textContent = s.repos;
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

  // Upsert the DOM against `fences`. The rect is always re-applied; the NAME is
  // not written while the operator is typing in it — an in-flight GET would
  // otherwise yank the caret back to a stale value mid-word.
  function renderFences() {
    const st = stage();
    if (!st) return;
    // Index the DOM by id rather than building an attribute SELECTOR from one:
    // an id is daemon data (a hand-edited `desk.toml` can carry any string) and
    // one quote in it throws a SyntaxError out of here — which is called from
    // `restoreDesk` right before `applyExtent`, so the whole restore, the
    // landing and the settle latch would all be skipped, swallowed by the outer
    // `.catch`, on every load thereafter.
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
    }
    for (const [id, el] of nodes) {
      if (!seen.has(id)) el.remove();
    }
    refreshFenceChrome();
  }

  function createFence() {
    const ws = workspace();
    const offset = { left: ws?.scrollLeft || 0, top: ws?.scrollTop || 0 };
    const viewport = { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 };
    const slot = nextFenceSlot(
      fences.map((f) => f.rect),
      offset,
      viewport,
    );
    const spawn = fenceSpawnRect(offset, viewport, slot);
    // `nextFenceSlot` runs out of free slots eventually (and a moved fence can
    // sit anywhere, not on the grid). REFUSE then — do not nudge the new fence
    // into whatever gap is left, which is a position the operator never chose.
    if (!fenceFits(fences, { id: null, rect: spawn })) {
      const hit = fences.find((x) => rectsOverlap(spawn, x.rect || {}));
      const el = hit && fenceEl(hit.id);
      if (el) {
        clearFenceFlash();
        el.classList.add("fence-invalid");
        fenceFlash = { el, timer: setTimeout(clearFenceFlash, 600) };
      }
      return;
    }
    saveFences(
      fences.concat([
        {
          id: newFenceId(),
          name: `Fence ${slot + 1}`,
          rect: spawn,
          ts: Date.now(),
        },
      ]),
    );
    renderFences();
    applyExtent();
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
    saveFences(fences.filter((f) => f.id !== id));
    renderFences();
    applyExtent();
  }

  // ---- navigating the plane ----------------------------------------------------
  // The scroll offsets that bring `target` (a STAGE-relative rect) into the
  // viewport, as a pure function: centre it, then clamp to `[0, extent -
  // viewport]`. It ALWAYS centres — a scroll-into-view-if-needed would need a
  // visibility predicate neither caller wants. Two callers: `reveal` below (the
  // Go-to picker, issue #337) and ADR-0051 §7's fence jump, where clicking a
  // name slides the viewport to that fence.
  // ONE clamp per axis, deliberately: the final `Math.max(0, …)` is what stops
  // a viewport bigger than the extent from asking for a negative offset the DOM
  // would silently swallow. Flooring the ceiling too would make that floor
  // unfalsifiable — both spellings answer 0 for every input, so the table's
  // negative control could never red either one.
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

  // Where the viewport lands on load (issue #339), pure. The stored per-client
  // offset wins — but only while it still SHOWS work: a stored pair is honoured
  // verbatim (after the same clamp) when some window intersects the viewport
  // placed there, and otherwise degrades to the bbox landing. That intersection
  // test is the whole "restoring on a smaller screen lands on a view that shows
  // work" criterion; without it a stored 0,0 from an empty session would strand
  // the operator on a corner of an empty plane.
  // The clamp comes BEFORE the test on purpose: an offset saved on a bigger
  // screen is a legitimate view pulled into this extent, not a corrupt one.
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

  function resizeRect(dir, rect, delta, min, bounds) {
    let { left, top, width, height } = rect;
    const right = rect.left + rect.width;
    const bottom = rect.top + rect.height;
    if (dir.includes("e")) {
      width = Math.max(min.width, Math.min(rect.width + delta.dx, bounds.width - left));
    } else if (dir.includes("w")) {
      // Anchor the right edge: clamp the new left, then derive the width from it.
      left = Math.max(0, Math.min(rect.left + delta.dx, right - min.width));
      width = right - left;
    }
    if (dir.includes("s")) {
      height = Math.max(min.height, Math.min(rect.height + delta.dy, bounds.height - top));
    } else if (dir.includes("n")) {
      top = Math.max(0, Math.min(rect.top + delta.dy, bottom - min.height));
      height = bottom - top;
    }
    return { left, top, width, height };
  }

  // Wire one handle: drag it and the window's rect follows `resizeRect`. Every
  // exit path (mouseup anywhere on the document) drops BOTH listeners and
  // persists exactly once.
  function startResize(win, dir) {
    return (e) => {
      if (e.button !== 0) return; // primary button only — see makeDraggable
      focusWin(win);
      if (win.classList.contains("maximized")) return;
      const rect = {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
      // The STAGE is the bound (ADR-0051 §5) — the pure `resizeRect` is
      // untouched, only its argument changed. Captured ONCE, deliberately: a
      // live re-read feeds back on itself, because the extent this gesture
      // grows becomes the bound of its own next move and an overshoot inflates
      // the window ~one margin per mousemove instead of stopping at the edge.
      // Nothing else moves during a resize, so the capture cannot go stale.
      const st = stage();
      const bounds = { width: st.offsetWidth, height: st.offsetHeight };
      const startX = e.clientX;
      const startY = e.clientY;
      const onMove = (ev) => {
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
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        applyExtent();
        persistWin(win);
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
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

  // How many consecutive failed re-opens before a socket is given up on (the
  // daemon is likely down), and how many a never-opened would-be writer spends
  // before it settles for watching. Module scope so `reconnectDecision` — the
  // pure rule below — can be tabled without an `attachTerminal` instance.
  const MAX_FAILED_REOPENS = 10;
  const WATCH_AFTER = 3;

  // The reconnect rule, pulled out of `ws.onclose` so it can be tabled (issue
  // #334). Pure: no DOM, no socket, no timers. Returns exactly one of
  // "reconnect" / "park-as-watcher" / "give-up".
  //
  // `announced` is the daemon's eviction reason when one arrived in a data frame
  // BEFORE the close ("taken-over" / "child-exited" / "daemon-shutdown"), else
  // null. It is the only trustworthy signal of a deliberate end: the close
  // metadata is lost on this path (the browser reports 1005/wasClean=false even
  // for a served Close frame), which is why an unannounced dirty close is read
  // as a flaky link and retried.
  function reconnectDecision({
    code,
    wasClean,
    opened,
    everOpened,
    announced,
    idKnown,
    failedReopens,
  }) {
    // R1: nothing to reattach TO — a fresh launch that dropped before its first
    // frame has no id, and reconnecting would spawn a SECOND session.
    if (!idKnown) return "give-up";
    // R2/R3: the daemon said why. Taken over → the session lives on elsewhere,
    // so park and watch it; any other reason → it is gone.
    if (announced === "taken-over") return "park-as-watcher";
    if (announced != null) return "give-up";
    if (failedReopens > MAX_FAILED_REOPENS) return "give-up";
    // R5: a clean/normal close of a socket that DID open is a deliberate server
    // end even without an announcement (an older daemon, a proxy closing).
    if (opened && (wasClean || code === 1000 || code === 1001)) return "give-up";
    // R6: this window has held the session before, so a drop is a flaky link —
    // keep the existing backoff rather than degrading into a watcher.
    if (everOpened) return "reconnect";
    // R7/R8: never opened. Retry as a would-be writer a bounded number of times
    // (an F5 racing the old bridge's teardown), then settle for watching.
    if (failedReopens < WATCH_AFTER) return "reconnect";
    return "park-as-watcher";
  }

  // Attach a real xterm.js terminal into `body`, wired to a PTY over `/ws/session`.
  // `opts` is one of: {repo, agent} (a NEW agent launch), {console:true[, repo]}
  // (a NEW free-console launch — home dir when `repo` absent), or
  // {id[, takeover][, watch]} (a REATTACH to a daemon-owned session; `watch`
  // reattaches read-only). Transplanted from index.html launch(). Returns a
  // handle so the window chrome can refit, take the baton, and close it.
  function attachTerminal(body, opts) {
    const term = new Terminal({ convertEol: false });
    const fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(body);
    // GPU glyph rendering with a DOM fallback: if WebGL is unavailable (headless,
    // no GPU) or the context is lost, dispose the addon and xterm falls back to
    // DOM without dropping the session.
    try {
      const webgl = new WebglAddon.WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {}
    term.loadAddon(new WebLinksAddon.WebLinksAddon());
    fit.fit();
    // Refit whenever THIS window's body changes size (a drag-resize, a maximize).
    // Per-window, so one window's resize never disturbs another — this is the
    // only ResizeObserver left in the file, and it resizes a TERMINAL, never a
    // window rect (issue #336 deleted the one that did).
    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {}
    });
    ro.observe(body);

    let currentSessionId = opts.id ?? null;
    let leaving = false;

    // Resilience on low-quality links. A dropped socket does NOT end the session:
    // the daemon keeps the child alive across a disconnect (see session_ws's
    // teardown invariant), so an unexpected close is recovered by reconnecting and
    // reattaching to the SAME session by id. The daemon replays scrollback on
    // reattach, so we reset the terminal on a reconnecting open to repaint cleanly
    // instead of appending a duplicate of the history. Backoff is exponential with
    // jitter, capped.
    //
    // NO RECONNECT EVER CARRIES `takeover` (issue #334). Reclaiming the writer
    // slot on a timer is how two open workbenches flapped: each side's reconnect
    // evicted the other, ~1.1s per flip, indefinitely. The baton changes hands
    // only when an operator clicks `takeOver()`. `reconnectDecision` above owns
    // the choice; this function only carries it out.
    const RECONNECT_BASE = 1000;
    const RECONNECT_MAX = 15000;
    let ws = null;
    let opened = false; // has the CURRENT socket opened
    let everOpened = false; // has ANY socket of this window opened
    // True on EVERY path into the watcher role: the park at `case
    // "park-as-watcher"` below, or a caller that attaches read-only from the
    // start via the documented `{id, watch}` opts shape. The `term.onData`
    // gate below reads this flag, so a future watch-from-start caller cannot
    // bypass it by skipping the park transition.
    let watching = !!opts.watch;
    let announced = null; // the daemon's reason, when it named one before closing
    let switching = false; // an intentional close on the way to a takeover
    let firstConnect = true;
    let retryDelay = 0;
    let retryTimer = null;
    let failedReopens = 0;

    function buildUrl(o) {
      let url = WS_ORIGIN + "/ws/session?";
      if (o.id != null) {
        url += "id=" + encodeURIComponent(o.id);
        if (o.takeover) url += "&takeover=1";
        if (o.watch) url += "&watch=1";
      } else if (o.console) {
        url += "console=1";
        if (o.repo) url += "&repo=" + encodeURIComponent(o.repo);
      } else {
        url +=
          "repo=" +
          encodeURIComponent(o.repo) +
          "&agent=" +
          encodeURIComponent(o.agent);
      }
      return url;
    }

    function giveUp() {
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
        connect({ id: currentSessionId, watch: watching });
      }, wait);
    }

    function connect(connOpts) {
      opened = false;
      announced = null;
      ws = new WebSocket(buildUrl(connOpts));
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
        // Written HERE, not in `onPark`: the park reattaches immediately and the
        // reset above would wipe a line written before the socket opened.
        if (watching) {
          term.write("\r\n[watching — driven in another window]\r\n");
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
            // The id a fresh launch is assigned is only known now; let the chrome
            // record the window's geometry under it so it persists from the start.
            if (typeof opts.onSession === "function") opts.onSession(currentSessionId);
          }
          term.write(a.subarray(9));
        } else if (a[0] === TAG_COMMAND) {
          // The daemon's deliberate-end announcement, sent as DATA before the
          // Close frame because the close metadata does not survive the trip
          // (issue #334): the browser reports 1005/wasClean=false either way.
          let c = null;
          try {
            c = JSON.parse(new TextDecoder().decode(a.subarray(1)));
          } catch {}
          if (c && c.verb === "session-end") {
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
            // Park ONCE, immediately: a client that stopped receiving output
            // could not satisfy "both contexts see the session's output". A
            // watch socket that itself drops falls back to the backoff, so a
            // refused watch cannot busy-loop.
            if (!watching) {
              watching = true;
              if (typeof opts.onPark === "function") opts.onPark(announced);
              connect({ id: currentSessionId, watch: true });
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

    term.onData((d) => {
      // The daemon-side drop in `Attachment::write` (session.rs:822) stays as
      // defence in depth — this gate exists so the operator SEES the refusal
      // instead of it being silently swallowed server-side (issue #335).
      if (watching) {
        if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
        return;
      }
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(encodeTerminal(d));
    });
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
      get watching() {
        return watching;
      },
      // The ONLY place in this file that sets `takeover` — operator-initiated,
      // from the parked banner's button. `switching` makes the current socket's
      // own onclose a no-op so the park logic does not race the new attach.
      takeOver() {
        if (currentSessionId == null) return;
        switching = true;
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        if (ws) {
          // Detach EVERY handler before closing: the events land AFTER this
          // function returns, by which time `switching` is false again and `ws`
          // names the new socket, so the flag alone only covers the synchronous
          // window. `onmessage` matters as much as `onclose` — a `session-end`
          // still queued on the outgoing socket would land after `connect`
          // cleared `announced` and attach a stale reason to the NEW connection,
          // turning its next flaky-link drop into a give-up.
          ws.onclose = null;
          ws.onmessage = null;
          ws.onopen = null;
          ws.onerror = null;
          if (ws.readyState <= 1) ws.close();
        }
        watching = false;
        announced = null;
        failedReopens = 0;
        retryDelay = 0;
        switching = false;
        if (typeof opts.onResume === "function") opts.onResume();
        connect({ id: currentSessionId, takeover: true });
      },
      dispose() {
        leaving = true;
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        ro.disconnect();
        if (ws && ws.readyState <= 1) ws.close();
        term.dispose();
      },
    };
  }

  // The floating-window chrome, shared by a live console and a placeholder: the
  // rect (restored from a desk record, else cascaded), the titlebar with its
  // maximize/close controls, the body, and the eight resize handles. `desk` is a
  // desk record (or a partial carrying at least `kind`); everything the record
  // needs to be rewritten later is hung off the element.
  function buildChrome(label, repo, desk, kind) {
    const win = document.createElement("div");
    win.className = "session-window";
    win._deskId = desk?.id || newDeskId();
    win._deskRepo = repo || "~";
    win._deskAgent = label;
    win._deskKind = desk?.kind || kind;
    const rect = desk?.rect;
    if (rect) {
      win.style.left = rect.left + "px";
      win.style.top = rect.top + "px";
      win.style.width = rect.width + "px";
      win.style.height = rect.height + "px";
    } else {
      cascade = (cascade + 1) % 8;
      win.style.left = 30 + cascade * 24 + "px";
      win.style.top = 20 + cascade * 24 + "px";
      win.style.width = "min(560px, 62%)";
      win.style.height = "min(340px, 60%)";
    }

    const titlebar = document.createElement("div");
    titlebar.className = "session-titlebar";
    const title = document.createElement("span");
    title.className = "session-title";
    title.innerHTML = `<i class="bi bi-terminal"></i> ${label} · ${repo || "home"}`;
    const actions = document.createElement("span");
    actions.className = "session-actions";
    const maxBtn = document.createElement("button");
    maxBtn.className = "session-max";
    maxBtn.title = "maximize";
    maxBtn.innerHTML = '<i class="bi bi-fullscreen"></i>';
    const closeBtn = document.createElement("button");
    closeBtn.className = "session-close";
    closeBtn.title = "close";
    closeBtn.innerHTML = '<i class="bi bi-x-lg"></i>';
    actions.append(maxBtn, closeBtn);
    titlebar.append(title, actions);

    const body = document.createElement("div");
    body.className = "session-body";
    const grip = document.createElement("div");
    grip.className = "session-resize";
    win.append(titlebar, body, grip);
    // Eight interaction handles (the corner grip above is decoration only, so
    // there is exactly ONE resize code path).
    for (const dir of DIRS) {
      const h = document.createElement("div");
      h.className = `session-handle h-${dir}`;
      h.addEventListener("mousedown", startResize(win, dir));
      win.append(h);
    }
    stage().append(win);
    applyExtent();

    win.addEventListener("mousedown", () => focusWin(win));
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
    // Re-apply a persisted maximized state (the inline rect above is the box it
    // restores to).
    if (rect && desk.max) toggleMax(win, maxBtn);
    focusWin(win);
    return { win, body, maxBtn, closeBtn };
  }

  // Build the chrome and attach a live terminal into it. Shared by `open()` (a
  // new console) and the load-time restore (one window per reconciled record);
  // `termOpts` is the `attachTerminal` opts, `label`/`repo` drive the titlebar,
  // `desk` is the record this window continues (absent for a fresh launch).
  function spawnWindow(termOpts, label, repo, desk) {
    const kind = termOpts.console ? "console" : "agent";
    const { win, body, closeBtn } = buildChrome(label, repo, desk, kind);

    // Debounced nudge feedback for a keystroke typed into a parked window
    // (issue #335): repeated typing EXTENDS the pulse rather than stacking
    // timers, so `clearTimeout` always runs before a new one is scheduled.
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

    const t = attachTerminal(body, {
      ...termOpts,
      // Once the daemon assigns/echoes this window's session id, record it on the
      // desk so the layout knows which live session this window is holding.
      onSession: () => persistWin(win),
      // Parked: this window is watching a session another window drives. It KEEPS
      // its window and its output — the strip is the visible state that replaced
      // the old `confirm("session busy — take over?")` prompt, and its button is
      // the only way `takeover` is ever sent (issue #334, ADR-0051 §9).
      onPark: () => {
        if (win.querySelector(".session-parked")) return;
        const strip = document.createElement("div");
        strip.className = "session-parked";
        const text = document.createElement("span");
        text.textContent = `watching ${label} · ${repo || "home"} — driven in another window`;
        const hint = document.createElement("span");
        hint.className = "session-parked-hint";
        const btn = document.createElement("button");
        btn.className = "session-reconnect";
        btn.dataset.act = "take-over";
        btn.textContent = "take over";
        btn.addEventListener("click", (e) => {
          e.stopPropagation();
          t.takeOver();
        });
        strip.append(text, hint, btn);
        win.insertBefore(strip, body);
      },
      // A watcher's keystroke never reaches the child (gated in `attachTerminal`);
      // this pulses the strip so the refusal is SEEN instead of silently swallowed
      // (issue #335, AC4).
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
      // A session that ENDED is not driven anywhere, so the parked strip's "take
      // over" button would only spin failed attaches at a dead id.
      onEnded: () => {
        clearNudge();
        win.querySelector(".session-parked")?.remove();
      },
    });
    win._term = t;
    // The id this window is attaching to, known before the terminal reports one.
    if (termOpts.id != null) win._wantsSession = termOpts.id;

    closeBtn.onclick = () => {
      const id = t.sessionId;
      const finish = () => {
        // A window closed mid-pulse must not leave `nudgeTimer` pending against
        // DOM nodes this call is about to remove.
        clearNudge();
        t.dispose();
        forgetRecord(win._deskId);
        win.remove();
        wins.delete(win);
        applyExtent();
        WB.emit("console-close", { repo: repo || null, agent: label });
        changed();
      };
      // End the daemon-owned session first (existing close endpoint), then drop
      // the window — mirrors index.html's closeBtn.
      // A WATCHER closes only its own window: it does not hold the baton, and
      // `/api/sessions/close` tree-kills the child another operator is driving.
      // Before #334 no window could exist for a session it did not own, so this
      // guard arrived with the watcher role.
      if (id != null && !t.watching) {
        fetch(`/api/sessions/close?id=${id}`, { method: "POST" }).then(finish, finish);
      } else {
        finish();
      }
    };

    wins.add(win);
    changed();
    persistWin(win);
    return win;
  }

  // The window's current placement as a desk record — used to carry a window's
  // identity and box across a rebuild (takeover, placeholder → live console).
  function deskOf(win) {
    return {
      id: win._deskId,
      repo: win._deskRepo,
      agent: win._deskAgent,
      kind: win._deskKind,
      rect: restoreRect(win),
      max: win.classList.contains("maximized"),
    };
  }

  // An agent console the daemon no longer runs: the same chrome and the same box,
  // but no session — one click relaunches it into this very record. Loading the
  // page must never spawn a vendor CLI on its own.
  function spawnPlaceholder(record) {
    const { win, body, closeBtn } = buildChrome(record.agent, record.repo, record, record.kind);
    win.classList.add("placeholder");

    const note = document.createElement("div");
    note.className = "session-offline";
    const text = document.createElement("p");
    text.textContent = "agent console — not running";
    const btn = document.createElement("button");
    btn.className = "session-reconnect";
    btn.textContent = "reconnect";
    note.append(text, btn);
    body.append(note);

    const drop = () => {
      win.remove();
      wins.delete(win);
      applyExtent();
      changed();
    };
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const carry = deskOf(win);
      drop();
      // The same launch path the agent menu uses, reusing this record's id, rect
      // and maximized state, so the relaunched console lands where it stood.
      spawnWindow({ repo: record.repo, agent: record.agent }, record.agent, record.repo, carry);
    });
    closeBtn.onclick = () => {
      forgetRecord(win._deskId);
      drop();
      WB.emit("console-close", { repo: record.repo || null, agent: record.agent });
    };

    wins.add(win);
    changed();
    persistWin(win);
    return win;
  }

  // `agent` names an adapter (claude/codex/opencode); when `plain` is set there
  // is no agent — a normal shell in the repo dir, labelled "console".
  function open({ repo, agent, plain }) {
    const label = agent || "console";
    spawnWindow(plain ? { console: true, repo } : { repo, agent }, label, repo);
    WB.emit("console-open", { repo: repo || null, agent: agent || null, plain: !!plain });
  }

  // Reach an ALREADY LIVE session by id: focus the window already holding it,
  // else attach a window to it. INVARIANT — no path here composes a
  // `?repo=&agent=` launch: an unknown or busy id stays on the attach path and,
  // when the session is busy, parks as a watcher of the SAME id (issue #334
  // retired the window-rebuilding takeover this comment used to name), so
  // "reach" can never become a second session (issue #304).
  function reach({ id, agent, repo }) {
    for (const win of wins) {
      // `_term.sessionId` lands only on the first terminal frame, and the daemon
      // skips the replay frame for a session that has printed nothing — so a
      // brand-new console's window still reads `null` here. `_wantsSession` is
      // recorded at spawn time, without which `reach` would miss its own window
      // and ask the operator to take over the console they are looking at.
      if (win._term?.sessionId === id || win._wantsSession === id) {
        // Reveal, not merely focus: reaching a live session the operator cannot
        // see was the same defect the Go-to picker exists to fix.
        reveal(win._deskId) || focusWin(win);
        return win;
      }
    }
    return spawnWindow({ id }, agent || "console", repo);
  }

  // Restore the desk: reconcile the saved layout against the daemon's live
  // sessions and dispatch one window per verdict. A REJECTED fetch leaves the
  // desk untouched — the static demo (and a daemon that is merely unreachable)
  // must not relaunch anything or show phantom placeholders.
  function restoreDesk() {
    // The static demo restores nothing, but its plane is still pannable — so it
    // must still LAND, or the latch never arms and the offset is never stored
    // for the life of the page.
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
      .then(([, sessions]) => {
        for (const { record, session, action } of reconcileDesk({
          layout: loadDesk(),
          sessions,
        })) {
          if (action === "attach") {
            spawnWindow({ id: session.id }, session.agent || "console", session.repo, record);
          } else if (action === "relaunch") {
            // The daemon labels a repo-less console "~"; passing that back as a
            // slug would hit `unknown repo`, so it relaunches with no repo at all.
            const repo = record.repo === "~" ? undefined : record.repo;
            spawnWindow({ console: true, repo }, record.agent, record.repo, record);
          } else if (action === "placeholder") {
            spawnPlaceholder(record);
          } else {
            // `adopt`: a cascaded window with a fresh record, keeping the live
            // session's own kind so the desk relaunches it correctly next time.
            spawnWindow({ id: session.id }, session.agent || "console", session.repo, {
              kind: session.kind,
            });
          }
        }
        // A desk saved on a larger screen keeps its rects verbatim (issue #336):
        // the STAGE grows to hold them and the viewport scrolls. Sizing it here
        // is what gives the restored windows their scroll room — inserting a
        // window is not a resize, so nothing else would fire. The fences must be
        // on the stage BEFORE that fold, or the plane will not have grown to
        // hold one that sits past the last window.
        renderFences();
        applyExtent();
        deskSettled = true;
        applyLanding();
      })
      .catch(() => {
        // A refused/unreachable desk restores nothing, but the operator can
        // still open consoles by hand — same reasoning as the demo above.
        deskSettled = true;
        applyLanding();
      });
  }
  // ---- the plane's own gestures ------------------------------------------------
  // Pan by dragging the BARE FLOOR. Deliberately calls neither `applyExtent` nor
  // `persistWin` nor `focusWin`: "panning moves the view, not the rects" is true
  // by construction here, not by a test that happens to pass.
  function onFloorDown(e) {
    // Primary button only — a right/middle press is followed by a `contextmenu`
    // and no `mouseup`, which would strand `onMove` on the document.
    if (e.button !== 0) return;
    const ws = workspace();
    const st = stage();
    // Element IDENTITY is the whole floor-vs-window hit test: a press anywhere
    // inside a console — titlebar, body, resize handle — targets that window,
    // never the stage. A fence is also a stage child, but it is
    // `pointer-events: none`, so a press over one still targets the stage and
    // panning survives inside a fence with no hit test here (issue #340).
    if (!ws || !st || e.target !== st) return;
    const startX = e.clientX;
    const startY = e.clientY;
    const startLeft = ws.scrollLeft;
    const startTop = ws.scrollTop;
    st.classList.add("panning");
    const onMove = (ev) => {
      // A swallowed mouseup (native context menu, alt-tab with the button
      // held) would otherwise leave a sticky pan tracking a button-less
      // pointer, with the `grabbing` cursor stuck on.
      if (ev.buttons === 0) {
        onUp();
        return;
      }
      ws.scrollLeft = startLeft - (ev.clientX - startX);
      ws.scrollTop = startTop - (ev.clientY - startY);
    };
    // INVARIANT: every exit path drops EVERY listener and the class — the
    // mouseup, a move that reveals the button is already up, and the focus loss
    // that means neither will arrive.
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

  // The wheel. The VERTICAL axis is native `overflow:auto` — no code. This adds
  // only the horizontal reach, and only where the platform does not already
  // provide it.
  function onWheel(e) {
    // The terminal owns its own wheel. The scrollback is reached by CSS
    // (`overscroll-behavior: contain`), never by `preventDefault` — cancelling a
    // wheel cancels the default scroll for the whole chain, the terminal's own
    // included, which would fix the hijack by breaking the feature.
    if (e.target?.closest?.(".session-window")) return;
    // A platform that converts shift-wheel itself delivers `deltaX`; the guard
    // makes this handler inert there instead of double-scrolling.
    if (!(e.shiftKey && e.deltaY !== 0 && e.deltaX === 0)) return;
    const ws = workspace();
    if (!ws) return;
    // `deltaY` is only pixels when `deltaMode` says so. Firefox reports LINE
    // (±3 per notch) and does not convert shift-wheel itself, so taking the raw
    // number would pan 3px a notch while `preventDefault` suppresses the
    // platform's own scroll — strictly worse than the default it replaces.
    const px =
      e.deltaMode === 1
        ? e.deltaY * WHEEL_LINE
        : e.deltaMode === 2
          ? e.deltaY * ws.clientHeight
          : e.deltaY;
    ws.scrollLeft += px;
    e.preventDefault();
  }

  // `passive: false` or the `preventDefault` above is a no-op — Chrome treats a
  // wheel listener on a scroll container as passive by default.
  function wireStage() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    st.addEventListener("mousedown", onFloorDown);
    ws.addEventListener("wheel", onWheel, { passive: false });
    // The one registration that makes the maximize pin a derived fact: a
    // gesture, the wheel, the scrollbar and `reveal`'s programmatic offset write
    // all end in a `scroll` on the viewport itself.
    ws.addEventListener("scroll", syncMaxPin);
    // A SECOND listener, deliberately: the maximize pin is derived state that
    // must stay exact, the view offset is debounced storage — one handler doing
    // both would have to pick one of those two rhythms.
    ws.addEventListener("scroll", saveOffset);
  }

  // The gestures are wired in the static demo too, where `restoreDesk` returns
  // early: an empty plane still pans.
  function boot() {
    wireStage();
    restoreDesk();
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }

  // Refit every open console. Called when the Consoles tab returns to view: a
  // terminal opened/reattached while the tab was display:none measured 0×0.
  function refitAll(attempt) {
    // Alpine's `$nextTick` fires BEFORE `x-show` applies the flip: measured on
    // the first call after switching back, `.consoles-tab` is still
    // `display:none` and everything under it — the viewport AND every window —
    // measures 0. Refitting THERE is worse than not refitting at all:
    // `applyExtent` reads a 0×0 union and collapses the stage to the bare
    // margin (measured 3200×2080 -> 200×200, never recomputed after), the
    // footer pill publishes that as the extent, `fit.fit()` sizes every
    // terminal to nothing, and `applyLanding` finds no viewport to clamp
    // against. Wait for a frame that can measure; give up rather than refit
    // blind if the tab never comes into view.
    const ws0 = workspace();
    if (ws0 && (!ws0.clientWidth || !ws0.clientHeight)) {
      const n = attempt || 0;
      if (n < 10) requestAnimationFrame(() => refitAll(n + 1));
      return;
    }
    // A desk restored while this tab was hidden measured a 0×0 viewport, so the
    // stage extent computed above was the bare bbox — this is the first moment
    // the viewport leg of the union can be measured for real.
    // The extent dispatch below is an EDGE, so a `workbench:stage-extent` that
    // fired before Alpine mounted (or never fired at all, in the static demo)
    // would leave the footer pill reading `stage 0 × 0` for good. Forcing the
    // edge here re-publishes it every time the tab comes back into view.
    lastExtent = { width: -1, height: -1 };
    applyExtent();
    for (const win of wins) {
      try {
        win._term?.fit.fit();
      } catch {}
    }
    // LAST, after `applyExtent`: returning to this tab is the other moment the
    // stored offset must be re-applied, because `x-show` threw it away.
    applyLanding();
  }

  // Tile ONE fence's members into its own rect (issue #342). On a plane a
  // global "tile everything" has no target — the stage is larger than the view
  // — so the act moved into the fence, which is exactly the region that names
  // the windows it should rearrange. Windows animate to place via the same CSS
  // transition the global Arrange used.
  //
  // The grid is inset by the fence's OWN chrome: the head band at the top and
  // the SE `.fence-grip` at the bottom sit at `z-index: 1`, BELOW every window,
  // so a member parked on either makes the fence's controls unhittable — the
  // arrange button would be usable exactly once and the fence unresizable. The
  // members are still strictly inside the fence rect.
  const FENCE_GRIP = 14;
  // `.session-window`'s CSS floor (`styles.css`, pinned by
  // `shell_arranges_into_the_fence`). It OUTRANKS an inline width, so a cell
  // smaller than it renders WIDER than the tile and the member escapes the
  // fence — containment is arithmetic in `tileIntoRect` but CSS in the box the
  // tile lands in. Measured: a 200x180 fence with one member tiles a 176x116
  // cell that renders 240x150, 52 px past the fence's right edge.
  const WIN_MIN_W = 240;
  const WIN_MIN_H = 150;

  function arrangeFence(id) {
    const st = stage();
    const el = fenceEl(id);
    if (!st || !el) return;
    const rect = restoreRect(el);
    const all = [...st.querySelectorAll(".session-window")].map((w) => ({
      el: w,
      id: w._deskId,
      rect: restoreRect(w),
    }));
    // The FULL fence list with this fence's LIVE rect substituted: the fold's
    // `break` is what decides an overlapping pair, and a singleton list bypasses
    // it — a window would be tiled here and reported under the other fence.
    const live = fences.map((x) => (x.id === id ? { id: x.id, rect } : x));
    const ids = new Set(fenceMembership(live, all)[id] || []);
    // A maximized console is NOT tiled. `.maximized` overrides all four offsets
    // with `!important`, so a tile rect written onto it is invisible on screen
    // while it silently REPLACES the pre-maximize rect the restore button and a
    // reload read back. Filtered before the grid so it stays hole-free (#338).
    const members = all
      .filter((m) => ids.has(m.id) && !m.el.classList.contains("maximized"))
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
      // Relaxed to the cell for exactly the tiles below the floor, and CLEARED
      // otherwise so a window tiled small once does not keep a shrunken floor
      // for every later manual resize.
      win.style.minWidth = t.width < WIN_MIN_W ? t.width + "px" : "";
      win.style.minHeight = t.height < WIN_MIN_H ? t.height + "px" : "";
      focusWin(win);
      setTimeout(() => win.classList.remove("tiling"), 260);
    });
    // A maximized console is not tiled, but it must not be BURIED by the tiles
    // either: `maxlock` (`overflow:hidden`) leaves no way to scroll away from a
    // full bleed whose titlebar is covered. Raising it last keeps its restore
    // button reachable by a real mouse.
    for (const win of wins) {
      if (win.classList.contains("maximized")) focusWin(win);
    }
    // AFTER the 0.24s tiling transition: writing the rects above is what STARTS
    // it, so an immediate fold would measure the pre-arrange boxes — the plane
    // would be sized to a layout that no longer exists and the terminals refit
    // to the box they are still leaving.
    //
    // The PERSIST is in here for the same reason, and it is not cosmetic:
    // `persistWin` snapshots `offsetLeft`/`offsetWidth`, which at the moment the
    // tile rects are written still read the PRE-arrange box (measured: the desk
    // kept 60,100,260,160 for a member tiled to 52,77,283,193.5, and a reload
    // replayed the old layout). Tiling is a layout act like any drag — it must
    // record where the member LANDED.
    setTimeout(() => {
      for (const win of members) {
        try {
          win._term?.fit.fit();
        } catch {}
        // The deferral opens a window this act did not have when it persisted
        // synchronously: the Consoles tab is Alpine `x-show`, and a hidden
        // window measures 0x0 at 0,0 — which `persistWin` would store as the
        // member's rect. Skip it; the inline rect written above survives, and
        // the next layout act persists it.
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

  // Re-read the daemon's desk and restore it. The `Session` policy answers the
  // pre-login `/api/desk` with 401, so the boot load found nothing; this is the
  // client half of that guard, called from `rehydrateAfterAuth` (issue #327).
  function afterLogin() {
    return reloadDesk().then(() => {
      if (deskLoaded && wins.size === 0) {
        restoreDesk();
        return;
      }
      // The desk landed but the windows are already up, so `restoreDesk` is not
      // called and nothing else would put the just-loaded fences on the stage.
      // Gated on the permit: with a REFUSED load `fences` is whatever this page
      // drew, and rendering it here would strip nothing but prove nothing —
      // worse, it presents a partial desk as the restored one.
      if (!deskLoaded) return;
      renderFences();
      applyExtent();
    });
  }

  return {
    open,
    arrangeFence,
    count,
    refitAll,
    resizeRect,
    stageExtent,
    bringIntoView,
    viewLanding,
    panNudge,
    reconnectDecision,
    reconcileDesk,
    pruneDesk,
    reach,
    list,
    reveal,
    afterLogin,
    fenceSpawnRect,
    nextFenceSlot,
    rectHolds,
    fenceMembership,
    fenceSummaries,
    fenceFits,
    fenceMoveDelta,
    tileIntoRect,
    fenceRepos,
    createFence,
    renameFence,
    removeFence,
  };
})();
