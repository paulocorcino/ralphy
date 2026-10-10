// The member lists of the two page instances that other code reads by name:
// `WBConsole` (`createConsole`) and `WBNotes` (`createNotes`). Browser checks,
// the `index.html` markup and the torn-off pages call these members directly,
// so a cut that moves code out of either factory keeps every member, with the
// type of its value (ADR-0073, the 2026-10-10 amendment; #623).
//
// Each instance is built with stub globals and no `boot()`, as the other
// tests of the two modules build it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createConsole } from "../assets/ui/wb-console.ts";
import { createNotes } from "../assets/ui/wb-notes.ts";

const CHANGE = "a member list changes only by a design decision (an ADR or an amendment); " +
  "a cut keeps every member, and a member whose last internal user left stays as a delegate";

// `name type` of each member, sorted by name.
function members(api) {
  return Object.keys(api).sort().map((k) => `${k} ${typeof api[k]}`);
}

// Node 22 ships a real `BroadcastChannel`, and `createConsole` subscribes:
// an open channel holds the event loop open, so it is hidden for the call.
function buildConsole() {
  const window = { addEventListener() {} };
  const document = { readyState: "loading", addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    return createConsole(window, document, location, {});
  } finally {
    globalThis.BroadcastChannel = realBC;
  }
}

function buildNotes() {
  const noop = () => {};
  const window = { addEventListener: noop, removeEventListener: noop, setTimeout, clearTimeout, setInterval, clearInterval };
  const document = { readyState: "loading", addEventListener: noop, removeEventListener: noop };
  return createNotes(window, document, { console: null });
}

// The `WBConsole` members at 823cc84c.
const CONSOLE_MEMBERS = [
  "CONNECT_TIMEOUT_MS number",
  "DESK_MAX number",
  "DETACH_MAX number",
  "DORMANT_AFTER_MS number",
  "DORMANT_MARGIN_PX number",
  "DOUBLE_TAP_MS number",
  "FENCE_MAX number",
  "FONT_DEFAULT number",
  "FONT_MAX number",
  "FONT_MIN number",
  "NOTE_MAX number",
  "PHONE_MAX_WIDTH number",
  "RESUME_DEBOUNCE_MS number",
  "RESUME_HIDDEN_MS number",
  "afterLogin function",
  "agentStateTitle function",
  "anchorIntoView function",
  "applyColumns function",
  "applyCtrlLatch function",
  "arrangeFence function",
  "askConfirm function",
  "askNotice function",
  "atDeskCap function",
  "atFenceCap function",
  "atNoteCap function",
  "autoPan function",
  "barKey function",
  "birthDecision function",
  "boot function",
  "bringIntoView function",
  "checkoutMenu function",
  "checkoutMenuRows function",
  "checkoutOf function",
  "checkouts function",
  "clipboardContent function",
  "columnClasses function",
  "columnMeasure function",
  "columnRoster function",
  "consolePrefix function",
  "count function",
  "createFence function",
  "deskFailure function",
  "deskRecords function",
  "detachFence function",
  "detachFold function",
  "dismissToast function",
  "dormancyDecision function",
  "dragBegins function",
  "dragThreshold function",
  "dropClosedElsewhere function",
  "encodeDetach function",
  "encodeResize function",
  "endNotice function",
  "ensureListing function",
  "fenceCycle function",
  "fenceFits function",
  "fenceHolds function",
  "fenceList function",
  "fenceMembership function",
  "fenceMoveDelta function",
  "fenceRecords function",
  "fenceRepos function",
  "fenceSpawnRect function",
  "fenceSummaries function",
  "flingStep function",
  "focusColumn function",
  "focusWin function",
  "focusedFence function",
  "focusedId function",
  "fontSize function",
  "forceSelectionKeys function",
  "freeSpawnRect function",
  "fullscreenOffered function",
  "gpuHolders function",
  "heldReturnDecision function",
  "holdMoveReport function",
  "inGesture function",
  "ingestFleet function",
  "ingestProjects function",
  "ingestSessions function",
  "ingestWorktrees function",
  "isDetached function",
  "isDoubleTap function",
  "isRelaunching function",
  "isTerminalReply function",
  "isWebKit function",
  "jumpToFence function",
  "jumpToNote function",
  "keyBarVisible function",
  "keySequence function",
  "keyboardInset function",
  "list function",
  "makeDraggable function",
  "markRelaunch function",
  "mountDetached function",
  "nextFenceName function",
  "nextFenceSlot function",
  "noteNameOk function",
  "notes function",
  "open function",
  "panNudge function",
  "pasteDecision function",
  "pasteOffered function",
  "peerFold function",
  "peerGate function",
  "peerHeld function",
  "peerOfflineView function",
  "peerReturnDecision function",
  "phoneBleed function",
  "placeholderSession function",
  "popupMatches function",
  "prefersDomRenderer function",
  "pressRoute function",
  "raiseMaximized function",
  "reattachFence function",
  "reconcileDesk function",
  "reconnectDecision function",
  "recordBirth function",
  "recordSession function",
  "rectHolds function",
  "refitAll function",
  "relaunchRequest function",
  "reloadDesk function",
  "reloadForRestoredDesk function",
  "removeFence function",
  "renameFence function",
  "renderNotes function",
  "resizeRect function",
  "restoreRect function",
  "resumeAll function",
  "resumeDecision function",
  "reveal function",
  "rightClickAction function",
  "saveNotes function",
  "selectionRow function",
  "sessionPresentation function",
  "sessionRowFor function",
  "setCheckout function",
  "setDeskFailureHook function",
  "setDeskGoneHook function",
  "setFont function",
  "setStaleProbe function",
  "setWin function",
  "spawnRectIn function",
  "stackWin function",
  "stageExtent function",
  "startNewDesk function",
  "startResize function",
  "stepFence function",
  "stepFont function",
  "terminalInputMode function",
  "tileIntoRect function",
  "toast function",
  "touchCentroid function",
  "touchGesture function",
  "touchScrollLines function",
  "touchScrollTarget function",
  "viewLanding function",
  "whenDeskLoaded function",
];

