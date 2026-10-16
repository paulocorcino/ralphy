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
import { dragBegins, dragThreshold } from "./wb-console-input.ts";
import { clampPan, fitScale, fitView, minScale, panBy, pinchView, wheelFactor, zoomAt, ZOOM_STEP } from "./wb-diagram-zoom.ts";
import type { Pair, Point, Size, View } from "./wb-diagram-zoom.ts";

type Picture = HTMLImageElement | SVGSVGElement;

/** An image or SVG narrower than this is an icon or a badge: no button. */
const MIN_PICTURE_WIDTH = 120;

/** A raster image is not drawn above its own pixels when the view opens; a vector is. */
const RASTER_CAP = 1;
const VECTOR_CAP = 4;

/** Two taps closer than this are a double tap. */
const DOUBLE_TAP_MS = 300;

const ENLARGE = "Enlarge";

function isRaster(el: Picture): el is HTMLImageElement {
  return "naturalWidth" in el;
}

// The size the picture has by itself. A Mermaid SVG says it in `viewBox`; its
// `width` is `100%`, which says nothing.
function pictureSize(el: Picture): Size {
  if (isRaster(el)) return { w: el.naturalWidth, h: el.naturalHeight };
  const box = el.viewBox?.baseVal;
  if (box && box.width > 0 && box.height > 0) return { w: box.width, h: box.height };
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
  if (isRaster(el)) el.addEventListener("load", () => considerPicture(el));
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
  article.querySelectorAll<Picture>("img, svg").forEach(considerPicture);
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
    button.dataset.zoom = item.act;
    button.title = item.label;
    button.setAttribute("aria-label", item.label);
    if (item.text) button.textContent = item.text;
    else {
      const icon = doc.createElement("i");
      icon.className = `bi ${item.icon}`;
      button.append(icon);
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

type Controller = {
  fit(): void;
  zoom(factor: number, at: Point): void;
  actual(at: Point): void;
  toggle(at: Point): void;
  pan(dx: number, dy: number): void;
  pinch(prev: Pair, next: Pair): void;
  resize(): void;
};

// The view's state. `fitted` stays true until the operator zooms or pans, so a
// window resize keeps a fitted picture fitted and leaves a zoomed one alone.
function createController(surface: HTMLElement, stage: HTMLElement, size: Size, cap: number): Controller {
  let view: View = { scale: 1, x: 0, y: 0 };
  let fitted = true;
  const box = (): Size => ({ w: surface.clientWidth, h: surface.clientHeight });
  const fitNow = () => fitScale(size, box(), cap);
  const show = (next: View) => {
    view = clampPan(next, size, box());
    stage.style.transform = `translate(${view.x}px, ${view.y}px) scale(${view.scale})`;
  };
  const change = (next: View) => {
    fitted = false;
    show(next);
  };
  const fit = () => {
    fitted = true;
    show(fitView(size, box(), cap));
  };
  const zoom = (factor: number, at: Point) => change(zoomAt(view, factor, at, minScale(fitNow())));
  return {
    fit,
    zoom,
    actual: (at) => zoom(1 / view.scale, at),
    toggle: (at) => (Math.abs(view.scale - fitNow()) < 1e-6 ? zoom(1 / view.scale, at) : fit()),
    pan: (dx, dy) => change(panBy(view, dx, dy)),
    pinch: (prev, next) => change(pinchView(view, prev, next, minScale(fitNow()))),
    resize: () => (fitted ? fit() : show(view)),
  };
}

// One finger drags, two fingers pinch, a tap on the empty ground closes, and
// two quick taps switch between the fit and the actual size. A press becomes
// a drag only past the drag threshold, and the first drag move covers the
// whole distance from where the press began (WORKBENCH-BUILD-GUIDE, Touch).
function attachGestures(surface: HTMLElement, ctl: Controller, local: (ev: MouseEvent) => Point, close: () => void) {
  const points = new Map<number, Point>();
  let start: Point = { x: 0, y: 0 };
  let last: Point = start;
  let moved = false;
  let tapAt = -Infinity;
  // Pointer capture makes the `pointerup` target the surface whatever was
  // under the finger, so the target of the press is the one that is read.
  let onGround = false;
  const pair = (): Pair => {
    const [a, b] = [...points.values()];
    return [a, b];
  };
  const tap = (ev: PointerEvent) => {
    if (onGround) return close();
    if (ev.timeStamp - tapAt < DOUBLE_TAP_MS) ctl.toggle(local(ev));
    tapAt = ev.timeStamp;
  };
  surface.addEventListener("pointerdown", (ev) => {
    surface.setPointerCapture?.(ev.pointerId);
    points.set(ev.pointerId, local(ev));
    if (points.size === 1) {
      start = last = local(ev);
      moved = false;
      onGround = ev.target === surface;
    } else moved = true;
  });
  surface.addEventListener("pointermove", (ev) => {
    if (!points.has(ev.pointerId)) return;
    const now = local(ev);
    if (points.size === 2) {
      const prev = pair();
      points.set(ev.pointerId, now);
      ctl.pinch(prev, pair());
      return;
    }
    points.set(ev.pointerId, now);
    if (!moved && !dragBegins(start, now, dragThreshold(ev.pointerType))) return;
    moved = true;
    ctl.pan(now.x - last.x, now.y - last.y);
    last = now;
  });
  const release = (ev: PointerEvent) => {
    if (!points.delete(ev.pointerId)) return;
    // A finger that stays down after a pinch drags from where it is, and never taps.
    if (points.size > 0) {
      last = [...points.values()][0];
      moved = true;
    } else if (!moved && ev.type === "pointerup") tap(ev);
  };
  surface.addEventListener("pointerup", release);
  surface.addEventListener("pointercancel", release);
  surface.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      ctl.zoom(wheelFactor(ev), local(ev));
    },
    { passive: false },
  );
}

