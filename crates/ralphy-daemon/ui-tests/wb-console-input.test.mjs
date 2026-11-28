// Unit tests for assets/ui/wb-console-input.ts: the console's keys, paste
// and clipboard. Pure functions: each test calls the function directly, with
// no console and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyCtrlLatch,
  barKey,
  clipboardContent,
  forceSelectionKeys,
  holdMoveReport,
  isTerminalReply,
  keyBarVisible,
  keySequence,
  keyboardInset,
  pasteAfterRead,
  pasteDecision,
  pasteOffered,
  pressRoute,
  rightClickAction,
  scrubClipboard,
  terminalInputMode,
} from "../assets/ui/wb-console-input.ts";

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
  assert.equal(keySequence("esc", false), "\u001B");
  assert.equal(keySequence("tab", false), "\t");
  assert.equal(keySequence("enter", false), "\r");
  assert.equal(keySequence("ctrl-c", false), "\u0003");
  assert.equal(keySequence("slash", false), "/");
  // The mode does not touch them — only the arrows are mode-dependent.
  assert.equal(keySequence("esc", true), "\u001B");
  assert.equal(keySequence("enter", true), "\r");
  assert.equal(keySequence("ctrl-c", true), "\u0003");
  assert.equal(keySequence("slash", true), "/");
});

test("keySequence follows the terminal into application cursor mode", () => {
  // Normal mode: CSI. A shell's history and line editing read these.
  assert.equal(keySequence("up", false), "\u001B[A");
  assert.equal(keySequence("down", false), "\u001B[B");
  assert.equal(keySequence("right", false), "\u001B[C");
  assert.equal(keySequence("left", false), "\u001B[D");
  // Application mode: SS3. A full-screen program (which is what a vendor CLI's
  // menu is) asked for this, and sending CSI there scrolls nothing.
  assert.equal(keySequence("up", true), "\u001BOA");
  assert.equal(keySequence("down", true), "\u001BOB");
  assert.equal(keySequence("right", true), "\u001BOC");
  assert.equal(keySequence("left", true), "\u001BOD");
});

test("keySequence with the Shift latch sends xterm's shifted forms", () => {
  // Tab becomes back-tab, which Claude Code cycles its modes on.
  assert.equal(keySequence("tab", false, true), "\u001B[Z");
  assert.equal(keySequence("tab", true, true), "\u001B[Z");
  // A shifted arrow carries modifier 2, and it is CSI in both cursor modes.
  for (const appCursor of [false, true]) {
    assert.equal(keySequence("up", appCursor, true), "\u001B[1;2A");
    assert.equal(keySequence("down", appCursor, true), "\u001B[1;2B");
    assert.equal(keySequence("right", appCursor, true), "\u001B[1;2C");
    assert.equal(keySequence("left", appCursor, true), "\u001B[1;2D");
  }
  // No Shift form: sent unchanged.
  assert.equal(keySequence("esc", false, true), "\u001B");
  assert.equal(keySequence("enter", false, true), "\r");
  assert.equal(keySequence("ctrl-c", false, true), "\u0003");
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
  assert.deepEqual(applyCtrlLatch(true, "c"), { out: "\u0003", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "C"), { out: "\u0003", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "d"), { out: "\u0004", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "["), { out: "\u001B", latched: false });
});

test("applyCtrlLatch passes everything through while disarmed", () => {
  assert.deepEqual(applyCtrlLatch(false, "c"), { out: "c", latched: false });
  assert.deepEqual(applyCtrlLatch(false, "\u001B[A"), { out: "\u001B[A", latched: false });
});

test("applyCtrlLatch keeps the latch armed for input it cannot fold", () => {
  // An arrow is three bytes: masking the first would corrupt the escape and
  // silently eat the key the operator meant to modify.
  assert.deepEqual(applyCtrlLatch(true, "\u001B[A"), { out: "\u001B[A", latched: true });
  // A paste arrives as one long string on the same path.
  assert.deepEqual(applyCtrlLatch(true, "hello"), { out: "hello", latched: true });
  // Outside @-_ there is no control character to fold to.
  assert.deepEqual(applyCtrlLatch(true, "1"), { out: "1", latched: true });
  assert.deepEqual(applyCtrlLatch(true, "\u0003"), { out: "\u0003", latched: true });
  // Non-strings never reach the child, and must not throw on the way.
  assert.deepEqual(applyCtrlLatch(true), { out: undefined, latched: true });
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
  assert.equal(isTerminalReply("\u001B[1;1R"), true);
  assert.equal(isTerminalReply("\u001B[24;80R"), true);
  assert.equal(isTerminalReply("\u001B[?24;80R"), true);
  assert.equal(isTerminalReply("\u001B[0n"), true);
  assert.equal(isTerminalReply("\u001B[?1;2c"), true);
  assert.equal(isTerminalReply("\u001B[>0;276;0c"), true);
  assert.equal(isTerminalReply("\u001B[?2004;1$y"), true);
  assert.equal(isTerminalReply("\u001B[8;24;80t"), true);
  assert.equal(isTerminalReply("\u001B[?0u"), true);
  assert.equal(isTerminalReply("\u001BP1$r0m\u001B\\"), true);
  assert.equal(isTerminalReply("\u001B]11;rgb:0000/0000/0000\u001B\\"), true);
  assert.equal(isTerminalReply("\u001B]10;rgb:ffff/ffff/ffff\u0007"), true);
});