// The `WBNotes` members at 823cc84c.
const NOTES_MEMBERS = [
  "DEFAULT_DIR string",
  "DEFAULT_FILL string",
  "DEFAULT_FONT string",
  "DEFAULT_INK string",
  "DEFAULT_SIZE string",
  "DEFAULT_TONE string",
  "FILLS object",
  "FONTS object",
  "INKS object",
  "MARKDOWN_HELP object",
  "NEW_NOTE_STYLE object",
  "NOTE_DEFAULT object",
  "NOTE_MIN object",
  "ON_TOP_BAND_BELOW number",
  "ON_TOP_FLOOR object",
  "ON_TOP_TOP number",
  "SAVE_AFTER_MS number",
  "SIZES object",
  "TONES object",
  "adoptDraft function",
  "anchorsOf function",
  "anyDirty function",
  "applyLock function",
  "bodyOf function",
  "buildCard function",
  "cardEl function",
  "closeCard function",
  "colorOf function",
  "create function",
  "draftOf function",
  "fillOf function",
  "flushAll function",
  "fontOf function",
  "inkOf function",
  "isAway function",
  "jump function",
  "keepOnTop function",
  "list function",
  "lockedBy function",
  "markdownHelp function",
  "mountDetached function",
  "noteDormancyDecision function",
  "noteSlug function",
  "onTopClamp function",
  "onTopNow function",
  "onTopRect function",
  "openFromExplorer function",
  "persistCards function",
  "putBack function",
  "render function",
  "savedLabel function",
  "sizeOf function",
  "spawnRect function",
  "stampName function",
  "styleOf function",
  "titleFieldOf function",
  "titleOf function",
  "toneOf function",
  "veiledOf function",
  "withStyle function",
  "withTitle function",
  "withVeil function",
];

test("the WBConsole member list is the decided one", () => {
  assert.deepEqual(members(buildConsole()), CONSOLE_MEMBERS, CHANGE);
});

test("the WBNotes member list is the decided one", () => {
  assert.deepEqual(members(buildNotes()), NOTES_MEMBERS, CHANGE);
});
