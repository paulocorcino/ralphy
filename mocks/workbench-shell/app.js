/* ---------------------------------------------------------------------------
   ralphy workbench shell — mock behaviour

   The sidebar is a project accordion (Alpine). The file tree inside the open
   project is a real Wunderbaum instance (mar10/wunderbaum) — a mature,
   dependency-free tree lib — loaded from a JSON tree the backend would send.

   The canvas is a tabbed workspace:
     • the first tab, "Agents", is fixed (never closes) and hosts the floating
       console windows (see wb-console.js);
     • every opened file rides in as its own closable tab, rendered by a viewer
       (source code via CodeMirror, Markdown rendered with mermaid — see
       wb-viewer.js).

   Every user gesture (open, rename, delete, save, console-open…) is turned into
   a single CustomEvent, `workbench:action`, on `document`. That event *is* the
   seam: a backend engine subscribes and performs the real work. The UI itself
   performs nothing destructive — it only intents.
--------------------------------------------------------------------------- */

// The one exit point, shared by the sidebar, the consoles, and the viewers:
// every gesture becomes a `workbench:action` event a backend listens for.
window.WB = {
  emit(action, detail = {}) {
    const full = { action, ...detail, at: new Date().toISOString() };
    document.dispatchEvent(new CustomEvent("workbench:action", { detail: full }));
    // eslint-disable-next-line no-console
    console.log("[workbench:action]", full);
  },
};

// Files whose bytes aren't source we can render — refuse to open them.
const BINARY_EXT = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "pdf", "zip", "gz",
  "tar", "rar", "7z", "exe", "dll", "so", "dylib", "bin", "class", "jar", "wasm",
  "mp3", "wav", "flac", "ogg", "mp4", "mov", "avi", "mkv", "webm", "woff",
  "woff2", "ttf", "eot", "otf",
]);

function extOf(name) {
  const n = name.toLowerCase();
  return n.includes(".") ? n.split(".").pop() : "";
}

// What kind of viewer a file gets: markdown gets the rendered pane, binaries
// are refused, everything else opens as source code.
function classify(name) {
  const ext = extOf(name);
  if (ext === "md" || ext === "markdown") return "markdown";
  if (BINARY_EXT.has(ext)) return "binary";
  return "code";
}