// The keys act inside the view and stop there, so an Escape that closes the
// view does not also close a dialog under it. Tab stays inside the bar.
function trapTab(ev: KeyboardEvent, overlay: HTMLElement, bar: HTMLElement) {
  const buttons = [...bar.querySelectorAll<HTMLElement>("button")];
  const first = buttons[0];
  const last = buttons[buttons.length - 1];
  const active = overlay.ownerDocument.activeElement;
  if (ev.shiftKey && (active === first || active === overlay)) last.focus();
  else if (!ev.shiftKey && active === last) first.focus();
  else return;
  ev.preventDefault();
}

function attachKeys(overlay: HTMLElement, bar: HTMLElement, actions: Record<string, () => void>) {
  const keys: Record<string, string> = { Escape: "close", "+": "in", "=": "in", "-": "out", "0": "fit" };
  overlay.addEventListener("keydown", (ev) => {
    if (ev.key === "Tab") return trapTab(ev, overlay, bar);
    const act = keys[ev.key];
    if (!act) return;
    ev.preventDefault();
    ev.stopPropagation();
    actions[act]();
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
  const ctl = createController(surface, stage, size, isRaster(src) ? RASTER_CAP : VECTOR_CAP);
  const local = (ev: MouseEvent): Point => {
    const rect = surface.getBoundingClientRect();
    return { x: ev.clientX - rect.left, y: ev.clientY - rect.top };
  };
  const middle = (): Point => ({ x: surface.clientWidth / 2, y: surface.clientHeight / 2 });
  const onResize = () => ctl.resize();
  const close = () => {
    win.removeEventListener("resize", onResize);
    overlay.remove();
    opener?.focus();
  };
  const actions: Record<string, () => void> = {
    out: () => ctl.zoom(1 / ZOOM_STEP, middle()),
    in: () => ctl.zoom(ZOOM_STEP, middle()),
    fit: () => ctl.fit(),
    actual: () => ctl.actual(middle()),
    close,
  };
  bar.addEventListener("click", (ev) => {
    const act = (ev.target as Element).closest("[data-zoom]")?.getAttribute("data-zoom");
    if (act) actions[act]();
  });
  attachGestures(surface, ctl, local, close);
  attachKeys(overlay, bar, actions);
  win.addEventListener("resize", onResize);
  ctl.fit();
  bar.querySelector<HTMLElement>('[data-zoom="close"]')?.focus();
  return { close };
}

/** Open the view when an "Enlarge" button is pressed, on any article of the page. */
export function wireDiagramZoom(win: Window, doc: Document) {
  doc.addEventListener("click", (ev) => {
    const button = (ev.target as Element | null)?.closest?.(".zoom-btn");
    const picture = button?.parentElement?.querySelector<Picture>("svg, img");
    if (!button || !picture) return;
    ev.preventDefault();
    openZoom(win, doc, picture, button as HTMLElement);
  });
}
