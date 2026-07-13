/* ---------------------------------------------------------------------------
   ralphy workbench shell — file viewers (the closable tabs)

   Two flavours, both opening as their own tab after the fixed Agents tab:
     • source code — CodeMirror 5: syntax highlight, in-place editing, and a
       find dialog (Ctrl-F). Binaries never reach here (app.js refuses them).
     • Markdown  — rendered with `marked`, sanitized with DOMPurify, mermaid
       fences drawn as diagrams (Cursor-style), a heading outline to jump around,
       an in-page find, and an edit/preview toggle over the raw source.

   Editing is allowed but never touches disk: a Save emits a `save` intent on the
   `workbench:action` seam carrying the new content, for a backend to persist.
--------------------------------------------------------------------------- */
(function () {
  let mermaidReady = false;
  function initMermaid() {
    if (mermaidReady || !window.mermaid) return;
    window.mermaid.initialize({ startOnLoad: false, securityLevel: "loose", theme: "dark" });
    mermaidReady = true;
  }

  const ext = (p) => {
    const n = p.toLowerCase();
    return n.includes(".") ? n.split(".").pop() : "";
  };

  // Map a filename to a CodeMirror mode/MIME (modes are vendored in index.html).
  function cmMode(path) {
    const e = ext(path);
    const m = {
      js: "text/javascript", mjs: "text/javascript", cjs: "text/javascript",
      json: "application/json",
      ts: "text/typescript", tsx: "text/typescript-jsx", jsx: "text/jsx",
      css: "text/css", scss: "text/css", less: "text/css",
      html: "htmlmixed", xml: "xml", svg: "xml",
      rs: "text/x-rustsrc", py: "text/x-python",
      toml: "text/x-toml", yml: "text/x-yaml", yaml: "text/x-yaml",
      sh: "text/x-sh", bash: "text/x-sh",
      sql: "text/x-sql", go: "text/x-go",
      prisma: "text/x-csrc", md: "text/x-markdown", markdown: "text/x-markdown",
    };
    return m[e] || "text/plain";
  }

  const viewers = document.getElementById("viewers");
  const map = new Map(); // tab id → viewer record

  // --- a source-code editor tab ------------------------------------------
  function buildCode(rec) {
    const el = document.createElement("div");
    el.className = "viewer code-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="find"><i class="bi bi-search"></i> Find</button>
        <button class="vbtn" data-act="reload"><i class="bi bi-arrow-clockwise"></i> Reload</button>
        <button class="vbtn save" data-act="save"><i class="bi bi-save"></i> Save</button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="viewer-body"></div>`;
    el.querySelector(".viewer-path").textContent = `${rec.project} / ${rec.path}`;
    viewers.append(el);

    const cm = CodeMirror(el.querySelector(".viewer-body"), {
      value: rec.content,
      mode: cmMode(rec.path),
      theme: "wb",
      lineNumbers: true,
      matchBrackets: true,
      styleActiveLine: true,
      lineWrapping: false,
      extraKeys: {
        "Ctrl-S": () => save(rec),
        "Cmd-S": () => save(rec),
      },
    });
    rec.cm = cm;
    const saveBtn = el.querySelector('[data-act="save"]');
    cm.on("change", () => {
      rec.dirty = true;
      saveBtn.classList.add("dirty");
    });
    el.querySelector('[data-act="find"]').onclick = () => {
      cm.focus();
      cm.execCommand("find");
    };
    saveBtn.onclick = () => save(rec);
    el.querySelector('[data-act="reload"]').onclick = () => reloadFile(rec);
    el.querySelector('[data-act="detach"]').onclick = () => detachClick(rec);
    rec.el = el;
    rec.saveBtn = saveBtn;
  }

  function save(rec) {
    const content = rec.editing ? rec.cm.getValue() : rec.cm ? rec.cm.getValue() : rec.content;
    rec.content = content;
    rec.dirty = false;
    rec.saveBtn?.classList.remove("dirty");
    WB.emit("save", { project: rec.project, path: rec.path, bytes: content.length, content });
    if (rec.kind === "markdown" && !rec.editing) renderMarkdown(rec); // keep preview fresh
  }

  // The pane's current bytes, whether shown as source (CodeMirror) or as a
  // rendered markdown preview.
  function contentOf(rec) {
    if (rec.cm && (rec.kind === "code" || rec.editing)) return rec.cm.getValue();
    return rec.content;
  }

  // A portable descriptor — enough to reopen this file anywhere (a tab or a
  // detached popup), carrying the *current* (possibly edited) content.
  function descOf(rec) {
    return { project: rec.project, path: rec.path, ftype: rec.kind, content: contentOf(rec) };
  }

  // Reload discards local edits and reloads from source. Daemon-backed repos
  // re-read the REAL file via `file.read` (#197); the `file://` mock regenerates
  // its synthesised bytes. The apply step is shared via `applyFresh`.
  function reloadFile(rec) {
    const daemonBacked = location.protocol !== "file:" && !!window.WBDaemon?.observe;
    if (daemonBacked) {
      WBDaemon.observe("file.read", { repo: rec.project, path: rec.path })
        .then((reply) => applyFresh(rec, reply.status === "ok" ? reply.content : fakeContent(rec.path, rec.kind)))
        .catch(() => applyFresh(rec, fakeContent(rec.path, rec.kind)));
    } else {
      applyFresh(rec, fakeContent(rec.path, rec.kind));
    }
  }

  function applyFresh(rec, fresh) {
    rec.content = fresh;
    // setValue fires CodeMirror's change event, so clear the dirty flag *after*
    // updating content, not before.
    if (rec.kind === "code" || rec.editing) {
      rec.cm.setValue(fresh);
    } else {
      if (rec.xlate) rec.xlate.cache = {}; // fresh bytes invalidate translations
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
      if (rec.xlate?.on) ensureMdXlate(rec);
    }
    rec.dirty = false;
    rec.saveBtn?.classList.remove("dirty");
    WB.emit("reload", { project: rec.project, path: rec.path });
  }

  // The Detach/Re-attach button. A file tab detaches into a standalone popup
  // (watch an agent in the main window, read the file in another); a detached
  // pane folds back in. wb-viewer only *requests* it — the shell (app.js) opens
  // the popup, and the popup (detached.html) folds back — so this module stays
  // agnostic to windows/tabs.
  function detachBtnHtml(rec) {
    return rec.detached
      ? '<button class="vbtn" data-act="detach"><i class="bi bi-box-arrow-in-down-left"></i> Re-attach</button>'
      : '<button class="vbtn" data-act="detach"><i class="bi bi-box-arrow-up-right"></i> Detach</button>';
  }

  function detachClick(rec) {
    const evt = rec.detached ? "workbench:reattach-request" : "workbench:detach-request";
    document.dispatchEvent(new CustomEvent(evt, { detail: descOf(rec) }));
  }

  // --- a Markdown tab -----------------------------------------------------
  function buildMarkdown(rec) {
    const el = document.createElement("div");
    el.className = "viewer md-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="find"><i class="bi bi-search"></i> Find</button>
        <button class="vbtn" data-act="reload"><i class="bi bi-arrow-clockwise"></i> Reload</button>
        <button class="vbtn" data-act="toggle"><i class="bi bi-pencil"></i> Edit</button>
        <!-- on-device translation of the rendered preview (not the editor) -->
        <button class="vbtn" data-act="xlate" title="translate the preview on-device"><i class="bi bi-translate"></i> Translate</button>
        <select class="vbtn md-xlate-target" data-act="xlate-target" title="translate to" style="display:none"></select>
        <span class="md-xlate-note" data-role="xlate-note"></span>
        <button class="vbtn save" data-act="save"><i class="bi bi-save"></i> Save</button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="md-find">
        <input class="md-find-input" placeholder="Find in page…" />
        <span class="md-find-count"></span>
        <button class="vbtn" data-find="prev"><i class="bi bi-chevron-up"></i></button>
        <button class="vbtn" data-find="next"><i class="bi bi-chevron-down"></i></button>
        <button class="vbtn" data-find="close"><i class="bi bi-x"></i></button>
      </div>
      <div class="md-split">
        <nav class="md-outline"></nav>
        <div class="md-scroll"><article class="md-body"></article></div>
        <div class="md-editor" style="display:none"></div>
      </div>`;
    el.querySelector(".viewer-path").textContent = `${rec.project} / ${rec.path}`;
    viewers.append(el);
    rec.el = el;
    rec.saveBtn = el.querySelector('[data-act="save"]');

    // edit / preview toggle
    el.querySelector('[data-act="toggle"]').onclick = () => toggleEdit(rec);
    el.querySelector('[data-act="save"]').onclick = () => {
      if (rec.editing) rec.content = rec.cm.getValue();
      save(rec);
    };
    el.querySelector('[data-act="reload"]').onclick = () => reloadFile(rec);
    el.querySelector('[data-act="detach"]').onclick = () => detachClick(rec);
    // in-page find over the rendered article
    const find = el.querySelector(".md-find");
    const input = el.querySelector(".md-find-input");
    el.querySelector('[data-act="find"]').onclick = () => {
      find.classList.add("open");
      input.focus();
      input.select();
    };
    input.addEventListener("input", () => mdSearch(rec, input.value));
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") mdSearchStep(rec, e.shiftKey ? -1 : 1);
      if (e.key === "Escape") mdSearchClose(rec);
    });
    el.querySelector('[data-find="next"]').onclick = () => mdSearchStep(rec, 1);
    el.querySelector('[data-find="prev"]').onclick = () => mdSearchStep(rec, -1);
    el.querySelector('[data-find="close"]').onclick = () => mdSearchClose(rec);

    // translate control — preview only; a shared on-device helper (WBTranslate).
    rec.xlate = { on: false, target: WBTranslate.browserLang(), busy: false, cache: {} };
    const xbtn = el.querySelector('[data-act="xlate"]');
    const xsel = el.querySelector('[data-act="xlate-target"]');
    WBTranslate.LANGS.forEach((l) => {
      const o = document.createElement("option");
      o.value = l.code;
      o.textContent = l.label;
      xsel.append(o);
    });
    xsel.value = rec.xlate.target;
    // no on-device Translator API → the control is hidden entirely, not disabled
    if (!WBTranslate.supported()) {
      xbtn.style.display = "none";
      xsel.style.display = "none";
    }
    xbtn.onclick = () => toggleMdXlate(rec);
    xsel.onchange = () => {
      rec.xlate.target = xsel.value;
      rec.xlate.cache = {}; // a new target is a fresh translation
      ensureMdXlate(rec);
    };

    renderMarkdown(rec);
  }

  // --- markdown translation (rendered preview) ---------------------------
  // The source to render: the cached translation when the toggle is on and
  // ready, else the original markdown. Editing always shows the raw source in
  // the editor, so translation never touches what you edit.
  function mdSourceForRender(rec) {
    if (rec.xlate?.on) {
      const t = rec.xlate.cache[rec.xlate.target];
      if (t != null) return t;
    }
    return rec.content;
  }

  function setMdXlateNote(rec, msg) {
    const n = rec.el.querySelector('[data-role="xlate-note"]');
    if (n) n.textContent = msg || "";
  }
  // hide the translate controls while editing (translation is a preview concern);
  // also stay hidden where the API is absent, so returning to preview never
  // re-reveals a control that can't work.
  function setMdXlateControls(rec, visible) {
    const show = visible && WBTranslate.supported();
    const xbtn = rec.el.querySelector('[data-act="xlate"]');
    const xsel = rec.el.querySelector('[data-act="xlate-target"]');
    if (xbtn) xbtn.style.display = show ? "" : "none";
    if (xsel) xsel.style.display = show && rec.xlate.on ? "" : "none";
    if (!visible) setMdXlateNote(rec, "");
  }

  function toggleMdXlate(rec) {
    if (!WBTranslate.supported() || rec.editing) return;
    rec.xlate.on = !rec.xlate.on;
    const xbtn = rec.el.querySelector('[data-act="xlate"]');
    const xsel = rec.el.querySelector('[data-act="xlate-target"]');
    xbtn.classList.toggle("on", rec.xlate.on);
    xsel.style.display = rec.xlate.on ? "" : "none";
    setMdXlateNote(rec, "");
    if (rec.xlate.on) {
      ensureMdXlate(rec);
    } else {
      renderMarkdown(rec); // back to the original
      if (rec.visible) drawMermaid(rec);
    }
  }

  // Translate the current markdown into the chosen target and re-render. A
  // same-language target is surfaced ("already X") so it never looks broken;
  // a failure reverts the toggle honestly.
  async function ensureMdXlate(rec) {
    const t = rec.xlate.target;
    const xbtn = rec.el.querySelector('[data-act="xlate"]');
    if (rec.xlate.cache[t] != null) {
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
      return;
    }
    rec.xlate.busy = true;
    xbtn.classList.add("busy");
    setMdXlateNote(rec, "translating…");
    try {
      const res = await WBTranslate.translate(rec.content, t);
      rec.xlate.cache[t] = res.text;
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
      setMdXlateNote(rec, res.same ? `already ${t.toUpperCase()}` : "");
    } catch (e) {
      rec.xlate.on = false;
      xbtn.classList.remove("on");
      rec.el.querySelector('[data-act="xlate-target"]').style.display = "none";
      renderMarkdown(rec);
      setMdXlateNote(rec, e?.message || "translate failed");
    } finally {
      rec.xlate.busy = false;
      xbtn.classList.remove("busy");
    }
  }

  function renderMarkdown(rec) {
    const article = rec.el.querySelector(".md-body");
    const html = DOMPurify.sanitize(marked.parse(mdSourceForRender(rec)));
    article.innerHTML = html;

    // mermaid fences: marked emits <pre><code class="language-mermaid">. Defer
    // the actual draw to first paint (a hidden container measures as 0).
    rec.mermaidPending = [];
    article.querySelectorAll("code.language-mermaid").forEach((code, i) => {
      const holder = document.createElement("div");
      holder.className = "mermaid";
      holder.dataset.src = code.textContent;
      holder.id = `mmd-${rec.uid}-${i}`;
      code.closest("pre").replaceWith(holder);
      rec.mermaidPending.push(holder);
    });

    buildOutline(rec, article);
    if (rec.visible) drawMermaid(rec);
  }

  function drawMermaid(rec) {
    if (!rec.mermaidPending || !rec.mermaidPending.length) return;
    initMermaid();
    const pending = rec.mermaidPending;
    rec.mermaidPending = [];
    pending.forEach((holder) => {
      window.mermaid
        .render(holder.id + "-svg", holder.dataset.src)
        .then(({ svg }) => (holder.innerHTML = svg))
        .catch((err) => {
          holder.classList.add("mermaid-error");
          holder.textContent = "mermaid error: " + (err?.message || err);
        });
    });
  }

  // Heading outline: the jump index, one entry per heading, indented by level.
  function buildOutline(rec, article) {
    const nav = rec.el.querySelector(".md-outline");
    nav.innerHTML = "";
    const heads = article.querySelectorAll("h1, h2, h3, h4");
    if (!heads.length) {
      nav.innerHTML = '<div class="outline-empty">no headings</div>';
      return;
    }
    heads.forEach((h, i) => {
      const id = `h-${rec.uid}-${i}`;
      h.id = id;
      const a = document.createElement("a");
      a.className = "outline-item lvl-" + h.tagName.toLowerCase();
      a.textContent = h.textContent;
      a.title = h.textContent;
      a.onclick = () => h.scrollIntoView({ behavior: "smooth", block: "start" });
      nav.append(a);
    });
  }

  // --- in-page find over rendered markdown -------------------------------
  function clearHits(rec) {
    (rec.hits || []).forEach((mk) => {
      const t = document.createTextNode(mk.textContent);
      mk.replaceWith(t);
    });
    rec.el.querySelector(".md-body").normalize();
    rec.hits = [];
    rec.hitIdx = -1;
  }

  function mdSearch(rec, term) {
    clearHits(rec);
    const count = rec.el.querySelector(".md-find-count");
    if (!term) {
      count.textContent = "";
      return;
    }
    const article = rec.el.querySelector(".md-body");
    const walker = document.createTreeWalker(article, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) =>
        n.nodeValue.trim() && !n.parentElement.closest("svg, script, style")
          ? NodeFilter.FILTER_ACCEPT
          : NodeFilter.FILTER_REJECT,
    });
    const targets = [];
    let node;
    while ((node = walker.nextNode())) targets.push(node);
    const needle = term.toLowerCase();
    const hits = [];
    for (const text of targets) {
      const val = text.nodeValue;
      const lower = val.toLowerCase();
      let idx = lower.indexOf(needle);
      if (idx < 0) continue;
      const frag = document.createDocumentFragment();
      let last = 0;
      while (idx >= 0) {
        if (idx > last) frag.append(document.createTextNode(val.slice(last, idx)));
        const mk = document.createElement("mark");
        mk.className = "find-hit";
        mk.textContent = val.slice(idx, idx + term.length);
        frag.append(mk);
        hits.push(mk);
        last = idx + term.length;
        idx = lower.indexOf(needle, last);
      }
      if (last < val.length) frag.append(document.createTextNode(val.slice(last)));
      text.replaceWith(frag);
    }
    rec.hits = hits;
    rec.hitIdx = -1;
    count.textContent = hits.length ? `0/${hits.length}` : "no matches";
    if (hits.length) mdSearchStep(rec, 1);
  }

  function mdSearchStep(rec, dir) {
    if (!rec.hits || !rec.hits.length) return;
    if (rec.hitIdx >= 0) rec.hits[rec.hitIdx].classList.remove("current");
    rec.hitIdx = (rec.hitIdx + dir + rec.hits.length) % rec.hits.length;
    const mk = rec.hits[rec.hitIdx];
    mk.classList.add("current");
    mk.scrollIntoView({ block: "center", behavior: "smooth" });
    rec.el.querySelector(".md-find-count").textContent = `${rec.hitIdx + 1}/${rec.hits.length}`;
  }

  function mdSearchClose(rec) {
    clearHits(rec);
    rec.el.querySelector(".md-find").classList.remove("open");
    rec.el.querySelector(".md-find-count").textContent = "";
    rec.el.querySelector(".md-find-input").value = "";
  }

  // Swap the markdown pane between rendered preview and a raw-source editor.
  function toggleEdit(rec) {
    const split = rec.el.querySelector(".md-split");
    const editor = rec.el.querySelector(".md-editor");
    const toggle = rec.el.querySelector('[data-act="toggle"]');
    rec.editing = !rec.editing;
    if (rec.editing) {
      split.classList.add("editing");
      editor.style.display = "block";
      if (!rec.cm) {
        rec.cm = CodeMirror(editor, {
          value: rec.content,
          mode: "text/x-markdown",
          theme: "wb",
          lineNumbers: true,
          lineWrapping: true,
          extraKeys: { "Ctrl-S": () => save(rec), "Cmd-S": () => save(rec) },
        });
        rec.cm.on("change", () => {
          rec.dirty = true;
          rec.saveBtn.classList.add("dirty");
        });
      } else {
        rec.cm.setValue(rec.content);
      }
      toggle.innerHTML = '<i class="bi bi-eye"></i> Preview';
      setMdXlateControls(rec, false); // translation is preview-only
      setTimeout(() => rec.cm.refresh(), 0);
    } else {
      rec.content = rec.cm.getValue();
      split.classList.remove("editing");
      editor.style.display = "none";
      toggle.innerHTML = '<i class="bi bi-pencil"></i> Edit';
      if (rec.xlate) rec.xlate.cache = {}; // edits invalidate any translation
      setMdXlateControls(rec, true);
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
      if (rec.xlate?.on) ensureMdXlate(rec); // re-translate the edited content
    }
  }

  // --- public API ---------------------------------------------------------
  let uidSeq = 0;
  const API = {
    open({ id, project, path, ftype, content, detached }) {
      if (map.has(id)) return;
      const rec = { id, project, path, kind: ftype, content, uid: ++uidSeq, editing: false, visible: false, detached: !!detached };
      map.set(id, rec);
      if (ftype === "markdown") buildMarkdown(rec);
      else buildCode(rec);
    },

    // Show one pane (or none, when the Agents tab is active). CodeMirror and
    // mermaid both need a laid-out container, so we (re)paint on first show.
    setActive(id) {
      for (const rec of map.values()) {
        const on = rec.id === id;
        rec.el.style.display = on ? "flex" : "none";
        rec.visible = on;
        if (on) {
          if (rec.cm) setTimeout(() => rec.cm.refresh(), 0);
          if (rec.kind === "markdown") drawMermaid(rec);
        }
      }
    },

    close(id) {
      const rec = map.get(id);
      if (!rec) return;
      rec.el.remove();
      map.delete(id);
    },
  };
  window.WBViewer = API;
})();

/* ---------------------------------------------------------------------------
   Fake file contents — this is a mock with no backend, so a file's "bytes" are
   synthesised from its name so every viewer feature is demonstrable. A real
   build fetches the actual file instead.
--------------------------------------------------------------------------- */
function fakeContent(path, ftype) {
  const base = path.split("/").pop();
  const e = base.toLowerCase().includes(".") ? base.toLowerCase().split(".").pop() : "";
  if (ftype === "markdown") return fakeMarkdown(base);
  const gen = {
    ts: fakeTs, tsx: fakeTsx, js: fakeTs, mjs: fakeTs,
    rs: fakeRs, json: fakeJson, css: fakeCss, toml: fakeToml,
    prisma: fakePrisma, py: fakePy,
  }[e];
  return gen ? gen(base) : `// ${path}\n// (mock) source for ${base}\n\nexport const answer = 42;\n`;
}

function fakeMarkdown(name) {
  const title = name.replace(/\.(md|markdown)$/i, "");
  return `# ${title}

A rendered Markdown tab — the outline on the left jumps between headings, the
toolbar's **Find** searches this page, and **Edit** flips to the raw source.

## Architecture

The workbench is an intent surface: gestures become events; a backend does the
real work.

\`\`\`mermaid
flowchart LR
  U[User gesture] --> UI[Workbench UI]
  UI -- workbench:action --> BE[Backend engine]
  BE --> FS[(Filesystem)]
  BE --> GH[(GitHub)]
\`\`\`

## Usage

1. Open a project in the sidebar.
2. Double-click a file to open it here.
3. Right-click for rename / copy path / delete.

### Notes

- Binary files refuse to open.
- Markdown always opens **rendered**, with mermaid support.
- Source files open with syntax highlighting.

## A table

| Kind     | Viewer        | Editable |
| -------- | ------------- | -------- |
| \`.md\`    | rendered      | yes      |
| \`.rs\`    | CodeMirror    | yes      |
| \`.png\`   | (refused)     | no       |

## Code sample

\`\`\`ts
export function greet(name: string) {
  return \`hello, \${name}\`;
}
\`\`\`

> Editing here emits a \`save\` intent — the mock never writes to disk.
`;
}

function fakeTs(name) {
  return `// ${name}
import { useEffect, useState } from "react";

export interface Session {
  id: number;
  repo: string;
  agent: "claude" | "codex" | "opencode";
}

export function useSessions(): Session[] {
  const [sessions, setSessions] = useState<Session[]>([]);
  useEffect(() => {
    fetch("/api/sessions")
      .then((r) => r.json())
      .then(setSessions)
      .catch(() => setSessions([]));
  }, []);
  return sessions;
}
`;
}

function fakeTsx(name) {
  return `// ${name}
import { useSessions } from "../lib/sessions";

export default function Sidebar() {
  const sessions = useSessions();
  return (
    <aside className="side">
      <h2>Sessions</h2>
      <ul>
        {sessions.map((s) => (
          <li key={s.id}>
            {s.repo} · {s.agent}
          </li>
        ))}
      </ul>
    </aside>
  );
}
`;
}

function fakeRs(name) {
  return `// ${name}
use std::collections::HashMap;

/// A registered repository the daemon can launch agents into.
#[derive(Debug, Clone)]
pub struct Repo {
    pub slug: String,
    pub path: std::path::PathBuf,
    pub reachable: bool,
}

impl Repo {
    pub fn new(slug: impl Into<String>, path: impl Into<std::path::PathBuf>) -> Self {
        Self { slug: slug.into(), path: path.into(), reachable: true }
    }
}

pub fn index(repos: &[Repo]) -> HashMap<&str, &Repo> {
    repos.iter().map(|r| (r.slug.as_str(), r)).collect()
}
`;
}

function fakeJson(name) {
  return `{
  "name": "${name.replace(/\.json$/, "")}",
  "version": "0.1.0",
  "private": true,
  "scripts": {
    "dev": "next dev",
    "build": "next build",
    "test": "vitest"
  },
  "dependencies": {
    "next": "15.0.0",
    "react": "19.0.0"
  }
}
`;
}

function fakeCss(name) {
  return `/* ${name} */
:root {
  --bg: #14110f;
  --text: #e8e2d9;
  --accent: #e8d9a8;
}
body {
  background: var(--bg);
  color: var(--text);
  font-family: ui-monospace, monospace;
}
.btn {
  border: 1px solid var(--accent);
  border-radius: 4px;
  padding: 0.3rem 0.7rem;
}
`;
}

function fakeToml(name) {
  return `# ${name}
[package]
name = "ralphy"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
`;
}

function fakePrisma(name) {
  return `// ${name}
datasource db {
  provider = "postgresql"
  url      = env("DATABASE_URL")
}

model User {
  id    Int    @id @default(autoincrement())
  email String @unique
  name  String?
}
`;
}

function fakePy(name) {
  return `# ${name}
from dataclasses import dataclass


@dataclass
class Repo:
    slug: str
    path: str
    reachable: bool = True


def index(repos: list[Repo]) -> dict[str, Repo]:
    return {r.slug: r for r in repos}
`;
}