function shell() {
  return {
    openSlug: null,
    _tree: null, // the live Wunderbaum instance, if any

    // --- chrome panels ----------------------------------------------------
    // Projects sidebar visibility (rail Projects button), the right-hand Runs
    // panel (rail Runs button), and the Kanban/tasks board (rail Kanban button,
    // a stub for now). Each is a pure layout flip driven by a body class.
    sideOpen: true,
    runsOpen: false,
    kanbanOpen: false,

    toggleSide() {
      this.sideOpen = !this.sideOpen;
    },
    toggleRuns() {
      this.runsOpen = !this.runsOpen;
    },
    toggleKanban() {
      // No board yet — announce the intent so a backend/next iteration can wire
      // the tasks view. Marks the button active as a visible affordance.
      this.kanbanOpen = !this.kanbanOpen;
      WB.emit("kanban-toggle", { open: this.kanbanOpen });
    },

    // --- canvas tabs ------------------------------------------------------
    // The Agents tab is permanent; file tabs are appended and closable.
    agents: ["claude", "codex", "opencode"],
    agentMenu: false,
    consoleCount: 0,
    tabs: [{ id: "agents", kind: "agents", title: "Agents", icon: "bi bi-robot", closable: false }],
    active: "agents",

    // Projects carry a *nested* file tree (folder → children), the shape a
    // backend would deliver as JSON. `state` is daemon reachability (the dot);
    // `remote` is provenance — a GitHub-backed repo vs one that lives only on
    // this disk. Icons are resolved at mount time.
    projects: [
      {
        slug: "lingopilot",
        branch: "main",
        state: "live",
        remote: "github",
        tree: [
          {
            title: "src",
            folder: true,
            expanded: true,
            children: [
              { title: "app", folder: true, children: [{ title: "page.tsx" }, { title: "layout.tsx" }] },
              { title: "components", folder: true, children: [{ title: "Chat.tsx" }, { title: "Sidebar.tsx" }] },
              { title: "lib", folder: true, children: [{ title: "db.ts" }, { title: "auth.ts" }] },
            ],
          },
          { title: "prisma", folder: true, children: [{ title: "schema.prisma" }] },
          { title: "package.json" },
          { title: "next.config.ts" },
          { title: "tsconfig.json" },
          { title: "logo.png" },
          { title: "README.md" },
        ],
      },
      {
        slug: "fincal",
        branch: "feat/triage",
        state: "idle",
        remote: "github",
        tree: [
          { title: ".ralphy", folder: true, children: [{ title: "plan.md" }, { title: "triage-draft.json" }] },
          {
            title: "docs",
            folder: true,
            children: [
              { title: "adr", folder: true, children: [{ title: "0001-vocabulary.md" }] },
              { title: "issues", folder: true, children: [] },
            ],
          },
          { title: "src", folder: true, children: [{ title: "index.ts" }, { title: "styles.css" }] },
          { title: "CONTEXT.md" },
          { title: "package.json" },
        ],
      },
      {
        slug: "ralphy",
        branch: "feat/xterm-v6-webgl",
        state: "idle",
        remote: "github",
        tree: [
          {
            title: "crates",
            folder: true,
            children: [
              { title: "ralphy-cli", folder: true, children: [{ title: "main.rs" }] },
              { title: "ralphy-core", folder: true, children: [{ title: "lib.rs" }] },
              { title: "ralphy-daemon", folder: true, children: [{ title: "protocol.rs" }, { title: "dispatch.rs" }] },
            ],
          },
          { title: "docs", folder: true, children: [{ title: "adr", folder: true, children: [{ title: "0035-daemon-ui-visual-language.md" }] }] },
          { title: "Cargo.toml" },
        ],
      },
      {
        slug: "bioledger",
        branch: "main",
        state: "offline",
        remote: "local", // never pushed anywhere — lives only on this disk
        tree: [
          { title: "src", folder: true, children: [{ title: "ocr.ts" }] },
          { title: "tests", folder: true, children: [{ title: "ocr.test.mjs" }] },
          { title: "notes.md" },
          { title: "package.json" },
        ],
      },
    ],

    // --- accordion --------------------------------------------------------
    toggle(slug) {
      this.openSlug = this.openSlug === slug ? null : slug;
      this.$nextTick(() => {
        this.destroyTree();
        if (this.openSlug) this.mountTree();
        window.lucide?.createIcons();
      });
    },

    // The status dot's colour = the project's daemon-reachability right now:
    //   live    → green  (a session/daemon is active there)
    //   idle    → grey   (registered & reachable, but stopped) — the default
    //   offline → red    (unreachable path: moved/deleted)
    // This is orthogonal to `remote` (GitHub vs local-only): a local-only repo
    // can be live, and a GitHub repo can be offline. A real daemon would derive
    // this the way the live UI does (repo.reachable in the daemon's /api/repos).
    dotClass(state) {
      return state === "live" ? "live" : state === "offline" ? "offline" : "";
    },

    // Wunderbaum marks folders via the source `folder:true` flag / a children
    // array; there is no isFolder() method on the node.
    isFolder(node) {
      return !!(node.folder || node.children);
    },

    // --- file-type icons (Devicon font; folders use Wunderbaum defaults) ---
    fileIcon(title) {
      const name = title.toLowerCase();
      if (name.endsWith("lock") || name === "package-lock.json") return "devicon-json-plain colored";
      const ext = name.includes(".") ? name.split(".").pop() : "";
      const map = {
        ts: "devicon-typescript-plain colored",
        tsx: "devicon-typescript-plain colored",
        js: "devicon-javascript-plain colored",
        mjs: "devicon-javascript-plain colored",
        cjs: "devicon-javascript-plain colored",
        json: "devicon-json-plain colored",
        md: "devicon-markdown-plain md-glyph",
        rs: "devicon-rust-plain rs-glyph",
        css: "devicon-css3-plain colored",
        html: "devicon-html5-plain colored",
        prisma: "devicon-prisma-plain colored",
        png: "bi bi-image",
        jpg: "bi bi-image",
        jpeg: "bi bi-image",
        gif: "bi bi-image",
        svg: "bi bi-image",
        toml: "bi bi-gear",
        yml: "bi bi-gear",
        yaml: "bi bi-gear",
      };
      return map[ext] || "bi bi-file-earmark";
    },

    // Attach an `icon` to every *file* node (folders keep the theme default),
    // recursively, without mutating the source shape the backend sent.
    withIcons(nodes) {
      return nodes.map((n) => {
        if (n.folder || n.children) {
          return { ...n, children: this.withIcons(n.children || []) };
        }
        return { ...n, icon: this.fileIcon(n.title) };
      });
    },

    // --- Wunderbaum mount / teardown --------------------------------------
    mountTree() {
      const host = document.querySelector(".project.open .wb-host");
      const project = this.projects.find((p) => p.slug === this.openSlug);
      if (!host || !project) return;

      this._tree = new mar10.Wunderbaum({
        element: host,
        header: false,
        source: this.withIcons(project.tree),
        edit: {
          trigger: ["F2", "macEnter"],
          // A committed rename is an intent, not a mutation done here.
          apply: (e) => {
            this.emit("rename", e.node, { from: e.oldValue, to: e.newValue });
            return true; // let the tree reflect it optimistically
          },
        },
        // Double-click / Enter on a leaf = "open this file".
        dblclick: (e) => {
          if (!this.isFolder(e.node)) this.openFile(e.node);
          return false;
        },
      });

      // Right-click anywhere in the tree → our own context menu.
      host.addEventListener("contextmenu", (ev) => {
        const node = mar10.Wunderbaum.getNode(ev);
        if (!node) return;
        ev.preventDefault();
        node.setActive();
        this.showMenu(ev.clientX, ev.clientY, node);
      });
    },

    destroyTree() {
      try {
        this._tree?.destroy?.();
      } catch {}
      this._tree = null;
      document.querySelectorAll(".wb-host").forEach((h) => (h.innerHTML = ""));
      this.hideMenu();
    },

    // --- opening a file into a tab ----------------------------------------
    // Decide the viewer, refuse binaries, and (for text) open — or focus — a
    // tab. The `open` intent still fires for the backend regardless.
    openFile(node) {
      const path = this.relPath(node);
      const ftype = classify(node.title);
      this.emit("open", node, { ftype });
      if (ftype === "binary") {
        WB.emit("open-refused", { project: this.openSlug, path, reason: "binary" });
        return;
      }
      this.openTab({ project: this.openSlug, path, title: node.title, ftype });
    },

    // `content` is optional: opening from the tree synthesises it, re-attaching
    // a detached popup passes the current (possibly edited) bytes back in.
    openTab({ project, path, title, ftype, content }) {
      const id = `file:${project}:${path}`;
      if (this.tabs.some((t) => t.id === id)) {
        this.activate(id);
        return;
      }
      const icon = ftype === "markdown" ? "bi bi-file-earmark-text" : "bi bi-file-earmark-code";
      this.tabs.push({ id, kind: ftype, title, path, project, icon, closable: true });
      this.active = id;
      this.$nextTick(() => {
        WBViewer.open({ id, project, path, ftype, content: content ?? fakeContent(path, ftype) });
        WBViewer.setActive(id);
        window.lucide?.createIcons();
      });
    },

    // Pop a file tab out into a standalone browser popup, so it can be read
    // side-by-side with an agent console in the main window. The descriptor is
    // handed over via a shared same-origin global (no serialisation limits); the
    // in-app tab then closes and we drop back to the Agents workspace.
    detachFile(desc) {
      const id = `file:${desc.project}:${desc.path}`;
      // file:// windows get opaque origins, so a shared global on window.opener
      // is unreadable (SecurityError). Hand the descriptor over in the URL hash
      // instead; the popup talks back over postMessage (see the listener below).
      const payload = encodeURIComponent(JSON.stringify(desc));
      const win = window.open("detached.html#" + payload, "_blank", "popup,width=920,height=760");
      if (!win) {
        WB.emit("detach-blocked", { project: desc.project, path: desc.path });
        return;
      }
      WB.emit("detach", { project: desc.project, path: desc.path });
      this.closeTab(id);
      this.activate("agents");
    },

    activate(id) {
      this.active = id;
      this.$nextTick(() => {
        WBViewer.setActive(this.active === "agents" ? null : this.active);
        window.lucide?.createIcons();
      });
    },

    closeTab(id) {
      const idx = this.tabs.findIndex((t) => t.id === id);
      const tab = this.tabs[idx];
      if (!tab || !tab.closable) return; // Agents never closes
      WBViewer.close(id);
      this.tabs.splice(idx, 1);
      if (this.active === id) {
        // fall back to the neighbour, else the Agents tab
        const next = this.tabs[idx] || this.tabs[idx - 1] || this.tabs[0];
        this.activate(next.id);
      }
    },

    // --- consoles (the Agents tab) ----------------------------------------
    newConsole(agent) {
      WBConsole.open({ repo: this.openSlug, agent });
      this.consoleCount = WBConsole.count();
    },

    arrangeConsoles() {
      WBConsole.arrange();
    },

    // --- context menu -----------------------------------------------------
    showMenu(x, y, node) {
      const menu = document.getElementById("ctxmenu");
      const isFolder = this.isFolder(node);
      const items = [
        !isFolder && { label: "Open", icon: "bi-box-arrow-up-right", run: () => this.openFile(node) },
        { label: "Rename…", icon: "bi-pencil", run: () => node.startEditTitle() },
        { label: "Copy relative path", icon: "bi-clipboard", run: () => this.copyPath(node) },
        isFolder && { label: "New file…", icon: "bi-file-earmark-plus", run: () => this.emit("create", node, { kind: "file" }) },
        isFolder && { label: "New folder…", icon: "bi-folder-plus", run: () => this.emit("create", node, { kind: "folder" }) },
        { sep: true },
        { label: "Delete", icon: "bi-trash", danger: true, run: () => this.emit("delete", node) },
      ].filter(Boolean);

      menu.innerHTML = "";
      for (const it of items) {
        if (it.sep) {
          const hr = document.createElement("div");
          hr.className = "ctx-sep";
          menu.append(hr);
          continue;
        }
        const b = document.createElement("button");
        b.className = "ctx-item" + (it.danger ? " danger" : "");
        b.innerHTML = `<i class="bi ${it.icon}"></i><span>${it.label}</span>`;
        b.onclick = () => {
          this.hideMenu();
          it.run();
        };
        menu.append(b);
      }
      // Keep the menu on-screen.
      menu.style.display = "block";
      const w = menu.offsetWidth,
        h = menu.offsetHeight;
      menu.style.left = Math.min(x, innerWidth - w - 8) + "px";
      menu.style.top = Math.min(y, innerHeight - h - 8) + "px";
    },

    hideMenu() {
      const menu = document.getElementById("ctxmenu");
      if (menu) menu.style.display = "none";
    },

    // --- the backend seam -------------------------------------------------
    // Build the repo-relative path by walking parent titles.
    relPath(node) {
      const parts = [];
      let n = node;
      while (n && n.title && n.parent) {
        parts.unshift(n.title);
        n = n.parent;
      }
      return parts.join("/");
    },

    copyPath(node) {
      const path = this.relPath(node);
      navigator.clipboard?.writeText(path).catch(() => {});
      this.emit("copy-path", node, { path });
    },

    // Node-shaped gestures funnel through the shared WB.emit.
    emit(action, node, extra = {}) {
      WB.emit(action, {
        project: this.openSlug,
        path: this.relPath(node),
        title: node.title,
        isFolder: this.isFolder(node),
        ...extra,
      });
    },
  };
}

