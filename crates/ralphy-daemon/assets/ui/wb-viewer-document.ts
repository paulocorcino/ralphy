// The document pane (ADR-0091 D10): a docx or odt file's bytes go to the
// document worker, and its model becomes DOM here. Text goes in only as text
// nodes, and an image only as a `blob:` address of an image type, so nothing
// in the file becomes markup or a fetch.
import { setMarkup } from "./wb-html.ts";
import { need, required } from "./wb-dom-need.ts";
import { fileBytesUrl } from "./wb-file-bytes.ts";
import { devError } from "./wb-log.ts";
import { showLoading } from "./wb-viewer-loading.ts";
import { readAnydoc } from "./wb-anydoc.ts";
import { liveFontDeps } from "./wb-viewer-font.ts";
import type { AnydocFormat, AnydocResult } from "./wb-anydoc.ts";
import type {
  Block,
  BlockKind,
  Cell,
  Document as DocModel,
  Inline,
  InlineKind,
  List,
  Table,
} from "./vendor/anydoc/anydoc_wasm.js";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

export const DOCUMENT_ERROR_TEXT = "This file could not be read as a document.";

/** The two steps of a document pane: the file's bytes, and their model. */
export type DocumentDeps = {
  fetchBytes: (url: string) => Promise<Uint8Array<ArrayBuffer>>;
  read: (bytes: Uint8Array, format: AnydocFormat) => Promise<AnydocResult>;
};

/** The browser's fetch and the page's document worker; a test passes its own. */
export function liveDocumentDeps(document: Pick<Document, "fonts">): DocumentDeps {
  // The font pane's reader already reads a whole file past the route's 8 MiB answer.
  return { fetchBytes: liveFontDeps(document).fetchBytes, read: readAnydoc };
}

type Ctx = { document: Document; model: DocModel };

const MARKER_TYPES: Record<List["marker"], string | null> = {
  bullet: null,
  decimal: "1",
  lowerAlpha: "a",
  upperAlpha: "A",
  lowerRoman: "i",
  upperRoman: "I",
};

function make(ctx: Ctx, tag: string, kids: Node[] = []) {
  const el = ctx.document.createElement(tag);
  el.append(...kids);
  return el;
}

function blocks(ctx: Ctx, items: Block[] | undefined): Node[] {
  return (items ?? []).map((block) => BLOCKS[block.kind](ctx, block));
}

function inlines(ctx: Ctx, items: Inline[] | undefined): Node[] {
  return (items ?? []).flatMap((inline) => INLINES[inline.kind](ctx, inline));
}

function styledText(ctx: Ctx, inline: Inline): Node[] {
  let node: Node = ctx.document.createTextNode(inline.text ?? "");
  const style = inline.style;
  if (style?.code === true) node = make(ctx, "code", [node]);
  if (style?.strike === true) node = make(ctx, "s", [node]);
  if (style?.italic === true) node = make(ctx, "em", [node]);
  if (style?.bold === true) node = make(ctx, "strong", [node]);
  return [node];
}

function isWebAddress(value: string) {
  return /^(https?|mailto):/iu.test(value);
}

function link(ctx: Ctx, inline: Inline): Node[] {
  const kids = inlines(ctx, inline.content);
  const target = inline.target;
  if (target?.kind !== "external" || !isWebAddress(target.value)) return kids;
  const a = make(ctx, "a", kids);
  a.setAttribute("href", target.value);
  a.setAttribute("target", "_blank");
  a.setAttribute("rel", "noopener noreferrer");
  return [a];
}

// An embedded image of an image type draws as a `blob:` address; any other
// asset, an outside address or a missing part draws nothing.
function image(ctx: Ctx, inline: Inline): Node[] {
  const id = inline.source?.kind === "asset" ? inline.source.assetId : undefined;
  const asset = ctx.model.assets.find((one) => one.id === id);
  if (asset?.mediaType.startsWith("image/") !== true) return [];
  const bytes = new Uint8Array(asset.data);
  const url = URL.createObjectURL(new Blob([bytes], { type: asset.mediaType }));
  const img = make(ctx, "img");
  img.setAttribute("alt", inline.alt ?? "");
  // A drawn image keeps its pixels, so its address goes once it loaded.
  const revoke = () => {
    URL.revokeObjectURL(url);
  };
  img.addEventListener("load", revoke);
  img.addEventListener("error", revoke);
  img.setAttribute("src", url);
  return [img];
}

function text(ctx: Ctx, value: string): Node[] {
  return [ctx.document.createTextNode(value)];
}

