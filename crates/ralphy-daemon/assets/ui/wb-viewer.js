/* ---------------------------------------------------------------------------
   ralphy workbench shell — file viewers (the closable tabs)

   Four flavours, each opening as its own tab after the fixed Consoles tab:
     • source code — Monaco: syntax highlight, in-place editing, and its own
       find widget. Binaries never reach here (app.js refuses them).
     • Markdown  — rendered with `marked`, sanitized with DOMPurify, mermaid
       fences drawn as diagrams (Cursor-style), a heading outline to jump around,
       an in-page find, and an edit/preview toggle over the raw source.
     • image     — an allowlisted image the daemon verified and served as a
       `data:` URL (ADR-0049), fit to the pane or shown 1:1. Read-only.
     • diff      — HEAD against the working tree, side by side. Read-only.

   Editing is allowed but never touches disk: a Save emits a `save` intent on the
   `workbench:action` seam carrying the new content, for a backend to persist.
--------------------------------------------------------------------------- */
(function () {
  let mermaidReady = false;
  function initMermaid() {
    if (mermaidReady || !window.mermaid) return;
    // `strict`, not `loose`: diagram source is repo bytes (an agent-written plan,
    // a PR fixture, a cloned README), i.e. untrusted. `loose` skips mermaid's
    // own sanitize pass over the emitted SVG and turns a `click A "javascript:…"`
    // directive into a live <a href>. Nothing here calls `bindFunctions`, so
    // click bindings were never wired up and `strict` costs no working feature.
    window.mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: "dark" });
    mermaidReady = true;
  }

  const viewers = document.getElementById("viewers");
  const map = new Map(); // tab id → viewer record

  // Monaco boots through an AMD loader, so an editor can only be created
  // asynchronously. Two invariants hold across that gap: `rec.content` is the
  // single source of truth until `rec.ed` exists (so bytes arriving mid-boot
  // are picked up by `create()`'s value), and a tab closed mid-boot must never
  // mount an orphan editor.
  //
  // The liveness check is `map.get(rec.id) === rec`, NOT `map.has(rec.id)`: tab
  // ids are stable per file (`file:<project>:<path>`), so closing and reopening
  // the same file inside the boot window puts a DIFFERENT record under the same
  // key — a `has` check would let the dead record mount an undisposable editor
  // on a detached container.
  const alive = (rec) => map.get(rec.id) === rec;

  function mountEditor(rec, container, opts) {
    const path = (opts && opts.path) || rec.path;
    if (rec.mounting || rec.mountFailed) return Promise.resolve();
    rec.mounting = true;
    return WBMonaco.ready()
      .catch((err) => {
        // The `file://` demo has no backend and may not boot the AMD loader —
        // degrade to read-only bytes rather than leave an empty pane (#308).
        // Only a BOOT failure lands here; a throw from create()/wiring below
        // must not masquerade as one.
        rec.mounting = false;
        rec.mountFailed = true;
        if (!alive(rec)) return null;
        const pre = document.createElement("pre");
        pre.className = "code-fallback";
        pre.textContent = rec.content;
        container.append(pre);
        rec.fallbackEl = pre;
        console.warn("[workbench] monaco did not boot; read-only fallback", err);
        return null;
      })
      .then((monaco) => {
        rec.mounting = false;
        if (!monaco || !alive(rec)) return;
        const ed = WBMonaco.create(container, {
          value: rec.content,
          path,
          uid: rec.uid,
          project: rec.project,
          wordWrap: opts && opts.wordWrap,
        });
        ed.onDidChangeModelContent(() => {
          rec.dirty = true;
          rec.saveBtn?.classList.add("dirty");
        });
        ed.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => save(rec));
        // Assigned LAST: a throw while wiring must not leave a half-live editor
        // that the rest of the module would treat as ready.
        rec.ed = ed;
        if (rec.visible) ed.layout();
      })
      .catch((err) => {
        // A create()/wiring failure is NOT a boot failure: the pane would look
        // editable while nothing is wired, so say so instead of degrading.
        rec.mounting = false;
        console.error("[workbench] monaco editor failed to mount", err);
        getShell()?._flashAction?.("editor failed to mount");
      });
  }

  function disposeEditor(rec) {
    if (!rec.ed) return;
    if (rec.kind === "diff") {
      // A diff editor holds TWO models, and BOTH must go before the editor on
      // EVERY path. Disposing the editor does NOT dispose its models, and a
      // reopen will not reveal the leak (createDiff's URIs carry a per-open
      // `uid`, so they never collide) — the leak is only visible as growth in
      // `monaco.editor.getModels()`, which is what wb_diff_311.py counts.
      const m = rec.ed.getModel();
      m?.original?.dispose();
      m?.modified?.dispose();
    } else {
      rec.ed.getModel()?.dispose();
    }
    rec.ed.dispose();
    rec.ed = undefined;
  }

  // --- a diff tab (read-only, two-sided) ----------------------------------
  function mountDiff(rec, container) {
    if (rec.mounting || rec.mountFailed) return Promise.resolve();
    rec.mounting = true;
    return WBMonaco.ready()
      .then((monaco) => {
        rec.mounting = false;
        // Same record-identity liveness rule as mountEditor: a diff tab closed
        // inside the boot window must mount nothing on its detached container.
        if (!monaco || !alive(rec)) return;
        const ed = WBMonaco.createDiff(container, {
          original: rec.original,
          modified: rec.content,
          path: rec.path,
          uid: rec.uid,
          project: rec.project,
        });
        rec.ed = ed;
        if (rec.visible) ed.layout();
      })
      .catch((err) => {
        // No `<pre>` degrade here: two texts side by side have no honest
        // single-pane fallback, and one of them rendered alone would read as
        // "no changes". Say it failed and close the tab (#311).
        rec.mounting = false;
        rec.mountFailed = true;
        console.error("[workbench] monaco diff failed to mount", err);
        getShell()?._flashAction?.("editor failed to mount");
        getShell()?.closeTab(rec.id);
      });
  }

  function buildDiff(rec) {
    const el = document.createElement("div");
    el.className = "viewer diff-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    // Find ONLY: no Save, no Reload, no disk badge, no Detach. This surface is
    // read-only by design (#311) — no commit, discard or staging control — and a
    // detached popup folds back a single-file descriptor a two-sided pane has no
    // representation in.
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="find"><i class="bi bi-search"></i> Find</button>
      </div>
      <div class="viewer-body"></div>`;
    el.querySelector(".viewer-path").textContent = `${rec.label} / ${rec.path} ↔ HEAD`;
    viewers.append(el);

    el.querySelector('[data-act="find"]').onclick = () => {
      const mod = rec.ed?.getModifiedEditor();
      mod?.focus();
      mod?.getAction("actions.find")?.run();
    };
    rec.el = el;
    mountDiff(rec, el.querySelector(".viewer-body"));
  }

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
        <button class="vbtn viewer-disk-badge" data-act="disk" style="display:none"><i class="bi bi-exclamation-triangle"></i> changed on disk — reload</button>
        <button class="vbtn save" data-act="save"><i class="bi bi-save"></i> Save</button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="viewer-body"></div>`;
    el.querySelector(".viewer-path").textContent = `${rec.label} / ${rec.path}`;
    viewers.append(el);

    const saveBtn = el.querySelector('[data-act="save"]');
    el.querySelector('[data-act="find"]').onclick = () => {
      rec.ed?.focus();
      rec.ed?.getAction("actions.find")?.run();
    };
    saveBtn.onclick = () => save(rec);
    el.querySelector('[data-act="reload"]').onclick = () => reloadFile(rec);
    el.querySelector('[data-act="disk"]').onclick = () => {
      applyFresh(rec, rec.pendingDisk);
      hideDiskBadge(rec);
    };
    el.querySelector('[data-act="detach"]').onclick = () => detachClick(rec);
    rec.el = el;
    rec.saveBtn = saveBtn;
    mountEditor(rec, el.querySelector(".viewer-body"), {});
  }

  function save(rec) {
    // A diff and an image are read-only: a `save` intent from either would be a
    // mutation those surfaces forbid, so it never reaches the action seam. (An
    // image's `content` is a `data:` URL, not the file's bytes — saving it would
    // write the URL over the image.)
    if (rec.kind === "diff" || rec.kind === "image") return;
    const content = contentOf(rec);
    rec.content = content;
    rec.dirty = false;
    rec.saveBtn?.classList.remove("dirty");
    hideDiskBadge(rec);
    WB.emit("save", { project: rec.project, path: rec.path, bytes: content.length, content });
    if (rec.kind === "markdown" && !rec.editing) renderMarkdown(rec); // keep preview fresh
  }

  // The pane's current bytes, whether shown as source (Monaco) or as a rendered
  // markdown preview. Before Monaco finishes booting `rec.ed` is undefined and
  // `rec.content` is still authoritative.
  function contentOf(rec) {
    if (rec.kind === "diff") return rec.content;
    if (rec.ed && (rec.kind === "code" || rec.editing)) return rec.ed.getValue();
    return rec.content;
  }

  // A portable descriptor — enough to reopen this file anywhere (a tab or a
  // detached popup), carrying the *current* (possibly edited) content.
  function descOf(rec) {
    return { project: rec.project, label: rec.label, path: rec.path, ftype: rec.kind, content: contentOf(rec) };
  }

  // Reload discards local edits and reloads from source. Daemon-backed repos
  // re-read the REAL file via `file.read` (#197); the `file://` demo regenerates
  // its synthesised bytes. The apply step is shared via `applyFresh`.
  function reloadFile(rec) {
    const daemonBacked = window.WBMode.isDaemon() && !!window.WBDaemon?.observe;
    if (daemonBacked) {
      // Daemon mode: a non-ok reply or a transport drop must NOT regenerate
      // synthetic bytes (C1) — flash the failure and close the tab, mirroring
      // the initial-open refusal path in app.js `fetchContent`.
      const fail = () => {
        getShell()?._flashAction?.("reload failed");
        getShell()?.closeTab(`file:${rec.project}:${rec.path}`);
      };
      // An image reloads through its own verb (ADR-0049): `file.read` refuses
      // its bytes, so routing it here would turn every image Reload into a
      // "reload failed" that closes the tab.
      if (rec.kind === "image") {
        WBDaemon.readImage(rec.project, rec.path)
          .then((url) => (url ? applyFresh(rec, url) : fail()))
          .catch(fail);
        return;
      }
      WBDaemon.observe("file.read", { repo: rec.project, path: rec.path })
        .then((reply) => (reply && reply.status === "ok" ? applyFresh(rec, reply.content) : fail()))
        .catch(fail);
    } else {
      applyFresh(rec, fakeContent(rec.path, rec.kind));
    }
  }

  function applyFresh(rec, fresh) {
    // A diff pane has no single "fresh bytes" to apply: reloading it means
    // re-resolving BOTH sides, which is a reopen, not a refresh.
    if (rec.kind === "diff") return;
    // `rec.content` FIRST: bytes that land before Monaco boots are picked up by
    // the pending `create()`, and setValue fires the change event, so the dirty
    // flag is cleared *after* the update, not before.
    rec.content = fresh;
    if (rec.kind === "image") {
      // A fresh `data:` URL repaints the pane; the `onload` handler re-reads the
      // intrinsic size, which an overwritten image may well have changed.
      const img = rec.el?.querySelector(".img-canvas");
      if (img) img.src = fresh;
    } else if (rec.kind === "code" || rec.editing) {
      if (rec.ed) rec.ed.setValue(fresh);
      // In the read-only fallback there is no editor to update, and leaving the
      // <pre> stale would show bytes that no longer match rec.content.
      else if (rec.fallbackEl) rec.fallbackEl.textContent = fresh;
    } else {
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
    }
    rec.dirty = false;
    rec.saveBtn?.classList.remove("dirty");
    hideDiskBadge(rec);
    WB.emit("reload", { project: rec.project, path: rec.path });
  }

  // The "changed on disk" badge: shown when an EXTERNAL write lands on a DIRTY
  // tab (never auto-applied — the operator's unsaved edits win until they click).
  function showDiskBadge(rec) {
    const b = rec.el?.querySelector(".viewer-disk-badge");
    if (b) b.style.display = "";
  }
  function hideDiskBadge(rec) {
    rec.pendingDisk = undefined;
    const b = rec.el?.querySelector(".viewer-disk-badge");
    if (b) b.style.display = "none";
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

  // --- an image tab (read-only) -------------------------------------------
  // `rec.content` is a `data:` URL the daemon's verified media type built
  // (ADR-0049 §2), so this pane never decides what bytes are. Read-only: no
  // Save, no Edit — the Write class is untouched by images.
  function buildImage(rec) {
    const el = document.createElement("div");
    el.className = "viewer image-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="img-meta"></span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="zoom"><i class="bi bi-arrows-angle-expand"></i> Actual size</button>
        <button class="vbtn" data-act="reload"><i class="bi bi-arrow-clockwise"></i> Reload</button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="viewer-body img-scroll"><img class="img-canvas" alt="" /></div>`;
    el.querySelector(".viewer-path").textContent = `${rec.label} / ${rec.path}`;
    viewers.append(el);

    const img = el.querySelector(".img-canvas");
    const meta = el.querySelector(".img-meta");
    // The intrinsic size is only known once the bytes decode, and a decode
    // failure is worth saying out loud: the daemon verified the type, so a
    // browser that still cannot paint it means an unsupported/corrupt file.
    img.onload = () => (meta.textContent = `${img.naturalWidth} × ${img.naturalHeight}`);
    img.onerror = () => (meta.textContent = "could not decode");
    img.src = rec.content;

    el.querySelector('[data-act="zoom"]').onclick = (ev) => {
      // Two states only: fit-to-pane (default) and 1:1 with scrollbars. A zoom
      // slider is a feature this pane does not need to read a screenshot.
      const actual = el.classList.toggle("actual-size");
      ev.currentTarget.innerHTML = actual
        ? '<i class="bi bi-arrows-angle-contract"></i> Fit'
        : '<i class="bi bi-arrows-angle-expand"></i> Actual size';
    };
    el.querySelector('[data-act="reload"]').onclick = () => reloadFile(rec);
    el.querySelector('[data-act="detach"]').onclick = () => detachClick(rec);
    rec.el = el;
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
        <button class="vbtn viewer-disk-badge" data-act="disk" style="display:none"><i class="bi bi-exclamation-triangle"></i> changed on disk — reload</button>
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
    el.querySelector(".viewer-path").textContent = `${rec.label} / ${rec.path}`;
    viewers.append(el);
    rec.el = el;
    rec.saveBtn = el.querySelector('[data-act="save"]');

    // edit / preview toggle
    el.querySelector('[data-act="toggle"]').onclick = () => toggleEdit(rec);
    el.querySelector('[data-act="save"]').onclick = () => save(rec);
    el.querySelector('[data-act="reload"]').onclick = () => reloadFile(rec);
    el.querySelector('[data-act="disk"]').onclick = () => {
      applyFresh(rec, rec.pendingDisk);
      hideDiskBadge(rec);
    };
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

    renderMarkdown(rec);
  }

  function renderMarkdown(rec) {
    const article = rec.el.querySelector(".md-body");
    const html = DOMPurify.sanitize(marked.parse(rec.content));
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

    resolveImages(rec, article);
    buildOutline(rec, article);
    if (rec.visible) drawMermaid(rec);
  }

  // Repo-relative `<img>` sources resolve through `file.image` (ADR-0049 §5),
  // against the DOCUMENT's own directory. This runs on the SANITIZED DOM, after
  // DOMPurify, so nothing set here re-enters the sanitizer's decision. An
  // absolute or `http(s)` source is the author's explicit request for a remote
  // asset and is left exactly as written; a source that REFUSES is left alone
  // too — a broken image is an honest rendering of a broken link, and a
  // placeholder would fabricate.
  function resolveImages(rec, article) {
    if (!window.WBMode?.isDaemon?.() || !window.WBDaemon?.readImage) return;
    const dir = rec.path.includes("/") ? rec.path.slice(0, rec.path.lastIndexOf("/")) : "";
    article.querySelectorAll("img[src]").forEach((img) => {
      const src = img.getAttribute("src") || "";
      // Anything carrying a scheme (`data:`, `https:`) or rooted at `/` is not
      // ours to resolve.
      if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith("/")) return;
      const rel = repoRelative(dir, src);
      if (!rel) return;
      WBDaemon.readImage(rec.project, rel)
        .then((url) => {
          if (url) img.src = url;
        })
        .catch(() => {});
    });
  }

  // A markdown `src` folded against `dir` into a repo-relative path: query and
  // fragment dropped, percent-escapes decoded (a `%20` in a filename is the
  // markdown spelling of a space), `.`/`..` segments resolved. Returns `null`
  // for anything that climbs OUT of the repo — the daemon would refuse it
  // anyway, and not asking is the honest way to spell "not ours".
  function repoRelative(dir, src) {
    let clean = src.split(/[?#]/)[0];
    try {
      clean = decodeURIComponent(clean);
    } catch {
      // A malformed escape is not a path we can resolve; use it verbatim and let
      // the daemon refuse it.
    }
    const out = [];
    for (const part of (dir ? dir.split("/") : []).concat(clean.split("/"))) {
      if (!part || part === ".") continue;
      if (part === "..") {
        if (!out.length) return null;
        out.pop();
        continue;
      }
      out.push(part);
    }
    return out.join("/") || null;
  }

  function drawMermaid(rec) {
    if (!rec.mermaidPending || !rec.mermaidPending.length) return;
    initMermaid();
    const pending = rec.mermaidPending;
    rec.mermaidPending = [];
    pending.forEach((holder) => {
      window.mermaid
        .render(holder.id + "-svg", holder.dataset.src)
        // The fence source is re-read RAW above (DOMPurify escaped it in the
        // markdown pass), so the rendered SVG is the one string on this path that
        // never met the sanitizer. Sanitize on insert. `foreignobject` is already
        // in DOMPurify's SVG allowlist, so mermaid's HTML labels survive.
        .then(({ svg }) => (holder.innerHTML = DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true, html: true } })))
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
      if (!rec.ed) {
        mountEditor(rec, editor, { wordWrap: "on" });
      } else {
        rec.ed.setValue(rec.content);
      }
      toggle.innerHTML = '<i class="bi bi-eye"></i> Preview';
      setTimeout(() => rec.ed?.layout(), 0);
    } else {
      // `rec.editing` is already false here, so read the editor directly —
      // contentOf() would hand back the pre-edit bytes.
      if (rec.ed) rec.content = rec.ed.getValue();
      split.classList.remove("editing");
      editor.style.display = "none";
      toggle.innerHTML = '<i class="bi bi-pencil"></i> Edit';
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
    }
  }

  // --- public API ---------------------------------------------------------
  let uidSeq = 0;
  const API = {
    // `original` is the diff's HEAD side and is read only by `ftype === "diff"`.
    //
    // `project` is the IDENTITY — the tab id, the save/reload wire field, the
    // Monaco model URI — and stays the full ref, routing head and all. `label`
    // is the same thing said to a human; when the caller supplies none (the
    // detached popup of an older shell), the routing head is dropped here so a
    // peer file is never headed by a ULID.
    open({ id, project, label, path, ftype, content, original, detached }) {
      if (map.has(id)) return;
      const shown = label || (window.WBFleet ? window.WBFleet.refSlug(project) : project);
      const rec = { id, project, label: shown, path, kind: ftype, content, original, uid: ++uidSeq, editing: false, visible: false, detached: !!detached };
      map.set(id, rec);
      if (ftype === "markdown") buildMarkdown(rec);
      else if (ftype === "diff") buildDiff(rec);
      else if (ftype === "image") buildImage(rec);
      else buildCode(rec);
    },

    // Show one pane (or none, when the Consoles tab is active). Monaco and
    // mermaid both need a laid-out container, so we (re)paint on first show.
    setActive(id) {
      for (const rec of map.values()) {
        const on = rec.id === id;
        rec.el.style.display = on ? "flex" : "none";
        rec.visible = on;
        if (on) {
          setTimeout(() => rec.ed?.layout(), 0);
          if (rec.kind === "markdown") drawMermaid(rec);
        }
      }
    },

    // The file behind an OPEN pane moved. Re-key the record instead of
    // close+open: reopening refetches the bytes and would discard the
    // operator's unsaved edits, and `open` returns early on a known id, so a
    // naive reopen is a silent no-op. A diff tab is skipped — its id comes from
    // the changes panel, not from this path.
    repath(oldId, { id, path }) {
      const rec = map.get(oldId);
      if (!rec || map.has(id) || rec.kind === "diff") return;
      map.delete(oldId);
      rec.id = id;
      rec.path = path;
      map.set(id, rec);
      if (rec.el) rec.el.dataset.tabId = id;
      const label = rec.el?.querySelector(".viewer-path");
      if (label) label.textContent = `${rec.label} / ${rec.path}`;
    },

    close(id) {
      const rec = map.get(id);
      if (!rec) return;
      // Delete from the map FIRST: a pending mountEditor() checks membership
      // before touching the DOM, so a tab closed mid-boot mounts nothing.
      map.delete(id);
      disposeEditor(rec);
      rec.el.remove();
    },

    // An external write to this file's bytes landed (a directory nudge → re-read).
    // A CLEAN tab auto-refreshes to the fresh bytes (criterion 3); a DIRTY tab
    // stashes them and shows the "changed on disk" badge, NEVER clobbering the
    // operator's unsaved edits (criterion 4). Equal bytes are a no-op (our own
    // save round-trips through the same nudge — a badge there would be noise).
    externalChange(id, content) {
      const rec = map.get(id);
      if (!rec) return;
      // A diff tab never auto-refreshes: it is a two-sided read, and a
      // single-side update would silently misrepresent the comparison.
      if (rec.kind === "diff") return;
      if (content === rec.content) return;
      if (!rec.dirty) {
        applyFresh(rec, content);
      } else {
        rec.pendingDisk = content;
        showDiskBadge(rec);
      }
    },
  };
  window.WBViewer = API;
})();

/* ---------------------------------------------------------------------------
   Seed file contents — used only in the static demo with no backend, so a
   file's "bytes" are synthesised from its name so every viewer feature is
   demonstrable. A daemon-backed build fetches the actual file instead.
--------------------------------------------------------------------------- */
function fakeContent(path, ftype) {
  const base = path.split("/").pop();
  const e = base.toLowerCase().includes(".") ? base.toLowerCase().split(".").pop() : "";
  if (ftype === "markdown") return fakeMarkdown(base);
  if (ftype === "image") return fakeImage(base);
  const gen = {
    ts: fakeTs, tsx: fakeTsx, js: fakeTs, mjs: fakeTs,
    rs: fakeRs, json: fakeJson, css: fakeCss, toml: fakeToml,
    prisma: fakePrisma, py: fakePy,
  }[e];
  return gen ? gen(base) : `// ${path}\n// (demo) source for ${base}\n\nexport const answer = 42;\n`;
}

// The image pane's demo bytes: a placeholder SVG naming the file, so the pane is
// demonstrable with no daemon to read the real one. Returned as a `data:` URL
// because that is exactly what the pane consumes in daemon mode.
function fakeImage(name) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="300">
  <rect width="480" height="300" fill="#14110f"/>
  <rect x="8" y="8" width="464" height="284" fill="none" stroke="#e8d9a8" stroke-dasharray="6 6"/>
  <text x="240" y="140" fill="#e8d9a8" font-family="ui-monospace, monospace" font-size="18" text-anchor="middle">${name}</text>
  <text x="240" y="172" fill="#8a8175" font-family="ui-monospace, monospace" font-size="13" text-anchor="middle">(demo) no daemon — placeholder image</text>
</svg>`;
  return "data:image/svg+xml;base64," + btoa(svg);
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

- Images open in their own pane; other binaries refuse to open.
- Markdown always opens **rendered**, with mermaid support.
- Source files open with syntax highlighting.

## A table

| Kind     | Viewer        | Editable |
| -------- | ------------- | -------- |
| \`.md\`    | rendered      | yes      |
| \`.rs\`    | Monaco        | yes      |
| \`.png\`   | image pane    | no       |
| \`.pdf\`   | (refused)     | no       |

## Code sample

\`\`\`ts
export function greet(name: string) {
  return \`hello, \${name}\`;
}
\`\`\`

> Editing here emits a \`save\` intent — the demo never writes to disk.
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
