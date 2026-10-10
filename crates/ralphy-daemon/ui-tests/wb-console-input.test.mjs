// Unit tests for assets/ui/wb-console-input.ts — the console's input folds
// (touch, pointer, key bar, clipboard, font). Pure functions: each test calls
// the function directly, with no console and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import {
  DOUBLE_TAP_MS,
  FONT_DEFAULT,
  FONT_MAX,
  FONT_MIN,
  PHONE_MAX_WIDTH,
  applyCtrlLatch,
  barKey,
  clipboardContent,
  dragBegins,
  dragThreshold,
  flingStep,
  forceSelectionKeys,
  fullscreenOffered,
  holdMoveReport,
  isDoubleTap,
  isTerminalReply,
  isWebKit,
  keyBarVisible,
  keySequence,
  keyboardInset,
  pasteAfterRead,
  pasteDecision,
  pasteOffered,
  phoneBleed,
  prefersDomRenderer,
  pressRoute,
  rightClickAction,
  scrubClipboard,
  selectionRow,
  stepFont,
  terminalInputMode,
  touchCentroid,
  touchGesture,
  touchScrollLines,
  touchScrollTarget,
} from "../assets/ui/wb-console-input.ts";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");

// --- pasteDecision: the image-paste rule (ADR-0055 §5) ----------------------

test("pasteDecision lets a text-only paste fall through to xterm", () => {
  assert.equal(pasteDecision({ types: ["text/plain"], size: -1, watching: false }), "passthrough");
  assert.equal(pasteDecision({ types: [], size: -1, watching: false }), "passthrough");
  assert.equal(pasteDecision({ types: undefined, size: -1, watching: false }), "passthrough");
  // Even a watcher's TEXT paste is xterm's business (its own gate refuses it).
  assert.equal(pasteDecision({ types: ["text/plain"], size: -1, watching: true }), "passthrough");
});

test("pasteDecision hands an image to image.write for the baton holder", () => {
  assert.equal(pasteDecision({ types: ["image/png"], size: 1234, watching: false }), "drop");
  // Text alongside the image (a browser copies both from a web page): the image wins.
  assert.equal(
    pasteDecision({ types: ["text/html", "text/plain", "image/png"], size: 10, watching: false }),
    "drop",
  );
  assert.equal(pasteDecision({ types: ["image/png"], size: 0, watching: false }), "drop");
});

test("pasteDecision refuses a watcher's image visibly, before any size question", () => {
  assert.equal(pasteDecision({ types: ["image/png"], size: 10, watching: true }), "watched");
  assert.equal(pasteDecision({ types: ["image/png"], size: 1e9, watching: true }), "watched");
});

test("pasteDecision refuses an image past the daemon's cap without sending it", () => {
  const cap = 4 * 1024 * 1024;
  assert.equal(pasteDecision({ types: ["image/png"], size: cap, watching: false }), "drop");
  assert.equal(pasteDecision({ types: ["image/png"], size: cap + 1, watching: false }), "too-large");
  // An image item whose file could not be read has no size: refuse, never send.
  assert.equal(pasteDecision({ types: ["image/png"], size: -1, watching: false }), "too-large");
  assert.equal(pasteDecision({ types: ["image/png"], size: undefined, watching: false }), "too-large");
});

// --- keyboardInset: the virtual keyboard's bite out of the viewport --------
// iOS pans the visual viewport instead of resizing the layout one, so the
// keyboard's height has to be measured rather than reported.

test("keyboardInset is zero when no keyboard is up", () => {
  // The resting state on every desktop, and on Android once the layout viewport
  // has already shrunk by itself (interactive-widget=resizes-content).
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 900, offsetTop: 0, scale: 1 }),
    0,
  );
});

test("keyboardInset measures the occluded strip, panned or not", () => {
  // Resized visual viewport, not scrolled: the plain case.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 560, offsetTop: 0, scale: 1 }),
    340,
  );
  // iOS panned the visual viewport down by 120: the strip we cannot paint into
  // is what is left over BELOW it, not the whole difference — counting the pan
  // twice would shrink the console by more than the keyboard takes.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 560, offsetTop: 120, scale: 1 }),
    220,
  );
});

test("keyboardInset refuses to read a pinch as a keyboard", () => {
  // Zoomed in, `height` shrinks for a reason that has nothing to do with an
  // occluded bottom; subtracting it would shrink the console the operator just
  // zoomed into.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 400, offsetTop: 0, scale: 2.5 }),
    0,
  );
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 400, offsetTop: 0, scale: 0.5 }),
    0,
  );
  // A scale that is 1 to within measurement noise is not a pinch.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 560, offsetTop: 0, scale: 1.004 }),
    340,
  );
});

