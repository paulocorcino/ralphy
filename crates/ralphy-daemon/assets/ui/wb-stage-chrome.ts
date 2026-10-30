/* ---------------------------------------------------------------------------
   The console's window chrome: the titlebar and its buttons, the drag by the
   titlebar, the titlebar's touch gestures, the eight resize handles, and the
   free cascade a new window is born at (ADR-0075 phase 5).

   `createChrome(deps)` returns `buildChrome`, `makeDraggable`,
   `wireTitleTouch` and `startResize` for one console (ADR-0075 D7). They read
   the console only through `deps`, and `ChromeDeps` lists every read, so
   `tsc` refuses a read outside it. The drag, the touch gestures and the
   resize are `wb-stage-window-gestures.ts`. `wb-console.ts` creates one per
   console and keeps the window state, the fences and the plane. The owner of
   the gestures is `wb-stage-stack.ts`.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBConsoleName } from "./wb-console-name.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import { sendDocument } from "./wb-events.ts";
import { inTab } from "./wb-console-place.ts";
import { required } from "./wb-dom-need.ts";
import { createWindowGestures } from "./wb-stage-window-gestures.ts";
import type { Gestures } from "./wb-stage-stack.ts";
import type {
  ConsoleWin,
  DeskFence,
  DeskRecord,
  DeskWindowFields,
  Point,
  Presentation,
  Rect,
} from "./wb-types.d.ts";

const { fullscreenOffered } = WBConsoleInput;
const { spawnRectIn, freeSpawnRect, WIN_MIN_W, WIN_MIN_H } = WBGeometry;
const { initWindow } = WBWindowState;

// ---- resize geometry ---------------------------------------------------------
// One pure function for all eight directions (`resizeRect`, wb-geometry):
// east/south move the far edge; west/north move `left`/`top` and derive the
// size, so the OPPOSITE edge stays put.
export const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

// The fences take the gestures' type from here too.
export type { Gestures };

// What `buildChrome` seeds a window from: a desk record, or a partial of one
// that carries at least `kind`.
export type ChromeRecord =
  | Partial<DeskRecord>
  | import("./wb-types.d.ts").SpawnCarry
  | null
  | undefined;

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
  // `buildChrome` run only on a page that has the stage, and read it with `required`.
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
    label: string | null | undefined,
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
    document,
    OPTS,
    stage,
    workspace,
    fences,
    focusedFence,
    applyExtent,
    applyLock,
    toggleLock,
    consolePrefix,
    takenNames,
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
  } = deps;
  const { makeDraggable, wireTitleTouch, startResize } = createWindowGestures(deps);

  // A new window's place in the free cascade: one of eight offsets.
  let cascade = 0;

  // Who the window is.
  function identityFields(
    label: string | null | undefined,
    repo: string | null | undefined,
    desk: ChromeRecord,
    kind: string | null | undefined,
  ) {
    return {
      _deskId: desk?.id || newDeskId(),
      _deskRepo: repo || "~",
      _deskAgent: label || "",
      _deskKind: desk?.kind || kind || "",
    };
  }

  // What a carried record hands the window: where it runs and how it is kept.
  function carriedFields(desk: ChromeRecord, consoleName: string) {
    return {
      _deskDaemonId: desk?.daemonId ?? null,
      _deskEnvironment: desk?.environment ?? null,
      // The worktree (#411), seeded from the record; the launch request and
      // then the daemon's `session-open` overwrite it.
      _deskCheckout: desk?.checkout ?? null,
      // Locked in place (ADR-0050 lock amendment), seeded from the record.
      _deskLocked: !!desk?.locked,
      _deskConsoleName: consoleName,
    };
  }

  // The element of a new window, with every field it will carry written HERE,
  // by the module that declares them (wb-window-state.ts): a field nothing
  // seeds reads as its declared default, not `undefined`.
  function seedWindow(
    label: string | null | undefined,
    repo: string | null | undefined,
    desk: ChromeRecord,
    kind: string | null | undefined,
    consoleName: string,
  ) {
    const win = document.createElement("div") as HTMLElement as ConsoleWin;
    win.className = "session-window";
    initWindow(win, {
      ...identityFields(label, repo, desk, kind),
      ...carriedFields(desk, consoleName),
    });
    return win;
  }

  function setBox(win: ConsoleWin, box: Partial<Rect>) {
    win.style.left = box.left + "px";
    win.style.top = box.top + "px";
    win.style.width = box.width + "px";
    win.style.height = box.height + "px";
  }

  // A console born INTO a fence (#343), below the fence's head band.
  function spawnInFence(win: ConsoleWin, host: HTMLElement) {
    const headH = host.querySelector<HTMLElement>(".fence-head")?.offsetHeight || 28;
    const box = spawnRectIn(restoreRect(host), cascade, headH);
    setBox(win, box);
    // The CSS floor outranks the inline size (#342); relax it for exactly
    // the axes below it.
    if (box.width < WIN_MIN_W) win.style.minWidth = box.width + "px";
    if (box.height < WIN_MIN_H) win.style.minHeight = box.height + "px";
  }

  // The free cascade is anchored at the VIEWPORT's current offset, not the
  // plane's origin, and sized from the viewport, not the stage (which
  // `applyExtent` grows well past it). True when it had to leave the viewport
  // (`freeSpawnRect`).
  function spawnFree(win: ConsoleWin) {
    const ws = workspace();
    const view = {
      left: ws?.scrollLeft || 0,
      top: ws?.scrollTop || 0,
      width: ws?.clientWidth || 0,
      height: ws?.clientHeight || 0,
    };
    // The popup gets no fences: its members' rects are measured from a new origin, so the
    // fold would match the wrong fence (`fenceHolds`).
    const spawn = freeSpawnRect(view, cascade, OPTS.autoBoot === false ? [] : fences());
    setBox(win, spawn.rect);
    return spawn.moved;
  }

  // The window's box: a record's own rect, else a place in the cascade. The
  // rect is written at CONSTRUCTION so a record never snapshots mid-transition
  // (#342). Returns whether the free cascade had to leave the viewport.
  function placeWindow(win: ConsoleWin, rect: Partial<Rect> | undefined) {
    if (rect) {
      setBox(win, rect);
      return false;
    }
    cascade = (cascade + 1) % 8;
    // Born INTO the focused fence when there is one (#343). The fence must be
    // MEASURABLE, not merely present: `openConsoleItem` calls `activate` then
    // `open` on the SAME synchronous stack, so a spawn can land while the tab
    // is still `display:none` and `restoreRect` reads all zeros — a 1x1 window
    // persisted to the shared desk. Fall back to the free cascade; the focus
    // survives for the next spawn.
    // A LOCKED focused fence is not a host: the console would be born held
    // by its lock. It takes the free cascade instead.
    const el = focusedFence() && !fenceLocked(focusedFence()) && fenceEl(focusedFence());
    const host = el && el.offsetWidth && el.offsetHeight ? el : null;
    if (!host) return spawnFree(win);
    spawnInFence(win, host);
    return false;
  }

  // A button of the titlebar.
  function chromeButton(className: string, title: string, icon: string) {
    const btn = document.createElement("button");
    btn.className = className;
    btn.title = title;
    const glyph = document.createElement("i");
    glyph.className = "bi " + icon;
    btn.append(glyph);
    return btn;
  }

  // The titlebar: the state dot and title, and the buttons.
  function buildTitlebar(
    win: ConsoleWin,
    label: string | null | undefined,
    repo: string | null | undefined,
    desk: ChromeRecord,
  ) {
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
    const colBtn = chromeButton("session-column", "Slice", "bi-arrows-expand-vertical");
    colBtn.hidden = true;
    win._colBtn = colBtn;
    // Restart is offered on a live session too, behind a confirm
    // (`restartWin`); hidden only where nothing can launch (the popup).
    const restartBtn = chromeButton("session-restart", "Restart session", "bi-arrow-clockwise");
    restartBtn.hidden = OPTS.canLaunch === false;
    const maxBtn = chromeButton("session-max", "Maximize", "bi-fullscreen");
    win._maxBtn = maxBtn;
    // Fullscreen is orthogonal to maximize (viewport vs physical screen). Built
    // only where the browser can HOLD it (`fullscreenOffered`).
    const fullBtn = chromeButton("session-full", "Full screen", "bi-arrows-fullscreen");
    fullBtn.hidden = !fullscreenOffered(document.fullscreenEnabled, navigator.vendor);
    const closeBtn = chromeButton("session-close", "Close", "bi-x-lg");
    // Lock in place. Glyph and title painted by `applyLock`.
    const lockBtn = document.createElement("button");
    lockBtn.className = "session-lock";
    actions.append(colBtn, fullBtn, maxBtn, restartBtn, lockBtn, closeBtn);
    // The dot sits WITH the title: the bar is space-between.
    const head = document.createElement("span");
    head.className = "session-head";
    head.append(stateDot, title);
    titlebar.append(head, actions);
    return { titlebar, title, colBtn, restartBtn, maxBtn, fullBtn, closeBtn, lockBtn };
  }

  // The body, the corner grip and the eight resize handles.
  function appendBody(win: ConsoleWin, titlebar: HTMLElement) {
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
    return body;
  }

  type Bar = ReturnType<typeof buildTitlebar>;

  // The listeners of the window and its buttons.
  function wireChromeEvents(win: ConsoleWin, bar: Bar) {
    const { titlebar, colBtn, maxBtn, fullBtn, lockBtn } = bar;
    // Pointer: a touch raises the window on contact, not after the tap resolves.
    win.addEventListener("pointerdown", () => focusWin(win));
    makeDraggable(win, titlebar);
    // Maximize/restore: the button, a double-click on the titlebar, or a
    // double tap on it. `wbColumns` (wb-consoles-tab.ts) owns the columns, so
    // a column's restore is its decision.
    // A console in a tab is not maximized; in a tab of two consoles or more
    // its Restore takes it back to the stage (ADR-0037 §3d).
    const maxOrRestore = () => {
      if (win.classList.contains("column") || win.classList.contains("tab-grid")) {
        sendDocument(document, "workbench:column-restore", { id: win._deskId });
        return;
      }
      if (inTab(win)) return;
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
      if (
        (e.target as HTMLElement).closest("button, .session-name, .session-name-input") ||
        isFull(win)
      )
        return;
      // A finger's double tap is `wireTitleTouch`'s.
      if (win._lastPointerType !== "mouse") return;
      maxOrRestore();
    });
    wireTitleTouch(win, titlebar, maxOrRestore);
    colBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      sendDocument(document, "workbench:column-open", {
        id: win._deskId,
        rect: colBtn.getBoundingClientRect(),
      });
    });
    lockBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleLock(win);
    });
    fullBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleFull(win);
    });
  }

  // The floating-window chrome, shared by a live console and a placeholder:
  // rect (from a desk record, else cascaded), titlebar, body, eight resize
  // handles. `desk` is a record (or a partial carrying at least `kind`);
  // everything the record needs later is hung off the element.
  function buildChrome(
    label: string | null | undefined,
    repo: string | null | undefined,
    desk: ChromeRecord,
    kind: string | null | undefined,
  ) {
    // The console name (ADR-0066 §2): a carried record keeps its own; a new
    // console takes the lowest free number of its prefix.
    const consoleName =
      desk?.consoleName || WBConsoleName.defaultName(consolePrefix(repo), takenNames(null));
    const win = seedWindow(label, repo, desk, kind, consoleName);
    const rect = desk?.rect;
    const spawnMoved = placeWindow(win, rect);
    const bar = buildTitlebar(win, label, repo, desk);
    const body = appendBody(win, bar.titlebar);
    required(stage(), "the stage").append(win);
    applyExtent();
    wireChromeEvents(win, bar);
    applyLock(win, !!desk?.locked);
    // Re-apply a persisted maximized state (the inline rect above is the box it
    // restores to).
    if (rect && desk.max) setMax(win, true);
    // A console pushed past a locked fence may be out of view: slide to it.
    if (spawnMoved) reveal(win._deskId);
    else focusWin(win);
    const { title, restartBtn, fullBtn, lockBtn, maxBtn, closeBtn } = bar;
    return { win, body, title, restartBtn, fullBtn, lockBtn, maxBtn, closeBtn };
  }

  return { buildChrome, makeDraggable, wireTitleTouch, startResize };
}
