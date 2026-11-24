// The font pane (ADR-0091 D1, D4): the file's bytes, read from the file bytes
// route, become a FontFace, and a sample line is drawn in it at three sizes.
// A font the browser refuses shows the message. The pane saves nothing.
import { setMarkup } from "./wb-html.ts";
import { need, required } from "./wb-dom-need.ts";
import { fileBytesUrl, readWholeFile } from "./wb-file-bytes.ts";
import { devError } from "./wb-log.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

export const FONT_ERROR_TEXT = "This browser cannot show this font file.";
export const FONT_SAMPLE_TEXT = "The quick brown fox jumps over the lazy dog 0123456789";
const SAMPLE_SIZES = [16, 32, 64];

/** The two steps of a font pane: the file's bytes, and a face the page draws with. */
export type FontDeps = {
  fetchBytes: (url: string) => Promise<Uint8Array<ArrayBuffer>>;
  addFace: (family: string, bytes: Uint8Array<ArrayBuffer>) => Promise<void>;
};

/** The browser's own fetch and FontFace; a test passes its own. */
export function liveFontDeps(document: Pick<Document, "fonts">): FontDeps {
  return {
    fetchBytes: async (url) => {
      const bytes = await readWholeFile(url);
      if (bytes === "too large") throw new Error("the font file is over its cap");
      return bytes;
    },
    addFace: async (family, bytes) => {
      const face = new FontFace(family, bytes);
      await face.load();
      document.fonts.add(face);
    },
  };
}

function sampleLine(document: Document, family: string, size: number) {
  const line = document.createElement("p");
  line.className = "font-sample";
  line.style.fontFamily = family;
  line.style.fontSize = `${size}px`;
  line.textContent = FONT_SAMPLE_TEXT;
  return line;
}

export function buildFont(
  host: ViewerReadOnlyDeps,
  rec: BytesRecord,
  deps: FontDeps = liveFontDeps(host.document),
) {
  const { document, setPathLabel, detachBtnHtml } = host;
  const el = document.createElement("div");
  el.className = "viewer font-viewer";
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="font-meta"></span>
      <span class="spacer"></span>
      ${detachBtnHtml(rec)}
    </div>
    <div class="viewer-body font-body"></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);

  const meta = need(el, ".font-meta");
  const body = need(el, ".font-body");
  need(el, '[data-act="detach"]').addEventListener("click", () => {
    host.detachClick(rec);
  });
  const family = `ralphy-font-${rec.uid}`;
  const url = fileBytesUrl(rec.project, rec.path, rec.checkout);
  void deps
    .fetchBytes(url)
    .then((bytes) => deps.addFace(family, bytes))
    .then(
      () => {
        for (const size of SAMPLE_SIZES) body.append(sampleLine(document, family, size));
        return null;
      },
      (err: unknown) => {
        devError("[workbench] font failed to load", err);
        meta.textContent = FONT_ERROR_TEXT;
      },
    );
  rec.el = el;
}