test("keyboardInset never returns a negative or a non-number", () => {
  // A visual viewport TALLER than the layout one is reported on some Android
  // builds mid-animation; a negative inset would grow the window off-screen.
  assert.equal(keyboardInset({ innerHeight: 900, height: 940, offsetTop: 0, scale: 1 }), 0);
  // Absent fields (a browser mid-teardown) must read as "no keyboard".
  assert.equal(keyboardInset({}), 0);
  assert.equal(keyboardInset({ innerHeight: NaN, height: 100, offsetTop: 0, scale: 1 }), 0);
  // Sub-pixel viewports are common on a scaled display: the CSS var is px.
  assert.equal(
    keyboardInset({ innerHeight: 900.4, height: 560.1, offsetTop: 0, scale: 1 }),
    340,
  );
});

// --- keySequence: the bytes a tapped key sends ----------------------------

test("keySequence sends the control characters a virtual keyboard has no key for", () => {
  assert.equal(keySequence("esc", false), "\x1b");
  assert.equal(keySequence("tab", false), "\t");
  assert.equal(keySequence("enter", false), "\r");
  assert.equal(keySequence("ctrl-c", false), "\x03");
  assert.equal(keySequence("slash", false), "/");
  // The mode does not touch them — only the arrows are mode-dependent.
  assert.equal(keySequence("esc", true), "\x1b");
  assert.equal(keySequence("enter", true), "\r");
  assert.equal(keySequence("ctrl-c", true), "\x03");
  assert.equal(keySequence("slash", true), "/");
});

test("keySequence follows the terminal into application cursor mode", () => {
  // Normal mode: CSI. A shell's history and line editing read these.
  assert.equal(keySequence("up", false), "\x1b[A");
  assert.equal(keySequence("down", false), "\x1b[B");
  assert.equal(keySequence("right", false), "\x1b[C");
  assert.equal(keySequence("left", false), "\x1b[D");
  // Application mode: SS3. A full-screen program (which is what a vendor CLI's
  // menu is) asked for this, and sending CSI there scrolls nothing.
  assert.equal(keySequence("up", true), "\x1bOA");
  assert.equal(keySequence("down", true), "\x1bOB");
  assert.equal(keySequence("right", true), "\x1bOC");
  assert.equal(keySequence("left", true), "\x1bOD");
});

test("keySequence with the Shift latch sends xterm's shifted forms", () => {
  // Tab becomes back-tab, which Claude Code cycles its modes on.
  assert.equal(keySequence("tab", false, true), "\x1b[Z");
  assert.equal(keySequence("tab", true, true), "\x1b[Z");
  // A shifted arrow carries modifier 2, and it is CSI in both cursor modes.
  for (const appCursor of [false, true]) {
    assert.equal(keySequence("up", appCursor, true), "\x1b[1;2A");
    assert.equal(keySequence("down", appCursor, true), "\x1b[1;2B");
    assert.equal(keySequence("right", appCursor, true), "\x1b[1;2C");
    assert.equal(keySequence("left", appCursor, true), "\x1b[1;2D");
  }
  // No Shift form: sent unchanged.
  assert.equal(keySequence("esc", false, true), "\x1b");
  assert.equal(keySequence("enter", false, true), "\r");
  assert.equal(keySequence("ctrl-c", false, true), "\x03");
  assert.equal(keySequence("slash", false, true), "/");
});

test("keySequence sends nothing for a name it does not know", () => {
  // The click handler routes `copy` and the font steps elsewhere; anything that
  // reaches here unrecognised must be silence, never a stray byte to the child.
  for (const name of ["copy", "font-up", "ctrl", "shift", "", null, undefined, "toString"]) {
    assert.equal(keySequence(name, false), "");
    assert.equal(keySequence(name, false, true), "");
  }
});

// --- applyCtrlLatch: a chord typed one finger at a time -------------------

test("applyCtrlLatch folds the next single character and disarms", () => {
  assert.deepEqual(applyCtrlLatch(true, "c"), { out: "\x03", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "C"), { out: "\x03", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "d"), { out: "\x04", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "["), { out: "\x1b", latched: false });
});

