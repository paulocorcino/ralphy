/* ---------------------------------------------------------------------------
   ralphy workbench shell — file viewers (the closable tabs)

   Four flavours, each opening as its own tab after the fixed Consoles tab:
     • source code — Monaco: syntax highlight, in-place editing, and its own
       find widget. Binaries never reach here (app.ts refuses them).
     • Markdown  — rendered with `marked`, sanitized with DOMPurify, mermaid
       fences drawn as diagrams (Cursor-style), a heading outline to jump around,
       an in-page find, and an edit/preview toggle over the raw source.
     • image     — an allowlisted image the daemon verified and served as a
       `data:` URL (ADR-0049), fit to the pane or shown 1:1. Read-only.
     • diff      — HEAD against the working tree, side by side. Read-only.

   Editing is allowed but never touches disk: a Save emits a `save` intent on the
   `workbench:action` seam carrying the new content, for a backend to persist.
--------------------------------------------------------------------------- */
import { WBMonaco } from "./wb-monaco.ts";
import { WBSplit } from "./wb-split.ts";
import { WBFail } from "./wb-fail.ts";
import { WBFleet } from "./wb-fleet.ts";
import { sendDocument } from "./wb-events.ts";
import type { MonacoDiffEditor, MonacoDisposable, MonacoEditor } from "./wb-monaco.ts";

/** The vendored mermaid (`vendor/mermaid`): the members this module calls. */
type Mermaid = {
  initialize(config: {
    startOnLoad: boolean;
    securityLevel: string;
    theme: string;
    htmlLabels: boolean;
    flowchart: { htmlLabels: boolean };
  }): void;
  render(id: string, source: string): Promise<{ svg: string }>;
};

type ViewerWindow = Window & { mermaid?: Mermaid };

/**
 * One open file pane. `open` fills the first group; the rest appear as the
 * pane is used. `ed` is a diff editor on a `diff` pane and a code editor on
 * every other.
 */
type ViewerRecord = {
  id: string;
  project: string;
  label: string;
  path: string;
  kind: string;
  content: string;
  original: string | undefined;
  uid: number;
  editing: boolean;
  visible: boolean;
  detached: boolean;
  checkout: string | null;
  encoding: string;
  bom: boolean;
  /** The daemon's reason for serving no bytes, or `null`. */
  refused: string | null;
  el?: HTMLElement;
  ed?: MonacoEditor | MonacoDiffEditor;
  dirty?: boolean;
  mounting?: boolean;
  mountFailed?: boolean;
  fallbackEl?: HTMLElement;
  saveBtn?: HTMLElement | null;
  mirrorBtn?: HTMLElement | null;
  saveKey?: MonacoDisposable;
  ro?: ResizeObserver;
  encMenu?: HTMLElement;
  /** A find asked for while the editor was not mounted or the pane was off screen. */
  pendingFind?: string | null;
  /** The bytes of an external write a dirty pane has not taken yet. */
  pendingDisk?: string;
  mermaidPending?: HTMLElement[];
  hits?: HTMLElement[];
  hitIdx?: number;
};

/** What `open` takes: the fields of a `FileDescriptor` and the reasons a pane may have no bytes. */
type OpenSpec = {
  id: string;
  project: string;
  label?: string;
  path: string;
  ftype: string;
  content: string;
  /** The diff's HEAD side. */
  original?: string;
  detached?: boolean;
  checkout?: string | null;
  encoding?: string;
  bom?: boolean;
  refused?: string | null;
};

/** The second pane the shell resolved (`WBSplit.resolve`). */
type ViewerSlot = { id: string; mirror?: boolean; focus?: boolean; ratio: number | null };

/** The mirror: a second editor over a pane's model, in a `.viewer` of its own. */
type Mirror = {
  rec: ViewerRecord;
  el: HTMLElement;
  ed: MonacoEditor | null;
  ro: ResizeObserver | undefined;
  saveKey: MonacoDisposable | undefined;
};

/** The ENCODINGS rows: the label, the daemon's name, and whether a BOM is written. */
type Encoding = [label: string, name: string, bom: boolean];

