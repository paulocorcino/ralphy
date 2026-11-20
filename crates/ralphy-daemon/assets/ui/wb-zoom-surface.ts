/* ---------------------------------------------------------------------------
   ralphy workbench — the zoom surface: the gestures and the view state

   `attachZoomSurface` wires an element and the stage inside it: wheel, pinch,
   drag and taps. The arithmetic is in `wb-diagram-zoom.ts`. The element needs
   nothing but pointer events and a size, so any pane can use it.
--------------------------------------------------------------------------- */
import { dragBegins, dragThreshold } from "./wb-console-input.ts";
import {
  clampPan,
  fitScale,
  fitView,
  minScale,
  panBy,
  pinchView,
  wheelFactor,
  zoomAt,
} from "./wb-diagram-zoom.ts";
import type { Pair, Point, Size, View } from "./wb-diagram-zoom.ts";

/** Two taps closer than this are a double tap. */
const DOUBLE_TAP_MS = 300;

export type Controller = {
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
function createController(
  surface: HTMLElement,
  stage: HTMLElement,
  size: Size,
  cap: number,
): Controller {
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
  const zoom = (factor: number, at: Point) => {
    change(zoomAt(view, factor, at, minScale(fitNow())));
  };
  return {
    fit,
    zoom,
    actual: (at) => {
      zoom(1 / view.scale, at);
    },
    toggle: (at) => {
      if (Math.abs(view.scale - fitNow()) < 1e-6) zoom(1 / view.scale, at);
      else fit();
    },
    pan: (dx, dy) => {
      change(panBy(view, dx, dy));
    },
    pinch: (prev, next) => {
      change(pinchView(view, prev, next, minScale(fitNow())));
    },
    resize: () => {
      if (fitted) fit();
      else show(view);
    },
  };
}

// One finger drags, two fingers pinch, a tap on the empty ground closes, and
// two quick taps switch between the fit and the actual size. A press becomes
// a drag only past the drag threshold, and the first drag move covers the
// whole distance from where the press began (WORKBENCH-BUILD-GUIDE, Touch).
function attachGestures(
  surface: HTMLElement,
  ctl: Controller,
  local: (ev: MouseEvent) => Point,
  close: () => void,
) {
  const points = new Map<number, Point>();
  let start: Point = { x: 0, y: 0 };
  let last: Point = start;
  let moved = false;
  let tapAt = -Infinity;
  // Pointer capture makes the `pointerup` target the surface whatever was
  // under the finger, so the target of the press is the one that is read.
  let onGround = false;
  const pair = (): Pair | null => {
    const [a, b] = [...points.values()];
    return a && b ? [a, b] : null;
  };
  const tap = (ev: PointerEvent) => {
    if (onGround) {
      close();
      return;
    }
    if (ev.timeStamp - tapAt < DOUBLE_TAP_MS) ctl.toggle(local(ev));
    tapAt = ev.timeStamp;
  };
  surface.addEventListener("pointerdown", (ev) => {
    // oxlint-disable-next-line typescript/no-unnecessary-condition -- a test's surface has none
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
      const next = pair();
      if (prev && next) ctl.pinch(prev, next);
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
    const remaining = [...points.values()][0];
    if (remaining) {
      last = remaining;
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

/** What the view shows: the moving `stage`, its own `size`, and the largest scale of a fit. */
export type ZoomBox = { stage: HTMLElement; size: Size; cap: number };

/**
 * Wire `el` to move and scale `box.stage`. A tap on the empty ground of `el`
 * calls `onGroundTap`. Returns the controller of the view.
 */
export function attachZoomSurface(el: HTMLElement, box: ZoomBox, onGroundTap: () => void) {
  const controller = createController(el, box.stage, box.size, box.cap);
  const local = (ev: MouseEvent): Point => {
    const rect = el.getBoundingClientRect();
    return { x: ev.clientX - rect.left, y: ev.clientY - rect.top };
  };
  attachGestures(el, controller, local, onGroundTap);
  return controller;
}