test("applyCtrlLatch passes everything through while disarmed", () => {
  assert.deepEqual(applyCtrlLatch(false, "c"), { out: "c", latched: false });
  assert.deepEqual(applyCtrlLatch(false, "\x1b[A"), { out: "\x1b[A", latched: false });
});

test("applyCtrlLatch keeps the latch armed for input it cannot fold", () => {
  // An arrow is three bytes: masking the first would corrupt the escape and
  // silently eat the key the operator meant to modify.
  assert.deepEqual(applyCtrlLatch(true, "\x1b[A"), { out: "\x1b[A", latched: true });
  // A paste arrives as one long string on the same path.
  assert.deepEqual(applyCtrlLatch(true, "hello"), { out: "hello", latched: true });
  // Outside @-_ there is no control character to fold to.
  assert.deepEqual(applyCtrlLatch(true, "1"), { out: "1", latched: true });
  assert.deepEqual(applyCtrlLatch(true, "\x03"), { out: "\x03", latched: true });
  // Non-strings never reach the child, and must not throw on the way.
  assert.deepEqual(applyCtrlLatch(true, undefined), { out: undefined, latched: true });
});

// --- terminalInputMode: the keyboard opens only from the key bar -----------

test("terminalInputMode hides the virtual keyboard until the bar opens it", () => {
  // A touch screen: the field keeps focus with no keyboard on screen.
  assert.equal(terminalInputMode(true, false), "none");
  // The keyboard key opened it: the browser shows its keyboard again.
  assert.equal(terminalInputMode(true, true), null);
  // No bar (a desktop): the attribute stays absent, whatever the state.
  assert.equal(terminalInputMode(false, false), null);
  assert.equal(terminalInputMode(false, true), null);
});

// --- isTerminalReply: the answers a replayed backlog must not send ----------

test("isTerminalReply knows each answer xterm writes back for a query", () => {
  // ConPTY's startup `ESC[6n`, answered again on every reattach: the stray `R`.
  assert.equal(isTerminalReply("\x1b[1;1R"), true);
  assert.equal(isTerminalReply("\x1b[24;80R"), true);
  assert.equal(isTerminalReply("\x1b[?24;80R"), true);
  assert.equal(isTerminalReply("\x1b[0n"), true);
  assert.equal(isTerminalReply("\x1b[?1;2c"), true);
  assert.equal(isTerminalReply("\x1b[>0;276;0c"), true);
  assert.equal(isTerminalReply("\x1b[?2004;1$y"), true);
  assert.equal(isTerminalReply("\x1b[8;24;80t"), true);
  assert.equal(isTerminalReply("\x1b[?0u"), true);
  assert.equal(isTerminalReply("\x1bP1$r0m\x1b\\"), true);
  assert.equal(isTerminalReply("\x1b]11;rgb:0000/0000/0000\x1b\\"), true);
  assert.equal(isTerminalReply("\x1b]10;rgb:ffff/ffff/ffff\x07"), true);
});

test("isTerminalReply lets typed keys and pastes through", () => {
  for (const typed of [
    "R",
    "c",
    "\r",
    "\x03",
    "\x1b",
    "\x1b[A",
    "\x1bOR",
    "\x1b[3~",
    "\x1b[200~text\x1b[201~",
    "\x1b[I",
    "ls -la\r",
    "\x1b[1;1Rx",
    undefined,
  ]) {
    assert.equal(isTerminalReply(typed), false, JSON.stringify(typed));
  }
});

// --- keyBarVisible: when the row appears ----------------------------------

test("keyBarVisible obeys an explicit choice over the device", () => {
  // The escape hatch in both directions: a desktop operator who wants the bar,
  // and a tablet operator with a hardware keyboard who does not.
  for (const coarse of [true, false]) {
    assert.equal(keyBarVisible("on", coarse), true);
    assert.equal(keyBarVisible("off", coarse), false);
  }
});

test("keyBarVisible defaults to whether the machine has a touch surface", () => {
  // Absent, null, or a spelling from a future version: all auto.
  for (const mode of [null, undefined, "unset", "auto", ""]) {
    assert.equal(keyBarVisible(mode, true), true);
    assert.equal(keyBarVisible(mode, false), false);
  }
});

// --- pasteOffered: the paste key's enabled state ---------------------------

