/* ---------------------------------------------------------------------------
   ralphy workbench shell — file viewers (the closable tabs)

   Four flavours, each opening as its own tab after the fixed Consoles tab:
     • source code — Monaco: syntax highlight, in-place editing, and its own
       find widget. Binaries never reach here (app.ts refuses them).
     • Markdown  — rendered with `marked`, sanitized with DOMPurify, mermaid
       fences drawn as diagrams (Cursor-style), a heading outline to jump around,
       an in-page find, and an edit/preview toggle over the raw source. A
       Mermaid file (`.mmd`) uses the same pane, drawn as one diagram
       (`wb-viewer-render.ts` fills the body by kind).
     • image     — an allowlisted image the daemon verified and served as a
       `data:` URL (ADR-0049), fit to the pane or shown 1:1. Read-only.
     • diff      — HEAD against the working tree, side by side. Read-only.

   Editing is allowed but never touches disk: a Save emits a `save` intent on the
   `workbench:action` seam carrying the new content, for a backend to persist.

   This module owns the panes, the split and the mirror. The Markdown pane,
   the find, the encoding menu and the save steps are its siblings
   `wb-viewer-markdown.ts`, `-find.ts`, `-encoding.ts` and `-save.ts`; it
   builds them in `createViewer`.
--------------------------------------------------------------------------- */
import { WBMonaco } from "./wb-monaco.ts";
import { WBSplit } from "./wb-split.ts";
import { WBFail } from "./wb-fail.ts";
import { WBFleet } from "./wb-fleet.ts";
import { sendDocument } from "./wb-events.ts";
import { isNullableString, isOptionalString, isRecord } from "./wb-api.ts";
import { focusTarget, isSplit, paneLayout } from "./wb-viewer-panes.ts";
import type { MonacoDiffEditor, MonacoDisposable, MonacoEditor } from "./wb-monaco.ts";
import type { Messages } from "./wb-messages.ts";
import { isRendered } from "./wb-viewer-render.ts";
import { createViewerEncoding, encodingBtnHtml, encodingLabel, refreshEncodingPill } from "./wb-viewer-encoding.ts";
import { createViewerFind } from "./wb-viewer-find.ts";
import { createViewerMarkdown, linkTarget, remoteImageNotice } from "./wb-viewer-markdown.ts";
import { createViewerSave } from "./wb-viewer-save.ts";
import type { ViewerMarkdown } from "./wb-viewer-markdown.ts";
import type { ReopenBytes } from "./wb-viewer-save.ts";
import type { ViewerSlot } from "./wb-viewer-panes.ts";
import type { MermaidWindow } from "./wb-viewer-render.ts";

/**
 * One open file pane. `open` fills the first group; the rest appear as the
 * pane is used. `ed` is a diff editor on a `diff` pane and a code editor on
 * every other.
 */
