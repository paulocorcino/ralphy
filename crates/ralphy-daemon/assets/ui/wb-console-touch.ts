/* ---------------------------------------------------------------------------
   The touch gesture of one console's terminal: the one-finger scroll and its
   fling, the two-finger pan of the plane, and the line selection the key
   bar's `sel` button arms.

   `wireTouch(deps)` adds the touch listeners to the window body and returns
   what the terminal's handle uses. `wb-console-terminal.ts` calls it once per
   terminal; the folds it applies are `wb-console-input.ts` (#621).
--------------------------------------------------------------------------- */
import * as WBConsoleInput from "./wb-console-input.ts";
import type { TerminalOpts } from "./wb-console-terminal.ts";
import type { ScrollWatch } from "./wb-console-probe.ts";

const { touchScrollLines, touchScrollTarget, touchGesture, touchCentroid, flingStep, selectionRow, FLING_MIN } =
  WBConsoleInput;

// What the touch gesture reads from the terminal, and nothing else.
export type TouchDeps = {
  // The terminal the gesture scrolls and selects in.
  term: XtermTerminal;
  // The window body the touch listeners go on.
  body: HTMLElement;
  // The plane and the viewport; null before the page has them.
  stage: () => HTMLElement | null;
  workspace: () => HTMLElement | null;
  // Stops a jump of the viewport in flight: a two-finger pan outranks it.
  cancelSlide: () => void;
  // The terminal's opts: `onSelecting` arms or disarms the key bar's button.
  opts: Pick<TerminalOpts, "onSelecting">;
  // The scroll probe's hooks, in a developer build only (wb-console-probe.ts).
  probe?: Pick<ScrollWatch, "moved" | "step">;
};