test("pasteOffered needs a clipboard that can READ", () => {
  // An insecure LAN origin has no `navigator.clipboard` at all.
  assert.equal(pasteOffered(undefined), false);
  assert.equal(pasteOffered(null), false);
  // A write-only shim (the clipboard test recorder, or an old engine) is not
  // enough: there is no `execCommand` fallback for a read.
  assert.equal(pasteOffered({ writeText() {} }), false);
  assert.equal(pasteOffered({ readText: "yes" }), false);
  assert.equal(pasteOffered({ readText() {} }), true);
});

// --- rightClickAction / holdMoveReport: the mouse under a TUI --------------

test("rightClickAction copies a selection and pastes without one", () => {
  // The selection wins over paste: copying it is what the press was for.
  assert.equal(rightClickAction(true, true), "copy");
  // The copy has an `execCommand` fallback, so an insecure origin copies too.
  assert.equal(rightClickAction(true, false), "copy");
  assert.equal(rightClickAction(false, true), "paste");
  // An insecure origin cannot read the clipboard, and the browser menu stays
  // closed: the press does nothing.
  assert.equal(rightClickAction(false, false), "none");
});

test("pressRoute holds only a plain left press under a TUI", () => {
  for (const mode of ["x10", "vt200", "drag", "any"]) {
    assert.equal(pressRoute(mode, 0, false), "hold");
    // Any modifier keeps xterm's routing: Shift selects, Alt drags the child.
    assert.equal(pressRoute(mode, 0, true), "pass");
    // Middle and right have their own paths.
    assert.equal(pressRoute(mode, 1, false), "pass");
    assert.equal(pressRoute(mode, 2, false), "pass");
  }
  // A plain shell already selects on a drag.
  assert.equal(pressRoute("none", 0, false), "pass");
  assert.equal(pressRoute(undefined, 0, false), "pass");
});

test("forceSelectionKeys is Option on macOS and Shift elsewhere", () => {
  assert.deepEqual(forceSelectionKeys("MacIntel"), { altKey: true });
  assert.deepEqual(forceSelectionKeys("iPad"), { altKey: true });
  // Alt outside macOS asks xterm for a column selection.
  assert.deepEqual(forceSelectionKeys("Win32"), { shiftKey: true });
  assert.deepEqual(forceSelectionKeys("Linux x86_64"), { shiftKey: true });
  assert.deepEqual(forceSelectionKeys(undefined), { shiftKey: true });
});

test("holdMoveReport holds only button-less moves over a selection under a TUI", () => {
  assert.equal(holdMoveReport("any", true, 0), true);
  // Nothing to protect: the TUI keeps its hover.
  assert.equal(holdMoveReport("any", false, 0), false);
  // A pressed button is a drag, not a reach for the right button.
  assert.equal(holdMoveReport("any", true, 1), false);
  // No tracking: xterm reports nothing, so nothing to hold.
  assert.equal(holdMoveReport("none", true, 0), false);
  assert.equal(holdMoveReport(undefined, true, 0), false);
});

// --- clipboardContent: what the paste key pastes ---------------------------

test("clipboardContent prefers an image, then text, then nothing", async () => {
  const item = (parts) => ({
    types: Object.keys(parts),
    getType: async (t) => parts[t],
  });
  const png = { type: "image/png", size: 3 };
  const text = { text: async () => "hello" };
  // An iOS screenshot: image only — `readText()` would have resolved "".
  assert.deepEqual(await clipboardContent([item({ "image/png": png })]), { image: png });
  // Both on one item: the image wins, as in the keyboard paste event.
  assert.deepEqual(
    await clipboardContent([item({ "text/plain": text, "image/png": png })]),
    { image: png },
  );
  assert.deepEqual(await clipboardContent([item({ "text/plain": text })]), { text: "hello" });
  assert.deepEqual(await clipboardContent([item({ "text/html": text })]), { text: "" });
  assert.deepEqual(await clipboardContent([]), { text: "" });
});

// --- pasteAfterRead: the paste key once its clipboard read settles ---------

