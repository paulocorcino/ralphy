/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Consoles tab)

   The canvas is a workspace where consoles live as draggable, resizable windows
   over the dotted floor. This module contributes the window chrome (workspace-
   relative drag/clampAll/tiling); the terminal body is the REAL thing, a live
   xterm.js attached to a PTY over the daemon's `/ws/session` WebSocket —
   transplanted verbatim from crates/ralphy-daemon/assets/ui/index.html
   (index.html contributes the truth, this module contributes the chrome).

   Opening/closing a console spawns/closes a daemon-owned session; on page load
   the live sessions are re-opened as windows so a reload reattaches with
   scrollback.
--------------------------------------------------------------------------- */
window.WBConsole = (function () {
  const workspace = () => document.getElementById("workspace");
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
  // Resolves once the daemon's desk has landed in `desk`; `restoreDesk` awaits
  // it before reconciling. Never rejects — an unreachable daemon means an empty
  // desk, the same verdict a fresh browser used to reach.
  const deskReady = window.WBMode?.isDaemon()
    ? fetch("/api/desk")
        .then((r) => (r.ok ? r.json() : []))
        .then((v) => {
          desk = Array.isArray(v) ? v : [];
        })
        .catch(() => {})
    : Promise.resolve();

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
    desk = pruneDesk(records, DESK_MAX, live);
    scheduleDeskFlush();
  }
  // The upload, debounced and fire-and-forget. INVARIANT: no drag, resize, close
  // or `persistWin` path may await or throw on this — a refused PUT costs a stale
  // position and the next mutation supersedes it (last write wins, no ETag).
  let deskFlush = null;
  function scheduleDeskFlush() {
    if (!window.WBMode?.isDaemon()) return;
    clearTimeout(deskFlush);
    deskFlush = setTimeout(() => {
      try {
        fetch("/api/desk", {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(desk),
        }).catch(() => {});
      } catch {}
    }, 250);
  }
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

  // Toggle a console between its floating rect and full-workspace bleed. The
  // pre-maximize rect stays in the inline styles (drag/resize are inert while
  // maximized), so restoring is just dropping the class.
  function toggleMax(win, btn) {
    const maxed = win.classList.toggle("maximized");
    btn.title = maxed ? "restore" : "maximize";
    btn.innerHTML = maxed
      ? '<i class="bi bi-fullscreen-exit"></i>'
      : '<i class="bi bi-fullscreen"></i>';
    focusWin(win);
    try {
      win._term?.fit.fit();
    } catch {}
    persistWin(win);
  }

  // Keep every window fully inside the workspace box. When a chrome panel toggles
  // (Projects hidden / Runs opened) the canvas — and thus the workspace — resizes;
  // without this a window wider/further-right than the new box is silently clipped
  // by the canvas `overflow:hidden`. Clamping resizes+repositions it to fit, so the
  // console reflows for *both* panels instead of only sliding with the sidebar.
  function clampAll() {
    const ws = workspace();
    if (!ws) return;
    const W = ws.clientWidth;
    const H = ws.clientHeight;
    if (!W || !H) return;
    for (const win of wins) {
      // A maximized window is pinned to the full workspace by CSS; leave its
      // stored restore-rect untouched so it re-inflates correctly on restore.
      if (win.classList.contains("maximized")) continue;
      const w = Math.min(win.offsetWidth, W);
      const h = Math.min(win.offsetHeight, H);
      const left = Math.min(Math.max(0, win.offsetLeft), Math.max(0, W - w));
      const top = Math.min(Math.max(0, win.offsetTop), Math.max(0, H - h));
      win.style.width = w + "px";
      win.style.height = h + "px";
      win.style.left = left + "px";
      win.style.top = top + "px";
    }
  }

  // Reflow on every workspace resize (grid transition fires this continuously).
  const _ro = new ResizeObserver(() => clampAll());
  const observeWorkspace = () => {
    const ws = workspace();
    if (ws) _ro.observe(ws);
  };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", observeWorkspace);
  } else {
    observeWorkspace();
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

  // Drag by the titlebar, clamped to the workspace box (control buttons still
  // click). Coordinates are relative to the workspace (its offsetParent).
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
      const ws = workspace().getBoundingClientRect();
      const rect = win.getBoundingClientRect();
      const offX = e.clientX - rect.left;
      const offY = e.clientY - rect.top;
      const onMove = (ev) => {
        const x = ev.clientX - ws.left - offX;
        const y = ev.clientY - ws.top - offY;
        win.style.left = Math.max(0, Math.min(x, ws.width - rect.width)) + "px";
        win.style.top = Math.max(0, Math.min(y, ws.height - rect.height)) + "px";
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
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
  // corners), the workspace-relative start `rect`, the pointer `delta`, the
  // minimum size and the workspace `bounds` yield a new rect. East/south move the
  // far edge; west/north move `left`/`top` and derive the size, so the OPPOSITE
  // edge stays put and the window does not slide under the cursor.
  const RESIZE_MIN = { width: 240, height: 150 }; // matches .session-window's CSS minimums
  const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

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
      const ws = workspace();
      const rect = {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
      const bounds = { width: ws.clientWidth, height: ws.clientHeight };
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

  // Attach a real xterm.js terminal into `body`, wired to a PTY over `/ws/session`.
  // `opts` is one of: {repo, agent} (a NEW agent launch), {console:true[, repo]}
  // (a NEW free-console launch — home dir when `repo` absent), or {id[, takeover]}
  // (a REATTACH to a daemon-owned session). Transplanted from index.html launch().
  // Returns a handle so the window chrome can refit and close it.
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
    // Refit whenever THIS window's body changes size (drag-resize, clampAll, a
    // panel toggle). Per-window, so one window's resize never disturbs another.
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
    // reattaching to the SAME session by id (with takeover=1 to reclaim the writer
    // slot the orphaned bridge still holds). The daemon replays scrollback on
    // reattach, so we reset the terminal on a reconnecting open to repaint cleanly
    // instead of appending a duplicate of the history. Backoff is exponential with
    // jitter, capped; we give up only after a bounded run of failed re-opens (e.g.
    // the daemon is actually down) or on a CLEAN server close (session genuinely
    // ended: child exited, taken over, or daemon shutdown).
    const RECONNECT_BASE = 1000;
    const RECONNECT_MAX = 15000;
    const MAX_FAILED_REOPENS = 10;
    let ws = null;
    let opened = false; // has the CURRENT socket opened
    let firstConnect = true;
    let retryDelay = 0;
    let retryTimer = null;
    let failedReopens = 0;

    function buildUrl(o) {
      let url = WS_ORIGIN + "/ws/session?";
      if (o.id != null) {
        url += "id=" + encodeURIComponent(o.id);
        if (o.takeover) url += "&takeover=1";
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
    }

    function scheduleReconnect() {
      retryDelay = Math.min(
        retryDelay ? retryDelay * 2 : RECONNECT_BASE,
        RECONNECT_MAX,
      );
      const wait = retryDelay + Math.random() * 0.3 * retryDelay; // jitter
      retryTimer = setTimeout(() => {
        retryTimer = null;
        connect({ id: currentSessionId, takeover: true });
      }, wait);
    }

    function connect(connOpts) {
      opened = false;
      ws = new WebSocket(buildUrl(connOpts));
      ws.binaryType = "arraybuffer";
      ws.onopen = () => {
        opened = true;
        retryDelay = 0;
        failedReopens = 0;
        // A reconnect reattaches and the daemon replays the whole backlog; clear
        // what's on screen first so the replay repaints instead of duplicating.
        if (!firstConnect) term.reset();
        firstConnect = false;
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
        }
      };
      // Swallow the error event; onclose drives recovery in every case.
      ws.onerror = () => {};
      ws.onclose = (event) => {
        if (leaving) return;
        // A reattach that closes WITHOUT ever opening is the server refusing a
        // busy session (a single writer is attached). Offer an explicit takeover,
        // once — this only applies to the initial, non-takeover attach.
        if (
          connOpts.id != null &&
          !opened &&
          !connOpts.takeover &&
          typeof opts.onRefused === "function"
        ) {
          if (confirm("session busy — take over?")) {
            leaving = true;
            ro.disconnect();
            term.dispose();
            opts.onRefused();
            return;
          }
          giveUp();
          return;
        }
        // A CLEAN close is a deliberate server-side end (child exited, taken over,
        // or daemon shutdown): the session is gone, do not reconnect.
        if (event && event.wasClean) {
          giveUp();
          return;
        }
        // Can't resume a session we never learned the id of (a fresh launch that
        // dropped before its first frame).
        if (currentSessionId == null) {
          giveUp();
          return;
        }
        // Abnormal drop → treat as a flaky link and reconnect, but stop if we
        // can't re-open after a bounded run of tries (the daemon is likely down).
        if (!opened) failedReopens += 1;
        if (failedReopens > MAX_FAILED_REOPENS) {
          giveUp();
          return;
        }
        if (retryDelay === 0) {
          term.write("\r\n[connection lost — reconnecting…]\r\n");
        }
        scheduleReconnect();
      };
    }

    term.onData((d) => {
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
    workspace().append(win);

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

    const t = attachTerminal(body, {
      ...termOpts,
      // Once the daemon assigns/echoes this window's session id, record it on the
      // desk so the layout knows which live session this window is holding.
      onSession: () => persistWin(win),
      // Busy-reattach → tear THIS window down and relaunch as a takeover, so no
      // dead empty window lingers. The desk record travels with it.
      onRefused: () => {
        const carry = deskOf(win);
        t.dispose();
        win.remove();
        wins.delete(win);
        changed();
        spawnWindow({ id: termOpts.id, takeover: true }, label, repo, carry);
      },
    });
    win._term = t;
    // The id this window is attaching to, known before the terminal reports one.
    if (termOpts.id != null) win._wantsSession = termOpts.id;

    closeBtn.onclick = () => {
      const id = t.sessionId;
      const finish = () => {
        t.dispose();
        forgetRecord(win._deskId);
        win.remove();
        wins.delete(win);
        WB.emit("console-close", { repo: repo || null, agent: label });
        changed();
      };
      // End the daemon-owned session first (existing close endpoint), then drop
      // the window — mirrors index.html's closeBtn.
      if (id != null) {
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
  // `?repo=&agent=` launch: an unknown or busy id stays on the attach-refusal
  // path (`onRefused` → takeover by the SAME id), so "reach" can never become a
  // second session (issue #304).
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
    for (const win of wins) {
      try {
        win._term?.fit.fit();
      } catch {}
    }
  }

  // Tile every open console into a grid that fills the workspace — the "heavy
  // lifting" button. Windows animate to place via a CSS transition.
  function arrange() {
    const ws = workspace();
    const r = ws.getBoundingClientRect();
    const list = [...wins];
    const n = list.length;
    if (!n) return;
    const cols = Math.ceil(Math.sqrt(n));
    const rows = Math.ceil(n / cols);
    const gap = 10;
    const pad = 12;
    const cw = (r.width - pad * 2 - gap * (cols - 1)) / cols;
    const ch = (r.height - pad * 2 - gap * (rows - 1)) / rows;
    list.forEach((win, i) => {
      const c = i % cols;
      const ro = Math.floor(i / cols);
      win.classList.add("tiling");
      win.style.left = pad + c * (cw + gap) + "px";
      win.style.top = pad + ro * (ch + gap) + "px";
      win.style.width = cw + "px";
      win.style.height = ch + "px";
      focusWin(win);
      // Tiling is a layout act like any drag: record it, or a reload would replay
      // the pre-Arrange rects and the desk would silently disagree with the screen.
      persistWin(win);
      setTimeout(() => win.classList.remove("tiling"), 260);
    });
  }

  function count() {
    return wins.size;
  }

  return { open, arrange, count, refitAll, resizeRect, reconcileDesk, pruneDesk, reach };
})();
