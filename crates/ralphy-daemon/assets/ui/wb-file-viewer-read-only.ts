// The read-only panes: a diff (HEAD against the working tree, side by side),
// an image, and a refused file that names why it has no bytes. None of them
// saves.
import { setMarkup } from "./wb-html.ts";
import { WBMonaco } from "./wb-monaco.ts";
import { WBFail } from "./wb-fail.ts";
import { canFocus, isElement, need, required } from "./wb-dom-need.ts";
import type { MonacoDiffEditor, MonacoEditor } from "./wb-monaco.ts";
import type { Messages } from "./wb-messages.ts";
import type { BytesRecord, RefusedRecord, ViewerRecord } from "./wb-file-viewer.ts";
import type { MermaidWindow } from "./wb-dbml-draw.ts";
import { diffEditor } from "./wb-file-viewer-records.ts";
import { devError } from "./wb-log.ts";
import { fileBytesUrl } from "./wb-file-bytes.ts";

/** What `createViewer` hands the read-only panes. */
export type ViewerReadOnlyDeps = {
  window: MermaidWindow;
  document: Document;
  /** The element that holds every pane; null where a test has none. */
  viewers: HTMLElement | null;
  /** The page's door for operator messages. */
  messages: Messages;
  alive: (rec: ViewerRecord) => boolean;
  setPathLabel: (el: HTMLElement, rec: ViewerRecord) => void;
  setCaption: (btn: HTMLElement, icon: string, caption: string) => void;
  detachBtnHtml: (rec: ViewerRecord) => string;
  detachClick: (rec: BytesRecord) => void;
  reloadFile: (rec: ViewerRecord) => unknown;
  isNarrow: (el: HTMLElement | undefined) => boolean;
  watchNarrow: (
    rec: { el?: HTMLElement; ro?: ResizeObserver | undefined },
    ed: MonacoEditor | MonacoDiffEditor,
  ) => void;
};

export function createViewerReadOnly(deps: ViewerReadOnlyDeps) {
  const { window, document, viewers, messages, alive, setPathLabel } = deps;
  const { reloadFile, isNarrow, watchNarrow } = deps;

  const holder = () => required(viewers, "the element that holds the panes");

  // --- a diff tab (read-only, two-sided) ----------------------------------
  function mountDiff(rec: BytesRecord, container: HTMLElement) {
    if (rec.mounting === true || rec.mountFailed === true) return Promise.resolve();
    rec.mounting = true;
    return WBMonaco.ready()
      .then(() => {
        rec.mounting = false;
        // Same record-identity liveness rule as mountEditor: a diff tab closed
        // inside the boot window must mount nothing on its detached container.
        // A diff record always carries its HEAD side; half a diff would read as "no changes".
        if (!alive(rec) || rec.original === undefined) return null;
        const ed = WBMonaco.createDiff(container, {
          original: rec.original,
          modified: rec.content,
          path: rec.path,
          uid: rec.uid,
          project: rec.project,
          narrow: isNarrow(rec.el),
        });
        rec.ed = ed;
        watchNarrow(rec, ed);
        if (rec.visible) ed.layout();
        return null;
      })
      .catch((err: unknown) => {
        // No `<pre>` degrade here: two texts side by side have no honest
        // single-pane fallback, and one of them rendered alone would read as
        // "no changes". Say it failed and close the tab (#311).
        rec.mounting = false;
        rec.mountFailed = true;
        devError("[workbench] monaco diff failed to mount", err);
        messages.flash("Could not open the editor.");
        window.Alpine.store("tabs").closeTab(rec.id);
      });
  }

  function buildDiff(rec: BytesRecord) {
    const el = document.createElement("div");
    el.className = "viewer diff-viewer";
    el.dataset["tabId"] = rec.id;
    el.style.display = "none";
    // Find ONLY: no Save, no Reload, no disk badge, no Detach. This surface is
    // read-only by design (#311) — no commit, discard or staging control — and a
    // detached popup folds back a single-file descriptor a two-sided pane has no
    // representation in.
    setMarkup(
      el,
      `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="spacer"></span>
        <button class="vbtn" data-act="find" title="Find" aria-label="Find"
          ><i class="bi bi-search"></i><span class="vbtn-label">Find</span></button>
      </div>
      <div class="viewer-body"></div>`,
    );
    setPathLabel(el, rec);
    holder().append(el);

    need(el, '[data-act="find"]').addEventListener("click", () => {
      const mod = diffEditor(rec.ed)?.getModifiedEditor();
      mod?.focus();
      mod
        ?.getAction("actions.find")
        ?.run()
        .catch((err: unknown) => {
          devError("[workbench] monaco find failed", err);
        });
    });
    rec.el = el;
    void mountDiff(rec, need(el, ".viewer-body"));
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
    const text = REFUSAL_TEXT[reason] ?? "";
    if (text !== "") return text;
    // `detached.html` loads no `wb-fail.ts`.
    return WBFail.failed(
      { message: reason },
      "Could not open the file: the daemon gave no reason.",
    );
  }

  function buildRefused(rec: RefusedRecord) {
    const el = document.createElement("div");
    el.className = "viewer refused-viewer";
    el.dataset["tabId"] = rec.id;
    el.style.display = "none";
    setMarkup(
      el,
      `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <span class="spacer"></span>
        <span class="viewer-save-err" style="display:none"></span>
        <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"
          ><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
      </div>
      <div class="viewer-body refused-body">
        <i class="bi bi-file-earmark-x"></i>
        <p class="refused-text"></p>
        <p class="refused-hint"></p>
      </div>`,
    );
    setPathLabel(el, rec);
    need(el, ".refused-text").textContent = refusalText(rec.refused);
    if (rec.refused === "binary") {
      need(el, ".refused-hint").textContent =
        "This file may be text in an older encoding. Set the fallback encoding in “Settings”, " +
        "or reopen the file with another encoding.";
    }
    holder().append(el);
    need(el, '[data-act="reload"]').addEventListener("click", () => {
      reloadFile(rec);
    });
    rec.el = el;
  }

  return { buildDiff, buildRefused };
}

