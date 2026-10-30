// The file panes' split: the slot beside the active pane, the mirror and the
// divider between the two columns.
import { WBMonaco } from "./wb-monaco.ts";
import { WBSplit } from "./wb-split.ts";
import { sendDocument } from "./wb-events.ts";
import { need } from "./wb-dom-need.ts";
import { focusTarget, isSplit, paneLayout } from "./wb-viewer-panes.ts";
import { isRendered } from "./wb-viewer-render.ts";
import type { MonacoDiffEditor, MonacoDisposable, MonacoEditor } from "./wb-monaco.ts";
import type { ViewerRecord } from "./wb-file-viewer.ts";
import type { ViewerSlot } from "./wb-viewer-panes.ts";
import type { MermaidWindow } from "./wb-dbml-draw.ts";

/** The mirror: a second editor over a pane's model, in a `.viewer` of its own. */
type Mirror = {
  rec: ViewerRecord;
  el: HTMLElement;
  ed: MonacoEditor | null;
  ro: ResizeObserver | undefined;
  saveKey: MonacoDisposable | undefined;
};

/** What `createViewer` hands the slot. */
export type ViewerSlotDeps = {
  window: MermaidWindow;
  document: Document;
  /** The element that holds every pane; null where a test has none. */
  viewers: HTMLElement | null;
  /** The open panes, by tab id. */
  map: Map<string, ViewerRecord>;
  alive: (rec: ViewerRecord) => boolean;
  findInEditor: (rec: ViewerRecord, term: string) => void;
  drawMermaid: (rec: ViewerRecord) => void;
  setPathLabel: (el: HTMLElement, rec: ViewerRecord) => void;
  setCaption: (btn: HTMLElement, icon: string, caption: string) => void;
  isNarrow: (el: HTMLElement | undefined) => boolean;
  watchNarrow: (
    rec: { el?: HTMLElement; ro?: ResizeObserver },
    ed: MonacoEditor | MonacoDiffEditor,
  ) => void;
  bindSave: (ed: MonacoEditor, rec: ViewerRecord) => MonacoDisposable;
};

export function createViewerSlot(deps: ViewerSlotDeps) {
  const { window, document, viewers, map, alive, findInEditor, drawMermaid } = deps;
  const { setPathLabel, setCaption, isNarrow, watchNarrow, bindSave } = deps;

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
      if (rec.el) Object.assign(rec.el.style, { display, gridColumn });
      rec.visible = on;
      if (on) showPane(rec);
    }
    if (!viewers) return;
    viewers.classList.toggle("split", split);
    if (split && slot && slotRec) paintSplit(viewers, slot, slotRec);
    else paintSingle(id);
  }

  function showPane(rec: ViewerRecord) {
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

  // Nothing on screen (Consoles, Spend): the mirror is HIDDEN like every
  // other pane, keeping its own scroll and cursor for the return — that
  // independence is what the second editor is for. Any other single
  // paint is a slot that was closed or moved: the mirror goes.
  function paintSingle(id: string | null) {
    if (id === null && mirror && alive(mirror.rec)) mirror.el.style.display = "none";
    else closeMirror();
  }

  function paintSplit(box: HTMLElement, slot: ViewerSlot, slotRec: ViewerRecord) {
    ensureDivider(box);
    box.style.setProperty("--wb-split", ratioPct(slot.ratio));
    if (slot.mirror) ensureMirror(slotRec, box);
    else closeMirror();
    if (slot.focus) {
      const target = focusTarget(slot, mirror, slotRec.ed as MonacoEditor | undefined);
      setTimeout(() => target?.focus(), 0);
    }
  }

  const ratioPct = (ratio: number | null) =>
    `${((typeof ratio === "number" && Number.isFinite(ratio) ? ratio : 0.5) * 100).toFixed(2)}%`;

  function ensureMirror(rec: ViewerRecord, box: HTMLElement) {
    if (mirror?.rec === rec) {
      mirror.el.style.display = "flex";
      return;
    }
    closeMirror();
    const model = (rec.ed as MonacoEditor).getModel();
    if (!model) return;
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
    need(el, '[data-act="mirror"]').onclick = () => window.Alpine?.store("tabs")?.toggleMirror();
    box.append(el);
    const holder: Mirror = { rec, el, ed: null, ro: undefined, saveKey: undefined };
    const ed = WBMonaco.createOver(need(el, ".viewer-body"), model, {
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
  function ensureDivider(box: HTMLElement) {
    if (divider) return;
    divider = document.createElement("div");
    divider.className = "viewers-divider";
    divider.innerHTML =
      '<button class="slot-close" title="Close the second pane" aria-label="Close the second pane"><i class="bi bi-x-lg"></i></button>';
    need(divider, ".slot-close").onclick = () => window.Alpine?.store("tabs")?.clearSlot();
    divider.addEventListener("pointerdown", (e: PointerEvent) => {
      if (e.button !== 0 || !e.isPrimary || (e.target as Element).closest(".slot-close")) return;
      e.preventDefault();
      const pointerId = e.pointerId;
      const bounds = box.getBoundingClientRect();
      let ratio: number | null = null;
      const onMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return;
        ratio = WBSplit.clampRatio(ev.clientX - bounds.left, bounds.width);
        box.style.setProperty("--wb-split", ratioPct(ratio));
      };
      const onUp = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("pointercancel", onUp);
        if (ratio != null) sendDocument(document, "workbench:split-ratio", { ratio });
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
    });
    box.append(divider);
  }

  // The canvas crossing the split's width floor is the shell's to re-decide:
  // it holds the slot, this module only paints what it was last told.
  if (viewers && typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      sendDocument(document, "workbench:canvas-resize", { width: viewers.clientWidth });
    }).observe(viewers);
  }

  // Show one pane (or none) and the slot the shell resolved beside it.
  function show(id: string | null, slot: ViewerSlot | null) {
    shown = { id, slot };
    paint();
  }

  // Whether the slot shows a mirror of the pane `id`.
  function mirrors(id: string) {
    return !!shown.slot?.mirror && shown.slot.id === id;
  }

  // The pane the mirror sits over, if there is a mirror.
  function mirrored() {
    return mirror?.rec ?? null;
  }

  return { refresh, show, mirrors, mirrored, closeMirror };
}
