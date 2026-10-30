/* ---------------------------------------------------------------------------
   The window gestures: the drag by the titlebar, the titlebar's touch
   gestures and the resize by the eight handles of a console window, or of a
   note card that drags and resizes the same way (ADR-0064 §8).

   `createWindowGestures(deps)` returns `makeDraggable`, `wireTitleTouch` and
   `startResize`. `createChrome` in `wb-stage-chrome.ts` builds it and returns
   the three functions with its own. They read the console only through
   `deps`. The owner of the elements under a gesture is `wb-stage-stack.ts`.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import { inTab } from "./wb-console-place.ts";
import { required } from "./wb-dom-need.ts";
import type { Gestures } from "./wb-stage-stack.ts";
import type { ConsoleWin, DeskWindowFields, Point, Rect, Size } from "./wb-types.d.ts";

const { dragThreshold, dragBegins, isDoubleTap, HOLD_MS } = WBConsoleInput;
const { RESIZE_MIN, resizeRect } = WBGeometry;

// The hooks of a gesture that is not a console window's: a note card drags and
// resizes by the same gesture (ADR-0064 §8), locked by its own record and
// persisted into the notes. `min` is the smallest size of a resize.
export type GestureHooks = { locked?: () => boolean; onDrop?: () => void };
export type ResizeHooks = GestureHooks & { min?: Size };

// What the window gestures read from the console, and nothing else.
export type WindowGestureDeps = {
  // The console's page: a drag ends on a `blur` of its window.
  window: Window;
  // The page the gestures listen on.
  document: Document;
  // The elements under a gesture; shared with the fence gestures.
  gestures: Gestures;
  // The plane; a gesture runs only on a page that has it.
  stage: () => HTMLElement | null;
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: { grow?: boolean }) => void;
  // Whether a window is locked, by itself or by its fence.
  isLocked: (win: ConsoleWin) => boolean;
  // The auto-pan loop of a drag at a viewport edge.
  autoPan: (
    node: HTMLElement,
    place: (pointer: Point) => void,
  ) => { follow: (pointer: Point) => void; stop: () => void };
  // Whether a rename is allowed here, and the rename itself.
  canRename: () => boolean;
  startRename: (win: ConsoleWin, span: HTMLElement) => void;
  // Raises a window or a card.
  focusWin: (win: HTMLElement) => void;
  // Whether a window is in full screen.
  isFull: (win: HTMLElement) => boolean;
  // A window's box read from the DOM, and the write of its fields to the desk.
  restoreRect: (el: HTMLElement) => Rect;
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
};

export function createWindowGestures(deps: WindowGestureDeps) {
  const {
    window,
    document,
    gestures,
    stage,
    applyExtent,
    isLocked,
    autoPan,
    canRename,
    startRename,
    focusWin,
    isFull,
    restoreRect,
    setWin,
  } = deps;

  // Drag by the titlebar, clamped to the STAGE (control buttons still click).
  // Coordinates are plane pixels: the stage's client rect carries the viewport's
  // scroll shift. The origin is pinned at 0, so no drag writes a negative
  // left/top. POINTER, not mouse: iOS synthesizes mouse events only AFTER a tap
  // resolves, never during a drag, so a `mousedown` titlebar fell through to
  // text selection. `touch-action: none` on the handle is required — without
  // it the browser claims the gesture as a scroll and fires `pointercancel`.
  // `opts` is the note card's seam (ADR-0064 §8): a card drags by the same
  // gesture — threshold, auto-pan, Escape, the lot — but it is locked by its
  // own record and persisted into `notes`, not into `desk`. Absent, the two
  // hooks are the window's, which is every existing caller.
  function makeDraggable(win: HTMLElement, handle: HTMLElement, opts?: GestureHooks) {
    // Without `opts` the element is a console window.
    const heldFast = opts?.locked || (() => isLocked(win as ConsoleWin));
    // The rect at the end of the drag, computed at the act.
    const persist = opts?.onDrop || (() => setWin(win as ConsoleWin, { rect: restoreRect(win) }));
    handle.addEventListener("pointerdown", (e) => {
      if ((e.target as HTMLElement).closest("button, .session-name-input")) return;
      // Primary button only: a right/middle press is followed by `contextmenu`
      // (or no `pointerup`), stranding `onMove` on the document. `isPrimary` is
      // the touch half: a second finger opens its own stream.
      if (e.button !== 0 || !e.isPrimary) return;
      const pointerId = e.pointerId;
      focusWin(win);
      // No drag while maximized (double-click still restores) or fullscreen —
      // the top layer ignores the move while the drag REWRITES the inline rect.
      if (win.classList.contains("maximized") || win.classList.contains("column") || inTab(win) || isFull(win)) return;
      // Locked in place — by its own record or by the fence holding it.
      if (heldFast()) return;
      gestures.begin(win);
      const rect = win.getBoundingClientRect();
      const offX = e.clientX - rect.left;
      const offY = e.clientY - rect.top;
      // Armed only past `dragThreshold`: until then a press is a tap that
      // focuses and nothing more. `offX/offY` were taken above, so the first
      // placement after arming lands the full distance from the grab point.
      const threshold = dragThreshold(e.pointerType);
      const pressed = { x: e.clientX, y: e.clientY };
      let armed = false;
      // Put the window under `pointer` (a CLIENT point). Read the stage LIVE:
      // `applyExtent({grow:true})` widens it, and a wheel or auto-pan mid-drag
      // shifts its origin under a cached rect.
      const place = (pointer: Point) => {
        const st = required(stage(), "the stage");
        const origin = st.getBoundingClientRect();
        const x = pointer.x - origin.left - offX;
        const y = pointer.y - origin.top - offY;
        win.style.left = Math.max(0, Math.min(x, st.offsetWidth - rect.width)) + "px";
        win.style.top = Math.max(0, Math.min(y, st.offsetHeight - rect.height)) + "px";
        applyExtent({ grow: true });
      };
      // Holding the window against the viewport edge scrolls the plane under it.
      // A window closed mid-drag ends the loop: nothing is left to carry.
      const pan = autoPan(win, place);
      const onMove = (ev: PointerEvent) => {
        // Another pointer's stream (a second finger, the mouse during a touch drag).
        if (ev.pointerId !== pointerId) return;
        // `pointerup` is NOT guaranteed: a native context menu mid-drag or an
        // alt-tab with the button held swallows it, and the next pointerdown
        // installs its OWN closures, so nothing later could remove this pair.
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const last = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins(pressed, last, threshold)) return;
          armed = true;
        }
        place(last);
        pan.follow(last);
      };
      // Escape ends the drag where the window sits — no revert: a keyboard exit
      // from a loop whose mouseup may never arrive.
      const onKey = (ev: KeyboardEvent) => {
        if (ev.key === "Escape") onUp();
      };
      const onUp = () => {
        pan.stop();
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch drag that the system takes over (an edge swipe, a call coming
        // in) ends in `pointercancel` and NEVER in `pointerup`.
        document.removeEventListener("pointercancel", onUp);
        document.removeEventListener("contextmenu", onUp);
        document.removeEventListener("keydown", onKey);
        window.removeEventListener("blur", onUp);
        applyExtent();
        // A tap persists NOTHING: it moved nothing.
        if (armed) persist();
        gestures.end(win);
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      // `blur`: a native menu or another window took focus and the pointer
      // never comes back to deliver `buttons === 0`.
      window.addEventListener("blur", onUp);
      // `contextmenu`: a menu over the page ends the gesture whether or not the
      // browser also blurs; a double `onUp` is idempotent.
      document.addEventListener("contextmenu", onUp);
      document.addEventListener("keydown", onKey);
      e.preventDefault();
    });
  }

  // The titlebar's touch gestures. A mouse keeps `dblclick`; a finger or a pen
  // gets two taps → `onDoubleTap` (anywhere but a button, the name included),
  // and a hold on the name → the rename. `win._lastPointerType` tells both
  // `dblclick` listeners which input made the press, so a browser that also
  // turns two taps into a `dblclick` (Chrome on Android) does not act twice.
  // The rename opens on the RELEASE after the hold, not when the timer fires:
  // iOS raises the keyboard only for a `focus()` inside an input event.
  function wireTitleTouch(win: ConsoleWin, titlebar: HTMLElement, onDoubleTap: () => void) {
    // Seen on an iPhone (2026-10-03): the double tap that maximized also
    // zoomed the page, although the bar has `touch-action: none`. Safari does
    // not zoom on a double tap whose `touchend` is cancelled. A button's
    // `touchend` is not cancelled: it is what makes the button's `click`.
    titlebar.addEventListener(
      "touchend",
      (e) => {
        if (!(e.target as HTMLElement).closest("button, .session-name-input")) e.preventDefault();
      },
      { passive: false },
    );
    titlebar.addEventListener("pointerdown", (e) => {
      win._lastPointerType = e.pointerType;
      if (e.pointerType === "mouse" || !e.isPrimary) return;
      if ((e.target as HTMLElement).closest("button, .session-name-input")) return;
      // No compatibility mouse events: a `mousedown` after the release would
      // take focus from the new name input, and its `blur` ends the edit.
      e.preventDefault();
      const pointerId = e.pointerId;
      const pressed = { x: e.clientX, y: e.clientY };
      const threshold = dragThreshold(e.pointerType);
      const span = canRename() ? (e.target as HTMLElement).closest<HTMLElement>(".session-name") : null;
      let moved = false;
      let held = false;
      let timer: ReturnType<typeof setTimeout> | null = span
        ? setTimeout(() => {
            timer = null;
            held = true;
          }, HOLD_MS)
        : null;
      const onMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return;
        // `pointerup` is not guaranteed (see `makeDraggable`).
        if (ev.buttons === 0) {
          finish(null);
          return;
        }
        if (!moved && dragBegins(pressed, { x: ev.clientX, y: ev.clientY }, threshold)) {
          moved = true;
          held = false;
          clearTimeout(timer);
          timer = null;
        }
      };
      const onUp = (ev: PointerEvent) => {
        if (ev.pointerId === pointerId) finish(ev);
      };
      const onCancel = (ev: PointerEvent) => {
        if (ev.pointerId === pointerId) finish(null);
      };
      // Android fires `contextmenu` on a held finger; its menu is not ours.
      const onMenu = (ev: MouseEvent) => ev.preventDefault();
      const finish = (up: PointerEvent | null) => {
        clearTimeout(timer);
        timer = null;
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("pointercancel", onCancel);
        document.removeEventListener("contextmenu", onMenu, true);
        if (!up || moved) {
          win._lastTap = null;
          return;
        }
        // `held` is set only by the timer, which exists only with a `span`.
        if (held && span) {
          win._lastTap = null;
          startRename(win, span);
          return;
        }
        const tap = { t: up.timeStamp, x: up.clientX, y: up.clientY };
        if (isDoubleTap(win._lastTap, tap) && !isFull(win)) {
          win._lastTap = null;
          onDoubleTap();
        } else {
          win._lastTap = tap;
        }
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onCancel);
      document.addEventListener("contextmenu", onMenu, true);
    });
  }

  // Wire one handle: drag it and the window's rect follows `resizeRect`. Every
  // exit path (mouseup anywhere on the document) drops BOTH listeners and
  // persists exactly once.
  // Same seam as `makeDraggable`'s, for the same second caller.
  function startResize(win: HTMLElement, dir: string, opts?: ResizeHooks) {
    // Without `opts` the element is a console window.
    const heldFast = opts?.locked || (() => isLocked(win as ConsoleWin));
    const persist = opts?.onDrop || (() => setWin(win as ConsoleWin, { rect: restoreRect(win) }));
    const min = opts?.min || RESIZE_MIN;
    return (e: PointerEvent) => {
      if (e.button !== 0 || !e.isPrimary) return; // see makeDraggable
      const pointerId = e.pointerId;
      focusWin(win);
      if (win.classList.contains("maximized") || win.classList.contains("column") || inTab(win) || isFull(win)) return;
      if (heldFast()) return; // the JS guard is the truth; the CSS only hides the bands
      gestures.begin(win);
      const rect = {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
      // The STAGE is the bound (ADR-0051 §5). Captured ONCE: a live re-read
      // feeds back on itself — the extent this gesture grows becomes the bound
      // of its next move, inflating the window ~one margin per mousemove.
      const st = required(stage(), "the stage");
      const bounds = { width: st.offsetWidth, height: st.offsetHeight };
      const startX = e.clientX;
      const startY = e.clientY;
      // See makeDraggable: a press under the threshold is a tap on the band.
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      const onMove = (ev: PointerEvent) => {
        // A second finger opens its own stream and is not this gesture.
        if (ev.pointerId !== pointerId) return;
        if (!armed) {
          if (!dragBegins({ x: startX, y: startY }, { x: ev.clientX, y: ev.clientY }, threshold))
            return;
          armed = true;
        }
        const out = resizeRect(
          dir,
          rect,
          { dx: ev.clientX - startX, dy: ev.clientY - startY },
          min,
          bounds,
        );
        win.style.left = out.left + "px";
        win.style.top = out.top + "px";
        win.style.width = out.width + "px";
        win.style.height = out.height + "px";
      };
      const onUp = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        // A touch resize the system takes over ends here and nowhere else.
        document.removeEventListener("pointercancel", onUp);
        applyExtent();
        if (armed) persist(); // a tap on a band changed nothing
        gestures.end(win);
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      e.preventDefault();
      e.stopPropagation();
    };
  }

  return { makeDraggable, wireTitleTouch, startResize };
}