window.shell = shell;

// The live Alpine component instance (Alpine stores it on the x-data element).
function getShell() {
  const root = document.querySelector("[x-data]");
  return root && root._x_dataStack ? root._x_dataStack[0] : null;
}

// Keep the Alpine mirror of the live console count fresh (windows can close
// themselves via their own chrome, outside the New-console button).
document.addEventListener("workbench:consoles-changed", (e) => {
  const c = getShell();
  if (c) c.consoleCount = e.detail.count;
});

// A viewer asked to detach → open the popup and close the tab.
document.addEventListener("workbench:detach-request", (e) => {
  getShell()?.detachFile(e.detail);
});

// Messages from detached popups (postMessage, since file:// blocks shared-global
// access): re-emit their save/reload intents on our seam so the backend sees
// them in one place, and fold a re-attached file back into the shell.
window.addEventListener("message", (e) => {
  const m = e.data;
  if (!m || typeof m !== "object") return;
  if (m.type === "wb-emit") {
    WB.emit(m.action, m.detail || {});
  } else if (m.type === "wb-reattach" && m.desc) {
    getShell()?.openTab({
      project: m.desc.project,
      path: m.desc.path,
      title: m.desc.path.split("/").pop(),
      ftype: m.desc.ftype,
      content: m.desc.content,
    });
  }
});

// Dismiss the context menu on any outside interaction.
document.addEventListener("click", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu").style.display = "none"));
document.addEventListener("scroll", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu").style.display = "none"), true);

document.addEventListener("alpine:initialized", () => window.lucide?.createIcons());
