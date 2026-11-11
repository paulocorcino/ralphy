// The source-code pane: its tab, the Monaco editor in it, the Save key and the
// pane-width rule that trims the editor's gutter.
import { WBMonaco } from "./wb-monaco.ts";
import { need, required } from "./wb-dom-need.ts";
import { encodingBtnHtml } from "./wb-viewer-encoding.ts";
import type { MonacoDiffEditor, MonacoEditor } from "./wb-monaco.ts";
import type { Messages } from "./wb-messages.ts";
import type { BytesRecord, ViewerRecord } from "./wb-file-viewer-records.ts";
import type { ViewerEncoding } from "./wb-viewer-encoding.ts";
import { wireFileButtons } from "./wb-viewer-save.ts";
import type { ViewerSave } from "./wb-viewer-save.ts";
import type { MermaidWindow } from "./wb-dbml-draw.ts";

/** What `createViewer` hands the code pane. */
export type ViewerCodeDeps = {
  window: MermaidWindow;
  document: Document;
  /** The element that holds every pane; null where a test has none. */
  viewers: HTMLElement | null;
  /** The page's door for operator messages. */
  messages: Messages;
  alive: (rec: ViewerRecord) => boolean;
  findInEditor: (rec: ViewerRecord, term: string) => void;
  /** Whether the mirror shows the pane `id`. */
  mirrors: (id: string) => boolean;
  refresh: () => void;
  setPathLabel: (el: HTMLElement, rec: ViewerRecord) => void;
  detachBtnHtml: (rec: ViewerRecord) => string;
  mirrorBtnHtml: (rec: ViewerRecord) => string;
  detachClick: (rec: BytesRecord) => void;
  saver: Pick<ViewerSave, "save" | "reloadFile" | "applyFresh" | "hideDiskBadge">;
  encoding: Pick<ViewerEncoding, "wireEncodingMenu">;
};

export function createViewerCode(deps: ViewerCodeDeps) {
  const { window, document, viewers, messages, alive, findInEditor, mirrors, refresh } = deps;
  const { setPathLabel, detachBtnHtml, mirrorBtnHtml, detachClick } = deps;
  const { save } = deps.saver;
  const { wireEncodingMenu } = deps.encoding;

  // The pane-width criterion, the SAME number as the stylesheet's
  // `@container viewer (max-width: 560px)`: the gutter trims exactly when the
  // captions fold to icons. Monaco takes it as options, not CSS, so it is
  // measured here and re-measured when the pane's box changes — only a
  // threshold CROSSING reaches `updateOptions`; a resize on the same side of
  // it is Monaco's own `automaticLayout` business. The observer is the pane's,
  // not the editor's: `disposeEditor` ends it with the editor it feeds.
  const NARROW_PX = 560;
  const isNarrow = (el: HTMLElement | undefined) =>
    el !== undefined && el.clientWidth > 0 && el.clientWidth <= NARROW_PX;
  function watchNarrow(
    rec: { el?: HTMLElement; ro?: ResizeObserver },
    ed: MonacoEditor | MonacoDiffEditor,
  ) {
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
      run: () => {
        save(rec);
      },
    });
  }

  function mountEditor(
    rec: BytesRecord,
    container: HTMLElement,
    opts: { path?: string; wordWrap?: string },
  ) {
    const path = (opts.path ?? "") || rec.path;
    if (rec.mounting === true || rec.mountFailed === true) return Promise.resolve();
    rec.mounting = true;
    return WBMonaco.ready()
      .catch((err: unknown) => {
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
        if (!monaco || !alive(rec)) return null;
        const ed = WBMonaco.create(container, {
          value: rec.content,
          path,
          uid: rec.uid,
          project: rec.project,
          wordWrap: opts.wordWrap,
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
        const term = rec.pendingFind ?? "";
        if (term !== "") {
          rec.pendingFind = null;
          findInEditor(rec, term);
        }
        // A mirror asked for before there was an editor to mirror (a reload
        // restores the slot while the bytes are still in flight): paint now.
        if (mirrors(rec.id)) refresh();
        return null;
      })
      .catch((err: unknown) => {
        // A create()/wiring failure is NOT a boot failure: the pane would look
        // editable while nothing is wired, so say so instead of degrading.
        rec.mounting = false;
        console.error("[workbench] monaco editor failed to mount", err);
        messages.flash("Could not open the editor.");
      });
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
        <button class="vbtn" data-act="find" title="Find" aria-label="Find"
          ><i class="bi bi-search"></i><span class="vbtn-label">Find</span></button>
        ${mirrorBtnHtml(rec)}
        <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"
          ><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
        <button class="vbtn viewer-disk-badge" data-act="disk" style="display:none"
          title="Changed on disk — reload" aria-label="Changed on disk — reload"
          ><i class="bi bi-exclamation-triangle"></i
          ><span class="vbtn-label">Changed on disk — reload</span></button>
        <span class="viewer-save-err" style="display:none"></span>
        ${encodingBtnHtml(rec)}
        <button class="vbtn save" data-act="save" title="Save" aria-label="Save"
          ><i class="bi bi-save"></i><span class="vbtn-label">Save</span></button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="viewer-body"></div>`;
    setPathLabel(el, rec);
    required(viewers, "the element that holds the panes").append(el);
    wireEncodingMenu(rec, el);

    const saveBtn = need(el, '[data-act="save"]');
    need(el, '[data-act="find"]').onclick = () => {
      (rec.ed as MonacoEditor | undefined)?.focus();
      (rec.ed as MonacoEditor | undefined)
        ?.getAction("actions.find")
        ?.run()
        .catch((err: unknown) => {
          // oxlint-disable-next-line no-console -- the failure is for a developer
          console.error("[workbench] monaco find failed", err);
        });
    };
    const mirrorBtn = el.querySelector<HTMLElement>('[data-act="mirror"]');
    mirrorBtn?.addEventListener("click", () => {
      window.Alpine.store("tabs").toggleMirror();
    });
    wireFileButtons(el, rec, { ...deps.saver, detachClick });
    rec.el = el;
    rec.saveBtn = saveBtn;
    rec.mirrorBtn = mirrorBtn;
    void mountEditor(rec, need(el, ".viewer-body"), {});
  }

  return { isNarrow, watchNarrow, bindSave, mountEditor, buildCode };
}