export function wireTouch(deps: TouchDeps) {
  const { term, body, stage, workspace, cancelSlide, opts, probe } = deps;

  // The touch gesture this terminal owns (`touchScrollLines`). Single finger
  // only; the stylesheet's `touch-action: none` already told the browser the
  // console is not a pan surface, and A+/A− is a terminal's zoom.
  let touchY: number | null = null;
  let touchX = 0;
  let touchLastY = 0;
  let touchAccum = 0;
  let touchLastAt = 0;
  let touchVelocity = 0;
  let fling = 0;
  // THE LINE-SELECTION MODE: xterm selects only through mouse events and the
  // touch handlers below spend the finger on scrolling. Armed by the key
  // bar's `sel` button for ONE gesture: a single-finger drag selects whole
  // buffer lines (`selectLines` is the public API; cell-precise is not), and
  // lifting the finger disarms it, leaving the selection for the copy button.
  let selecting = false;
  let selStart: number | null = null;
  const setSelecting = (on: boolean) => {
    selecting = !!on;
    selStart = null;
    if (typeof opts.onSelecting === "function") opts.onSelecting(selecting);
  };
  const rowAt = (clientY: number) => {
    const screen = term.element?.querySelector(".xterm-screen");
    const top = screen ? screen.getBoundingClientRect().top : 0;
    return selectionRow(clientY, top, cellHeight(), term.rows, term.buffer.active.viewportY);
  };
  const cellHeight = () => {
    const el = term.element;
    const rows = term.rows;
    return el && rows > 0 ? el.clientHeight / rows : 0;
  };
  // Scroll by a fractional number of lines, carrying the remainder (a slow
  // drag moves less than one row per event). Coalesced to ONE scroll per
  // frame: `touchmove` fires faster than the display refreshes, and several
  // paints in one frame show up as a half-updated screen on a slow renderer.
  let scrollRaf = 0;
  // The app's share goes in through xterm's OWN wheel listener as line-mode
  // wheel events — one per line, so `consumeWheelEvent` neither dampens nor
  // batches them — at the finger's coordinates (a mouse report carries the
  // cell). xterm then does what it does for the trackpad.
  const wheelToApp = (lines: number) => {
    const el = term.element;
    if (!el) return;
    const deltaY = Math.sign(lines);
    for (let n = Math.abs(lines); n > 0; n--) {
      el.dispatchEvent(
        new WheelEvent("wheel", {
          deltaY,
          deltaMode: WheelEvent.DOM_DELTA_LINE,
          clientX: touchX,
          clientY: touchY ?? touchLastY,
          bubbles: true,
          cancelable: true,
        }),
      );
    }
  };
  const flushScroll = () => {
    scrollRaf = 0;
    const whole = Math.trunc(touchAccum);
    if (whole === 0) return;
    touchAccum -= whole;
    // Decided per flush, not per gesture: an app can take the mouse or drop
    // into the alternate buffer while a finger is still down.
    if (touchScrollTarget(term.modes.mouseTrackingMode, term.buffer.active.type) === "app") wheelToApp(whole);
    else term.scrollLines(whole);
  };
  // One scroll step, timed for the scroll probe when there is one.
  const timedFlush = (input: "touch" | "glide") => {
    if (!probe) return flushScroll();
    const started = performance.now();
    flushScroll();
    probe.moved(input);
    probe.step(performance.now() - started);
  };
  const scrollByPixels = (dy: number) => {
    touchAccum += touchScrollLines(dy, cellHeight());
    if (!scrollRaf) scrollRaf = requestAnimationFrame(() => timedFlush("touch"));
  };
  const stopScroll = () => {
    if (scrollRaf) cancelAnimationFrame(scrollRaf);
    scrollRaf = 0;
  };
  const stopFling = () => {
    if (fling) cancelAnimationFrame(fling);
    fling = 0;
  };
  // Repaint every row: a renderer that left a row half-drawn mid-gesture is
  // corrected once at the end rather than every frame.
  const refreshScreen = () => {
    try {
      term.refresh(0, term.rows - 1);
    } catch {}
  };
  // A two-finger pan of the plane, live until either finger lifts. The
  // remaining finger does NOT resume a scroll: it never had a `touchstart`.
  let pan: { x: number; y: number; left: number; top: number } | null = null;
  const stopPan = () => {
    if (!pan) return;
    pan = null;
    stage()?.classList.remove("panning");
  };
  body.addEventListener(
    "touchstart",
    (e: TouchEvent) => {
      stopFling();
      // Armed selection. Prevented so the synthesized click never reaches
      // xterm's mousedown, which would clear the selection; the textarea
      // keeps its focus, so the keyboard stays up.
      if (selecting && e.touches.length === 1) {
        e.preventDefault();
        touchY = null;
        selStart = rowAt(e.touches[0].clientY);
        try {
          term.selectLines(selStart, selStart);
        } catch {}
        return;
      }
      const ws = workspace();
      const gesture = touchGesture(e.touches.length, !!ws?.classList.contains("maxlock"));
      if (gesture === "canvas") {
        // A second finger ends the terminal's gesture: the plane owns the touch.
        touchY = null;
        stopScroll();
        touchAccum = 0;
        // The operator's hand outranks a jump in flight (as `onFloorDown`).
        cancelSlide();
        const c = touchCentroid(e.touches);
        pan = { x: c.x, y: c.y, left: ws!.scrollLeft, top: ws!.scrollTop };
        stage()?.classList.add("panning");
        return;
      }
      stopPan();
      if (gesture !== "terminal") {
        touchY = null;
        return;
      }
      touchY = e.touches[0].clientY;
      touchX = e.touches[0].clientX;
      touchAccum = 0;
      touchVelocity = 0;
      touchLastAt = e.timeStamp;
      // NOT prevented: the tap must reach xterm, or the terminal never takes
      // focus and the on-screen keyboard never opens.
    },
    // Non-passive ONLY for the armed-selection branch above.
    { passive: false },
  );
  body.addEventListener(
    "touchmove",
    (e: TouchEvent) => {
      if (selecting && selStart != null) {
        dragSelection(e, selStart);
        return;
      }
      if (pan) {
        dragPan(e, pan);
        return;
      }
      if (touchY == null || e.touches.length !== 1) return;
      const y = e.touches[0].clientY;
      touchX = e.touches[0].clientX;
      const dy = y - touchY;
      touchY = y;
      const dt = e.timeStamp - touchLastAt;
      touchLastAt = e.timeStamp;
      if (dt > 0) touchVelocity = dy / dt;
      scrollByPixels(dy);
      // Without this the canvas underneath pans instead.
      e.preventDefault();
    },
    { passive: false },
  );
  // One finger on an armed selection selects the lines from the first row
  // touched to the row under the finger.
  function dragSelection(e: TouchEvent, from: number) {
    if (e.touches.length === 1) {
      const row = rowAt(e.touches[0].clientY);
      try {
        term.selectLines(Math.min(from, row), Math.max(from, row));
      } catch {
        // xterm refused the range: the selection keeps its last lines.
      }
    }
    e.preventDefault();
  }
  // Two fingers pan the workspace under the console.
  function dragPan(e: TouchEvent, start: NonNullable<typeof pan>) {
    if (e.touches.length !== 2) return;
    const ws = workspace();
    const c = touchCentroid(e.touches);
    if (ws) {
      ws.scrollLeft = start.left - (c.x - start.x);
      ws.scrollTop = start.top - (c.y - start.y);
    }
    e.preventDefault();
  }
  const endTouch = (e: TouchEvent) => {
    // The selection stays (the copy button reads it); the arming does not.
    if (selecting) {
      if (e.touches.length === 0) setSelecting(false);
      return;
    }
    if (pan) {
      if (e.touches.length < 2) stopPan();
      return;
    }
    if (touchY == null) return;
    touchLastY = touchY;
    touchY = null;
    // A finger lifted long after it stopped moving is a hold, not a flick.
    if (e.timeStamp - touchLastAt > 80 || Math.abs(touchVelocity) < FLING_MIN) {
      refreshScreen();
      return;
    }
    let v = touchVelocity;
    let last = performance.now();
    const glide = (now: number) => {
      const step = flingStep(v, now - last);
      last = now;
      v = step.velocity;
      // Already inside a frame: flush HERE, not through `scrollByPixels`.
      touchAccum += touchScrollLines(step.dy, cellHeight());
      stopScroll();
      timedFlush("glide");
      fling = v ? requestAnimationFrame(glide) : 0;
      // The glide has stopped: correct a dropped partial paint once.
      if (!fling) refreshScreen();
    };
    fling = requestAnimationFrame(glide);
  };
  body.addEventListener("touchend", endTouch, { passive: true });
  body.addEventListener("touchcancel", endTouch, { passive: true });

  return {
    setSelecting,
    get selecting() {
      return selecting;
    },
    stopFling,
    stopScroll,
  };
}