test("pasteAfterRead pastes only into the terminal still attached when the read settles", async () => {
  const fakeTerm = (log) => ({
    pasteImage: (blob) => log.push(["image", blob.type]),
    term: { paste: (text) => log.push(["text", text]), focus: () => log.push(["focus"]) },
  });
  const png = { type: "image/png" };
  // [case, what the read gives, what happens to `_term` while it waits,
  //  what the pressed terminal gets, what a new terminal gets]
  const rows = [
    ["text, still attached", { text: "ls" }, "keep", [["text", "ls"], ["focus"]], []],
    ["image, still attached", { image: png }, "keep", [["image", "image/png"], ["focus"]], []],
    ["a refused read, still attached", null, "keep", [["focus"]], []],
    ["asleep while the read waits", { text: "ls" }, "sleep", [], []],
    ["another terminal attached while the read waits", { text: "ls" }, "replace", [], []],
  ];
  for (const [name, content, during, wantPressed, wantOther] of rows) {
    const pressedLog = [];
    const otherLog = [];
    const pressed = fakeTerm(pressedLog);
    const win = { _term: pressed };
    let settle;
    const read = new Promise((resolve, reject) => {
      settle = () => (content ? resolve(content) : reject(new Error("NotAllowedError")));
    });
    const done = pasteAfterRead(win, pressed, read);
    if (during === "sleep") win._term = null;
    if (during === "replace") win._term = fakeTerm(otherLog);
    settle();
    // A rejection here is the unhandled error the key bar would leave behind.
    await done;
    assert.deepEqual(pressedLog, wantPressed, name);
    assert.deepEqual(otherLog, wantOther, name);
  }
});

// --- phoneBleed: when a maximized console folds the chrome away ------------

