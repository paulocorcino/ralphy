/* ---------------------------------------------------------------------------
   The parts of the stage: the window states, the column paint, the fence
   list, the view and the window chrome, built together because each takes
   functions of the others.

   `createStageParts(deps)` builds the five factories, passes each one its
   `deps`, and returns them. `createConsole` in `wb-console.ts` builds it once
   per console and gives the five parts to the rest of the console. They read
   the console only through `deps`.
   --------------------------------------------------------------------------- */
import { createFenceList } from "./wb-stage-fence-list.ts";
import { createView } from "./wb-stage-view.ts";
import { createChrome } from "./wb-stage-chrome.ts";
import { createStageWindow } from "./wb-stage-window.ts";
import { createStageColumns } from "./wb-stage-columns.ts";
import type { ChromeDeps } from "./wb-stage-chrome.ts";
import type { FenceListDeps } from "./wb-stage-fence-list.ts";
import type { StageColumnsDeps } from "./wb-stage-columns.ts";
import type { StageWindowDeps } from "./wb-stage-window.ts";
import type { ViewDeps } from "./wb-stage-view.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleWin, DeskFence } from "./wb-types.d.ts";

// What the parts of the stage read from the console, and nothing else. The
// desk records, the desk state, the detach and the fences are built after the
// parts, so the console passes those as functions that it calls later.
export type StagePartsDeps = Pick<
  StageWindowDeps,
  "document" | "workspace" | "stage" | "focusWin" | "setWin" | "refreshCover"
> &
  Pick<StageColumnsDeps, "OPTS" | "wins" | "pageWindows" | "popups" | "focusWin" | "list" | "tearDownMember"> &
  Pick<
    FenceListDeps,
    "window" | "document" | "OPTS" | "popups" | "tabsPane" | "newFenceId" | "saveFences" | "buildFence"
  > &
  Pick<ViewDeps, "window" | "document" | "pageWindows" | "viewStore" | "focusWin" | "isDeskSettled" | "wins"> &
  Pick<
    ChromeDeps,
    | "OPTS"
    | "gestures"
    | "canRename"
    | "consolePrefix"
    | "newDeskId"
    | "renderTitle"
    | "sessionPresentation"
    | "startRename"
    | "takenNames"
  > & {
    currentDesk: StageColumnsDeps["desk"];
    currentFences: FenceListDeps["fences"];
    currentNotes: FenceListDeps["notes"];
  };

export function createStageParts(deps: StagePartsDeps) {
  const {
    window,
    document,
    OPTS,
    workspace,
    stage,
    tabsPane,
    pageWindows,
    wins,
    popups,
    gestures,
    viewStore,
    focusWin,
    setWin,
    saveFences,
    refreshCover,
    list,
    newFenceId,
    newDeskId,
    currentDesk,
    currentFences,
    currentNotes,
    isDeskSettled,
    tearDownMember,
    buildFence,
    canRename,
    consolePrefix,
    renderTitle,
    sessionPresentation,
    startRename,
    takenNames,
  } = deps;

  // ---- the window states: `wb-stage-window.ts` ---------------------------------
  // Built before the fence list, which takes three of its functions.
  const stageWindow = createStageWindow({
    document,
    workspace,
    stage,
    focusWin,
    setWin,
    heldByFence: (win: HTMLElement) => heldByFence(win),
    refreshCover,
  });
  const { restoreRect, setMax, toggleMax, paintMaxButton, isFull, isLocked, applyLock, paintLockGlyph, toggleLock, toggleFull, syncFullState, syncMaxLock, syncMaxPin, applyExtent, resetExtentEdge } = stageWindow;
  // ---- the column paint: `wb-stage-columns.ts` (ADR-0051 §5) -------------
  // Built before the fence list, which takes `columnMeasure`.
  const columns = createStageColumns({
    OPTS,
    wins,
    pageWindows,
    workspace,
    stage,
    popups,
    desk: () => currentDesk(),
    focusWin,
    setMax,
    paintMaxButton,
    syncMaxLock,
    syncMaxPin,
    applyExtent,
    list,
    paintFenceColumns: () => paintFenceColumns(),
    fenceList: () => fenceList(),
    readFenceRects: (st: HTMLElement) => readFenceRects(st),
    readWindowRects: (st: HTMLElement) => readWindowRects(st),
    findWindow: (id: string) => findWindow(id),
    tearDownMember: (win: ConsoleWin, reason: DetachReason) => tearDownMember(win, reason),
  });
  const { columnMeasure } = columns;
  // ---- the fence list: `wb-stage-fence-list.ts` --------------------------------
  // Built before the view and the fences, which it reaches through lazy arrows.
  const fenceFloor = createFenceList({
    window,
    document,
    OPTS,
    popups,
    fences: () => currentFences(),
    notes: () => currentNotes(),
    stage,
    workspace,
    tabsPane,
    restoreRect,
    applyExtent,
    columnMeasure,
    inGesture: gestures.active,
    newFenceId,
    paintLockGlyph,
    saveFences,
    jumpToFence: (id: string) => jumpToFence(id),
    buildFence: (f: DeskFence) => buildFence(f),
  });
  const { fenceLocked, heldByFence, fenceEl, refreshFenceChrome, paintFenceColumns, readFenceRects, readWindowRects, fenceList, focusedFenceId, focusFence, clearFenceFocus } = fenceFloor;

  // ---- the view: `wb-stage-view.ts` --------------------------------------------
  // Built before the terminal, which takes `cancelSlide`.
  const view = createView({
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
    focusedFence: focusedFenceId,
    isDeskSettled: () => isDeskSettled(),
    wins,
    resetExtentEdge,
    applyExtent,
    refreshFenceChrome,
  });
  const { reveal, findWindow, jumpToFence, autoPan } = view;

  // ---- the window chrome: `wb-stage-chrome.ts` ---------------------------------
  const chrome = createChrome({
    window,
    document,
    OPTS,
    gestures,
    stage,
    workspace,
    fences: () => currentFences(),
    focusedFence: focusedFenceId,
    applyExtent,
    applyLock,
    autoPan,
    canRename,
    consolePrefix,
    fenceEl,
    fenceLocked,
    focusWin,
    isFull,
    isLocked,
    newDeskId,
    renderTitle,
    restoreRect,
    reveal,
    sessionPresentation,
    setMax,
    setWin,
    startRename,
    takenNames,
    toggleFull,
    toggleLock,
    toggleMax,
  });

  return { stageWindow, columns, fenceFloor, view, chrome };
}