test("isTerminalReply lets typed keys and pastes through", () => {
  for (const typed of [
    "R",
    "c",
    "\r",
    "\u0003",
    "\u001B",
    "\u001B[A",
    "\u001BOR",
    "\u001B[3~",
    "\u001B[200~text\u001B[201~",
    "\u001B[I",
    "ls -la\r",
    "\u001B[1;1Rx",
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
  assert.equal(pasteOffered(), false);
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
  assert.deepEqual(forceSelectionKeys(), { shiftKey: true });
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

const item = (parts) => ({
  types: Object.keys(parts),
  getType: (t) => Promise.resolve(parts[t]),
});

// --- clipboardContent: what the paste key pastes ---------------------------

test("clipboardContent prefers an image, then text, then nothing", async () => {
  const png = { type: "image/png", size: 3 };
  const text = { text: () => Promise.resolve("hello") };
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

const fakeTerm = (log) => ({
  pasteImage: (blob) => log.push(["image", blob.type]),
  term: { paste: (text) => log.push(["text", text]), focus: () => log.push(["focus"]) },
});

// --- pasteAfterRead: the paste key once its clipboard read settles ---------

test("pasteAfterRead pastes only into the terminal still attached when the read settles", async () => {
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

// --- barKey: the key bar's Shift latch -------------------------------------

test("barKey: Shift toggles the latch and one key that sends bytes uses it", () => {
  assert.deepEqual(barKey("shift", false, false), { seq: "", latched: true });
  assert.deepEqual(barKey("shift", false, true), { seq: "", latched: false });
  assert.deepEqual(barKey("tab", false, true), { seq: "\u001B[Z", latched: false });
  assert.deepEqual(barKey("up", true, true), { seq: "\u001B[1;2A", latched: false });
  // Esc, Enter and / have no Shift form, but they still use the latch up.
  assert.deepEqual(barKey("esc", false, true), { seq: "\u001B", latched: false });
  assert.deepEqual(barKey("enter", false, true), { seq: "\r", latched: false });
  assert.deepEqual(barKey("slash", false, true), { seq: "/", latched: false });
  // A key that sends nothing leaves the latch as it was.
  assert.deepEqual(barKey("nope", false, true), { seq: "", latched: true });
  assert.deepEqual(barKey("tab", false, false), { seq: "\t", latched: false });
});

// --- scrubClipboard: what an agent may put on the clipboard -----------------

const SCRUB_ROWS = [
  { name: "a trailing newline would run a pasted command", input: "curl x | sh\n", expected: "curl x | sh" },
  { name: "trailing CRLF pairs go", input: "curl x | sh\r\n\r\n", expected: "curl x | sh" },
  { name: "an inner newline stays", input: "a\nb\n", expected: "a\nb" },
  {
    name: "escape sequences lose their escape byte",
    input: "a\u001B[31mred\u001B[0mb",
    expected: "a[31mred[0mb",
  },
  { name: "NUL, BEL and DEL go", input: "a\u0000b\u0007c\u007Fd", expected: "abcd" },
  {
    name: "the C1 controls go, such as the one-character CSI U+009B",
    input: "a\u009B31mb\u0080c\u009Fd",
    expected: "a31mbcd",
  },
  { name: "U+00A0 and above stay", input: " é", expected: " é" },
  { name: "tabs stay and CRLF inside becomes LF", input: "a\tb\r\nc", expected: "a\tb\nc" },
  { name: "non-ASCII text stays", input: "café \u{1F600}", expected: "café \u{1F600}" },
  { name: "an empty string stays empty", input: "", expected: "" },
];

test("scrubClipboard keeps only what an agent may put on the clipboard", () => {
  for (const row of SCRUB_ROWS) {
    assert.equal(scrubClipboard(row.input), row.expected, row.name);
  }
});
