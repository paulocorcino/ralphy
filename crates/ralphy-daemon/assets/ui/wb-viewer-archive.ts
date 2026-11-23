// The archive viewer (ADR-0091 D7): the entries of a zip or tar as a table,
// read from the `file.entries` verb. Every name goes in through `textContent`.
import { setMarkup } from "./wb-html.ts";
import { need, required } from "./wb-dom-need.ts";
import { formatSize } from "./wb-properties-format.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

const PARTIAL_NOTE = "This list is partial: the archive has more entries than are shown.";
const FAILED_NOTE = "Could not list this archive.";

type Entry = { path: string; size: number; folder: boolean };

/** The depth of an entry (0 at the root) and the last part of its path. */
export function entryName(path: string): { depth: number; name: string } {
  const parts = path.split("/").filter((part) => part !== "");
  return { depth: Math.max(parts.length - 1, 0), name: parts.at(-1) ?? path };
}

// A row shows the name under its folder; the full path and the exact size are its title.
function row(document: Document, entry: Entry) {
  const { depth, name } = entryName(entry.path);
  const el = document.createElement("div");
  el.className = entry.folder ? "archive-row archive-folder" : "archive-row";
  el.setAttribute(
    "title",
    entry.folder ? entry.path : `${entry.path}\n${entry.size.toLocaleString("en")} bytes`,
  );
  const label = document.createElement("span");
  label.className = "archive-path";
  label.style.paddingLeft = `${String(depth)}rem`;
  const icon = document.createElement("i");
  icon.className = entry.folder ? "bi bi-folder" : "bi bi-file-earmark";
  const text = document.createElement("span");
  text.textContent = name;
  label.append(icon, text);
  const size = document.createElement("span");
  size.className = "archive-size";
  size.textContent = entry.folder ? "" : formatSize(entry.size);
  el.append(label, size);
  return el;
}

function header(document: Document) {
  const el = document.createElement("div");
  el.className = "archive-row archive-head";
  const name = document.createElement("span");
  name.className = "archive-path";
  name.textContent = "Name";
  const size = document.createElement("span");
  size.className = "archive-size";
  size.textContent = "Size";
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
  body.append(header(document));
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
