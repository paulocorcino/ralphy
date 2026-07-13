/* ---------------------------------------------------------------------------
   ralphy workbench shell — floating consoles (the Agents tab)

   The canvas is a workspace where consoles live as draggable, resizable windows
   over the dotted floor. This mirrors the real daemon UI window chrome
   (crates/ralphy-daemon/assets/ui/index.html): a titlebar drag-handle, a body,
   and a bottom-right resize grip. There the body is a live xterm.js attached to
   a PTY over a WebSocket; here — a throwaway mock with no backend — the body is
   a faux terminal that echoes locally so the layout/behaviour can be felt.

   Opening/closing a console is an intent on the `workbench:action` seam; a real
   backend would spawn/attach the agent session.
--------------------------------------------------------------------------- */
window.WBConsole = (function () {
  const workspace = () => document.getElementById("workspace");
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

  // A tiny local-echo terminal so a console *feels* live without a backend.
  function fauxTerminal(body, repo, agent) {
    const term = document.createElement("div");
    term.className = "term";
    term.tabIndex = 0;
    const printed = [
      ` ${agent} · ${repo || "~"}  (mock console — local echo only)`,
      `type freely; a real daemon would attach a PTY here.`,
      "",
    ];
    let input = "";
    const prompt = `${repo || "~"} λ `;

    function render() {
      term.textContent = printed.join("\n") + "\n" + prompt + input;
      const cur = document.createElement("span");
      cur.className = "term-cursor";
      cur.textContent = "█";
      term.append(cur);
      term.scrollTop = term.scrollHeight;
    }
    term.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        printed.push(prompt + input);
        if (input.trim()) printed.push(`  ↳ (mock) '${input.trim()}' would run in ${repo || "home"}`);
        input = "";
      } else if (e.key === "Backspace") {
        input = input.slice(0, -1);
      } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
        input += e.key;
      } else {
        return;
      }
      e.preventDefault();
      render();
    });
    body.append(term);
    render();
    // focus the terminal when the window is first shown
    setTimeout(() => term.focus(), 0);
  }

  function open({ repo, agent }) {
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
    title.innerHTML = `<i class="bi bi-terminal"></i> ${agent} · ${repo || "home"}`;
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
    fauxTerminal(body, repo, agent);

    closeBtn.onclick = () => {
      win.remove();
      wins.delete(win);
      WB.emit("console-close", { repo: repo || null, agent });
      changed();
    };

    wins.add(win);
    WB.emit("console-open", { repo: repo || null, agent });
    changed();
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

  return { open, arrange, count };
})();
