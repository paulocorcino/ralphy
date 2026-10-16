/* ---------------------------------------------------------------------------
   ralphy workbench — the zoom view for a diagram or an image

   A Mermaid diagram or a large image in a markdown file is shrunk to the width
   of the article, and its text is too small to read. This module gives each
   one an "Enlarge" button that opens a window-sized view with zoom and pan.

   This part is the arithmetic of that view, with no DOM: a view is a scale and
   an offset of the picture inside the window, and each function returns a new
   view. The picture is drawn at its own size with `transform-origin: 0 0`, so
   `x` and `y` are the offset of its top-left corner in the window.

   Importing this module does nothing.
--------------------------------------------------------------------------- */

export type Point = { x: number; y: number };
export type Size = { w: number; h: number };
export type View = { scale: number; x: number; y: number };

/** The largest scale, against the picture's own size. */
export const MAX_SCALE = 8;

/** One press of a zoom button or key. */
export const ZOOM_STEP = 1.25;

/** `scale` held between `min` and `MAX_SCALE`. */
export function clampScale(scale: number, min: number) {
  return Math.min(MAX_SCALE, Math.max(min, scale));
}

/**
 * The scale at which `content` fills `box` on its tighter side, never above
 * `cap` (a raster image is not drawn larger than its own pixels). A picture
 * with no size has nothing to fit and stays at 1.
 */
export function fitScale(content: Size, box: Size, cap: number) {
  if (content.w <= 0 || content.h <= 0 || box.w <= 0 || box.h <= 0) return 1;
  return Math.min(cap, box.w / content.w, box.h / content.h);
}

/**
 * The smallest scale the view may take: the fit, or the picture's own size
 * when the fit is larger (a small diagram can be shown at 100%, not only
 * enlarged).
 */
export function minScale(fit: number) {
  return Math.min(fit, 1);
}

// One axis: a picture smaller than the window is centered; a larger one may
// not leave an empty strip between its edge and the window's.
function clampAxis(pos: number, size: number, room: number) {
  if (size <= room) return (room - size) / 2;
  return Math.min(0, Math.max(room - size, pos));
}

/** `view` moved back so the picture never leaves the window. */
export function clampPan(view: View, content: Size, box: Size): View {
  return {
    scale: view.scale,
    x: clampAxis(view.x, content.w * view.scale, box.w),
    y: clampAxis(view.y, content.h * view.scale, box.h),
  };
}

/** The view that shows the whole picture, centered. */
export function fitView(content: Size, box: Size, cap: number): View {
  return clampPan({ scale: fitScale(content, box, cap), x: 0, y: 0 }, content, box);
}

/**
 * `view` zoomed by `factor` around `at`, a point in the window: the point of
 * the picture under `at` stays under it.
 */
export function zoomAt(view: View, factor: number, at: Point, min: number): View {
  const scale = clampScale(view.scale * factor, min);
  const k = scale / view.scale;
  return { scale, x: at.x - (at.x - view.x) * k, y: at.y - (at.y - view.y) * k };
}

/** The same view, moved by `dx`, `dy`. */
export function panBy(view: View, dx: number, dy: number): View {
  return { scale: view.scale, x: view.x + dx, y: view.y + dy };
}

export type Pair = readonly [Point, Point];

function middle(pair: Pair): Point {
  return { x: (pair[0].x + pair[1].x) / 2, y: (pair[0].y + pair[1].y) / 2 };
}

function spread(pair: Pair) {
  return Math.hypot(pair[0].x - pair[1].x, pair[0].y - pair[1].y);
}

/**
 * Two fingers moved from `prev` to `next`: the picture follows them. It
 * scales by the change of the distance between them and moves by the change
 * of their middle point.
 */
export function pinchView(view: View, prev: Pair, next: Pair, min: number): View {
  const before = spread(prev);
  const factor = before > 0 ? spread(next) / before : 1;
  const zoomed = zoomAt(view, factor, middle(prev), min);
  const from = middle(prev);
  const to = middle(next);
  return panBy(zoomed, to.x - from.x, to.y - from.y);
}

/** What a wheel event reports, as far as the zoom reads it. */
export type WheelLike = { deltaY: number; deltaMode: number; ctrlKey: boolean };

// A trackpad pinch arrives as a wheel event with `ctrlKey` set and small
// deltas, so it needs a larger rate than a mouse wheel.
const WHEEL_RATE = 0.002;
const PINCH_WHEEL_RATE = 0.01;
const LINE_HEIGHT = 16;

/** The factor one wheel event asks for, held to a halving or a doubling. */
export function wheelFactor(ev: WheelLike) {
  const pixels = ev.deltaMode === 1 ? ev.deltaY * LINE_HEIGHT : ev.deltaY;
  const rate = ev.ctrlKey ? PINCH_WHEEL_RATE : WHEEL_RATE;
  return Math.min(2, Math.max(0.5, Math.exp(-pixels * rate)));
}
