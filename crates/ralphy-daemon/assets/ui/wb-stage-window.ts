/* ---------------------------------------------------------------------------
   The console window's own states: maximize, the lock, full screen, the
   stage's extent and a window's restore box.

   `createStageWindow(deps)` returns the functions the console, its chrome,
   its fences, its view and the `WBConsole` API use (ADR-0075 D7). They read
   the console only through `deps`, and `StageWindowDeps` lists every read, so
   `tsc` refuses a read outside it. `wb-console.ts` creates one per console and
   keeps the columns and the desk records.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import { sendDocument } from "./wb-events.ts";
import type { ConsoleWin, DeskWindowFields, ExtentOpts, Stacked } from "./wb-types.d.ts";

// Plane geometry is `wb-geometry.ts` (ADR-0057): pure folds over rects.
const { STAGE_MARGIN, stageExtent } = WBGeometry;

// What the window states read from the console, and nothing else.
export type StageWindowDeps = {
  // The page: the fullscreen element and the body class of a maximize.
  document: Document;
  // The viewport (the scrolling box) and the stage (the plane inside it);
  // null before the page has them.
  workspace: () => HTMLElement | null;
  stage: () => HTMLElement | null;
  // Raises and focuses a window.
  focusWin: (win: Stacked) => void;
  // Writes the fields a window keeps on the desk.
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
  // Whether a locked fence holds the window. The fence list is built after
  // this factory, so it is a lazy arrow.
  heldByFence: (win: HTMLElement) => boolean;
  // Re-asks which windows are covered. It runs after a maximize or a
  // full-screen change.
  refreshCover: () => void;
};

export function createStageWindow(deps: StageWindowDeps) {
  const { document, workspace, stage, focusWin, setWin, heldByFence, refreshCover } = deps;

  // Columns, a maximize and the physical screen each fill the whole viewport,
  // so every other console is under them.
  function fillsViewport(win: ConsoleWin) {
    return win.classList.contains("maximized") || win.classList.contains("column") || isFull(win);
  }

  // A window's RESTORE box, from the inline styles. Three states make a live
  // read lie: `.maximized` pins all four offsets via `!important` (a read would
  // persist 0,0); fullscreen sizes the element to the DISPLAY at 0,0 (a read
  // would grow the stage to screen size and persist a monitor-sized box); a
  // `display:none` ancestor — the Consoles tab is Alpine `x-show` and
  // `restoreDesk` runs whatever tab is showing — measures 0×0 at 0,0 (measured
  // 2026-09-09: a reload from a file tab wrote `0,0,0,0`, the next load rendered
  // the CSS floor 240×150 at the origin and stored THAT). The inline rect is
  // untouched by all three; it is what `buildChrome` wrote from the record.
  function measurable(win: HTMLElement) {
    return !!(win.offsetWidth || win.offsetHeight);
  }
  function restoreRect(win: HTMLElement) {
    const inline = (prop: "left" | "top" | "width" | "height", fallback: number) => parseInt(win.style[prop], 10) || fallback;
    const fromInline = () => ({
      left: inline("left", win.offsetLeft),
      top: inline("top", win.offsetTop),
      width: inline("width", win.offsetWidth),
      height: inline("height", win.offsetHeight),
    });
    if (!measurable(win)) return fromInline();
    // A column's painted box is view state (ADR-0051 §5): like a maximize, the
    // inline rect is the desk rect underneath it. So is a card on top's
    // (ADR-0064, 2026-09-26 amendment).
    if (win.classList.contains("on-top")) return fromInline();
    if (!win.classList.contains("maximized") && !win.classList.contains("column") && !isFull(win)) {
      return {
        left: win.offsetLeft,
        top: win.offsetTop,
        width: win.offsetWidth,
        height: win.offsetHeight,
      };
    }
    return fromInline();
  }

  // Toggle a console between its floating rect and a full-VIEWPORT bleed. The
  // pre-maximize rect stays in the inline styles, so restoring drops the class.
  //
  // On a scrollable stage the bleed is pinned to what the operator is looking
  // at: `--max-left`/`--max-top` carry the viewport's scroll offsets, re-derived
  // by `syncMaxPin`. Re-asserted after the class flip because `maxlock`
  // (`overflow:hidden`) drops the scrollbars, which can clamp the offsets.
  //
  // `persist` writes `max`: the operator's own toggle, and the columns
  // (`wbColumns`) moving the maximize to another console (ADR-0051 §5). A restore
  // (`buildChrome`) and the torn-off fence window's columns write nothing: a
  // restore already reads the record, and that window's grid is never stored.
  function setMax(win: ConsoleWin, on: boolean, persist = false) {
    if (win.classList.contains("maximized") === on) return;
    const ws = workspace();
    const offsets = ws ? { left: ws.scrollLeft, top: ws.scrollTop } : null;
    const maxed = win.classList.toggle("maximized", on);
    if (!maxed && !win.classList.contains("column")) {
      win.style.removeProperty("--max-left");
      win.style.removeProperty("--max-top");
    }
    syncMaxLock();
    if (ws && offsets) {
      ws.scrollLeft = offsets.left;
      ws.scrollTop = offsets.top;
    }
    // AFTER the restore: the pin must come from the offsets that SURVIVED the
    // `maxlock` flip, not the pair read before it.
    syncMaxPin();
    paintMaxButton(win);
    focusWin(win);
    try {
      win._term?.fit.fit();
    } catch {}
    applyExtent();
    if (persist) setWin(win, { max: on });
  }

  function toggleMax(win: ConsoleWin) {
    setMax(win, !win.classList.contains("maximized"), true);
  }

  // A column restores like a maximize, so it shows the same control.
  function paintMaxButton(win: ConsoleWin) {
    const btn = win._maxBtn;
    if (!btn) return;
    const on = win.classList.contains("maximized") || win.classList.contains("column");
    btn.title = on ? "Restore" : "Maximize";
    btn.innerHTML = on
      ? '<i class="bi bi-fullscreen-exit"></i>'
      : '<i class="bi bi-fullscreen"></i>';
  }

  // Raise ONE console to the physical screen, or drop it back. A different axis
  // from `toggleMax`: maximize fills the workspace VIEWPORT, fullscreen fills the
  // DISPLAY. They compose — entering fullscreen never touches `.maximized`.
  //
  // Nothing here writes geometry: the top layer's UA `!important` sizing
  // outranks every author rule, the `.maximized` pin included, and the
  // per-window ResizeObserver refits the terminal.
  // The live browser fact, not our class: the guards must hold in the instant
  // between the state change and the event that mirrors it.
  function isFull(win: HTMLElement) {
    return document.fullscreenElement === win;
  }

  // ---- locked in place (ADR-0050 / ADR-0051 lock amendment) -----------------
  // A lock is a property of the DESK, honoured on every device: the gesture
  // handlers consult it and refuse; nothing else changes (maximize, fullscreen
  // and close do not rewrite the rect). A console is locked by its own record
  // OR by the fence holding its centre — the same `fenceOf` fold as membership.
  function isLocked(win: ConsoleWin) {
    if (win._deskLocked) return true;
    return heldByFence(win);
  }
  // The one place a window's own lock is set: flag, class, glyph.
  function applyLock(win: ConsoleWin, locked: boolean) {
    win._deskLocked = !!locked;
    win.classList.toggle("locked", !!locked);
    paintLockGlyph(win);
  }
  // The glyph shows the EFFECTIVE lock: a console held by a locked fence reads
  // closed like its fence. Held-only, the button is disabled — its own toggle
  // would change nothing the operator can see; the fence's lock is the one to
  // open.
  function paintLockGlyph(win: ConsoleWin) {
    const btn = win.querySelector<HTMLButtonElement>(".session-lock");
    if (!btn) return;
    const own = !!win._deskLocked;
    const held = !own && heldByFence(win);
    const locked = own || held;
    btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
    btn.title = held ? "Locked by its fence. Unlock the fence first." : own ? "Unlock" : "Lock in place";
    btn.setAttribute("aria-pressed", locked ? "true" : "false");
    btn.disabled = held;
  }
  function toggleLock(win: ConsoleWin) {
    applyLock(win, !win._deskLocked);
    setWin(win, { locked: !!win._deskLocked });
  }

  function toggleFull(win: ConsoleWin) {
    if (document.fullscreenElement === win) {
      // The promise rejects if we are already leaving; there is nothing to
      // recover, and `syncFullState` runs off the event either way.
      document.exitFullscreen().catch(() => {});
      return;
    }
    focusWin(win);
    // Requesting while ANOTHER element is fullscreen is a legal swap — browsers
    // move the top layer without a round trip through the exit.
    win.requestFullscreen().catch((err) => {
      // The browser refusing (no user gesture, a policy header). Say so; the
      // console is still usable maximized.
      console.warn("fullscreen refused", err);
    });
  }

  // The fullscreen control's LOOK, derived from `document.fullscreenElement`,
  // never written by the click handler: the operator leaves fullscreen by paths
  // this code never sees (Esc, the iPad swipe, a tab switch, a permission
  // prompt), and on a tablet a stale "exit" button is the only exit there is.
  function syncFullState() {
    const el = document.fullscreenElement;
    for (const btn of document.querySelectorAll<HTMLElement>(".session-full")) {
      const win = btn.closest(".session-window");
      const on = !!win && el === win;
      btn.title = on ? "Exit full screen" : "Full screen";
      btn.innerHTML = on
        ? '<i class="bi bi-fullscreen-exit"></i>'
        : '<i class="bi bi-arrows-fullscreen"></i>';
      // The touch-target grow lives on the WINDOW, not the button: the whole
      // titlebar is the operator's exit affordance on a tablet.
      win?.classList.toggle("fullscreen", on);
    }
    refreshCover();
  }

  // A maximized console is a FULL BLEED: anything stacked on top of it is a
  // window the operator cannot see the rest of. A restore spawns windows in
  // record order and each raises itself, so a maximized record restored early
  // ended up underneath the rest. Fixed at the END of the restore rather than
  // by a fixed z-index on `.maximized`, which would have to out-rank the focus
  // ladder and then nothing could be raised over it on purpose.
  function raiseMaximized() {
    const st = stage();
    if (!st) return;
    // Columns are a maximize too (ADR-0051 §5): all of them, in reading order.
    const at = (w: HTMLElement, v: string) => parseInt(w.style.getPropertyValue(v), 10) || 0;
    const cols = [...st.querySelectorAll<HTMLElement>(".session-window.column")].sort(
      (a, b) => at(a, "--col-index") - at(b, "--col-index") || at(a, "--row-index") - at(b, "--row-index"),
    );
    if (cols.length) {
      for (const w of cols) focusWin(w);
      return;
    }
    // The LAST one, if a desk somehow carries two: it is the one whose record
    // was written most recently, and exactly one window can usefully be on top.
    const all = st.querySelectorAll<HTMLElement>(".session-window.maximized");
    const win = all[all.length - 1];
    if (win) focusWin(win);
  }

  // The scroll freeze that keeps a maximized window's viewport pin honest.
  // DERIVED from the DOM at every layout mutation, never toggled by hand:
  // closing a maximized console never passes through `toggleMax`, and a
  // hand-held lock stranded `overflow:hidden` on the viewport for the rest of
  // the page's life (the unreachable-window state of ADR-0051 §4).
  function syncMaxLock() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    const maxed = !!st.querySelector(".session-window.maximized");
    ws.classList.toggle("maxlock", maxed);
    // The body-level mirror, for the phone bleed: the rail and the sidebar are
    // `#workspace`'s cousins, unreachable from `maxlock`. The width gate is CSS's.
    document.body?.classList.toggle("console-max", maxed);
    refreshCover();
  }

  // The maximize pin, DERIVED the same way: `--max-left`/`--max-top` are only
  // honest while they equal the viewport's CURRENT offsets.
  // INVARIANT: every path that changes `#workspace`'s scroll offsets ends here.
  // The viewport's `scroll` event (`wireStage`) covers gesture, wheel, scrollbar
  // AND `reveal`'s programmatic write; `toggleMax` calls it after the flip. Only
  // ever writes to `.maximized` and `.column` windows — un-maximize REMOVES both
  // properties.
  function syncMaxPin() {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    for (const win of st.querySelectorAll<HTMLElement>(".session-window.maximized, .session-window.column")) {
      win.style.setProperty("--max-left", ws.scrollLeft + "px");
      win.style.setProperty("--max-top", ws.scrollTop + "px");
    }
  }

  // The last extent published to the shell, so the dispatch below can be an
  // edge and not a level.
  let lastExtent = { width: 0, height: 0 };

  // Size the stage to hold every window. NOTHING here moves or resizes a
  // window: the plane grows under them and a small viewport scrolls (#336
  // deleted the clamp-and-refit). `grow` floors each axis at the current pixels
  // — shrinking mid-drag would clamp `scrollLeft` under the cursor; the exact
  // recompute runs on mouseup.
  // INVARIANT: every path that creates, moves, resizes, closes or restores a
  // window ends here.
  function applyExtent(opts?: ExtentOpts) {
    const ws = workspace();
    const st = stage();
    if (!ws || !st) return;
    // The one place the freeze is kept in step with what is on screen.
    syncMaxLock();
    // Read the DOM, not `wins`: a window is on the stage from `buildChrome`'s
    // append (before `spawnWindow` registers it) to its removal. Fences count
    // too — ADR-0051 §2 sizes the plane to windows AND fences.
    const rects = [...st.querySelectorAll<HTMLElement>(".session-window, .fence, .note-card")].map(restoreRect);
    const ext = stageExtent(
      rects,
      { width: ws.clientWidth, height: ws.clientHeight },
      STAGE_MARGIN,
    );
    const width = opts?.grow ? Math.max(ext.width, st.offsetWidth) : ext.width;
    const height = opts?.grow ? Math.max(ext.height, st.offsetHeight) : ext.height;
    st.style.width = width + "px";
    st.style.height = height + "px";
    // Publish the extent to the footer pill (#338), ONLY on a real change: a
    // drag folds the extent per mousemove and would re-render Alpine per frame.
    if (width !== lastExtent.width || height !== lastExtent.height) {
      lastExtent = { width, height };
      sendDocument(document, "workbench:stage-extent", { width, height });
    }
  }

  // The first extent after the stage can be measured is an edge again: the
  // shell has not heard one yet.
  function resetExtentEdge() {
    lastExtent = { width: -1, height: -1 };
  }

  return {
    fillsViewport,
    restoreRect,
    setMax,
    toggleMax,
    paintMaxButton,
    isFull,
    isLocked,
    applyLock,
    paintLockGlyph,
    toggleLock,
    toggleFull,
    syncFullState,
    raiseMaximized,
    syncMaxLock,
    syncMaxPin,
    applyExtent,
    resetExtentEdge,
  };
}
