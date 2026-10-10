/* ---------------------------------------------------------------------------
   The console's window chrome: the titlebar and its buttons, the drag by the
   titlebar, the titlebar's touch gestures, the eight resize handles, and the
   free cascade a new window is born at (ADR-0075 phase 5).

   `createChrome(deps)` returns `buildChrome`, `makeDraggable`,
   `wireTitleTouch` and `startResize` for one console (ADR-0075 D7). They read
   the console only through `deps`, and `ChromeDeps` lists every read, so
   `tsc` refuses a read outside it. `wb-console.ts` creates one per console
   and keeps the window state, the fences and the plane.

   `createGestures()` owns the elements under a gesture of the operator. The
   chrome and the fence gestures both begin and end gestures, so the set has
   its own owner that each takes through its `deps`, and not one of them.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import { sendDocument } from "./wb-events.ts";
import type { ConsoleWin, DeskFence, DeskRecord, DeskWindowFields, Point, Presentation, Rect, Size } from "./wb-types.d.ts";

const { fullscreenOffered, dragThreshold, dragBegins, isDoubleTap, HOLD_MS } = WBConsoleInput;
const { RESIZE_MIN, resizeRect, spawnRectIn, freeSpawnRect, WIN_MIN_W, WIN_MIN_H } = WBGeometry;
const { initWindow } = WBWindowState;

// ---- resize geometry ---------------------------------------------------------
// One pure function for all eight directions (`resizeRect`, wb-geometry):
// east/south move the far edge; west/north move `left`/`top` and derive the
// size, so the OPPOSITE edge stays put.
export const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

// The elements under a gesture of the operator, from the press to the
// release: windows, fences, cards, and every member a fence move carries.
// `begin` at the press, `end` at the release; `active` is the question a desk
// this page takes asks before it moves an element.
export function createGestures() {
  const gestures = new Set<HTMLElement>();
  return {
    begin: (el: HTMLElement) => {
      gestures.add(el);
    },
    end: (el: HTMLElement) => {
      gestures.delete(el);
    },
    active: (el: HTMLElement) => gestures.has(el),
  };
}

export type Gestures = ReturnType<typeof createGestures>;

// The hooks of a gesture that is not a console window's: a note card drags and
// resizes by the same gesture (ADR-0064 §8), locked by its own record and
// persisted into the notes. `min` is the smallest size of a resize.
export type GestureHooks = { locked?: () => boolean; onDrop?: () => void };
export type ResizeHooks = GestureHooks & { min?: Size };

// What `buildChrome` seeds a window from: a desk record, or a partial of one
// that carries at least `kind`.
export type ChromeRecord = Partial<DeskRecord> | null | undefined;

// What the chrome reads from the console, and nothing else.
export type ChromeDeps = {
  // The console's page: the drag ends on a `blur` of its window.
  window: Window;
  // The page the chrome builds its elements in and listens on.
  document: Document;
  // The console's options: the torn-off fence window boots without
  // `autoBoot` (no fences for the free cascade) and cannot launch.
  OPTS: { autoBoot?: boolean; canLaunch?: boolean };
  // The elements under a gesture; shared with the fence gestures.
  gestures: Gestures;
  // The plane and the viewport; null before the page has them. A gesture and
  // `buildChrome` run only on a page that has the stage, and read it with `!`.
  stage: () => HTMLElement | null;
  workspace: () => HTMLElement | null;
  // The fences on the stage, and the focused fence's id: the console and
  // the fence list reassign them, so they are read when a window is built.
  fences: () => DeskFence[];
  focusedFence: () => string | null;
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: { grow?: boolean }) => void;
  // A window's lock, painted and toggled; `isLocked` also counts its fence.
  applyLock: (win: ConsoleWin, locked: boolean) => void;
  isLocked: (win: ConsoleWin) => boolean;
  toggleLock: (win: ConsoleWin) => void;
  // The auto-pan loop of a drag at a viewport edge.
  autoPan: (
    node: HTMLElement,
    place: (pointer: Point) => void,
  ) => { follow: (pointer: Point) => void; stop: () => void };
  // The console name: its prefix, the names taken, whether a rename is
  // allowed here, and the rename itself.
  consolePrefix: (repo: string | null | undefined) => string;
  takenNames: (exceptId: string | null) => (string | null | undefined)[];
  canRename: () => boolean;
  startRename: (win: ConsoleWin, span: HTMLElement) => void;
  // A fence's element and lock, for a window born into the focused fence.
  fenceEl: (id: string | null) => HTMLElement | null;
  fenceLocked: (id: string | null) => boolean;
  // Raises a window or a card, and slides the viewport to one out of view.
  focusWin: (win: HTMLElement) => void;
  reveal: (deskId: string) => void;
  // Full screen, maximize, and their toggles.
  isFull: (win: HTMLElement) => boolean;
  toggleFull: (win: ConsoleWin) => void;
  setMax: (win: ConsoleWin, on: boolean, persist?: boolean) => void;
  toggleMax: (win: ConsoleWin) => void;
  // A new desk record id, and the box of a window, a fence or a card read from
  // the DOM.
  newDeskId: () => string;
  restoreRect: (el: HTMLElement) => Rect;
  // The title: what it says, and its painting.
  sessionPresentation: (
    label: string | null,
    repo: string | null | undefined,
    prior: ChromeRecord,
    owner: HostedSession | null | undefined,
  ) => Presentation;
  renderTitle: (win: ConsoleWin, title: HTMLElement, presentation: Presentation) => void;
  // Writes the window's fields to the desk.
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
};

export function createChrome(deps: ChromeDeps) {
  const {
    window,
    document,
    OPTS,
    gestures,
    stage,
    workspace,
    fences,
    focusedFence,
    applyExtent,
    applyLock,
    isLocked,
    toggleLock,
    autoPan,
    consolePrefix,
    takenNames,
    canRename,
    startRename,
    fenceEl,
    fenceLocked,
    focusWin,
    reveal,
    isFull,
    toggleFull,
    setMax,
    toggleMax,
    newDeskId,
    restoreRect,
    sessionPresentation,
    renderTitle,
    setWin,
  } = deps;

  // A new window's place in the free cascade: one of eight offsets.
  let cascade = 0;

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
      if (win.classList.contains("maximized") || win.classList.contains("column") || isFull(win)) return;
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
        const st = stage()!;
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
        if (held) {
          win._lastTap = null;
          // `held` is set only by the timer, which exists only with a `span`.
          startRename(win, span!);
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
      if (win.classList.contains("maximized") || win.classList.contains("column") || isFull(win)) return;
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
      const st = stage()!;
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

  // The floating-window chrome, shared by a live console and a placeholder:
  // rect (from a desk record, else cascaded), titlebar, body, eight resize
  // handles. `desk` is a record (or a partial carrying at least `kind`);
  // everything the record needs later is hung off the element.
  function buildChrome(label: string | null, repo: string | null | undefined, desk: ChromeRecord, kind: string) {
    const win = document.createElement("div") as HTMLElement as ConsoleWin;
    win.className = "session-window";
    // Every field this element will carry is written HERE, by the module that
    // declares them (wb-window-state.ts): a field nothing seeds reads as its
    // declared default, not `undefined`.
    initWindow(win, {
      _deskId: desk?.id || newDeskId(),
      _deskRepo: repo || "~",
      _deskAgent: label,
      _deskKind: desk?.kind || kind,
      _deskDaemonId: desk?.daemonId ?? null,
      _deskEnvironment: desk?.environment ?? null,
      // The worktree (#411), seeded from the record; the launch request and
      // then the daemon's `session-open` overwrite it.
      _deskCheckout: desk?.checkout ?? null,
      // Locked in place (ADR-0050 lock amendment), seeded from the record.
      _deskLocked: !!desk?.locked,
      // The console name (ADR-0066 §2): a carried record keeps its own; a new
      // console takes the lowest free number of its prefix.
      _deskConsoleName:
        desk?.consoleName || WBConsoleName.defaultName(consolePrefix(repo), takenNames(null)),
    });
    const rect = desk?.rect;
    // Set when the free cascade had to leave the viewport (`freeSpawnRect`).
    let spawnMoved = false;
    if (rect) {
      win.style.left = rect.left + "px";
      win.style.top = rect.top + "px";
      win.style.width = rect.width + "px";
      win.style.height = rect.height + "px";
    } else {
      cascade = (cascade + 1) % 8;
      // Born INTO the focused fence when there is one (#343). The rect is
      // written at CONSTRUCTION so a record never snapshots mid-transition
      // (#342). MEASURABLE, not merely present: `openConsoleItem` calls
      // `activate` then `open` on the SAME synchronous stack, so a spawn can
      // land while the tab is still `display:none` and `restoreRect` reads all
      // zeros — a 1x1 window persisted to the shared desk. Fall back to the
      // free cascade; the focus survives for the next spawn.
      // A LOCKED focused fence is not a host: the console would be born held
      // by its lock. It takes the free cascade instead.
      const el = focusedFence() && !fenceLocked(focusedFence()) && fenceEl(focusedFence());
      const host = el && el.offsetWidth && el.offsetHeight ? el : null;
      if (host) {
        const headH = host.querySelector<HTMLElement>(".fence-head")?.offsetHeight || 28;
        const box = spawnRectIn(restoreRect(host), cascade, headH);
        win.style.left = box.left + "px";
        win.style.top = box.top + "px";
        win.style.width = box.width + "px";
        win.style.height = box.height + "px";
        // The CSS floor outranks the inline size (#342); relax it for exactly
        // the axes below it.
        if (box.width < WIN_MIN_W) win.style.minWidth = box.width + "px";
        if (box.height < WIN_MIN_H) win.style.minHeight = box.height + "px";
      } else {
        // The free cascade is anchored at the VIEWPORT's current offset, not
        // the plane's origin, and sized from the viewport, not the stage
        // (which `applyExtent` grows well past it).
        const ws = workspace();
        const view = {
          left: ws?.scrollLeft || 0,
          top: ws?.scrollTop || 0,
          width: ws?.clientWidth || 0,
          height: ws?.clientHeight || 0,
        };
        // The popup gets no fences: its members' rects are re-origined, so the
        // fold would match the wrong fence (`fenceHolds`).
        const spawn = freeSpawnRect(view, cascade, OPTS.autoBoot === false ? [] : fences());
        win.style.left = spawn.rect.left + "px";
        win.style.top = spawn.rect.top + "px";
        win.style.width = spawn.rect.width + "px";
        win.style.height = spawn.rect.height + "px";
        spawnMoved = spawn.moved;
      }
    }

    const titlebar = document.createElement("div");
    titlebar.className = "session-titlebar";
    // The agent's state as a dot before the label (ADR-0059). Hidden until a
    // session row says something; a shell console never does.
    const stateDot = document.createElement("span");
    stateDot.className = "session-state";
    stateDot.hidden = true;
    win._stateDot = stateDot;
    const title = document.createElement("span");
    title.className = "session-title";
    const presentation = sessionPresentation(label, repo, desk, null);
    win._title = title;
    renderTitle(win, title, presentation);
    title.title = presentation.tooltip;
    const actions = document.createElement("span");
    actions.className = "session-actions";
    // Open another console beside this maximized one (ADR-0051 §5). Shown and
    // enabled by `applyColumns`. The torn-off fence window calls it too, and
    // never shows the button: it boots without `autoBoot`.
    const colBtn = document.createElement("button");
    colBtn.className = "session-column";
    colBtn.title = "Slice";
    colBtn.innerHTML = '<i class="bi bi-arrows-expand-vertical"></i>';
    colBtn.hidden = true;
    win._colBtn = colBtn;
    // Restart is offered on a live session too, behind a confirm
    // (`restartWin`); hidden only where nothing can launch (the popup).
    const restartBtn = document.createElement("button");
    restartBtn.className = "session-restart";
    restartBtn.title = "Restart session";
    restartBtn.innerHTML = '<i class="bi bi-arrow-clockwise"></i>';
    restartBtn.hidden = OPTS.canLaunch === false;
    const maxBtn = document.createElement("button");
    maxBtn.className = "session-max";
    maxBtn.title = "Maximize";
    maxBtn.innerHTML = '<i class="bi bi-fullscreen"></i>';
    win._maxBtn = maxBtn;
    // Fullscreen is orthogonal to maximize (viewport vs physical screen). Built
    // only where the browser can HOLD it (`fullscreenOffered`).
    const fullBtn = document.createElement("button");
    fullBtn.className = "session-full";
    fullBtn.title = "Full screen";
    fullBtn.innerHTML = '<i class="bi bi-arrows-fullscreen"></i>';
    fullBtn.hidden = !fullscreenOffered(document.fullscreenEnabled, navigator.vendor);
    const closeBtn = document.createElement("button");
    closeBtn.className = "session-close";
    closeBtn.title = "Close";
    closeBtn.innerHTML = '<i class="bi bi-x-lg"></i>';
    // Lock in place. Glyph and title painted by `applyLock`.
    const lockBtn = document.createElement("button");
    lockBtn.className = "session-lock";
    actions.append(colBtn, fullBtn, maxBtn, restartBtn, lockBtn, closeBtn);
    // The dot sits WITH the title: the bar is space-between.
    const head = document.createElement("span");
    head.className = "session-head";
    head.append(stateDot, title);
    titlebar.append(head, actions);

    const body = document.createElement("div");
    body.className = "session-body";
    const grip = document.createElement("div");
    grip.className = "session-resize";
    win.append(titlebar, body, grip);
    // Eight handles; the corner grip above is decoration only.
    for (const dir of DIRS) {
      const h = document.createElement("div");
      h.className = `session-handle h-${dir}`;
      h.addEventListener("pointerdown", startResize(win, dir));
      win.append(h);
    }
    stage()!.append(win);
    applyExtent();

    // Pointer: a touch raises the window on contact, not after the tap resolves.
    win.addEventListener("pointerdown", () => focusWin(win));
    makeDraggable(win, titlebar);
    // Maximize/restore: the button, a double-click on the titlebar, or a
    // double tap on it. `wbColumns` (wb-consoles-tab.ts) owns the columns, so
    // a column's restore is its decision.
    const maxOrRestore = () => {
      if (win.classList.contains("column")) {
        sendDocument(document, "workbench:column-restore", { id: win._deskId });
        return;
      }
      toggleMax(win);
      sendDocument(document, "workbench:columns-stale");
    };
    maxBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      maxOrRestore();
    });
    titlebar.addEventListener("dblclick", (e) => {
      // Fullscreen hides the maximize control; a double-click must not toggle
      // it unseen underneath.
      // Nor may a double-click on the name, which renames (ADR-0066 §3).
      if ((e.target as HTMLElement).closest("button, .session-name, .session-name-input") || isFull(win)) return;
      // A finger's double tap is `wireTitleTouch`'s.
      if (win._lastPointerType !== "mouse") return;
      maxOrRestore();
    });
    wireTitleTouch(win, titlebar, maxOrRestore);
    colBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      sendDocument(document, "workbench:column-open", { id: win._deskId, rect: colBtn.getBoundingClientRect() });
    });
    lockBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleLock(win);
    });
    applyLock(win, !!desk?.locked);
    fullBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleFull(win);
    });
    // Re-apply a persisted maximized state (the inline rect above is the box it
    // restores to).
    if (rect && desk.max) setMax(win, true);
    // A console pushed past a locked fence may be out of view: slide to it.
    if (spawnMoved) reveal(win._deskId);
    else focusWin(win);
    return { win, body, title, restartBtn, fullBtn, lockBtn, maxBtn, closeBtn };
  }

  return { buildChrome, makeDraggable, wireTitleTouch, startResize };
}
