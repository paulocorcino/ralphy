/* ---------------------------------------------------------------------------
   ralphy workbench — the "Enlarge" button and the zoom view of a picture

   `decorateZoomable` puts an "Enlarge" button on each Mermaid diagram and each
   large image or SVG of a rendered markdown article. The button opens a view
   that fills the window, with zoom (wheel, pinch, keys, buttons) and pan
   (drag). The arithmetic of the view is in `wb-diagram-zoom.ts`.

   The view is built with DOM calls and appended to `body`, so it works in the
   detached file window too, which has no Alpine. It holds no state outside
   its own closure and a page shows one at a time.

   Importing this module does nothing: the entry module of a page calls
   `wireDiagramZoom` once.
--------------------------------------------------------------------------- */
import { ZOOM_STEP } from "./wb-diagram-zoom.ts";
import type { Point, Size } from "./wb-diagram-zoom.ts";
import { targetClosest } from "./wb-dom-need.ts";
import { attachZoomSurface } from "./wb-zoom-surface.ts";

type Picture = HTMLImageElement | SVGSVGElement;

/** An image or SVG narrower than this is an icon or a badge: no button. */
const MIN_PICTURE_WIDTH = 120;

/** A raster image is not drawn above its own pixels when the view opens; a vector is. */
const RASTER_CAP = 1;
const VECTOR_CAP = 4;

const ENLARGE = "Enlarge";

function isRaster(el: Picture): el is HTMLImageElement {
  return "naturalWidth" in el;
}

// The size the picture has by itself. A Mermaid SVG says it in `viewBox`; its
// `width` is `100%`, which says nothing.
function pictureSize(el: Picture): Size {
  if (isRaster(el)) return { w: el.naturalWidth, h: el.naturalHeight };
  // oxlint-disable-next-line typescript/no-unnecessary-condition -- a test's picture has no viewBox
  const box = el.viewBox?.baseVal ?? { width: 0, height: 0 };
  if (box.width > 0 && box.height > 0) return { w: box.width, h: box.height };
  const rect = el.getBoundingClientRect();
  return { w: rect.width, h: rect.height };
}

/* --- the button ----------------------------------------------------------- */

function wrap(el: Element, tag: "div" | "span") {
  const doc = el.ownerDocument;
  const box = doc.createElement(tag);
  box.className = tag === "div" ? "zoomable zoomable-block" : "zoomable";
  const button = doc.createElement("button");
  button.type = "button";
  button.className = "zoom-btn";
  button.title = ENLARGE;
  button.setAttribute("aria-label", ENLARGE);
  const icon = doc.createElement("i");
  icon.className = "bi bi-arrows-fullscreen";
  button.append(icon);
  el.replaceWith(box);
  box.append(el, button);
}

// A picture inside a link is a badge or a button of the document; a blocked
// image has been replaced by a notice and never loads.
function considerPicture(el: Picture) {
  if (el.closest(".zoomable, a")) return;
  if (pictureSize(el).w >= MIN_PICTURE_WIDTH) {
    wrap(el, "span");
    return;
  }
  // The image resolves its bytes after the article is built, so its size is
  // known only when it loads.
  if (isRaster(el)) {
    el.addEventListener("load", () => {
      considerPicture(el);
    });
  }
}

/**
 * Put the "Enlarge" button on each diagram and large picture of `article`.
 * Safe to call again on the same article: a picture that has its button is
 * skipped. The button is a sibling of the Mermaid holder, never a child,
 * because the holder's content is replaced when the diagram is drawn.
 */
export function decorateZoomable(article: Element) {
  article.querySelectorAll<HTMLElement>(".mermaid").forEach((holder) => {
    if (!holder.closest(".zoomable")) wrap(holder, "div");
  });
  article.querySelectorAll<Picture>("img, svg").forEach((el) => {
    considerPicture(el);
  });
}

/* --- the view ------------------------------------------------------------- */

// The copy of the picture the view shows. A Mermaid SVG keeps its `id`: its
// own stylesheet is written against that `id`, so the copy and the original
// are drawn by the same rules.
function copyOf(doc: Document, src: Picture, size: Size) {
  if (isRaster(src)) {
    const img = doc.createElement("img");
    img.src = src.currentSrc || src.src;
    img.alt = src.alt;
    img.draggable = false;
    return img;
  }
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- lib.dom types any clone as Node
  const svg = src.cloneNode(true) as SVGSVGElement;
  svg.style.maxWidth = "none";
  svg.setAttribute("width", String(size.w));
  svg.setAttribute("height", String(size.h));
  return svg;
}

const BAR: ReadonlyArray<{ act: string; icon: string; label: string; text?: string }> = [
  { act: "out", icon: "bi-zoom-out", label: "Zoom out" },
  { act: "in", icon: "bi-zoom-in", label: "Zoom in" },
  { act: "fit", icon: "bi-arrows-angle-contract", label: "Fit to window" },
  { act: "actual", icon: "", label: "Actual size", text: "100%" },
  { act: "close", icon: "bi-x-lg", label: "Close" },
];