// --- an image tab (read-only) ---------------------------------------------
// `<img src>` reads the file bytes route (ADR-0091 D1), whose verified media
// type decides what the bytes are. Read-only: no Save, no Edit.
export function buildImage(host: ViewerReadOnlyDeps, rec: BytesRecord) {
  const { document, setPathLabel, detachBtnHtml } = host;
  const el = document.createElement("div");
  el.className = "viewer image-viewer";
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="img-meta"></span>
      <span class="spacer"></span>
      <button class="vbtn" data-act="zoom" title="Actual size" aria-label="Actual size"
        ><i class="bi bi-arrows-angle-expand"></i
        ><span class="vbtn-label">Actual size</span></button>
      <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"
        ><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
      ${detachBtnHtml(rec)}
    </div>
    <div class="viewer-body img-scroll"><img class="img-canvas" alt="" /></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);

  const img = required(
    el.querySelector<HTMLImageElement>(".img-canvas"),
    "the element .img-canvas",
  );
  const meta = need(el, ".img-meta");
  const url = fileBytesUrl(rec.project, rec.path, rec.checkout);
  img.addEventListener("load", () => {
    meta.textContent = `${img.naturalWidth} × ${img.naturalHeight}`;
  });
  img.addEventListener("error", () => {
    void imageFailure(url).then((text) => {
      meta.textContent = text;
      return null;
    });
  });
  img.src = url;
  wireImageButtons(host, el, rec, { img, url });
  rec.el = el;
}

// An `<img>` error carries no status: one byte asked again names the cause.
async function imageFailure(url: string) {
  const fallback = "Image could not be displayed.";
  try {
    const reply = await fetch(url, { headers: { Range: "bytes=0-0" } });
    if (reply.status === 413) return "This image is too large to show.";
    if (reply.status === 404) return "This file is not an image the workbench can display.";
    return fallback;
  } catch {
    return fallback;
  }
}

function wireImageButtons(
  host: ViewerReadOnlyDeps,
  el: HTMLElement,
  rec: BytesRecord,
  { img, url }: { img: HTMLImageElement; url: string },
) {
  need(el, '[data-act="zoom"]').addEventListener("click", (ev: MouseEvent) => {
    // Two states only: fit-to-pane (default) and 1:1 with scrollbars.
    const actual = el.classList.toggle("actual-size");
    const btn = ev.currentTarget;
    if (!isElement(btn) || !canFocus(btn)) return;
    host.setCaption(
      btn,
      actual ? "bi-arrows-angle-contract" : "bi-arrows-angle-expand",
      actual ? "Fit" : "Actual size",
    );
  });
  // A new address, so the browser reads the bytes again.
  let reloads = 0;
  need(el, '[data-act="reload"]').addEventListener("click", () => {
    reloads += 1;
    img.src = `${url}&reload=${reloads}`;
  });
  need(el, '[data-act="detach"]').addEventListener("click", () => {
    host.detachClick(rec);
  });
}
