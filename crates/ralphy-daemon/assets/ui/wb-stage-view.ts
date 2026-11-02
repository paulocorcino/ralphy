/* ---------------------------------------------------------------------------
   The console's view of the plane: where this browser profile lands on the
   plane and the offset it stores, the reveal of a window, the slide to a
   fence or a note card, the auto-pan of a drag, and the plane's own pan and
   wheel.

   `createView(deps)` returns the functions the console, its chrome, its
   fences, its desk and the `WBConsole` API use (ADR-0075 D7). They read the
   console only through `deps`, and `ViewDeps` lists every read.
   `wb-console.ts` creates one per console, before its terminal.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import { sendDocument } from "./wb-events.ts";
import type { WBView } from "./wb-client-view.ts";
import type { ConsoleWin, ExtentOpts, Offset, Point, Rect, Size } from "./wb-types.d.ts";

// Plane geometry is `wb-geometry.ts` (ADR-0057): pure folds over rects.
const {
  rectHolds,
  bringIntoView,
  anchorIntoView,
  slideEase,
  viewLanding,
  panNudge,
  PAN_BAND,
  PAN_STEP,
} = WBGeometry;

// What the view reads from the console, and nothing else.
export type ViewDeps = {
  // The console's page: the reduced-motion query, `WBNotes` and the floor pan's `blur`.
  window: Window;
  // The page the floor pan and the fullscreen change are heard on.
  document: Document;
  // The viewport (the scrolling box) and the stage (the plane inside it);
  // null before the page has them.
  workspace: () => HTMLElement | null;
  stage: () => HTMLElement | null;
  // Every console window of the page, also one in a console tab.
  pageWindows: () => ConsoleWin[];
  // The per-client view store (`WBView`); the popup's store reads nothing.
  viewStore: Pick<typeof WBView, "read" | "patch">;
  // A window's or a fence's rect, as the desk stores it.
  restoreRect: (el: HTMLElement) => Rect;
  // Raises and focuses a window or a note card.
  focusWin: (el: HTMLElement) => void;
  // The fullscreen control's look and the maximize pin, derived from page events.
  syncFullState: () => void;
  syncMaxPin: () => void;
  // A fence's element by id, and the focused fence; read at each use.
  fenceEl: (id: string | null) => HTMLElement | null;
  focusFence: (id: string | null) => void;
  clearFenceFocus: () => void;
  focusedFence: () => string | null;
  // The desk restore has settled. The desk is built after the view: a lazy arrow.
  isDeskSettled: () => boolean;
  // The console windows: `refitAll` fits each terminal.
  wins: Set<ConsoleWin>;
  // The stage's extent: forces its edge, then grows or fits the stage.
  resetExtentEdge: () => void;
  applyExtent: (opts?: ExtentOpts) => void;
  // Derives the fence chrome again from the measured rects.
  refreshFenceChrome: () => void;
};

export function createView(deps: ViewDeps) {
  const {
    window,
    document,
    workspace,
    stage,
    pageWindows,
    viewStore,
    restoreRect,
    focusWin,
    syncFullState,
    syncMaxPin,
    fenceEl,
    focusFence,
    clearFenceFocus,
    focusedFence,
    isDeskSettled,
    wins,
    resetExtentEdge,
    applyExtent,
    refreshFenceChrome,
  } = deps;

  // ---- the per-client view (issue #339) ----------------------------------------
  // `landed` latches the FIRST paint's bbox landing so a later refit cannot
  // re-centre a panned plane; a STORED offset is re-applied on every call (a
  // viewport that measured 0 lost its scroll position).
  // INVARIANT: never runs before `applyExtent()`: the clamp needs its extent.
  let landed = false;
  // A reveal that arrived while the viewport measured 0: honoured by the first
  // `applyLanding` that CAN measure, AHEAD of the stored offset.
  let pendingReveal: string | null = null;
  function applyLanding() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // A hidden tab measures 0×0: every landing centres on nothing.
    if (!ws.clientWidth || !ws.clientHeight) return;
    const rects = [...st.querySelectorAll<ConsoleWin>(".session-window")].map(restoreRect);
    if (landParkedReveal(rects)) return;
    const stored = storedOffset();
    if (landed && !stored) return;
    const at = viewLanding(
      stored,
      rects,
      { width: ws.clientWidth, height: ws.clientHeight },
      { width: st.offsetWidth, height: st.offsetHeight },
    );
    ws.scrollLeft = at.left;
    ws.scrollTop = at.top;
    latchLanding(rects);
  }

  // Latch only once the stage HOLDS something (or is known final): latching on
  // the still-empty frame would make the real landing return early.
  function latchLanding(rects: Rect[]) {
    if (rects.length || isDeskSettled()) landed = true;
  }

  // A parked reveal outranks the stored offset. Answers true when it landed; a
  // reveal whose window is gone leaves the latch as it was.
  function landParkedReveal(rects: Rect[]) {
    if (pendingReveal == null) return false;
    const wanted = pendingReveal;
    pendingReveal = null;
    // Latch FIRST: `revealNow` stores the offset it scrolls to, and that
    // store is suppressed until the landing has happened.
    const wasLanded = landed;
    latchLanding(rects);
    if (revealNow(wanted)) return true;
    landed = wasLanded;
    return false;
  }

  function storedOffset() {
    return viewStore?.read()?.off || null;
  }

  // The offset half of the store, debounced like the desk flush. SUPPRESSED until
  // the landing: `applyExtent` and the `x-show` flip fire `scroll` before the
  // restore, and would persist 0,0 over the stored pan on every boot.
  let offsetFlush: ReturnType<typeof setTimeout> | null = null;
  let pendingOffset: Offset | null = null;
  function flushOffset() {
    // The offset CAPTURED at schedule time, never a fresh read: a viewport that
    // measures 0 reads 0 (measured: `off:{0,0}` over a real 1500,850 pan).
    if (!pendingOffset) return;
    viewStore?.patch({ off: pendingOffset });
    pendingOffset = null;
  }
  function saveOffset() {
    const ws = workspace();
    if (!landed || !ws || !ws.clientWidth || !ws.clientHeight) return;
    pendingOffset = { left: ws.scrollLeft, top: ws.scrollTop };
    clearTimeout(offsetFlush);
    offsetFlush = setTimeout(() => {
      offsetFlush = null;
      flushOffset();
    }, 250);
  }

  // The page is going away: a scheduled offset write runs now (`pagehide`).
  function flushPendingOffset() {
    if (offsetFlush) {
      clearTimeout(offsetFlush);
      offsetFlush = null;
      flushOffset();
    }
  }

  // The "one action" that reaches a far window (ADR-0051 §4): focus it and centre
  // it. Returns the element, or null when no window carries that desk id.
  function reveal(deskId: string) {
    const ws = workspace();
    const it = findWindow(deskId);
    if (!it) return null;
    if (ws && ws.clientWidth && ws.clientHeight) return revealNow(deskId);
    // A viewport measuring 0 is a tab still `display:none` (docs/TESTING-TRAPS.md
    // → The workbench page in a browser): focus now, park the centring.
    focusWin(it);
    pendingReveal = deskId;
    return it;
  }

  function findWindow(deskId: string) {
    return pageWindows().find((w) => w._deskId === deskId) || null;
  }

  // The centring half, on a viewport that is known to measure.
  function revealNow(deskId: string) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return null;
    const it = findWindow(deskId);
    if (!it) return null;
    focusWin(it);
    // A maximized TARGET already fills the frame. Only the target is checked:
    // `maxlock` does NOT refuse a programmatic offset write; the `scroll`
    // re-derives the pin (`syncMaxPin`, #338).
    if (it.classList.contains("maximized") || it.classList.contains("column")) return it;
    const to = bringIntoView(
      restoreRect(it),
      { width: ws.clientWidth, height: ws.clientHeight },
      { width: st.offsetWidth, height: st.offsetHeight },
    );
    ws.scrollLeft = to.left;
    ws.scrollTop = to.top;
    // The reveal IS the new view, stored NOW: `refitAll` re-applies the STORED
    // offset, which for 250 ms is still the old one. A pending flush is dropped.
    if (landed) {
      pendingOffset = null;
      clearTimeout(offsetFlush);
      offsetFlush = null;
      viewStore?.patch({ off: { left: to.left, top: to.top } });
    }
    return it;
  }

  // ---- the slide itself --------------------------------------------------------
  // Hand-rolled, not `scrollTo({behavior:'smooth'})`: its duration is the browser's,
  // it cannot be cancelled, and Chrome ignores it while a `scroll` gesture is live.
  // INVARIANT: the tween is a VIEW effect only; `slideTo` runs after the
  // destination is stored, so a dropped tween never loses the jump.
  const SLIDE_MS = 260;
  let slideRaf: number | null = null;

  function cancelSlide() {
    if (slideRaf == null) return;
    cancelAnimationFrame(slideRaf);
    slideRaf = null;
  }

  function reducedMotion() {
    try {
      return !!window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches;
    } catch {
      return false;
    }
  }

  function slideTo(ws: HTMLElement, to: Offset) {
    cancelSlide();
    const from = { left: ws.scrollLeft, top: ws.scrollTop };
    const dx = to.left - from.left;
    const dy = to.top - from.top;
    // Nothing to travel, no rAF (a harness), or reduced motion: land now.
    if ((!dx && !dy) || typeof requestAnimationFrame !== "function" || reducedMotion()) {
      ws.scrollLeft = to.left;
      ws.scrollTop = to.top;
      return;
    }
    const t0 = performance.now();
    const step = (now: number) => {
      slideRaf = null;
      // The viewport was torn out mid-flight.
      if (!ws.isConnected) return;
      const k = slideEase((now - t0) / SLIDE_MS);
      ws.scrollLeft = from.left + dx * k;
      ws.scrollTop = from.top + dy * k;
      if (k < 1) slideRaf = requestAnimationFrame(step);
    };
    slideRaf = requestAnimationFrame(step);
  }

  // One click on a fence's name slides the viewport to it — the map's anchor.
  // Returns the fence element, or null when no fence carries that id.
  function jumpToFence(id: string) {
    const el = fenceEl(id);
    if (!el) return null;
    focusFence(id);
    // Under a maximize (a column included) the view stays put: the console covers
    // the plane, and `syncMaxPin` chases every frame of a slide (ADR-0051 §7).
    const st = stage();
    if (st?.querySelector(".session-window.maximized")) return el;
    // A fence is a REGION: its corner is anchored (ADR-0051 §7 amended).
    return jumpToEl(el, anchorIntoView);
  }

  // The note card's jump (ADR-0064 §10): a card is a POINT OF INTEREST, so it is
  // CENTRED, unlike a fence.
  function jumpToNote(id: string) {
    const el = window.WBNotes?.cardEl(id);
    if (!el) return null;
    focusWin(el);
    return jumpToEl(el, bringIntoView);
  }

  // Put `el` in view with `fold` and keep it there: the part both jumps share,
  // so the stored-offset invariant is derived once.
  function jumpToEl(el: HTMLElement, fold: (rect: Rect, view: Size, ext: Size) => Offset) {
    const ws = workspace();
    const st = stage();
    // A viewport measuring 0 is a tab still `display:none`; the focus above holds.
    if (!ws || !st || !ws.clientWidth || !ws.clientHeight) return el;
    const view = { width: ws.clientWidth, height: ws.clientHeight };
    const ext = { width: st.offsetWidth, height: st.offsetHeight };
    const to = fold(restoreRect(el), view, ext);
    slideTo(ws, to);
    // A parked reveal would slide the plane off the jump; the jump is newer.
    pendingReveal = null;
    // INVARIANT: this write is not optional (issue #337, `revealNow`): else
    // `refitAll` re-applies the PRE-jump stored offset and undoes the slide.
    if (landed) {
      pendingOffset = null;
      clearTimeout(offsetFlush);
      offsetFlush = null;
      viewStore?.patch({ off: { left: to.left, top: to.top } });
    }
    return el;
  }

  // One "line" of wheel delta in pixels, for a browser that reports
  // `deltaMode: DOM_DELTA_LINE` (Firefox) instead of pixels.
  const WHEEL_LINE = 16;

  // The auto-pan loop of a drag: while the pointer presses a viewport edge, the
  // plane scrolls each frame and `place(pointer)` keeps the drop in stage
  // coordinates. The gesture calls `follow` after its own `place`, `stop` at its end.
  function autoPan(node: HTMLElement, place: (pointer: Point) => void) {
    let panRaf: number | null = null;
    let last: Point | null = null;
    // INVARIANT: an uncancelled loop pans the plane forever after the button
    // is released, so `stop` is the FIRST statement of each gesture's `onUp`.
    const stop = () => {
      if (panRaf != null) cancelAnimationFrame(panRaf);
      panRaf = null;
    };
    const nudge = () => {
      const ws = workspace();
      if (!ws || !last) return { dx: 0, dy: 0 };
      return panNudge(last, ws.getBoundingClientRect(), PAN_BAND, PAN_STEP);
    };
    const tick = () => {
      panRaf = null;
      // The dragged node left the page mid-drag.
      if (!node.isConnected) {
        stop();
        return;
      }
      const { dx, dy } = nudge();
      if (!dx && !dy) return; // leaving the band ENDS the loop
      // `nudge` answers zero without a viewport or a pointer, so both are here.
      const ws = workspace();
      const pointer = last;
      if (!ws || !pointer) return;
      ws.scrollLeft += dx;
      ws.scrollTop += dy;
      place(pointer);
      panRaf = requestAnimationFrame(tick);
    };
    const follow = (pointer: Point) => {
      last = pointer;
      if (panRaf != null) return;
      const { dx, dy } = nudge();
      if (dx || dy) panRaf = requestAnimationFrame(tick);
    };
    return { follow, stop };
  }

  // ---- the plane's own gestures ------------------------------------------------
  // Pan by dragging the BARE FLOOR. Calls neither `applyExtent` nor a desk
  // write nor `focusWin`: panning moves the view, not the rects.
  function onFloorDown(e: MouseEvent) {
    // Primary button only — see makeDraggable.
    if (e.button !== 0) return;
    const ws = workspace();
    const st = stage();
    // Element IDENTITY is the floor-vs-window hit test; a fence is
    // `pointer-events: none`, so a press over one targets the stage (#340).
    if (!ws || !st || e.target !== st) return;
    // The operator's hand outranks a jump in flight.
    cancelSlide();
    // A press on the bare floor OUTSIDE the focused fence leaves it (#343).
    if (focusedFence()) {
      const el = fenceEl(focusedFence());
      const box = st.getBoundingClientRect();
      const point = { x: e.clientX - box.left, y: e.clientY - box.top };
      if (!el || !rectHolds(restoreRect(el), point)) clearFenceFocus();
    }
    const startX = e.clientX;
    const startY = e.clientY;
    const startLeft = ws.scrollLeft;
    const startTop = ws.scrollTop;
    st.classList.add("panning");
    const onMove = (ev: MouseEvent) => {
      // A swallowed mouseup would leave a sticky pan.
      if (ev.buttons === 0) {
        onUp();
        return;
      }
      ws.scrollLeft = startLeft - (ev.clientX - startX);
      ws.scrollTop = startTop - (ev.clientY - startY);
    };
    // INVARIANT: every exit path drops EVERY listener and the class.
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      window.removeEventListener("blur", onUp);
      st.classList.remove("panning");
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
    window.addEventListener("blur", onUp);
    // No text selection starts, and the focused terminal keeps the keyboard.
    e.preventDefault();
  }

  function wheelPixels(e: WheelEvent, pageHeight: number) {
    if (e.deltaMode === 1) return e.deltaY * WHEEL_LINE;
    return e.deltaMode === 2 ? e.deltaY * pageHeight : e.deltaY;
  }

  // The wheel. The VERTICAL axis is native `overflow:auto`; this adds only the
  // horizontal reach where the platform does not provide it.
  function onWheel(e: WheelEvent) {
    // The terminal owns its wheel (CSS `overscroll-behavior: contain`); a
    // `preventDefault` would cancel its own scroll too.
    if ((e.target as Partial<Element> | null)?.closest?.(".session-window")) return;
    // Any wheel reaching the PLANE abandons a jump in flight, the vertical one too.
    cancelSlide();
    // A platform that converts shift-wheel itself delivers `deltaX`.
    if (!(e.shiftKey && e.deltaY !== 0 && e.deltaX === 0)) return;
    const ws = workspace();
    if (!ws) return;
    // `deltaY` is only pixels when `deltaMode` says so: Firefox reports LINE
    // (±3 per notch) and does not convert shift-wheel itself.
    ws.scrollLeft += wheelPixels(e, ws.clientHeight);
    e.preventDefault();
  }

  // `passive: false` or `preventDefault` is a no-op: Chrome treats a wheel
  // listener on a scroll container as passive by default.
  function wireStage() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    st.addEventListener("mousedown", onFloorDown);
    ws.addEventListener("wheel", onWheel, { passive: false });
    // Gesture, wheel, scrollbar and `reveal`'s programmatic write all end in a
    // `scroll` on the viewport: the maximize pin is derived from it.
    ws.addEventListener("scroll", syncMaxPin);
    // A SECOND listener: the pin must stay exact, the offset is debounced.
    ws.addEventListener("scroll", saveOffset);
    // The ONLY writer of the fullscreen control's look (`syncFullState`). Each
    // surface (shell, popup) runs `wireStage` in its own document.
    document.addEventListener("fullscreenchange", syncFullState);
  }

  // Refit every open console. Called when the Consoles tab returns to view.
  function refitAll(attempt?: number) {
    // A viewport that measures 0 (before the first layout) cannot be fitted.
    // Refitting there collapses the stage to the bare margin (3200×2080 →
    // 200×200) and sizes every terminal to nothing. Wait for a frame that can
    // measure; give up rather than refit blind.
    const ws0 = workspace();
    if (ws0 && (!ws0.clientWidth || !ws0.clientHeight)) {
      const n = attempt || 0;
      if (n < 10) requestAnimationFrame(() => refitAll(n + 1));
      return;
    }
    // The extent dispatch is an EDGE: one fired before Alpine mounted would leave
    // the footer pill at `stage 0 × 0`; force the edge.
    resetExtentEdge();
    applyExtent();
    for (const win of wins) {
      try {
        win._term?.fit.fit();
      } catch {}
    }
    // The chrome folds MEASURED rects: a refresh while hidden wrote `0
    // consoles`. Re-derive on the first frame that can measure.
    refreshFenceChrome();
    // LAST, after `applyExtent`: `x-show` threw the stored offset away.
    applyLanding();
    // The first frame that can measure the column cap.
    sendDocument(document, "workbench:columns-stale");
  }

  return {
    applyLanding,
    flushPendingOffset,
    reveal,
    findWindow,
    jumpToFence,
    jumpToNote,
    cancelSlide,
    autoPan,
    wireStage,
    refitAll,
  };
}
