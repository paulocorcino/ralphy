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

  function ingestDesk(records) {
    const fetched = Array.isArray(records) ? records : [];
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
    const body = JSON.stringify(desk);
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
    if (!deskLoaded || !deskFlush) return;
    clearTimeout(deskFlush);
    deskFlush = null;
    try {
      fetch("/api/desk", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(desk),
        keepalive: true,
      }).catch(() => {});
    } catch {}
  });
  function newDeskId() {
    // `crypto.randomUUID` is undefined in a non-secure context and the daemon can
    // bind a plain-http LAN address (ADR-0032), so build the id by hand.
    return "w-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 10);
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
  }
  function forgetRecord(deskId) {
    if (!deskId) return;
    saveDesk(loadDesk().filter((r) => r.id !== deskId));
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
  // viewport's scroll offsets and `maxlock` freezes them, so the pin cannot
  // desync without a scroll handler. The offsets are re-asserted after the class
  // flip because `maxlock` (`overflow:hidden`) drops the scrollbars, which can
  // clamp `scrollLeft`/`scrollTop` to 0 on the way.
  function toggleMax(win, btn) {
    const ws = workspace();
    const offsets = ws ? { left: ws.scrollLeft, top: ws.scrollTop } : null;
    const maxed = win.classList.toggle("maximized");
    if (ws) {
      if (maxed) {
        win.style.setProperty("--max-left", offsets.left + "px");
        win.style.setProperty("--max-top", offsets.top + "px");
        ws.classList.add("maxlock");
        ws.scrollLeft = offsets.left;
        ws.scrollTop = offsets.top;
      } else {
        win.style.removeProperty("--max-left");
        win.style.removeProperty("--max-top");
        // Only the LAST window to leave maximized unlocks the scroll.
        if (![...wins].some((w) => w.classList.contains("maximized"))) {
          ws.classList.remove("maxlock");
        }
        ws.scrollLeft = offsets.left;
        ws.scrollTop = offsets.top;
      }
    }
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
  function applyExtent(opts) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // Read the DOM, not `wins`: a window is on the stage from the moment
    // `buildChrome` appends it (before `spawnWindow` registers it) and gone the
    // moment it is removed, so this can never count a phantom or miss a new one.
    const rects = [...st.querySelectorAll(".session-window")].map(restoreRect);
    const ext = stageExtent(
      rects,
      { width: ws.clientWidth, height: ws.clientHeight },
      STAGE_MARGIN,
    );
    const width = opts?.grow ? Math.max(ext.width, st.offsetWidth) : ext.width;
    const height = opts?.grow ? Math.max(ext.height, st.offsetHeight) : ext.height;
    st.style.width = width + "px";
    st.style.height = height + "px";
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
      const origin = stage().getBoundingClientRect();
      const rect = win.getBoundingClientRect();
      const offX = e.clientX - rect.left;
      const offY = e.clientY - rect.top;
      const onMove = (ev) => {
        // Read the stage LIVE: `applyExtent({grow:true})` below widens it as the
        // window nears the far edge, so the next move has room to keep going.
        const st = stage();
        const x = ev.clientX - origin.left - offX;
        const y = ev.clientY - origin.top - offY;
        win.style.left = Math.max(0, Math.min(x, st.offsetWidth - rect.width)) + "px";
        win.style.top = Math.max(0, Math.min(y, st.offsetHeight - rect.height)) + "px";
        applyExtent({ grow: true });
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
        focusWin(win);
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
    if (!window.WBMode?.isDaemon()) return;
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
        // window is not a resize, so nothing else would fire.
        applyExtent();
      })
      .catch(() => {});
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", restoreDesk);
  } else {
    restoreDesk();
  }

  // Refit every open console. Called when the Consoles tab returns to view: a
  // terminal opened/reattached while the tab was display:none measured 0×0.
  function refitAll() {
    // A desk restored while this tab was hidden measured a 0×0 viewport, so the
    // stage extent computed above was the bare bbox — this is the first moment
    // the viewport leg of the union can be measured for real.
    applyExtent();
    for (const win of wins) {
      try {
        win._term?.fit.fit();
      } catch {}
    }
  }

  // Tile every open console into a grid that fills the VISIBLE region — the
  // "heavy lifting" button. On a plane, "tile everything" only means anything
  // within the frame the operator is looking at, so the grid is laid out at the
  // viewport's current scroll offsets. Windows animate to place via a CSS
  // transition.
  function arrange() {
    const ws = workspace();
    const list = [...wins];
    const n = list.length;
    if (!n) return;
    const originX = ws.scrollLeft;
    const originY = ws.scrollTop;
    const cols = Math.ceil(Math.sqrt(n));
    const rows = Math.ceil(n / cols);
    const gap = 10;
    const pad = 12;
    const cw = (ws.clientWidth - pad * 2 - gap * (cols - 1)) / cols;
    const ch = (ws.clientHeight - pad * 2 - gap * (rows - 1)) / rows;
    list.forEach((win, i) => {
      const c = i % cols;
      const ro = Math.floor(i / cols);
      win.classList.add("tiling");
      win.style.left = originX + pad + c * (cw + gap) + "px";
      win.style.top = originY + pad + ro * (ch + gap) + "px";
      win.style.width = cw + "px";
      win.style.height = ch + "px";
      focusWin(win);
      // Tiling is a layout act like any drag: record it, or a reload would replay
      // the pre-Arrange rects and the desk would silently disagree with the screen.
      persistWin(win);
      setTimeout(() => win.classList.remove("tiling"), 260);
    });
    applyExtent();
  }

  function count() {
    return wins.size;
  }

  // Re-read the daemon's desk and restore it. The `Session` policy answers the
  // pre-login `/api/desk` with 401, so the boot load found nothing; this is the
  // client half of that guard, called from `rehydrateAfterAuth` (issue #327).
  function afterLogin() {
    return reloadDesk().then(() => {
      if (deskLoaded && wins.size === 0) restoreDesk();
    });
  }

  return {
    open,
    arrange,
    count,
    refitAll,
    resizeRect,
    stageExtent,
    reconnectDecision,
    reconcileDesk,
    pruneDesk,
    reach,
    afterLogin,
  };
})();