const INLINES: Record<InlineKind, (ctx: Ctx, inline: Inline) => Node[]> = {
  text: styledText,
  link,
  image,
  anchor: () => [],
  noteRef: (ctx, inline) => [make(ctx, "sup", text(ctx, `[${inline.noteId ?? ""}]`))],
  lineBreak: (ctx) => [make(ctx, "br")],
  math: (ctx, inline) => [make(ctx, "code", text(ctx, inline.text ?? ""))],
  checkbox: (ctx, inline) => text(ctx, inline.checked === true ? "[x] " : "[ ] "),
};

function heading(ctx: Ctx, block: Block) {
  const level = Math.min(6, Math.max(1, block.level ?? 1));
  return make(ctx, `h${level}`, inlines(ctx, block.content));
}

function list(ctx: Ctx, block: Block) {
  const model = block.list;
  if (!model) return make(ctx, "ul");
  const type = MARKER_TYPES[model.marker];
  const el = make(
    ctx,
    type === null ? "ul" : "ol",
    model.items.map((item) => make(ctx, "li", blocks(ctx, item.blocks))),
  );
  if (type !== null) {
    el.setAttribute("type", type);
    el.setAttribute("start", String(model.start));
  }
  return el;
}

function cell(ctx: Ctx, model: Cell, header: boolean) {
  const el = make(ctx, header ? "th" : "td", blocks(ctx, model.blocks));
  if (model.colSpan > 1) el.setAttribute("colspan", String(model.colSpan));
  if (model.rowSpan > 1) el.setAttribute("rowspan", String(model.rowSpan));
  return el;
}

// A covered slot belongs to the cell that spans over it, so it draws nothing.
function table(ctx: Ctx, block: Block) {
  const model: Table | undefined = block.table;
  const rows = (model?.grid ?? []).map((row, at) => {
    const header = at < (model?.headerRows ?? 0);
    return make(
      ctx,
      "tr",
      row.flatMap((slot) => (slot.cell ? [cell(ctx, slot.cell, header)] : [])),
    );
  });
  return make(ctx, "table", [make(ctx, "tbody", rows)]);
}

function code(ctx: Ctx, block: Block) {
  return make(ctx, "pre", [make(ctx, "code", text(ctx, block.text ?? ""))]);
}

const BLOCKS: Record<BlockKind, (ctx: Ctx, block: Block) => Node> = {
  heading,
  paragraph: (ctx, block) => make(ctx, "p", inlines(ctx, block.content)),
  list,
  table,
  blockQuote: (ctx, block) => make(ctx, "blockquote", blocks(ctx, block.blocks)),
  codeBlock: code,
  rule: (ctx) => make(ctx, "hr"),
  math: code,
};

function notes(ctx: Ctx): Node[] {
  if (ctx.model.notes.length === 0) return [];
  const items = ctx.model.notes.map((note) =>
    make(ctx, "li", [make(ctx, "sup", text(ctx, `[${note.id}]`)), ...blocks(ctx, note.blocks)]),
  );
  return [make(ctx, "hr"), make(ctx, "ol", items)];
}

/** The DOM of a document's model: its blocks, then its notes. */
export function renderDocument(document: Document, model: DocModel): Node[] {
  const ctx = { document, model };
  return [...blocks(ctx, model.blocks), ...notes(ctx)];
}

function formatOf(path: string): AnydocFormat {
  return path.toLowerCase().endsWith(".odt") ? "odt" : "docx";
}

export function buildDocument(
  host: ViewerReadOnlyDeps,
  rec: BytesRecord,
  deps: DocumentDeps = liveDocumentDeps(host.document),
) {
  const { document, setPathLabel, detachBtnHtml } = host;
  const el = document.createElement("div");
  el.className = "viewer document-viewer";
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="document-meta"></span>
      <span class="spacer"></span>
      ${detachBtnHtml(rec)}
    </div>
    <div class="viewer-body document-body"></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);

  const meta = need(el, ".document-meta");
  const body = need(el, ".document-body");
  need(el, '[data-act="detach"]').addEventListener("click", () => {
    host.detachClick(rec);
  });
  const loaded = showLoading(document, body);
  const fail = (err: unknown) => {
    loaded();
    devError("[workbench] document failed to load", err);
    meta.textContent = DOCUMENT_ERROR_TEXT;
  };
  void deps
    .fetchBytes(fileBytesUrl(rec.project, rec.path, rec.checkout))
    .then((bytes) => deps.read(bytes, formatOf(rec.path)))
    .then((result) => {
      if (!result.ok) {
        fail(result.code);
        return null;
      }
      loaded();
      body.append(...renderDocument(document, result.document));
      return null;
    }, fail);
  rec.el = el;
}