function buildBar(doc: Document) {
  const bar = doc.createElement("div");
  bar.className = "zoom-bar";
  for (const item of BAR) {
    const button = doc.createElement("button");
    button.type = "button";
    button.className = "zoom-bar-btn";
    button.dataset["zoom"] = item.act;
    button.title = item.label;
    button.setAttribute("aria-label", item.label);
    const text = item.text ?? "";
    if (text === "") {
      const icon = doc.createElement("i");
      icon.className = `bi ${item.icon}`;
      button.append(icon);
    } else {
      button.textContent = text;
    }
    bar.append(button);
  }
  return bar;
}

function buildOverlay(doc: Document, content: Element, size: Size) {
  const overlay = doc.createElement("div");
  overlay.className = "zoom-overlay";
  overlay.setAttribute("role", "dialog");
  overlay.setAttribute("aria-modal", "true");
  overlay.setAttribute("aria-label", "Enlarged view");
  overlay.tabIndex = -1;
  const surface = doc.createElement("div");
  surface.className = "zoom-surface";
  const stage = doc.createElement("div");
  stage.className = "zoom-stage";
  stage.style.width = `${size.w}px`;
  stage.style.height = `${size.h}px`;
  stage.append(content);
  surface.append(stage);
  const bar = buildBar(doc);
  overlay.append(surface, bar);
  return { overlay, surface, stage, bar };
}

// The keys act inside the view and stop there, so an Escape that closes the
// view does not also close a dialog under it. Tab stays inside the bar.
function trapTab(ev: KeyboardEvent, overlay: HTMLElement, bar: HTMLElement) {
  const buttons = [...bar.querySelectorAll<HTMLElement>("button")];
  const first = buttons[0];
  const last = buttons.at(-1);
  if (!first || !last) return;
  const active = overlay.ownerDocument.activeElement;
  if (ev.shiftKey && (active === first || active === overlay)) last.focus();
  else if (!ev.shiftKey && active === last) first.focus();
  else return;
  ev.preventDefault();
}

function attachKeys(overlay: HTMLElement, bar: HTMLElement, actions: Record<string, () => void>) {
  const keys: Record<string, string> = {
    Escape: "close",
    "+": "in",
    "=": "in",
    "-": "out",
    "0": "fit",
  };
  overlay.addEventListener("keydown", (ev) => {
    if (ev.key === "Tab") {
      trapTab(ev, overlay, bar);
      return;
    }
    const act = keys[ev.key];
    if (act === undefined) return;
    ev.preventDefault();
    ev.stopPropagation();
    actions[act]?.();
  });
}

/**
 * Open the zoom view over `src`, a diagram or an image of the page. It
 * returns `null` when a view is already open or the picture has no size
 * (a diagram that is not drawn yet). Closing it gives the focus back to
 * `opener`.
 */
export function openZoom(win: Window, doc: Document, src: Picture, opener: HTMLElement | null) {
  const size = pictureSize(src);
  if (doc.querySelector(".zoom-overlay") || size.w <= 0 || size.h <= 0) return null;
  const { overlay, surface, stage, bar } = buildOverlay(doc, copyOf(doc, src, size), size);
  doc.body.append(overlay);
  const cap = isRaster(src) ? RASTER_CAP : VECTOR_CAP;
  const ctl = attachZoomSurface(surface, { stage, size, cap }, () => {
    close();
  });
  const middle = (): Point => ({ x: surface.clientWidth / 2, y: surface.clientHeight / 2 });
  const onResize = () => {
    ctl.resize();
  };
  const close = () => {
    win.removeEventListener("resize", onResize);
    overlay.remove();
    opener?.focus();
  };
  const actions: Record<string, () => void> = {
    out: () => {
      ctl.zoom(1 / ZOOM_STEP, middle());
    },
    in: () => {
      ctl.zoom(ZOOM_STEP, middle());
    },
    fit: () => {
      ctl.fit();
    },
    actual: () => {
      ctl.actual(middle());
    },
    close,
  };
  bar.addEventListener("click", (ev) => {
    const act = targetClosest(ev, "[data-zoom]")?.dataset["zoom"];
    if (act !== undefined && act !== "") actions[act]?.();
  });
  attachKeys(overlay, bar, actions);
  win.addEventListener("resize", onResize);
  ctl.fit();
  bar.querySelector<HTMLElement>('[data-zoom="close"]')?.focus();
  return { close };
}

/** Open the view when an "Enlarge" button is pressed, on any article of the page. */
export function wireDiagramZoom(win: Window, doc: Document) {
  doc.addEventListener("click", (ev) => {
    const button = targetClosest(ev, ".zoom-btn");
    const picture = button?.parentElement?.querySelector<Picture>("svg, img");
    if (!button || !picture) return;
    ev.preventDefault();
    openZoom(win, doc, picture, button);
  });
}
