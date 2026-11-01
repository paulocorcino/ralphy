/* ---------------------------------------------------------------------------
   ralphy workbench shell: the `WBConsole` object

   `buildConsoleApi` names every member of the console that the rest of the page
   reads (`window.WBConsole`), in one place. `wb-console.ts` builds the parts and
   hands them over; this module holds the folds it re-exports, so the list of
   members is read here and nowhere else. `ui-tests/wb-api-members.test.mjs`
   pins the list.
--------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { resumeDecision, CONNECT_TIMEOUT_MS, RESUME_DEBOUNCE_MS } from "./wb-resume.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { gpuHolders, dormantAfterMs, DORMANT_MARGIN_PX } from "./wb-console-gpu.ts";
import type { createDeskRecords } from "./wb-desk-records.ts";
import type { createFeeds } from "./wb-console-feeds.ts";
import type { createTitle } from "./wb-console-title.ts";
import type { createPopupRegistry } from "./wb-desk-popups.ts";
import type { createDormancy } from "./wb-console-dormancy.ts";
import type { createStageWindow } from "./wb-stage-window.ts";
import type { createStageColumns } from "./wb-stage-columns.ts";
import type { createFenceList } from "./wb-stage-fence-list.ts";
import type { createDetach } from "./wb-desk-detach.ts";
import type { createView } from "./wb-stage-view.ts";
import type { createConsoleKeys } from "./wb-console-keys.ts";
import type { createFences } from "./wb-stage-fences.ts";
import type { createDesk } from "./wb-desk.ts";
import type { createSpawn } from "./wb-console-spawn.ts";
import type { createPlaceholders } from "./wb-console-placeholder.ts";
import type { createChrome } from "./wb-stage-chrome.ts";
import type { createMessages } from "./wb-messages.ts";
import type { createStack, Gestures } from "./wb-stage-stack.ts";
import type { createPlace } from "./wb-console-place.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

// What `createConsole` hands over: the result object of each part, the
// console windows and the document's gestures.
export type ConsoleParts = {
  records: ReturnType<typeof createDeskRecords>;
  feeds: ReturnType<typeof createFeeds>;
  title: ReturnType<typeof createTitle>;
  popups: ReturnType<typeof createPopupRegistry>;
  dormancy: ReturnType<typeof createDormancy>;
  stageWindow: ReturnType<typeof createStageWindow>;
  columns: ReturnType<typeof createStageColumns>;
  fenceFloor: ReturnType<typeof createFenceList>;
  detach: ReturnType<typeof createDetach>;
  view: ReturnType<typeof createView>;
  keys: ReturnType<typeof createConsoleKeys>;
  fences: ReturnType<typeof createFences>;
  desk: ReturnType<typeof createDesk>;
  spawn: ReturnType<typeof createSpawn>;
  placeholders: ReturnType<typeof createPlaceholders>;
  chrome: ReturnType<typeof createChrome>;
  messages: ReturnType<typeof createMessages>;
  stack: ReturnType<typeof createStack>;
  gestures: Gestures;
  wins: Set<ConsoleWin>;
  place: ReturnType<typeof createPlace>;
};

export function buildConsoleApi(parts: ConsoleParts) {
  const {
    records,
    feeds,
    title,
    popups,
    dormancy,
    stageWindow,
    columns,
    fenceFloor,
    detach,
    view,
    keys,
    fences,
    desk,
    spawn,
    placeholders,
    chrome,
    messages,
    stack,
    gestures,
    wins,
  } = parts;
  return {
    boot: desk.boot,
    open: spawn.open,
    relaunchRequest: WBConsoleSession.relaunchRequest,
    // The store `checkouts` says its worktree listing of a ref changed: the
    // titles of that repo are drawn again.
    ingestWorktrees: title.ingestWorktrees,
    ingestSessions: feeds.ingestSessions,
    ingestProjects: feeds.ingestProjects,
    ingestFleet: feeds.ingestFleet,
    peerOfflineView: WBConsoleSession.peerOfflineView,
    peerReturnDecision: WBConsoleSession.peerReturnDecision,
    heldReturnDecision: WBConsoleSession.heldReturnDecision,
    peerHeld: WBConsoleSession.peerHeld,
    sessionRowFor: WBConsoleSession.sessionRowFor,
    arrangeFence: fences.arrangeFence,
    count: () => wins.size,
    refitAll: view.refitAll,
    resizeRect: WBGeometry.resizeRect,
    stageExtent: WBGeometry.stageExtent,
    bringIntoView: WBGeometry.bringIntoView,
    anchorIntoView: WBGeometry.anchorIntoView,
    viewLanding: WBGeometry.viewLanding,
    panNudge: WBGeometry.panNudge,
    autoPan: view.autoPan,
    reconnectDecision: WBConsoleSession.reconnectDecision,
    peerGate: WBConsoleSession.peerGate,
    endNotice: WBConsoleSession.endNotice,
    resumeDecision,
    resumeAll: dormancy.resumeAll,
    dormancyDecision: WBConsoleSession.dormancyDecision,
    birthDecision: WBConsoleSession.birthDecision,
    encodeDetach: WBConsoleSession.encodeDetach,
    encodeResize: WBConsoleSession.encodeResize,
    dormantAfterMs,
    DORMANT_MARGIN_PX,
    keyboardInset: WBConsoleInput.keyboardInset,
    raiseMaximized: stageWindow.raiseMaximized,
    touchScrollLines: WBConsoleInput.touchScrollLines,
    touchScrollTarget: WBConsoleInput.touchScrollTarget,
    touchGesture: WBConsoleInput.touchGesture,
    touchCentroid: WBConsoleInput.touchCentroid,
    dragThreshold: WBConsoleInput.dragThreshold,
    dragBegins: WBConsoleInput.dragBegins,
    isDoubleTap: WBConsoleInput.isDoubleTap,
    DOUBLE_TAP_MS: WBConsoleInput.DOUBLE_TAP_MS,
    prefersDomRenderer: WBConsoleInput.prefersDomRenderer,
    gpuHolders,
    isWebKit: WBConsoleInput.isWebKit,
    fullscreenOffered: WBConsoleInput.fullscreenOffered,
    flingStep: WBConsoleInput.flingStep,
    keySequence: WBConsoleInput.keySequence,
    barKey: WBConsoleInput.barKey,
    applyCtrlLatch: WBConsoleInput.applyCtrlLatch,
    terminalInputMode: WBConsoleInput.terminalInputMode,
    isTerminalReply: WBConsoleInput.isTerminalReply,
    keyBarVisible: WBConsoleInput.keyBarVisible,
    pasteOffered: WBConsoleInput.pasteOffered,
    rightClickAction: WBConsoleInput.rightClickAction,
    pressRoute: WBConsoleInput.pressRoute,
    forceSelectionKeys: WBConsoleInput.forceSelectionKeys,
    holdMoveReport: WBConsoleInput.holdMoveReport,
    clipboardContent: WBConsoleInput.clipboardContent,
    phoneBleed: WBConsoleInput.phoneBleed,
    PHONE_MAX_WIDTH: WBConsoleInput.PHONE_MAX_WIDTH,
    selectionRow: WBConsoleInput.selectionRow,
    stepFont: WBConsoleInput.stepFont,
    setFont: keys.setFont,
    fontSize: keys.fontSize,
    FONT_MIN: WBConsoleInput.FONT_MIN,
    FONT_MAX: WBConsoleInput.FONT_MAX,
    FONT_DEFAULT: WBConsoleInput.FONT_DEFAULT,
    setStaleProbe: feeds.setStaleProbe,
    RESUME_HIDDEN_MS: WBConsoleSession.RESUME_HIDDEN_MS,
    RESUME_DEBOUNCE_MS,
    CONNECT_TIMEOUT_MS,
    pasteDecision: WBConsoleInput.pasteDecision,
    reconcileDesk: WBDeskFolds.reconcileDesk,
    placeholderSession: WBDeskFolds.placeholderSession,
    restoreRect: stageWindow.restoreRect,
    columnClasses: WBDeskFolds.columnClasses,
    columnMeasure: columns.columnMeasure,
    applyColumns: columns.applyColumns,
    isRelaunching: placeholders.isRelaunching,
    markRelaunch: placeholders.markRelaunch,
    focusColumn: columns.focusColumn,
    focusedId: columns.focusedId,
    deskRecords: records.deskRecords,
    reloadDesk: desk.reloadDesk,
    deskFailure: desk.currentDeskFailure,
    setDeskFailureHook: desk.setDeskFailureHook,
    setDeskGoneHook: desk.setDeskGoneHook,
    startNewDesk: desk.startNewDesk,
    reloadForRestoredDesk: desk.reloadForRestoredDesk,
    dropClosedElsewhere: columns.dropClosedElsewhere,
    columnRoster: columns.columnRoster,
    sessionPresentation: title.sessionPresentation,
    consolePrefix: title.consolePrefix,
    list: feeds.list,
    reveal: view.reveal,
    // A console in its own tab (ADR-0037 §3d).
    placeTabs: parts.place.placeTabs,
    afterLogin: desk.afterLogin,
    checkoutOf: records.checkoutOf,
    setCheckout: records.setCheckout,
    checkouts: records.allCheckouts,
    checkoutMenu: title.checkoutMenu,
    checkoutMenuRows: title.checkoutMenuRows,
    // Asks the store `checkouts` to read the listing of a ref.
    ensureListing: title.ensureListing,
    askNotice: messages.askNotice,
    // The note card asks the same question the stage's own verbs do (ADR-0064
    // §11's delete), so there is one dialog in this workbench and not two.
    askConfirm: messages.askConfirm,
    whenDeskLoaded: desk.whenDeskLoaded,
    fenceSpawnRect: WBGeometry.fenceSpawnRect,
    nextFenceSlot: WBDeskFolds.nextFenceSlot,
    rectHolds: WBGeometry.rectHolds,
    fenceMembership: WBGeometry.fenceMembership,
    fenceSummaries: WBDeskFolds.fenceSummaries,
    fenceFits: WBGeometry.fenceFits,
    fenceMoveDelta: WBGeometry.fenceMoveDelta,
    tileIntoRect: WBGeometry.tileIntoRect,
    fenceRepos: WBDeskFolds.fenceRepos,
    fenceList: fenceFloor.fenceList,
    fenceCycle: WBDeskFolds.fenceCycle,
    fenceHolds: WBGeometry.fenceHolds,
    detachFold: WBDeskFolds.detachFold,
    peerFold: detach.peerFold,
    DETACH_MAX: WBDeskFolds.DETACH_MAX,
    detachFence: fences.detachFence,
    reattachFence: detach.reattachFence,
    isDetached: popups.isDetached,
    mountDetached: desk.mountDetached,
    noteNameOk: WBDeskFolds.noteNameOk,
    popupMatches: WBDeskFolds.popupMatches,
    stepFence: fenceFloor.stepFence,
    jumpToFence: view.jumpToFence,
    jumpToNote: view.jumpToNote,
    focusedFence: fenceFloor.focusedFenceId,
    spawnRectIn: WBGeometry.spawnRectIn,
    freeSpawnRect: WBGeometry.freeSpawnRect,
    createFence: fenceFloor.createFence,
    atFenceCap: fenceFloor.atFenceCap,
    nextFenceName: WBDeskFolds.nextFenceName,
    FENCE_MAX: WBDeskFolds.FENCE_MAX,
    renameFence: fenceFloor.renameFence,
    removeFence: fenceFloor.removeFence,
    // The note card's seam (ADR-0064 §2): the desk state lives here, the card
    // lives in `wb-notes.ts`, and these are everything it needs.
    notes: records.loadNotes,
    saveNotes: records.saveNotes,
    atNoteCap: records.atNoteCap,
    NOTE_MAX: WBDeskFolds.NOTE_MAX,
    atDeskCap: records.atDeskCap,
    DESK_MAX: WBDeskFolds.DESK_MAX,
    fenceRecords: fenceFloor.fenceRecords,
    // A desk this page takes never moves an element under a gesture.
    inGesture: gestures.active,
    // The window writes that are not a gesture (a birth, a reconnect, a first
    // act on an adopted console), for the ui-test: each must send only the
    // fields it changed.
    recordBirth: spawn.recordBirth,
    recordSession: spawn.recordSession,
    setWin: records.setWin,
    makeDraggable: chrome.makeDraggable,
    startResize: chrome.startResize,
    focusWin: stack.focusWin,
    stackWin: stack.stackWin,
    toast: messages.toast,
    dismissToast: messages.dismissToast,
    agentStateTitle: WBConsoleSession.agentStateTitle,
    renderNotes: fenceFloor.renderNotes,
  };
}