export function createViewer(window: ViewerWindow, document: Document) {
  let mermaidReady = false;
  function initMermaid() {
    if (mermaidReady || !window.mermaid) return;
    // `strict`, not `loose`: diagram source is repo bytes (an agent-written plan,
    // a PR fixture, a cloned README), i.e. untrusted. `loose` skips mermaid's
    // own sanitize pass over the emitted SVG and turns a `click A "javascript:…"`
    // directive into a live <a href>. Nothing here calls `bindFunctions`, so
    // click bindings were never wired up and `strict` costs no working feature.
    // `htmlLabels: false` is LOAD-BEARING, not a style (found while building
    // ADR-0064 §15): mermaid's default label is HTML inside a
    // `<foreignObject>`, and DOMPurify 3.4 dropped that tag from its SVG
    // allowlist — so `drawMermaid`'s sanitize pass below removed every label
    // and the diagram arrived as unlabelled boxes. MEASURED against 3.4.12.
    // Plain `<text>` labels survive it untouched.
    window.mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "dark",
      htmlLabels: false,
      flowchart: { htmlLabels: false },
    });
    mermaidReady = true;
  }

  const viewers = document.getElementById("viewers");
  const map = new Map<string, ViewerRecord>(); // tab id → viewer record

  // Monaco boots through an AMD loader, so an editor is created
  // asynchronously. INVARIANTS across that gap: `rec.content` is the single
  // source of truth until `rec.ed` exists, and a tab closed mid-boot must
  // never mount an orphan editor. Liveness is `map.get(rec.id) === rec`, NOT
  // `has`: tab ids are stable per file, so a close+reopen inside the boot
  // window puts a DIFFERENT record under the same key.
  const alive = (rec: ViewerRecord) => map.get(rec.id) === rec;

  // --- the secondary pane: the slot (ADR-0037 §3c) ---------------------------
  // What is on screen: the active pane and, beside it, the slot the shell
  // resolved (`WBSplit.resolve`) — a pinned pane's id, or the active pane's
  // own id with `mirror` set. The decision is the shell's; this module only
  // paints it, and paints SINGLE whenever the slot's pane is not here yet (a
  // reload restores the slot while its bytes are in flight) — the next
  // `setActive`/`refresh` converges, the same rule the shell's late opener
  // follows.
  let shown: { id: string | null; slot: ViewerSlot | null } = { id: null, slot: null };
  // The mirror: a second Monaco editor over the active pane's model, in a
  // sibling `.viewer` of its own so the grid, the toolbar and the narrow-pane
  // container query all see one more pane. At most one, ever. It owns its
  // EDITOR only — the model is the mirrored pane's, and `disposeEditor` closes
  // the mirror before the model on every path.
  let mirror: Mirror | null = null;
  let divider: HTMLElement | null = null;

  function refresh() {
    paint();
  }

  // Monaco and mermaid both need a laid-out container, so a pane is
  // (re)painted on every show.
  function paint() {
    const { id, slot } = shown;
    const slotRec = slot ? map.get(slot.id) : null;
    // A mirror needs an editor to mirror; a pin needs its pane open.
    const split = !!slotRec && (!slot!.mirror || (slotRec.kind === "code" && !!slotRec.ed));
    for (const rec of map.values()) {
      const on = rec.id === id || (split && rec.id === slot!.id);
      rec.el!.style.display = on ? "flex" : "none";
      rec.el!.style.gridColumn = on && split ? (rec.id === id ? "1" : "3") : "";
      rec.visible = on;
      if (on) {
        setTimeout(() => {
          rec.ed?.layout();
          // A find asked for while the pane was off screen (see findInEditor).
          if (rec.pendingFind && rec.ed) {
            const term: string = rec.pendingFind;
            rec.pendingFind = null;
            findInEditor(rec, term);
          }
        }, 0);
        if (rec.kind === "markdown") drawMermaid(rec);
      }
    }
    if (!viewers) return;
    viewers.classList.toggle("split", split);
    if (!split) {
      // Nothing on screen (Consoles, Spend): the mirror is HIDDEN like every
      // other pane, keeping its own scroll and cursor for the return — that
      // independence is what the second editor is for. Any other single
      // paint is a slot that was closed or moved: the mirror goes.
      if (id === null && mirror && alive(mirror.rec)) mirror.el.style.display = "none";
      else closeMirror();
      return;
    }
    ensureDivider();
    viewers.style.setProperty("--wb-split", ratioPct(slot!.ratio));
    if (slot!.mirror) ensureMirror(slotRec!);
    else closeMirror();
    if (slot!.focus) {
      const target = slot!.mirror ? mirror?.ed : (slotRec!.ed as MonacoEditor | undefined);
      setTimeout(() => target?.focus(), 0);
    }
  }

  const ratioPct = (ratio: number | null) => `${((Number.isFinite(ratio) ? ratio! : 0.5) * 100).toFixed(2)}%`;

  function ensureMirror(rec: ViewerRecord) {
    if (mirror?.rec === rec) {
      mirror.el.style.display = "flex";
      return;
    }
    closeMirror();
    const el = document.createElement("div");
    el.className = "viewer code-viewer mirror-viewer";
    el.dataset.mirrorOf = rec.id;
    // `.viewer` takes its `display` inline (paint sets flex/none per pane):
    // without it the body has no flex to fill and Monaco mounts 0px tall.
    el.style.display = "flex";
    el.style.gridColumn = "3";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="viewer-mirror-tag">Mirror</span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="mirror" title="Close mirror" aria-label="Close mirror"><i class="bi bi-x-lg"></i><span class="vbtn-label">Close mirror</span></button>
      </div>
      <div class="viewer-body"></div>`;
    setPathLabel(el, rec);
    el.querySelector<HTMLElement>('[data-act="mirror"]')!.onclick = () => window.getShell?.()?.toggleMirror?.();
    viewers!.append(el);
    const holder: Mirror = { rec, el, ed: null, ro: undefined, saveKey: undefined };
    const ed = WBMonaco.createOver(el.querySelector<HTMLElement>(".viewer-body")!, (rec.ed as MonacoEditor).getModel()!, {
      narrow: isNarrow(el),
    });
    // No content listener: the model is shared, so the pane's own listener
    // already marks it dirty for an edit made here.
    holder.saveKey = bindSave(ed, rec);
    watchNarrow(holder, ed);
    holder.ed = ed;
    mirror = holder;
    if (rec.mirrorBtn) setCaption(rec.mirrorBtn, "bi-x-lg", "Close mirror");
  }

  function closeMirror() {
    if (!mirror) return;
    const { rec, el, ed, ro, saveKey } = mirror;
    mirror = null;
    ro?.disconnect();
    saveKey?.dispose();
    ed?.dispose();
    el.remove();
    if (rec.mirrorBtn) setCaption(rec.mirrorBtn, "bi-files", "Mirror");
  }

  // The divider between the two columns: a drag moves the split, the × clears
  // the slot. Pointer-driven the way the desk's resize bands are
  // (`wb-console.ts`): every exit path drops all three listeners, and the
  // ratio is announced ONCE, on release, for the shell to keep.
  function ensureDivider() {
    if (divider) return;
    divider = document.createElement("div");
    divider.className = "viewers-divider";
    divider.innerHTML =
      '<button class="slot-close" title="Close the second pane" aria-label="Close the second pane"><i class="bi bi-x-lg"></i></button>';
    divider.querySelector<HTMLElement>(".slot-close")!.onclick = () => window.getShell?.()?.clearSlot?.();
    divider.addEventListener("pointerdown", (e: PointerEvent) => {
      if (e.button !== 0 || !e.isPrimary || (e.target as Element).closest(".slot-close")) return;
      e.preventDefault();
      const pointerId = e.pointerId;
      const box = viewers!.getBoundingClientRect();
      let ratio: number | null = null;
      const onMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return;
        ratio = WBSplit.clampRatio(ev.clientX - box.left, box.width);
        viewers!.style.setProperty("--wb-split", ratioPct(ratio));
      };
      const onUp = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("pointercancel", onUp);
        if (ratio != null)
          sendDocument(document, "workbench:split-ratio", { ratio });
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
    });
    viewers!.append(divider);
  }

  // The canvas crossing the split's width floor is the shell's to re-decide:
  // it holds the slot, this module only paints what it was last told.
  if (viewers && typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      sendDocument(document, "workbench:canvas-resize", { width: viewers.clientWidth });
    }).observe(viewers);
  }

  function mountEditor(rec: ViewerRecord, container: HTMLElement, opts: { path?: string; wordWrap?: string }) {
    const path = (opts && opts.path) || rec.path;
    if (rec.mounting || rec.mountFailed) return Promise.resolve();
    rec.mounting = true;
    return WBMonaco.ready()
      .catch((err) => {
        // The AMD loader did not boot — degrade to read-only bytes rather
        // than leave an empty pane (#308).
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
          path: path!,
          uid: rec.uid,
          project: rec.project,
          wordWrap: opts && opts.wordWrap,
          narrow: isNarrow(rec.el),
        });
        watchNarrow(rec, ed);
        ed.onDidChangeModelContent(() => {
          rec.dirty = true;
          rec.saveBtn?.classList.add("dirty");
        });
        rec.saveKey = bindSave(ed, rec);
        // Assigned LAST: a throw while wiring must not leave a half-live editor
        // that the rest of the module would treat as ready.
        rec.ed = ed;
        if (rec.visible) ed.layout();
        // A content-search open asked for a term before there was an editor
        // to ask; now there is one.
        if (rec.pendingFind) {
          const term: string = rec.pendingFind;
          rec.pendingFind = null;
          findInEditor(rec, term);
        }
        // A mirror asked for before there was an editor to mirror (a reload
        // restores the slot while the bytes are still in flight): paint now.
        if (shown.slot?.mirror && shown.slot.id === rec.id) refresh();
      })
      .catch((err) => {
        // A create()/wiring failure is NOT a boot failure: the pane would look
        // editable while nothing is wired, so say so instead of degrading.
        rec.mounting = false;
        console.error("[workbench] monaco editor failed to mount", err);
        window.getShell?.()?._flashAction?.("Could not open the editor.");
      });
  }

  // The pane-width criterion, the SAME number as the stylesheet's
  // `@container viewer (max-width: 560px)`: the gutter trims exactly when the
  // captions fold to icons. Monaco takes it as options, not CSS, so it is
  // measured here and re-measured when the pane's box changes — only a
  // threshold CROSSING reaches `updateOptions`; a resize on the same side of
  // it is Monaco's own `automaticLayout` business. The observer is the pane's,
  // not the editor's: `disposeEditor` ends it with the editor it feeds.
  const NARROW_PX = 560;
  const isNarrow = (el: HTMLElement | undefined) => !!el && el.clientWidth > 0 && el.clientWidth <= NARROW_PX;
  function watchNarrow(rec: { el?: HTMLElement; ro?: ResizeObserver }, ed: MonacoEditor | MonacoDiffEditor) {
    if (!rec.el || typeof ResizeObserver !== "function") return;
    let narrow = isNarrow(rec.el);
    rec.ro = new ResizeObserver(() => {
      const now = isNarrow(rec.el);
      if (now === narrow) return;
      narrow = now;
      ed.updateOptions(WBMonaco.gutterOptions(now));
    });
    rec.ro.observe(rec.el);
  }

  // Ctrl+S on ONE editor. `addAction` is scoped to the editor it is added to
  // (Monaco ANDs `editorId == <this editor>` into the precondition);
  // `addCommand` is not — it is a page-wide keybinding on the shared standalone
  // service, so with two editors on screen the last one registered would take
  // every Ctrl+S. The disposable is kept: Monaco does not tie it to the editor's
  // own lifetime, and a binding that outlives its editor holds `rec` forever.
  function bindSave(ed: MonacoEditor, rec: ViewerRecord) {
    const monaco = window.monaco;
    return ed.addAction({
      id: "wb.save",
      label: "Save",
      keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS],
      run: () => save(rec),
    });
  }

  function disposeEditor(rec: ViewerRecord) {
    // The mirror sits over THIS pane's model: its editor goes before the model
    // does, on every path — an editor over a disposed model throws on render.
    if (mirror?.rec === rec) closeMirror();
    rec.ro?.disconnect();
    rec.ro = undefined;
    rec.saveKey?.dispose();
    rec.saveKey = undefined;
    if (!rec.ed) return;
    if (rec.kind === "diff") {
      // A diff editor holds TWO models and BOTH must be disposed on EVERY
      // path — disposing the editor does NOT (the leak is only visible in
      // `monaco.editor.getModels()`, which wb_diff_311.py counts). The EDITOR
      // goes first: a model disposed while attached raises a page error (#407).
      const m = (rec.ed as MonacoDiffEditor).getModel();
      rec.ed.dispose();
      m?.original?.dispose();
      m?.modified?.dispose();
    } else {
      (rec.ed as MonacoEditor).getModel()?.dispose();
      rec.ed.dispose();
    }
    rec.ed = undefined;
  }

  // --- a diff tab (read-only, two-sided) ----------------------------------
  function mountDiff(rec: ViewerRecord, container: HTMLElement) {
    if (rec.mounting || rec.mountFailed) return Promise.resolve();
    rec.mounting = true;
    return WBMonaco.ready()
      .then((monaco) => {
        rec.mounting = false;
        // Same record-identity liveness rule as mountEditor: a diff tab closed
        // inside the boot window must mount nothing on its detached container.
        if (!monaco || !alive(rec)) return;
        const ed = WBMonaco.createDiff(container, {
          original: rec.original!,
          modified: rec.content,
          path: rec.path,
          uid: rec.uid,
          project: rec.project,
          narrow: isNarrow(rec.el),
        });
        rec.ed = ed;
        watchNarrow(rec, ed);
        if (rec.visible) ed.layout();
      })
      .catch((err) => {
        // No `<pre>` degrade here: two texts side by side have no honest
        // single-pane fallback, and one of them rendered alone would read as
        // "no changes". Say it failed and close the tab (#311).
        rec.mounting = false;
        rec.mountFailed = true;
        console.error("[workbench] monaco diff failed to mount", err);
        window.getShell?.()?._flashAction?.("Could not open the editor.");
        window.getShell?.()?.closeTab(rec.id);
      });
  }

  function buildDiff(rec: ViewerRecord) {
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
        <button class="vbtn" data-act="find" title="Find" aria-label="Find"><i class="bi bi-search"></i><span class="vbtn-label">Find</span></button>
      </div>
      <div class="viewer-body"></div>`;
    setPathLabel(el, rec);
    viewers!.append(el);

    el.querySelector<HTMLElement>('[data-act="find"]')!.onclick = () => {
      const mod = (rec.ed as MonacoDiffEditor | undefined)?.getModifiedEditor();
      mod?.focus();
      mod?.getAction("actions.find")?.run();
    };
    rec.el = el;
    mountDiff(rec, el.querySelector<HTMLElement>(".viewer-body")!);
  }

  // --- a source-code editor tab ------------------------------------------
  function buildCode(rec: ViewerRecord) {
    const el = document.createElement("div");
    el.className = "viewer code-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="find" title="Find" aria-label="Find"><i class="bi bi-search"></i><span class="vbtn-label">Find</span></button>
        ${mirrorBtnHtml(rec)}
        <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
        <button class="vbtn viewer-disk-badge" data-act="disk" style="display:none" title="Changed on disk — reload" aria-label="Changed on disk — reload"><i class="bi bi-exclamation-triangle"></i><span class="vbtn-label">Changed on disk — reload</span></button>
        <span class="viewer-save-err" style="display:none"></span>
        ${encodingBtnHtml(rec)}
        <button class="vbtn save" data-act="save" title="Save" aria-label="Save"><i class="bi bi-save"></i><span class="vbtn-label">Save</span></button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="viewer-body"></div>`;
    setPathLabel(el, rec);
    viewers!.append(el);
    wireEncodingMenu(rec, el);

    const saveBtn = el.querySelector<HTMLElement>('[data-act="save"]')!;
    el.querySelector<HTMLElement>('[data-act="find"]')!.onclick = () => {
      (rec.ed as MonacoEditor | undefined)?.focus();
      (rec.ed as MonacoEditor | undefined)?.getAction("actions.find")?.run();
    };
    const mirrorBtn = el.querySelector<HTMLElement>('[data-act="mirror"]');
    if (mirrorBtn) mirrorBtn.onclick = () => window.getShell?.()?.toggleMirror?.();
    saveBtn.onclick = () => save(rec);
    el.querySelector<HTMLElement>('[data-act="reload"]')!.onclick = () => reloadFile(rec);
    el.querySelector<HTMLElement>('[data-act="disk"]')!.onclick = () => {
      applyFresh(rec, rec.pendingDisk!);
      hideDiskBadge(rec);
    };
    el.querySelector<HTMLElement>('[data-act="detach"]')!.onclick = () => detachClick(rec);
    rec.el = el;
    rec.saveBtn = saveBtn;
    rec.mirrorBtn = mirrorBtn;
    mountEditor(rec, el.querySelector<HTMLElement>(".viewer-body")!, {});
  }

  function save(rec: ViewerRecord) {
    // A diff and an image are read-only: a `save` intent from either would be a
    // mutation those surfaces forbid, so it never reaches the action seam. (An
    // image's `content` is a `data:` URL, not the file's bytes — saving it would
    // write the URL over the image.) A refused pane has no bytes to save.
    if (rec.kind === "diff" || rec.kind === "image" || rec.refused) return;
    const content = contentOf(rec);
    rec.content = content;
    hideDiskBadge(rec);
    clearSaveError(rec);
    // The dirty mark is cleared by `saveDone`, on the daemon's ack — not here:
    // a refused write (`unencodable`, a denylisted path) must leave the pane
    // looking unsaved, because it is.
    // `checkout` is the tab's PIN (#406): the write goes to the tree the bytes
    // came from, whatever the project's selection is by now. `encoding`/`bom`
    // are what the read reported (or a "save with…" chose), so the bytes go
    // back the way they came (ADR-0036 amendment 2026-09-22).
    window.WB.emit("save", {
      project: rec.project,
      path: rec.path,
      bytes: content.length,
      content,
      checkout: rec.checkout,
      encoding: rec.encoding,
      bom: !!rec.bom,
    });
    if (rec.kind === "markdown" && !rec.editing) renderMarkdown(rec); // keep preview fresh
  }

  // The inline "not saved" line beside the Save button: a write refusal must
  // be read from the pane it concerns, not from a flash in another panel.
  function showSaveError(rec: ViewerRecord, text: string) {
    const el = rec.el?.querySelector<HTMLElement>(".viewer-save-err");
    if (!el) return;
    el.textContent = text;
    el.style.display = "";
  }
  function clearSaveError(rec: ViewerRecord) {
    const el = rec.el?.querySelector<HTMLElement>(".viewer-save-err");
    if (el) el.style.display = "none";
  }

  // The pane's current bytes, whether shown as source (Monaco) or as a rendered
  // markdown preview. Before Monaco finishes booting `rec.ed` is undefined and
  // `rec.content` is still authoritative.
  function contentOf(rec: ViewerRecord) {
    if (rec.kind === "diff") return rec.content;
    if (rec.ed && (rec.kind === "code" || rec.editing)) return (rec.ed as MonacoEditor).getValue();
    return rec.content;
  }

  // A portable descriptor — enough to reopen this file anywhere (a tab or a
  // detached popup), carrying the *current* (possibly edited) content.
  // The pin rides the descriptor (#406): a detached pane saves and reloads
  // against the tree its bytes came from, and re-attaches pinned to it.
  function descOf(rec: ViewerRecord): FileDescriptor {
    return {
      project: rec.project,
      label: rec.label,
      path: rec.path,
      ftype: rec.kind,
      content: contentOf(rec),
      checkout: rec.checkout ?? null,
      encoding: rec.encoding,
      bom: !!rec.bom,
    };
  }

  // Reload discards local edits and reloads from source. Daemon-backed repos
  // re-read the REAL file via `file.read` (#197). The apply step is shared via
  // `applyFresh`. With no daemon client loaded (a unit test) it does nothing.
  function reloadFile(rec: ViewerRecord) {
    const daemonBacked = !!window.WBDaemon?.observe;
    if (daemonBacked) {
      // Daemon mode: a non-ok reply or a transport drop must NOT regenerate
      // synthetic bytes (C1). The tab stays — the operator's bytes are still
      // the best answer the pane has — and the reason lands in the pane.
      const fail = (reply?: DaemonReply) => {
        showSaveError(rec, WBFail.failed(reply, "Could not reload the file: the daemon gave no reason."));
        window.getShell?.()?._flashAction?.("Could not reload the file.");
      };
      // An image reloads through its own verb (ADR-0049): `file.read` refuses
      // its bytes, so routing it here would turn every image Reload into a
      // "reload failed".
      if (rec.kind === "image") {
        WBDaemon.readImage(rec.project, rec.path, undefined, rec.checkout)
          .then((url) => (url ? applyFresh(rec, url) : fail()))
          .catch(() => fail());
        return;
      }
      readWith(rec, rec.encoding)
        .then((reply) => {
          if (!reply || reply.status !== "ok") return fail(reply);
          rec.bom = !!reply.bom;
          if (reply.encoding) rec.encoding = reply.encoding;
          refreshEncodingPill(rec);
          if (rec.refused) return reopenRefused(rec, reply);
          applyFresh(rec, reply.content!);
        })
        .catch(() => fail());
    }
  }

  // `file.read` for this pane, with `encoding` as the hint: a tab's encoding is
  // sticky once opened (the daemon's detection, or a "reopen with…"), so a
  // reload never silently re-detects it.
  function readWith(rec: ViewerRecord, encoding: string | undefined) {
    const payload = WBDaemon.withCheckout({ repo: rec.project, path: rec.path }, rec.checkout);
    if (encoding) payload.encoding = encoding;
    return WBDaemon.observe("file.read", payload);
  }

  // A pane that opened refused now has bytes: rebuild it as the pane its
  // kind deserves. `open` returns early on a known id, so the record is
  // replaced under the same id — the tab in the shell is untouched.
  function reopenRefused(rec: ViewerRecord, reply: FileReadReply) {
    const desc = { ...descOf(rec), content: reply.content!, encoding: reply.encoding, bom: !!reply.bom };
    API.close(rec.id);
    API.open({ id: rec.id, ...desc, detached: rec.detached });
    if (rec.visible) refresh();
    window.WB.emit("reload", { project: rec.project, path: rec.path });
  }

  function applyFresh(rec: ViewerRecord, fresh: string) {
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
      const img = rec.el?.querySelector<HTMLImageElement>(".img-canvas");
      if (img) img.src = fresh;
    } else if (rec.kind === "code" || rec.editing) {
      if (rec.ed) (rec.ed as MonacoEditor).setValue(fresh);
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
    window.WB.emit("reload", { project: rec.project, path: rec.path });
  }

  // The "changed on disk" badge: shown when an EXTERNAL write lands on a DIRTY
  // tab (never auto-applied — the operator's unsaved edits win until they click).
  function showDiskBadge(rec: ViewerRecord) {
    const b = rec.el?.querySelector<HTMLElement>(".viewer-disk-badge");
    if (b) b.style.display = "";
  }
  function hideDiskBadge(rec: ViewerRecord) {
    rec.pendingDisk = undefined;
    const b = rec.el?.querySelector<HTMLElement>(".viewer-disk-badge");
    if (b) b.style.display = "none";
  }

  // The Detach/Re-attach button. A file tab detaches into a standalone popup
  // (watch an agent in the main window, read the file in another); a detached
  // pane folds back in. wb-viewer only *requests* it — the shell (app.ts) opens
  // the popup, and the popup (detached.html) folds back — so this module stays
  // agnostic to windows/tabs.
  // --- the encoding pill and its menu ---------------------------------------
  // What the pane will save with, as the daemon named it (ADR-0036 amendment
  // 2026-09-22), and the two things an operator can do about it: REOPEN the
  // same bytes under another encoding (the daemon re-decodes; the operator
  // judges by the glyphs), or SAVE the text under another one (a deliberate
  // conversion — the daemon never converts on its own).
  const ENCODINGS: Encoding[] = [
    ["UTF-8", "utf-8", false],
    ["UTF-8 BOM", "utf-8", true],
    ["UTF-16 LE", "utf-16le", true],
    ["UTF-16 BE", "utf-16be", true],
    ["Windows-1252", "windows-1252", false],
    ["ISO-8859-2", "iso-8859-2", false],
    ["ISO-8859-15", "iso-8859-15", false],
    ["Windows-1250", "windows-1250", false],
    ["Windows-1251", "windows-1251", false],
    ["KOI8-R", "koi8-r", false],
    ["Shift_JIS", "shift_jis", false],
    ["EUC-JP", "euc-jp", false],
    ["GBK", "gbk", false],
    ["Big5", "big5", false],
    ["EUC-KR", "euc-kr", false],
  ];

  // The pill's text: the daemon's canonical name made readable (`UTF-16LE`
  // → `UTF-16 LE`, `windows-1252` → `Windows-1252`), plus ` BOM` when one
  // was read and will be written back.
  function encodingLabel(name: string | null | undefined, bom: boolean) {
    let text = String(name || "UTF-8");
    text = text.replace(/^utf-16(le|be)$/i, (_, e: string) => `UTF-16 ${e.toUpperCase()}`);
    text = text.replace(/^windows-/i, "Windows-");
    return bom ? `${text} BOM` : text;
  }

  function encodingBtnHtml(rec: ViewerRecord) {
    return `<button class="vbtn viewer-enc" data-act="encoding" title="Encoding — reopen or save with another" aria-label="Encoding"><span class="viewer-enc-label">${encodingLabel(rec.encoding, rec.bom)}</span></button>`;
  }

  function refreshEncodingPill(rec: ViewerRecord) {
    const label = rec.el?.querySelector<HTMLElement>(".viewer-enc-label");
    if (label) label.textContent = encodingLabel(rec.encoding, rec.bom);
  }

  function wireEncodingMenu(rec: ViewerRecord, el: HTMLElement) {
    const btn = el.querySelector<HTMLElement>('[data-act="encoding"]');
    if (!btn) return;
    const menu = document.createElement("div");
    menu.className = "enc-menu";
    menu.style.display = "none";
    const group = (head: string, act: string) =>
      `<div class="dropdown-head">${head}</div>` +
      ENCODINGS.map(
        ([label, name, bom], i) =>
          `<button class="enc-item" data-${act}="${i}"><span>${label}</span></button>`,
      ).join("");
    menu.innerHTML = group("Reopen with", "reopen") + group("Save with", "savewith");
    el.append(menu);
    rec.encMenu = menu;
    const close = () => {
      menu.style.display = "none";
      document.removeEventListener("click", onOutside, true);
      document.removeEventListener("keydown", onKey, true);
    };
    const onOutside = (ev: MouseEvent) => {
      if (!menu.contains(ev.target as Node) && ev.target !== btn) close();
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") close();
    };
    btn.onclick = (ev: MouseEvent) => {
      ev.stopPropagation();
      if (menu.style.display !== "none") return close();
      markCurrentEncoding(rec, menu);
      menu.style.display = "";
      document.addEventListener("click", onOutside, true);
      document.addEventListener("keydown", onKey, true);
    };
    menu.onclick = (ev: MouseEvent) => {
      const item = (ev.target as Element).closest?.<HTMLElement>(".enc-item");
      if (!item) return;
      ev.stopPropagation();
      close();
      if (item.dataset.reopen != null) reopenWith(rec, ENCODINGS[Number(item.dataset.reopen)]);
      else if (item.dataset.savewith != null) saveWith(rec, ENCODINGS[Number(item.dataset.savewith)]);
    };
  }

  function markCurrentEncoding(rec: ViewerRecord, menu: HTMLElement) {
    for (const item of menu.querySelectorAll<HTMLElement>(".enc-item")) {
      const i = Number(item.dataset.reopen ?? item.dataset.savewith);
      const [, name, bom] = ENCODINGS[i];
      const current =
        String(rec.encoding).toLowerCase() === name && !!rec.bom === bom;
      item.classList.toggle("current", current);
    }
  }

  // Reopen: the same bytes, decoded as `name`. Unsaved edits would be lost
  // to the re-read, so a dirty pane asks first.
  function reopenWith(rec: ViewerRecord, [label, name]: Encoding) {
    const shell = window.getShell?.();
    const go = () => {
      readWith(rec, name)
        .then((reply) => {
          if (!reply || reply.status !== "ok") {
            showSaveError(rec, WBFail.failed(reply, `Could not reopen as ${label}: the daemon gave no reason.`));
            return;
          }
          rec.encoding = reply.encoding || name;
          rec.bom = !!reply.bom;
          refreshEncodingPill(rec);
          if (rec.refused) return reopenRefused(rec, reply);
          applyFresh(rec, reply.content!);
        })
        .catch(() => showSaveError(rec, `Could not reopen as ${label}: the daemon did not answer.`));
    };
    if (!rec.dirty) return go();
    // The design-system dialog where there is a shell; `window.confirm` only
    // in a detached popup, which has no shell and no other dialog.
    const ask = shell?.askConfirm
      ? shell.askConfirm({
          title: `Reopen as ${label}?`,
          message: "Unsaved changes in this tab are discarded.",
          confirmLabel: "Reopen",
          danger: true,
        })
      : Promise.resolve(window.confirm(`Reopen as ${label}? Unsaved changes are discarded.`));
    ask.then((ok) => ok && go());
  }

  // Save with: the text as it is, written under `name` — the conversion the
  // operator asked for, by name.
  function saveWith(rec: ViewerRecord, [, name, bom]: Encoding) {
    rec.encoding = name;
    rec.bom = bom;
    refreshEncodingPill(rec);
    save(rec);
  }

  function detachBtnHtml(rec: ViewerRecord) {
    return rec.detached
      ? '<button class="vbtn" data-act="detach" title="Re-attach" aria-label="Re-attach"><i class="bi bi-box-arrow-in-down-left"></i><span class="vbtn-label">Re-attach</span></button>'
      : '<button class="vbtn" data-act="detach" title="Detach" aria-label="Detach"><i class="bi bi-box-arrow-up-right"></i><span class="vbtn-label">Detach</span></button>';
  }

  // The mirror toggle (ADR-0037 §3c) is the attached canvas's: a detached
  // popup has one pane and no slot to put a second editor in.
  function mirrorBtnHtml(rec: ViewerRecord) {
    return rec.detached
      ? ""
      : '<button class="vbtn" data-act="mirror" title="Mirror" aria-label="Mirror"><i class="bi bi-files"></i><span class="vbtn-label">Mirror</span></button>';
  }

  // What the toolbar says a pane IS: the path only (the tab and the sidebar
  // already name the file and the repo). A DETACHED pane keeps the full label;
  // the full form always rides the `title`. `dir` / `file` are split so the
  // CSS ellipsises the directory first.
  function pathLabel(rec: ViewerRecord) {
    const suffix = rec.kind === "diff" ? " ↔ HEAD" : "";
    const full = `${rec.label} / ${rec.path}${suffix}`;
    const cut = rec.path.lastIndexOf("/") + 1;
    const head = rec.detached ? `${rec.label} / ` : "";
    return { dir: head + rec.path.slice(0, cut), file: rec.path.slice(cut) + suffix, full };
  }

  function setPathLabel(el: HTMLElement, rec: ViewerRecord) {
    const span = el.querySelector<HTMLElement>(".viewer-path")!;
    const { dir, file, full } = pathLabel(rec);
    span.textContent = "";
    const d = document.createElement("span");
    d.className = "viewer-dir";
    d.textContent = dir;
    const f = document.createElement("span");
    f.className = "viewer-file";
    f.textContent = file;
    span.append(d, f);
    span.title = full;
  }

  // A caption swap keeps the button's shape: icon, then the label span a narrow
  // pane hides, and the same words in `title`/`aria-label` for when it does.
  function setCaption(btn: HTMLElement, icon: string, caption: string) {
    btn.innerHTML = `<i class="bi ${icon}"></i><span class="vbtn-label"></span>`;
    btn.querySelector(".vbtn-label")!.textContent = caption;
    btn.title = caption;
    btn.setAttribute("aria-label", caption);
  }

  function detachClick(rec: ViewerRecord) {
    const evt = rec.detached ? "workbench:reattach-request" : "workbench:detach-request";
    sendDocument(document, evt, descOf(rec));
  }

  // --- an image tab (read-only) -------------------------------------------
  // `rec.content` is a `data:` URL the daemon's verified media type built
  // (ADR-0049 §2), so this pane never decides what bytes are. Read-only: no
  // Save, no Edit — the Write class is untouched by images.
  function buildImage(rec: ViewerRecord) {
    const el = document.createElement("div");
    el.className = "viewer image-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="img-meta"></span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="zoom" title="Actual size" aria-label="Actual size"><i class="bi bi-arrows-angle-expand"></i><span class="vbtn-label">Actual size</span></button>
        <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="viewer-body img-scroll"><img class="img-canvas" alt="" /></div>`;
    setPathLabel(el, rec);
    viewers!.append(el);

    const img = el.querySelector<HTMLImageElement>(".img-canvas")!;
    const meta = el.querySelector(".img-meta")!;
    // The intrinsic size is only known once the bytes decode, and a decode
    // failure is worth saying out loud: the daemon verified the type, so a
    // browser that still cannot paint it means an unsupported/corrupt file.
    img.onload = () => (meta.textContent = `${img.naturalWidth} × ${img.naturalHeight}`);
    img.onerror = () => (meta.textContent = "Image could not be displayed.");
    img.src = rec.content;

    el.querySelector<HTMLElement>('[data-act="zoom"]')!.onclick = (ev: MouseEvent) => {
      // Two states only: fit-to-pane (default) and 1:1 with scrollbars. A zoom
      // slider is a feature this pane does not need to read a screenshot.
      const actual = el.classList.toggle("actual-size");
      setCaption(ev.currentTarget as HTMLElement, actual ? "bi-arrows-angle-contract" : "bi-arrows-angle-expand", actual ? "Fit" : "Actual size");
    };
    el.querySelector<HTMLElement>('[data-act="reload"]')!.onclick = () => reloadFile(rec);
    el.querySelector<HTMLElement>('[data-act="detach"]')!.onclick = () => detachClick(rec);
    rec.el = el;
  }

  // --- a refused file: the tab stays and says why -------------------------
  // What the daemon would not serve as text (`binary`, `too large`, an
  // encoding it cannot name) used to close the tab under the click with a
  // flash nobody saw. The tab now holds its place, names the reason in the
  // pane, and Reload retries (the file may have been converted meanwhile).
  const REFUSAL_TEXT: Record<string, string> = {
    binary: "This file is binary and cannot be shown as text.",
    "too large": "This file is larger than 2 MiB. The workbench shows files up to 2 MiB.",
    "not an image": "This file is not an image the workbench can display.",
    "unknown encoding": "That encoding is not one the workbench knows.",
    unencodable: "This file cannot be decoded with that encoding.",
  };
  function refusalText(reason: string) {
    if (REFUSAL_TEXT[reason]) return REFUSAL_TEXT[reason];
    // `detached.html` loads no `wb-fail.ts`.
    return WBFail.failed({ message: reason }, "Could not open the file: the daemon gave no reason.");
  }

  function buildRefused(rec: ViewerRecord) {
    const el = document.createElement("div");
    el.className = "viewer refused-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="spacer"></span>
        <span class="viewer-save-err" style="display:none"></span>
        <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
      </div>
      <div class="viewer-body refused-body">
        <i class="bi bi-file-earmark-x"></i>
        <p class="refused-text"></p>
        <p class="refused-hint"></p>
      </div>`;
    setPathLabel(el, rec);
    el.querySelector(".refused-text")!.textContent = refusalText(rec.refused!);
    if (rec.refused === "binary") {
      el.querySelector(".refused-hint")!.textContent =
        "This file may be text in an older encoding. Set the fallback encoding in “Settings”, or reopen the file with another encoding.";
    }
    viewers!.append(el);
    el.querySelector<HTMLElement>('[data-act="reload"]')!.onclick = () => reloadFile(rec);
    rec.el = el;
  }

  // --- a Markdown tab -----------------------------------------------------
  function buildMarkdown(rec: ViewerRecord) {
    const el = document.createElement("div");
    el.className = "viewer md-viewer";
    el.dataset.tabId = rec.id;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <button class="vbtn md-toc-btn" data-act="outline" title="Contents" aria-label="Contents"><i class="bi bi-list-ul"></i></button>
        <span class="spacer"></span>
        <button class="vbtn" data-act="find" title="Find" aria-label="Find"><i class="bi bi-search"></i><span class="vbtn-label">Find</span></button>
        <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
        <button class="vbtn" data-act="toggle" title="Edit" aria-label="Edit"><i class="bi bi-pencil"></i><span class="vbtn-label">Edit</span></button>
        <button class="vbtn viewer-disk-badge" data-act="disk" style="display:none" title="Changed on disk — reload" aria-label="Changed on disk — reload"><i class="bi bi-exclamation-triangle"></i><span class="vbtn-label">Changed on disk — reload</span></button>
        <span class="viewer-save-err" style="display:none"></span>
        ${encodingBtnHtml(rec)}
        <button class="vbtn save" data-act="save" title="Save" aria-label="Save"><i class="bi bi-save"></i><span class="vbtn-label">Save</span></button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="md-find">
        <input class="md-find-input" placeholder="Find in page…" />
        <span class="md-find-count"></span>
        <button class="vbtn" data-find="prev" title="Previous match" aria-label="Previous match"><i class="bi bi-chevron-up"></i></button>
        <button class="vbtn" data-find="next" title="Next match" aria-label="Next match"><i class="bi bi-chevron-down"></i></button>
        <button class="vbtn" data-find="close" title="Close search" aria-label="Close search"><i class="bi bi-x"></i></button>
      </div>
      <div class="md-split">
        <nav class="md-outline"></nav>
        <div class="md-scroll"><article class="md-body"></article></div>
        <div class="md-editor" style="display:none"></div>
      </div>`;
    setPathLabel(el, rec);
    viewers!.append(el);
    wireEncodingMenu(rec, el);
    rec.el = el;
    rec.saveBtn = el.querySelector<HTMLElement>('[data-act="save"]');

    // edit / preview toggle
    el.querySelector<HTMLElement>('[data-act="toggle"]')!.onclick = () => toggleEdit(rec);
    // The heading outline on a NARROW pane: the same <nav>, laid over the
    // article by CSS while `.md-split` carries `toc-open` (on a wide pane the
    // button is not shown and the nav is the column it always was). A jump or
    // a tap on the article closes it — the index is a way in, not a fixture.
    const split = el.querySelector(".md-split")!;
    el.querySelector<HTMLElement>('[data-act="outline"]')!.onclick = () => split.classList.toggle("toc-open");
    el.querySelector(".md-outline")!.addEventListener("click", (ev) => {
      if ((ev.target as Element).closest(".outline-item")) split.classList.remove("toc-open");
    });
    el.querySelector(".md-scroll")!.addEventListener("pointerdown", () => split.classList.remove("toc-open"));
    el.querySelector<HTMLElement>('[data-act="save"]')!.onclick = () => save(rec);
    el.querySelector<HTMLElement>('[data-act="reload"]')!.onclick = () => reloadFile(rec);
    el.querySelector<HTMLElement>('[data-act="disk"]')!.onclick = () => {
      applyFresh(rec, rec.pendingDisk!);
      hideDiskBadge(rec);
    };
    el.querySelector<HTMLElement>('[data-act="detach"]')!.onclick = () => detachClick(rec);
    // in-page find over the rendered article
    const find = el.querySelector(".md-find")!;
    const input = el.querySelector<HTMLInputElement>(".md-find-input")!;
    el.querySelector<HTMLElement>('[data-act="find"]')!.onclick = () => {
      find.classList.add("open");
      input.focus();
      input.select();
    };
    input.addEventListener("input", () => mdSearch(rec, input.value));
    input.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key === "Enter") mdSearchStep(rec, e.shiftKey ? -1 : 1);
      if (e.key === "Escape") mdSearchClose(rec);
    });
    el.querySelector<HTMLElement>('[data-find="next"]')!.onclick = () => mdSearchStep(rec, 1);
    el.querySelector<HTMLElement>('[data-find="prev"]')!.onclick = () => mdSearchStep(rec, -1);
    el.querySelector<HTMLElement>('[data-find="close"]')!.onclick = () => mdSearchClose(rec);
    // Links inside the rendered article: one delegated listener for the pane's
    // lifetime, so a re-render (reload, edit→preview) never re-wires anything.
    el.querySelector(".md-body")!.addEventListener("click", (ev) => linkClick(rec, ev));

    renderMarkdown(rec);
  }

  function renderMarkdown(rec: ViewerRecord) {
    const article = rec.el!.querySelector(".md-body")!;
    const html = DOMPurify.sanitize(marked.parse(rec.content));
    article.innerHTML = html;

    // mermaid fences: marked emits <pre><code class="language-mermaid">. Defer
    // the actual draw to first paint (a hidden container measures as 0). An
    // inline `<code class="language-mermaid">` from raw HTML has no <pre> and
    // stays as code.
    rec.mermaidPending = [];
    article.querySelectorAll("code.language-mermaid").forEach((code, i) => {
      const pre = code.closest("pre");
      if (!pre) return;
      const holder = document.createElement("div");
      holder.className = "mermaid";
      holder.dataset.src = code.textContent;
      holder.id = `mmd-${rec.uid}-${i}`;
      pre.replaceWith(holder);
      rec.mermaidPending!.push(holder);
    });

    resolveImages(rec, article);
    buildOutline(rec, article);
    if (rec.visible) drawMermaid(rec);
  }

  // Repo-relative `<img>` sources resolve through `file.image` (ADR-0049 §5),
  // against the DOCUMENT's own directory. This runs on the SANITIZED DOM, after
  // DOMPurify, so nothing set here re-enters the sanitizer's decision. An
  // absolute or `http(s)` source is the author's explicit request for a remote
  // asset and is left exactly as written — the daemon's CSP decides whether it
  // loads, and `blockedImage` explains a refusal; a source that REFUSES is left alone
  // too — a broken image is an honest rendering of a broken link, and a
  // placeholder would fabricate.
  function resolveImages(rec: ViewerRecord, article: Element) {
    if (!window.WBDaemon?.readImage) return;
    const dir = rec.path.includes("/") ? rec.path.slice(0, rec.path.lastIndexOf("/")) : "";
    article.querySelectorAll<HTMLImageElement>("img[src]").forEach((img) => {
      const src = img.getAttribute("src") || "";
      // Anything carrying a scheme (`data:`, `https:`) or rooted at `/` is not
      // ours to resolve.
      if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith("/")) return;
      const rel = repoRelative(dir, src);
      if (!rel) return;
      WBDaemon.readImage(rec.project, rel, undefined, rec.checkout)
        .then((url) => {
          if (url) img.src = url;
        })
        .catch(() => {});
    });
  }

  // An image the page's CSP refused (a remote one, while remote images are
  // off: ADR-0032 amendment 2026-09-27) says so, instead of a broken icon. The
  // event comes from the browser, so the notice only shows when the policy of
  // THIS page really blocked the image. Chromium 140 fires it at the document
  // with no element (measured), so the article's images are matched by URL.
  // Built with `textContent`, never `innerHTML`: the alt text is document
  // content.
  function blockedImage(ev: SecurityPolicyViolationEvent) {
    if (ev.effectiveDirective !== "img-src" || !ev.blockedURI) return;
    document.querySelectorAll<HTMLImageElement>(".md-body img").forEach((img) => {
      if (img.src !== ev.blockedURI) return;
      const note = document.createElement("span");
      note.className = "md-img-blocked";
      const icon = document.createElement("i");
      icon.className = "bi bi-image";
      const { text, reason } = remoteImageNotice(img.getAttribute("src") || "", img.getAttribute("alt") || "");
      note.title = reason;
      note.append(icon, text);
      img.replaceWith(note);
    });
  }

  // The words of that notice, from the image's `src` and `alt` alone: a short
  // `text` that keeps a row of badges one row, and the `reason` for its
  // tooltip. A plain-`http:` image stays blocked even with remote images on
  // (the policy admits `https:` only), so it gets its own reason.
  function remoteImageNotice(src: string, alt: string) {
    let url: URL | null = null;
    try {
      url = new URL(src);
    } catch {
      // Not an absolute URL: no host to name.
    }
    const what = alt.trim() || "image";
    const from = url?.host ? ` (${url.host})` : "";
    const reason =
      url?.protocol === "http:"
        ? "Images over plain http are not shown."
        : "Turn on Remote images in Security settings, then reload the page.";
    return { text: ` Image not shown: ${what}${from}`, reason };
  }

  // A markdown `src` folded against `dir` into a repo-relative path: query and
  // fragment dropped, percent-escapes decoded (a `%20` in a filename is the
  // markdown spelling of a space), `.`/`..` segments resolved. Returns `null`
  // for anything that climbs OUT of the repo — the daemon would refuse it
  // anyway, and not asking is the honest way to spell "not ours".
  function repoRelative(dir: string, src: string) {
    let clean = src.split(/[?#]/)[0];
    try {
      clean = decodeURIComponent(clean);
    } catch {
      // A malformed escape is not a path we can resolve; use it verbatim and let
      // the daemon refuse it.
    }
    const out: string[] = [];
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

  // What a rendered link points at, decided from its `href` alone (no DOM, no
  // daemon): the pure half of `linkClick`, so the decision table is testable.
  //   • `#frag`                → { kind: "fragment", fragment }  — same document
  //   • scheme or `/`-rooted   → { kind: "external" }            — the author's
  //     explicit request for something outside the repo; left to the browser
  //   • anything else          → { kind: "file", path, fragment } — a repo file,
  //     folded against the document's own directory like an `<img src>`
  //   • climbs out of the repo → null — not ours, and the daemon would refuse it
  function linkTarget(dir: string, href: string | null) {
    if (!href) return null;
    if (href.startsWith("#")) return { kind: "fragment" as const, fragment: href.slice(1) };
    if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("/")) return { kind: "external" as const };
    const path = repoRelative(dir, href);
    if (!path) return null;
    const hash = href.indexOf("#");
    return { kind: "file" as const, path, fragment: hash < 0 ? "" : href.slice(hash + 1) };
  }

  // A click on a rendered `<a>`: a raw `href` would navigate the WHOLE
  // window. A repo file becomes an open REQUEST to the shell; external links
  // open in a new tab so the workbench is not what gets replaced.
  function linkClick(rec: ViewerRecord, ev: Event) {
    const a = (ev.target as Element).closest?.<HTMLAnchorElement>("a[href]");
    if (!a || !rec.el!.contains(a)) return;
    const dir = rec.path.includes("/") ? rec.path.slice(0, rec.path.lastIndexOf("/")) : "";
    const target = linkTarget(dir, a.getAttribute("href"));
    if (!target) {
      ev.preventDefault();
      return;
    }
    if (target.kind === "external") {
      a.target = "_blank";
      a.rel = "noopener";
      return;
    }
    ev.preventDefault();
    if (target.kind === "fragment") {
      jumpTo(rec, target.fragment);
      return;
    }
    sendDocument(document, "workbench:open-request", {
      project: rec.project,
      path: target.path,
      fragment: target.fragment,
      checkout: rec.checkout ?? null,
    });
  }

  // Scroll a rendered document to the heading a `#fragment` names. `marked`
  // emits no heading ids (the outline assigns positional ones), so the match is
  // by GitHub-style slug of the heading text — the spelling authors write.
  function slugOf(text: string) {
    return text
      .trim()
      .toLowerCase()
      .replace(/[^\p{L}\p{N}\s-]/gu, "")
      .replace(/\s+/g, "-");
  }
  function jumpTo(rec: ViewerRecord, fragment: string) {
    if (!fragment) return;
    let want = fragment;
    try {
      want = decodeURIComponent(fragment);
    } catch {
      // A malformed escape still names SOMETHING; match it as written.
    }
    want = want.toLowerCase();
    const heads = rec.el!.querySelectorAll(".md-body h1, .md-body h2, .md-body h3, .md-body h4, .md-body h5, .md-body h6");
    for (const h of heads) {
      if (slugOf(h.textContent!) === want) {
        h.scrollIntoView({ behavior: "smooth", block: "start" });
        return;
      }
    }
  }

  function drawMermaid(rec: ViewerRecord) {
    if (!rec.mermaidPending || !rec.mermaidPending.length) return;
    initMermaid();
    const pending = rec.mermaidPending;
    rec.mermaidPending = [];
    pending.forEach((holder) => {
      window.mermaid!
        .render(holder.id + "-svg", holder.dataset.src!)
        // The fence source is re-read RAW above (DOMPurify escaped it in the
        // markdown pass), so the rendered SVG is the one string on this path that
        // never met the sanitizer. Sanitize on insert. `foreignobject` is already
        // in DOMPurify's SVG allowlist, so mermaid's HTML labels survive.
        .then(({ svg }) => (holder.innerHTML = DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true, html: true } })))
        .catch((err) => {
          holder.classList.add("mermaid-error");
          holder.textContent = "Mermaid error: " + (err?.message || err);
        });
    });
  }

  // Heading outline: the jump index, one entry per heading, indented by level.
  function buildOutline(rec: ViewerRecord, article: Element) {
    const nav = rec.el!.querySelector(".md-outline")!;
    nav.innerHTML = "";
    const heads = article.querySelectorAll("h1, h2, h3, h4");
    if (!heads.length) {
      nav.innerHTML = '<div class="outline-empty">No headings</div>';
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
  function clearHits(rec: ViewerRecord) {
    (rec.hits || []).forEach((mk) => {
      const t = document.createTextNode(mk.textContent!);
      mk.replaceWith(t);
    });
    rec.el!.querySelector(".md-body")!.normalize();
    rec.hits = [];
    rec.hitIdx = -1;
  }

  // Land a mounted code editor on the first occurrence of `term` and open the
  // find widget seeded with it, so F3/Enter walks the rest — the content
  // search's "jump to line", done with what Monaco already has. Literal and
  // case-insensitive, like the search that produced the hit.
  function findInEditor(rec: ViewerRecord, term: string) {
    const ed = rec.ed as MonacoEditor | undefined;
    if (!ed || !term) return;
    // The pane must be laid out BEFORE the widget opens: Monaco keeps the
    // widest "N of M" measurement in a module-wide maximum, and one taken in a
    // settling pane squeezed every find input on the page to 12px (MEASURED
    // 2026-09-15). A pane not on screen waits for `setActive`.
    if (!rec.visible) {
      rec.pendingFind = term;
      return;
    }
    ed.layout();
    requestAnimationFrame(() => {
      if (!alive(rec) || !rec.ed) return;
      const model = ed.getModel();
      const hit = model?.findNextMatch?.(term, { lineNumber: 1, column: 1 }, false, false, null, false);
      if (hit) {
        ed.setSelection(hit.range);
        ed.revealRangeInCenter(hit.range);
      }
      ed.focus();
      ed.trigger("wb-file-search", "actions.find", { searchString: term, isRegex: false, matchCase: false });
    });
  }

  // The markdown pane's equivalent: its own find bar, opened and seeded.
  function findInMarkdown(rec: ViewerRecord, term: string) {
    const find = rec.el?.querySelector(".md-find");
    const input = rec.el?.querySelector<HTMLInputElement>(".md-find-input");
    if (!find || !input) return;
    find.classList.add("open");
    input.value = term;
    mdSearch(rec, term);
  }

  function mdSearch(rec: ViewerRecord, term: string) {
    clearHits(rec);
    const count = rec.el!.querySelector(".md-find-count")!;
    if (!term) {
      count.textContent = "";
      return;
    }
    const article = rec.el!.querySelector(".md-body")!;
    const walker = document.createTreeWalker(article, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) =>
        n.nodeValue!.trim() && !n.parentElement!.closest("svg, script, style")
          ? NodeFilter.FILTER_ACCEPT
          : NodeFilter.FILTER_REJECT,
    });
    const targets: Text[] = [];
    let node;
    while ((node = walker.nextNode())) targets.push(node as Text);
    const needle = term.toLowerCase();
    const hits: HTMLElement[] = [];
    for (const text of targets) {
      const val = text.nodeValue!;
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
    count.textContent = hits.length ? `0/${hits.length}` : "No matches";
    if (hits.length) mdSearchStep(rec, 1);
  }

  function mdSearchStep(rec: ViewerRecord, dir: number) {
    if (!rec.hits || !rec.hits.length) return;
    if (rec.hitIdx! >= 0) rec.hits[rec.hitIdx!].classList.remove("current");
    rec.hitIdx = (rec.hitIdx! + dir + rec.hits.length) % rec.hits.length;
    const mk = rec.hits[rec.hitIdx];
    mk.classList.add("current");
    mk.scrollIntoView({ block: "center", behavior: "smooth" });
    rec.el!.querySelector(".md-find-count")!.textContent = `${rec.hitIdx + 1}/${rec.hits.length}`;
  }

  function mdSearchClose(rec: ViewerRecord) {
    clearHits(rec);
    rec.el!.querySelector(".md-find")!.classList.remove("open");
    rec.el!.querySelector(".md-find-count")!.textContent = "";
    rec.el!.querySelector<HTMLInputElement>(".md-find-input")!.value = "";
  }

  // Swap the markdown pane between rendered preview and a raw-source editor.
  function toggleEdit(rec: ViewerRecord) {
    const split = rec.el!.querySelector(".md-split")!;
    const editor = rec.el!.querySelector<HTMLElement>(".md-editor")!;
    const toggle = rec.el!.querySelector<HTMLElement>('[data-act="toggle"]')!;
    rec.editing = !rec.editing;
    if (rec.editing) {
      split.classList.add("editing");
      split.classList.remove("toc-open");
      editor.style.display = "block";
      if (!rec.ed) {
        mountEditor(rec, editor, { wordWrap: "on" });
      } else {
        (rec.ed as MonacoEditor).setValue(rec.content);
      }
      setCaption(toggle, "bi-eye", "Preview");
      setTimeout(() => rec.ed?.layout(), 0);
    } else {
      // `rec.editing` is already false here, so read the editor directly —
      // contentOf() would hand back the pre-edit bytes.
      if (rec.ed) rec.content = (rec.ed as MonacoEditor).getValue();
      split.classList.remove("editing");
      editor.style.display = "none";
      setCaption(toggle, "bi-pencil", "Edit");
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
    }
  }

  // --- public API ---------------------------------------------------------
  let uidSeq = 0;
  const API = {
    // Whether any open file tab holds an edit not yet saved: the work a
    // reload onto a new build must not lose (ADR-0070 D6).
    anyDirty() {
      for (const rec of map.values()) if (rec.dirty) return true;
      return false;
    },
    // `original` is the diff's HEAD side. `project` is the IDENTITY (tab id,
    // wire field, model URI) and stays the full ref; `label` is the human
    // form, derived here when the caller supplies none. `checkout` pins the
    // pane to the worktree its bytes came from (#406), `null` for the primary.
    // `encoding`/`bom` are what `file.read` reported (ADR-0036 amendment
    // 2026-09-22) and ride the record so a save gives the bytes back the way
    // they came; absent (a diff) is UTF-8 without a BOM. `refused` is
    // the daemon's reason for serving nothing: the pane says so and keeps the
    // tab, instead of the tab closing under the click.
    open({ id, project, label, path, ftype, content, original, detached, checkout, encoding, bom, refused }: OpenSpec) {
      if (map.has(id)) return;
      const shown = label || (WBFleet ? WBFleet.refSlug(project) : project);
      const rec: ViewerRecord = {
        id, project, label: shown, path, kind: ftype, content, original, uid: ++uidSeq,
        editing: false, visible: false, detached: !!detached, checkout: checkout || null,
        encoding: encoding || "UTF-8", bom: !!bom, refused: refused || null,
      };
      map.set(id, rec);
      if (rec.refused) buildRefused(rec);
      else if (ftype === "markdown") buildMarkdown(rec);
      else if (ftype === "diff") buildDiff(rec);
      else if (ftype === "image") buildImage(rec);
      else buildCode(rec);
    },

    // The shell's `file.write` answered. `saveDone` is the ack the dirty mark
    // waits for; `saveFailed` puts the mark back and names the reason in the
    // pane. `reply` rides along so an `unencodable` refusal can say which
    // character (`char_index`) the encoding could not take.
    saveDone(id: string) {
      const rec = map.get(id);
      if (!rec) return;
      rec.dirty = false;
      rec.saveBtn?.classList.remove("dirty");
      clearSaveError(rec);
    },
    saveFailed(id: string, reason: string, reply?: WriteReply) {
      const rec = map.get(id);
      if (!rec) return;
      rec.dirty = true;
      rec.saveBtn?.classList.add("dirty");
      const text =
        reason === "unencodable"
          ? `Could not save: character ${Number(reply?.char_index ?? 0) + 1} cannot be written in ${rec.encoding}.`
          : WBFail.failed(reply || { reason }, "Could not save: the daemon gave no reason.");
      showSaveError(rec, text);
    },

    // The encoding a pane will save with, and the change of it (a "save
    // with…" or the `unencodable` dialog's "save as UTF-8").
    encodingOf(id: string) {
      const rec = map.get(id);
      return rec ? { encoding: rec.encoding, bom: !!rec.bom } : null;
    },
    setEncoding(id: string, encoding: string, bom: boolean) {
      const rec = map.get(id);
      if (!rec) return;
      rec.encoding = encoding;
      rec.bom = !!bom;
      rec.dirty = true;
      rec.saveBtn?.classList.add("dirty");
      refreshEncodingPill(rec);
    },

    // Exposed for its test: the pill's text for a daemon-named encoding.
    encodingLabel,

    // The shell never calls it — a detach goes through the pane's own button.
    // detached.html reads it on unload, to send the edited bytes home.
    descOf(id: string) {
      const rec = map.get(id);
      return rec ? descOf(rec) : null;
    },

    // Show one pane (or none, when the Consoles tab is active), and beside it
    // the slot the shell resolved (ADR-0037 §3c): `{ id, mirror, focus, ratio }`
    // or nothing. One argument is the single pane every caller had; the
    // detached popup never passes a second.
    setActive(id: string | null, slot?: ViewerSlot | "" | null) {
      shown = { id, slot: slot || null };
      paint();
    },

    // The canvas width the split decision is made against.
    width() {
      return viewers?.clientWidth ?? 0;
    },

    // The file behind an OPEN pane moved. Re-key the record instead of
    // close+open: reopening refetches the bytes and would discard the
    // operator's unsaved edits, and `open` returns early on a known id, so a
    // naive reopen is a silent no-op. A diff tab is skipped — its id comes from
    // the changes panel, not from this path.
    repath(oldId: string, { id, path }: { id: string; path: string }) {
      const rec = map.get(oldId);
      if (!rec || map.has(id) || rec.kind === "diff") return;
      map.delete(oldId);
      rec.id = id;
      rec.path = path;
      map.set(id, rec);
      if (rec.el) rec.el.dataset.tabId = id;
      if (rec.el) setPathLabel(rec.el, rec);
    },

    close(id: string) {
      const rec = map.get(id);
      if (!rec) return;
      // Delete from the map FIRST: a pending mountEditor() checks membership
      // before touching the DOM, so a tab closed mid-boot mounts nothing.
      map.delete(id);
      disposeEditor(rec);
      rec.el!.remove();
    },

    // Scroll an open markdown pane to a `#fragment`, once the bytes landed.
    jumpTo(id: string, fragment: string) {
      const rec = map.get(id);
      if (rec && rec.kind === "markdown") jumpTo(rec, fragment);
    },

    // Land a tab on the first occurrence of `term` (a content-search open).
    // A code pane whose editor has not mounted yet remembers the term and
    // acts once it has; a rendered markdown pane uses its own find bar; a
    // diff or an image has nothing to find in.
    find(id: string, term: string) {
      const rec = map.get(id);
      if (!rec || !term) return;
      if (rec.kind === "diff" || rec.kind === "image") return;
      if (rec.kind === "markdown" && !rec.editing) {
        findInMarkdown(rec, term);
        return;
      }
      if (rec.ed) findInEditor(rec, term);
      else rec.pendingFind = term;
    },

    // Exposed for the decision table's test; `linkClick` is the only caller.
    linkTarget,
    // Exposed for its test; `setPathLabel` is the only caller.
    pathLabel,
    // Exposed for its test; `blockedImage` is the only caller.
    remoteImageNotice,

    // An external write to this file's bytes landed. A CLEAN tab
    // auto-refreshes; a DIRTY tab stashes them and shows the "changed on disk"
    // badge, NEVER clobbering unsaved edits. Equal bytes are a no-op (our own
    // save round-trips through the same nudge).
    externalChange(id: string, content: string) {
      const rec = map.get(id);
      if (!rec) return;
      // A diff tab never auto-refreshes: it is a two-sided read, and a
      // single-side update would silently misrepresent the comparison. A
      // refused pane has no bytes to compare; its Reload is the retry.
      if (rec.kind === "diff" || rec.refused) return;
      if (content === rec.content) return;
      if (!rec.dirty) {
        applyFresh(rec, content);
      } else {
        rec.pendingDisk = content;
        showDiskBadge(rec);
      }
    },
  };
  // One listener for every markdown pane; a test's stub document has none.
  document.addEventListener?.("securitypolicyviolation", blockedImage);
  return API;
}