test("phoneBleed is maximize AND a phone-width viewport", () => {
  // The breakpoint is the workbench's phone breakpoint: 01-base.css gates the
  // phone layout on the same number, so a drift between the two fails here.
  const css = readFileSync(join(UI, "styles/01-base.css"), "utf8");
  const phone = css.match(/@media \(max-width: (\d+)px\) \{/);
  assert.ok(phone, "01-base.css has a phone-width @media block");
  assert.equal(PHONE_MAX_WIDTH, Number(phone[1]));
  assert.equal(phoneBleed(true, 390), true);
  assert.equal(phoneBleed(true, PHONE_MAX_WIDTH), true);
  assert.equal(phoneBleed(true, PHONE_MAX_WIDTH + 1), false);
  assert.equal(phoneBleed(true, 1280), false);
  // Not maximized: nothing folds, whatever the width.
  assert.equal(phoneBleed(false, 390), false);
  // A viewport that has never laid out reports NaN — no bleed, no throw.
  assert.equal(phoneBleed(true, NaN), false);
  assert.equal(phoneBleed(true, undefined), false);
});

// --- selectionRow: the buffer line under a finger --------------------------

test("selectionRow maps a touch to a buffer-absolute row", () => {
  // Screen at y=100, 20px rows, 24 rows on screen, scrolled 50 lines in.
  assert.equal(selectionRow(100, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(119, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(120, 100, 20, 24, 50), 51);
  assert.equal(selectionRow(345, 100, 20, 24, 50), 62);
  // Unscrolled: the row IS the buffer line.
  assert.equal(selectionRow(140, 100, 20, 24, 0), 2);
});

test("selectionRow clamps to the screen", () => {
  // Above the screen: the top row. Below it: the last row, never a line that
  // is not on screen.
  assert.equal(selectionRow(0, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(9999, 100, 20, 24, 50), 73);
  assert.equal(selectionRow(9999, 100, 20, 1, 0), 0);
});

test("selectionRow never answers NaN", () => {
  // A terminal that has not laid out has a zero cell height; a missing
  // viewportY is 0. Either way the answer is a row `selectLines` accepts.
  assert.equal(selectionRow(140, 100, 0, 24, 50), 50);
  assert.equal(selectionRow(140, 100, NaN, 24, 50), 50);
  assert.equal(selectionRow(NaN, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(140, 100, 20, 24, undefined), 2);
  assert.equal(selectionRow(140, undefined, 20, 24, 0), 7);
});

// --- stepFont: the A− / A+ range ------------------------------------------

test("stepFont walks one px at a time and stops at both ends", () => {
  assert.equal(stepFont(15, 1), 16);
  assert.equal(stepFont(15, -1), 14);
  assert.equal(stepFont(FONT_MAX, 1), FONT_MAX);
  assert.equal(stepFont(FONT_MIN, -1), FONT_MIN);
  // Past the ends from outside the range — a store hand-edited before the
  // normalisation in wb-view.ts was added.
  assert.equal(stepFont(400, 1), FONT_MAX);
  assert.equal(stepFont(1, -1), FONT_MIN);
});

test("stepFont starts from xterm's own default when nothing is stored", () => {
  // `fontSize()` answers null-ish when the profile has no preference; stepping
  // from there must land next to the size the operator is actually looking at.
  assert.equal(stepFont(null, 1), FONT_DEFAULT + 1);
  assert.equal(stepFont(undefined, -1), FONT_DEFAULT - 1);
  assert.equal(stepFont(NaN, 1), FONT_DEFAULT + 1);
  // Always an integer: a half-px size is a blurred glyph grid.
  assert.equal(Number.isInteger(stepFont(15.4, 1)), true);
});

// --- touchGesture / touchCentroid: how many fingers, whose gesture ---------
test("touchGesture gives one finger to the terminal, two to the canvas, the rest to the system", () => {
  // [case, finger count, maxlock, expected owner]
  const rows = [
    ["one finger", 1, false, "terminal"],
    ["two fingers", 2, false, "canvas"],
    // Under maxlock one finger stays the terminal's, and two go to nobody.
    ["one finger under maxlock", 1, true, "terminal"],
    ["two fingers under maxlock", 2, true, "none"],
    ["three fingers", 3, false, "none"],
    ["no fingers", 0, false, "none"],
    ["an unknown count", undefined, false, "none"],
  ];
  for (const [name, fingers, maxlock, want] of rows) {
    assert.equal(touchGesture(fingers, maxlock), want, name);
  }
});

test("touchCentroid is the point between the fingers", () => {
  assert.deepEqual(touchCentroid([{ clientX: 10, clientY: 20 }, { clientX: 30, clientY: 60 }]), { x: 20, y: 40 });
  assert.deepEqual(touchCentroid([{ clientX: 5, clientY: 5 }]), { x: 5, y: 5 });
  assert.deepEqual(touchCentroid([]), { x: 0, y: 0 });
  assert.deepEqual(touchCentroid(undefined), { x: 0, y: 0 });
});

// --- dragThreshold / dragBegins: a tap is not a drag ------------------------
test("dragThreshold is 4px for a mouse and 10px for a finger, a pen or an unknown pointer", () => {
  assert.equal(dragThreshold("mouse"), 4);
  assert.equal(dragThreshold("touch"), 10);
  assert.equal(dragThreshold("pen"), 10);
  assert.equal(dragThreshold(undefined), 10);
  assert.equal(dragThreshold(""), 10);
});

const BEGINS = [
  // [start, pointer, threshold, expected, why]
  [{ x: 0, y: 0 }, { x: 0, y: 0 }, 4, false, "no travel is a tap"],
  [{ x: 100, y: 100 }, { x: 103.99, y: 100 }, 4, false, "just under the mouse threshold"],
  [{ x: 100, y: 100 }, { x: 104, y: 100 }, 4, true, "exactly the mouse threshold arms"],
  [{ x: 0, y: 0 }, { x: 3, y: 4 }, 4, true, "the diagonal counts as 5, not 3 or 4"],
  [{ x: 0, y: 0 }, { x: 3, y: 4 }, 10, false, "5px is a finger's slip"],
  [{ x: 0, y: 0 }, { x: -6, y: 8 }, 10, true, "direction does not matter"],
  [{ x: 50, y: 50 }, { x: 50, y: 41 }, 10, false, "9px straight up is still a tap for a finger"],
];

for (const [start, pointer, threshold, want, why] of BEGINS) {
  test(`dragBegins: ${why}`, () => {
    assert.equal(dragBegins(start, pointer, threshold), want);
  });
}

// --- isDoubleTap: two taps on a titlebar ----------------------------------
const TAPS = [
  // [prev, tap, expected, why]
  [null, { t: 100, x: 0, y: 0 }, false, "a first tap is not a double tap"],
  [{ t: 0, x: 50, y: 50 }, { t: 200, x: 52, y: 49 }, true, "a second tap close in time and place"],
  [{ t: 0, x: 50, y: 50 }, { t: 300, x: 50, y: 50 }, true, "exactly the time limit still counts"],
  [{ t: 0, x: 50, y: 50 }, { t: 301, x: 50, y: 50 }, false, "one ms past the time limit is a new first tap"],
  [{ t: 0, x: 0, y: 0 }, { t: 100, x: 6, y: 8 }, false, "10px away is another place"],
  [{ t: 0, x: 0, y: 0 }, { t: 100, x: 5, y: 8 }, true, "just under 10px is the same place"],
  [{ t: 500, x: 0, y: 0 }, { t: 400, x: 0, y: 0 }, false, "a tap from the future is not a pair"],
];

for (const [prev, tap, want, why] of TAPS) {
  test(`isDoubleTap: ${why}`, () => {
    assert.equal(DOUBLE_TAP_MS, 300);
    assert.equal(isDoubleTap(prev, tap), want);
  });
}

// --- touchScrollTarget: whose gesture a finger's drag is -------------------
// The finger must be the trackpad, and xterm gives the trackpad's wheel to
// three different owners. Under a TUI that tracks the mouse the viewport's
// history is a heap of the app's stale frames — the "ghosts" an iPad showed.
test("touchScrollTarget moves the viewport only in the plain case", () => {
  // [case, mouse-tracking mode, buffer, expected owner]
  const rows = [
    // An app that is tracking the mouse gets the gesture, in either buffer.
    ...["x10", "vt200", "drag", "any"].flatMap((mode) => [
      [`${mode} tracking, normal buffer`, mode, "normal", "app"],
      [`${mode} tracking, alternate buffer`, mode, "alternate", "app"],
    ]),
    // The alternate buffer has no history, so the app gets it there too.
    ["no tracking, alternate buffer", "none", "alternate", "app"],
    ["no tracking, normal buffer", "none", "normal", "viewport"],
    ["an unknown mode, normal buffer", undefined, "normal", "viewport"],
    ["nothing known", null, undefined, "viewport"],
  ];
  for (const [name, mode, buffer, want] of rows) {
    assert.equal(touchScrollTarget(mode, buffer), want, name);
  }
});

// --- touchScrollLines: the gesture the console had to take back -----------
// A drag over a console used to pan the whole canvas: the touch lands on
// `.xterm-screen`, and the element that scrolls is its sibling, not its
// ancestor, so the browser walked up to `#workspace` (xterm.js #3613/#594).

test("touchScrollLines converts a drag into lines at the terminal's cell height", () => {
  // Dragging the content DOWN moves the VIEW up, hence the sign flip.
  assert.equal(touchScrollLines(-34, 17), 2);
  assert.equal(touchScrollLines(34, 17), -2);
  // Fractional on purpose: a slow drag moves less than a row per event, and
  // truncating each one on its own rounds the whole gesture away to nothing.
  assert.equal(touchScrollLines(-8.5, 17), 0.5);
});

test("touchScrollLines refuses to divide by a cell height it does not have", () => {
  // A terminal mid-teardown, or one that has never laid out, reports 0 —
  // and `-dy / 0` is Infinity, which `scrollLines` would take literally.
  assert.equal(touchScrollLines(-100, 0), 0);
  assert.equal(touchScrollLines(-100, -1), 0);
  assert.equal(touchScrollLines(-100, NaN), 0);
  assert.equal(touchScrollLines(NaN, 17), 0);
  assert.equal(touchScrollLines(undefined, 17), 0);
});

// --- flingStep: the glide that makes scrollback reachable by hand ---------

test("flingStep decays toward a stop and reports the distance for the frame", () => {
  const first = flingStep(1, 16);
  assert.equal(first.dy, 16);
  // One frame of decay, not a fixed subtraction: a longer frame decays more.
  assert.ok(first.velocity < 1 && first.velocity > 0.9);
  assert.ok(flingStep(1, 32).velocity < first.velocity);
});

test("flingStep cuts the glide once it stops being motion", () => {
  // Below the floor it is drift, not a fling — and a velocity that never
  // reaches zero is a requestAnimationFrame loop that never ends.
  assert.equal(flingStep(0.001, 16).velocity, 0);
  assert.equal(flingStep(0, 16).velocity, 0);
  // A long enough frame gap must also land on a stop rather than overshooting.
  assert.equal(flingStep(1, 100000).velocity, 0);
  // Garbage in never produces a moving glide.
  assert.deepEqual(flingStep(NaN, 16), { dy: 0, velocity: 0 });
  assert.deepEqual(flingStep(1, 0), { dy: 0, velocity: 0 });
});

test("a fling always terminates", () => {
  let v = 5;
  let frames = 0;
  while (v !== 0 && frames < 10000) {
    v = flingStep(v, 16).velocity;
    frames += 1;
  }
  assert.equal(v, 0, "the glide must reach a stop");
  assert.ok(frames < 200, `and get there quickly, not in ${frames} frames`);
});

// --- fullscreenOffered: where the fullscreen button is worth building -----
// Two independent reasons to withhold it, and the table keeps them separable:
// no API at all (sandboxed frame, standalone PWA), and an API that WebKit hands
// back the moment a text field takes focus.
test("fullscreenOffered builds the button only where the engine can hold it", () => {
  // [case, fullscreen API present, vendor, expected]
  const rows = [
    ["no API", false, "Google Inc.", false],
    ["an unknown API", undefined, "Google Inc.", false],
    ["no API and no vendor", null, "", false],
    // WebKit hands fullscreen back the moment the keyboard takes focus.
    ["WebKit", true, "Apple Computer, Inc.", false],
    ["Chromium", true, "Google Inc.", true],
    ["an empty vendor", true, "", true],
  ];
  for (const [name, api, vendor, want] of rows) {
    assert.equal(fullscreenOffered(api, vendor), want, name);
  }
});

test("isWebKit is the one engine question both decisions ask", () => {
  assert.equal(isWebKit("Apple Computer, Inc."), true);
  assert.equal(isWebKit("Google Inc."), false);
  assert.equal(isWebKit(""), false);
  assert.equal(isWebKit(undefined), false);
});

// --- prefersDomRenderer: which engines must not get the GPU renderer ------
// The WebGL addon draws scrolled rows twice on WebKit — reported from an iPad
// as the text "distorting", and reproducible by dragging the scrollbar, a path
// this module does not touch. Upstream has carried it for years (xterm.js
// #3357, #5816) and the standing answer is to not use the addon there.

test("prefersDomRenderer asks about the ENGINE, not the brand", () => {
  // [case, vendor, expected]
  const rows = [
    // Safari, and every other browser on iPadOS — all WebKit underneath, all
    // reporting the same vendor. That is exactly why the vendor is the question.
    ["WebKit", "Apple Computer, Inc.", true],
    ["Chromium", "Google Inc.", false],
    ["Firefox, which reports an empty vendor", "", false],
    // A browser that reports nothing at all keeps the faster renderer: the DOM
    // fallback is the safe answer for a KNOWN-bad engine, not a default.
    ["no vendor", undefined, false],
    ["a null vendor", null, false],
    ["a vendor that is not a string", 42, false],
  ];
  for (const [name, vendor, want] of rows) {
    assert.equal(prefersDomRenderer(vendor), want, name);
  }
});

// --- barKey: the key bar's Shift latch -------------------------------------

test("barKey: Shift toggles the latch and one key that sends bytes uses it", () => {
  assert.deepEqual(barKey("shift", false, false), { seq: "", latched: true });
  assert.deepEqual(barKey("shift", false, true), { seq: "", latched: false });
  assert.deepEqual(barKey("tab", false, true), { seq: "\x1b[Z", latched: false });
  assert.deepEqual(barKey("up", true, true), { seq: "\x1b[1;2A", latched: false });
  // Esc, Enter and / have no Shift form, but they still use the latch up.
  assert.deepEqual(barKey("esc", false, true), { seq: "\x1b", latched: false });
  assert.deepEqual(barKey("enter", false, true), { seq: "\r", latched: false });
  assert.deepEqual(barKey("slash", false, true), { seq: "/", latched: false });
  // A key that sends nothing leaves the latch as it was.
  assert.deepEqual(barKey("nope", false, true), { seq: "", latched: true });
  assert.deepEqual(barKey("tab", false, false), { seq: "\t", latched: false });
});

// --- scrubClipboard: what an agent may put on the clipboard -----------------

test("scrubClipboard drops the trailing newlines that would run a pasted command", () => {
  assert.equal(scrubClipboard("curl x | sh\n"), "curl x | sh");
  assert.equal(scrubClipboard("curl x | sh\r\n\r\n"), "curl x | sh");
  assert.equal(scrubClipboard("a\nb\n"), "a\nb");
});

test("scrubClipboard drops escape sequences and other control characters", () => {
  assert.equal(scrubClipboard("a\x1b[31mred\x1b[0mb"), "a[31mred[0mb");
  assert.equal(scrubClipboard("a\x00b\x07c\x7fd"), "abcd");
});

test("scrubClipboard drops the C1 controls, such as the one-character CSI U+009B", () => {
  assert.equal(scrubClipboard("a\u009b31mb\u0080c\u009fd"), "a31mbcd");
  assert.equal(scrubClipboard("\u00a0\u00e9"), "\u00a0\u00e9", "U+00A0 and above stay");
});

test("scrubClipboard keeps tabs, inner newlines and non-ASCII text", () => {
  assert.equal(scrubClipboard("a\tb\r\nc"), "a\tb\nc");
  assert.equal(scrubClipboard("café \u{1f600}"), "café \u{1f600}");
  assert.equal(scrubClipboard(""), "");
});
