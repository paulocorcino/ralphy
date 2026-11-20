// The archive viewer (ADR-0091 D7): the entries of a zip or tar as text rows,
// read from the `file.entries` verb. Every name goes in through `textContent`.
import { setMarkup } from "./wb-html.ts";
import { need, required } from "./wb-dom-need.ts";
import { formatBytes } from "./wb-properties-format.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

const PARTIAL_NOTE = "This list is partial: the archive has more entries than are shown.";
const FAILED_NOTE = "Could not list this archive.";

function row(document: Document, entry: { path: string; size: number; folder: boolean }) {
  const el = document.createElement("div");
  el.className = "archive-row";
  const name = document.createElement("span");
  name.className = "archive-path";
  name.textContent = entry.path;
  const size = document.createElement("span");
  size.className = "archive-size";
  size.textContent = entry.folder ? "" : formatBytes(entry.size);
  el.append(name, size);
  return el;
}

function note(document: Document, text: string) {
  const el = document.createElement("div");
  el.className = "archive-note";
  el.textContent = text;
  return el;
}

async function fill(document: Document, body: HTMLElement, rec: BytesRecord) {
  const payload = WBDaemon.withCheckout({ repo: rec.project, path: rec.path }, rec.checkout);
  const reply = await WBDaemon.observe("file.entries", payload).catch(() => null);
  body.replaceChildren();
  if (reply?.status !== "ok") {
    body.append(note(document, FAILED_NOTE));
    return;
  }
  for (const entry of reply.entries ?? []) body.append(row(document, entry));
  if (reply.truncated === true) body.append(note(document, PARTIAL_NOTE));
}

/** Builds an archive pane; its rows arrive when the daemon answers. */
export function buildArchive(host: ViewerReadOnlyDeps, rec: BytesRecord) {
  const { document, setPathLabel } = host;
  const el = document.createElement("div");
  el.className = "viewer archive-viewer";
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="spacer"></span>
      <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"
        ><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
    </div>
    <div class="viewer-body archive-body"></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);
  const body = need(el, ".viewer-body");
  const load = () => {
    void fill(document, body, rec);
  };
  need(el, '[data-act="reload"]').addEventListener("click", load);
  load();
  rec.el = el;
}