type PaneRecord = {
  id: string;
  project: string;
  label: string;
  path: string;
  kind: string;
  original: string | undefined;
  uid: number;
  editing: boolean;
  visible: boolean;
  detached: boolean;
  checkout: string | null;
  encoding: string;
  bom: boolean;
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

/** A pane over the file's bytes. A record never changes between the two
 * kinds: a refused pane that gets bytes is replaced (`reopenRefused`). */
export type BytesRecord = PaneRecord & { content: string; refused: null };
/** A pane that holds the daemon's reason for serving no bytes. */
export type RefusedRecord = PaneRecord & { refused: string };
export type ViewerRecord = BytesRecord | RefusedRecord;

/** What `open` takes: the fields of a `FileDescriptor`, or the reason a pane has no bytes. */
type OpenSpec = {
  id: string;
  project: string;
  label?: string;
  path: string;
  ftype: string;
  /** The diff's HEAD side. */
  original?: string;
  detached?: boolean;
  checkout?: string | null;
  encoding?: string;
  bom?: boolean;
} & ({ content: string; refused?: undefined } | { content?: undefined; refused: string });

/** The mirror: a second editor over a pane's model, in a `.viewer` of its own. */
type Mirror = {
  rec: ViewerRecord;
  el: HTMLElement;
  ed: MonacoEditor | null;
  ro: ResizeObserver | undefined;
  saveKey: MonacoDisposable | undefined;
};

/** Whether `v` has the shape `descOf` gives a file: what one window posts to
 * another when a file pane is detached or comes home. */
export function isFileDescriptor(v: unknown): v is FileDescriptor {
  return (
    isRecord(v) &&
    typeof v.project === "string" &&
    typeof v.label === "string" &&
    typeof v.path === "string" &&
    typeof v.ftype === "string" &&
    typeof v.content === "string" &&
    isNullableString(v.checkout) &&
    isOptionalString(v.encoding) &&
    typeof v.bom === "boolean"
  );
}

/** Whether `v` has the shape of the `workbench:open-request` detail this
 * module sends: the file a rendered link asks to open. */
export function isOpenRequest(v: unknown): v is OpenRequest {
  return (
    isRecord(v) &&
    isNullableString(v.project) &&
    typeof v.path === "string" &&
    isNullableString(v.checkout) &&
    isOptionalString(v.fragment) &&
    (v.as === undefined || v.as === "bytes")
  );
}

/** What the entry module hands the file pane. */
export type ViewerDeps = {
  /** The page's door for operator messages. */
  messages: Messages;
};

export function createViewer(window: MermaidWindow, document: Document, deps: ViewerDeps) {
  const { messages } = deps;
  const viewers = document.getElementById("viewers");
  const map = new Map<string, ViewerRecord>(); // tab id → viewer record

  // Monaco boots through an AMD loader, so an editor is created
  // asynchronously. INVARIANTS across that gap: `rec.content` is the single
  // source of truth until `rec.ed` exists, and a tab closed mid-boot must
  // never mount an orphan editor. Liveness is `map.get(rec.id) === rec`, NOT
  // `has`: tab ids are stable per file, so a close+reopen inside the boot
  // window puts a DIFFERENT record under the same key.
  const alive = (rec: ViewerRecord) => map.get(rec.id) === rec;

  // The viewer's parts, built in the order of what each one reads. The save
  // steps read `renderMarkdown` and `drawMermaid`, which are built after
  // them, so they take those two late.
  const find = createViewerFind({ document, alive });
  const { findInEditor, findInMarkdown } = find;
  const saver = createViewerSave({
    window, messages, reopenRefused,
    renderMarkdown: (rec) => markdown.renderMarkdown(rec),
    drawMermaid: (rec) => markdown.drawMermaid(rec),
  });
  const { save, showSaveError, clearSaveError, descOf, reloadFile, applyFresh, showDiskBadge, hideDiskBadge } = saver;
  const encoding = createViewerEncoding({ window, document, messages, saver, reopenRefused });
  const { wireEncodingMenu } = encoding;
  const markdown: ViewerMarkdown = createViewerMarkdown({
    window, document, viewers, setPathLabel, setCaption, detachBtnHtml, detachClick, mountEditor, saver, encoding, find,
  });
  const { buildMarkdown, drawMermaid, jumpTo } = markdown;


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
    const split = isSplit(slot, slotRec);
    for (const rec of map.values()) {
      const { on, display, gridColumn } = paneLayout(rec.id, id, split, slot?.id);
      Object.assign(rec.el!.style, { display, gridColumn });
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
        if (isRendered(rec.kind)) drawMermaid(rec);
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
      const target = focusTarget(slot!, mirror, slotRec!.ed as MonacoEditor | undefined);
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
    el.querySelector<HTMLElement>('[data-act="mirror"]')!.onclick = () => window.Alpine?.store("tabs")?.toggleMirror();
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
    divider.querySelector<HTMLElement>(".slot-close")!.onclick = () => window.Alpine?.store("tabs")?.clearSlot();
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

  function mountEditor(rec: BytesRecord, container: HTMLElement, opts: { path?: string; wordWrap?: string }) {
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
          path,
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
        messages.flash("Could not open the editor.");
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
  function mountDiff(rec: BytesRecord, container: HTMLElement) {
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
        messages.flash("Could not open the editor.");
        window.Alpine?.store("tabs")?.closeTab(rec.id);
      });
  }

  function buildDiff(rec: BytesRecord) {
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
  function buildCode(rec: BytesRecord) {
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
    if (mirrorBtn) mirrorBtn.onclick = () => window.Alpine?.store("tabs")?.toggleMirror();
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

  // A pane that opened refused now has bytes: rebuild it as the pane its
  // kind deserves. `open` returns early on a known id, so the record is
  // replaced under the same id — the tab in the shell is untouched.
  function reopenRefused(rec: RefusedRecord, bytes: ReopenBytes) {
    const { id, project, label, path, kind, checkout, detached } = rec;
    API.close(id);
    API.open({ id, project, label, path, ftype: kind, checkout, detached, content: bytes.content, encoding: bytes.encoding, bom: bytes.bom });
    if (rec.visible) refresh();
    window.WB.emit("reload", { project: rec.project, path: rec.path });
  }

  // The Detach/Re-attach button. A file tab detaches into a standalone popup
  // (watch an agent in the main window, read the file in another); a detached
  // pane folds back in. wb-file-viewer only *requests* it — the shell (app.ts) opens
  // the popup, and the popup (detached.html) folds back — so this module stays
  // agnostic to windows/tabs.
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
  // CSS ellipsizes the directory first.
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

  function detachClick(rec: BytesRecord) {
    const evt = rec.detached ? "workbench:reattach-request" : "workbench:detach-request";
    sendDocument(document, evt, descOf(rec));
  }

  // --- an image tab (read-only) -------------------------------------------
  // `rec.content` is a `data:` URL the daemon's verified media type built
  // (ADR-0049 §2), so this pane never decides what bytes are. Read-only: no
  // Save, no Edit — the Write class is untouched by images.
  function buildImage(rec: BytesRecord) {
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

  function buildRefused(rec: RefusedRecord) {
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
    el.querySelector(".refused-text")!.textContent = refusalText(rec.refused);
    if (rec.refused === "binary") {
      el.querySelector(".refused-hint")!.textContent =
        "This file may be text in an older encoding. Set the fallback encoding in “Settings”, or reopen the file with another encoding.";
    }
    viewers!.append(el);
    el.querySelector<HTMLElement>('[data-act="reload"]')!.onclick = () => reloadFile(rec);
    rec.el = el;
  }

  // --- public API ---------------------------------------------------------
  let uidSeq = 0;
  const API = {
    // Whether any open file tab holds an edit not yet saved: the work a
    // reload onto a new build must not lose (ADR-0070 D6).
    anyDirty() {
      return [...map.values()].some((rec) => rec.dirty);
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
    open(spec: OpenSpec) {
      const { id, project, label, path, ftype, original, detached, checkout, encoding, bom } = spec;
      if (map.has(id)) return;
      const shown = label || (WBFleet ? WBFleet.refSlug(project) : project);
      const pane: PaneRecord = {
        id, project, label: shown, path, kind: ftype, original, uid: ++uidSeq,
        editing: false, visible: false, detached: !!detached, checkout: checkout || null,
        encoding: encoding || "UTF-8", bom: !!bom,
      };
      if (spec.refused !== undefined) {
        const refused: RefusedRecord = { ...pane, refused: spec.refused };
        map.set(id, refused);
        buildRefused(refused);
        return;
      }
      const rec: BytesRecord = { ...pane, content: spec.content, refused: null };
      map.set(id, rec);
      if (isRendered(ftype)) buildMarkdown(rec);
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
    // detached.html reads it on unload, to send the edited bytes home. A
    // refused pane has no bytes to describe.
    descOf(id: string) {
      const rec = map.get(id);
      return rec && rec.refused === null ? descOf(rec) : null;
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
      if (rec && isRendered(rec.kind)) jumpTo(rec, fragment);
    },

    // Land a tab on the first occurrence of `term` (a content-search open).
    // A code pane whose editor has not mounted yet remembers the term and
    // acts once it has; a rendered markdown pane uses its own find bar; a
    // diff or an image has nothing to find in.
    find(id: string, term: string) {
      const rec = map.get(id);
      if (!rec || !term) return;
      if (rec.kind === "diff" || rec.kind === "image") return;
      if (isRendered(rec.kind) && !rec.editing) {
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
      if (rec.kind === "diff" || rec.refused !== null) return;
      if (content === rec.content) return;
      if (!rec.dirty) {
        applyFresh(rec, content);
      } else {
        rec.pendingDisk = content;
        showDiskBadge(rec);
      }
    },
  };
  return API;
}
