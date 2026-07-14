/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Agents tab)

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
  let z = 60;
  let cascade = 0;

  function changed() {
    document.dispatchEvent(new CustomEvent("workbench:consoles-changed", { detail: { count: wins.size } }));
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
      focusWin(win);
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
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
      e.preventDefault();
    });
  }

  function makeResizable(win, handle) {
    handle.addEventListener("mousedown", (e) => {
      focusWin(win);
      const rect = win.getBoundingClientRect();
      const startX = e.clientX;
      const startY = e.clientY;
      const startW = rect.width;
      const startH = rect.height;
      const onMove = (ev) => {
        win.style.width = Math.max(240, startW + ev.clientX - startX) + "px";
        win.style.height = Math.max(150, startH + ev.clientY - startY) + "px";
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
      e.preventDefault();
    });
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

    let url = WS_ORIGIN + "/ws/session?";
    if (opts.id != null) {
      url += "id=" + encodeURIComponent(opts.id);
      if (opts.takeover) url += "&takeover=1";
    } else if (opts.console) {
      url += "console=1";
      if (opts.repo) url += "&repo=" + encodeURIComponent(opts.repo);
    } else {
      url +=
        "repo=" +
        encodeURIComponent(opts.repo) +
        "&agent=" +
        encodeURIComponent(opts.agent);
    }
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    let opened = false;
    ws.onopen = () => {
      opened = true;
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
        }
        term.write(a.subarray(9));
      }
    };
    term.onData((d) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(encodeTerminal(d));
    });
    term.onResize(({ rows, cols }) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(encodeResize(rows, cols));
    });
    ws.onclose = () => {
      if (leaving) return;
      // A reattach that closes WITHOUT ever opening is the server refusing a busy
      // session (a single writer is attached). Offer an explicit takeover, once.
      if (
        opts.id != null &&
        !opened &&
        !opts.takeover &&
        typeof opts.onRefused === "function"
      ) {
        if (confirm("session busy — take over?")) {
          leaving = true;
          ro.disconnect();
          term.dispose();
          opts.onRefused();
          return;
        }
      }
      // Session ended server-side (or takeover declined): stop observing so a
      // dead-ws terminal doesn't keep firing fit() until the window is closed.
      ro.disconnect();
      term.write("\r\n[session closed]\r\n");
    };

    return {
      term,
      fit,
      ws,
      get sessionId() {
        return currentSessionId;
      },
      dispose() {
        leaving = true;
        ro.disconnect();
        if (ws.readyState <= 1) ws.close();
        term.dispose();
      },
    };
  }

  // Build the floating-window chrome and attach a live terminal into it. Shared
  // by `open()` (a new console) and the load-time reattach (one window per live
  // session). Keeps the shared workspace-relative drag/tiling; `termOpts` is the
  // `attachTerminal` opts, `label`/`repo` drive the titlebar.
  function spawnWindow(termOpts, label, repo) {
    const win = document.createElement("div");
    win.className = "session-window";
    cascade = (cascade + 1) % 8;
    win.style.left = 30 + cascade * 24 + "px";
    win.style.top = 20 + cascade * 24 + "px";
    win.style.width = "min(560px, 62%)";
    win.style.height = "min(340px, 60%)";

    const titlebar = document.createElement("div");
    titlebar.className = "session-titlebar";
    const title = document.createElement("span");
    title.className = "session-title";
    title.innerHTML = `<i class="bi bi-terminal"></i> ${label} · ${repo || "home"}`;
    const actions = document.createElement("span");
    actions.className = "session-actions";
    const closeBtn = document.createElement("button");
    closeBtn.className = "session-close";
    closeBtn.textContent = "close";
    actions.append(closeBtn);
    titlebar.append(title, actions);

    const body = document.createElement("div");
    body.className = "session-body";
    const grip = document.createElement("div");
    grip.className = "session-resize";
    win.append(titlebar, body, grip);
    workspace().append(win);

    win.addEventListener("mousedown", () => focusWin(win));
    makeDraggable(win, titlebar);
    makeResizable(win, grip);
    focusWin(win);

    const t = attachTerminal(body, {
      ...termOpts,
      // Busy-reattach → tear THIS window down and relaunch as a takeover, so no
      // dead empty window lingers.
      onRefused: () => {
        t.dispose();
        win.remove();
        wins.delete(win);
        changed();
        spawnWindow({ id: termOpts.id, takeover: true }, label, repo);
      },
    });
    win._term = t;

    closeBtn.onclick = () => {
      const id = t.sessionId;
      const finish = () => {
        t.dispose();
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
    return win;
  }

  // `agent` names an adapter (claude/codex/opencode); when `plain` is set there
  // is no agent — a normal shell in the repo dir, labelled "console".
  function open({ repo, agent, plain }) {
    const label = agent || "console";
    spawnWindow(plain ? { console: true, repo } : { repo, agent }, label, repo);
    WB.emit("console-open", { repo: repo || null, agent: agent || null, plain: !!plain });
  }

  // Reattach every live daemon session as its own floating window, so reopening
  // the browser restores the running consoles (with replayed scrollback).
  function reattachLive() {
    fetch("/api/sessions")
      .then((r) => (r.ok ? r.json() : []))
      .then((sessions) => {
        for (const s of sessions) {
          spawnWindow({ id: s.id }, s.agent || "console", s.repo);
        }
      })
      .catch(() => {});
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", reattachLive);
  } else {
    reattachLive();
  }

  // Refit every open console. Called when the Agents tab returns to view: a
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
      setTimeout(() => win.classList.remove("tiling"), 260);
    });
  }

  function count() {
    return wins.size;
  }

  return { open, arrange, count, refitAll };
})();
