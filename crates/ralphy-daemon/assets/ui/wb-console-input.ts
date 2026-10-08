/* ---------------------------------------------------------------------------
   The console's input folds — touch, pointer, key bar, clipboard and font
   rules, as pure functions of their arguments (ADR-0075 phase 5).

   Nothing here reads the DOM, the clock, a timer, `window` or a module-scope
   `let`: the same arguments give the same answer. `wb-console.ts` holds the
   gestures, the view store and the terminals, feeds them through these
   functions, and acts on the answer. Every function and constant is a member
   of `window.WBConsole` under its own name.

   `wb-console.ts` imports this module, on both documents (`index.html` and
   `detached-fence.html`).
   --------------------------------------------------------------------------- */

// The engine, not the brand: WebKit answers "Apple Computer, Inc." in every
// browser on iPadOS; Chromium "Google Inc."; Firefox "". Pure so the string
// table is the contract.
export function isWebKit(vendor: any) {
  return typeof vendor === "string" && vendor.startsWith("Apple");
}

// Whether this engine must render in the DOM instead of on the GPU: the WebGL
// addon draws scrolled rows twice on WebKit.
export function prefersDomRenderer(vendor: any) {
  return isWebKit(vendor);
}

// Whether to BUILD the fullscreen button. `fullscreenEnabled` is false in a
// sandboxed frame and a standalone PWA. On WebKit it is true but iOS drops
// out of fullscreen the moment a text field takes focus, so on an iPad the
// first keystroke would cancel it; maximize is the honest control there.
export function fullscreenOffered(enabled: any, vendor: any) {
  return enabled === true && !isWebKit(vendor);
}

// TOUCH SCROLLING is ours. MEASURED: the touch lands on `.xterm-screen`, and
// `.xterm-viewport` (the scroller) is its SIBLING, so the browser walks up to
// `#workspace` and pans the workbench; `overscroll-behavior: contain` on the
// viewport is inert for the same reason. A wheel works only because xterm
// forwards `wheel` in JS. Upstream: xterm.js #3613, #594, #5377.
// Pure: pixels dragged → lines, at the cell height, sign flipped. A
// zero/absent cell height yields 0, not Infinity.
export function touchScrollLines(dyPx: any, cellHeight: any) {
  if (!Number.isFinite(dyPx) || !Number.isFinite(cellHeight) || cellHeight <= 0) return 0;
  return -dyPx / cellHeight;
}

// WHO the gesture belongs to. xterm hands a wheel to the APPLICATION when it
// asked for mouse events (Claude Code and every full-screen TUI), turns it
// into arrow keys in the alternate buffer, and moves its own viewport only in
// the plain case — under a TUI the viewport's history is stale frames
// ("ghost" text). `mode` is `term.modes.mouseTrackingMode`; `bufferType` is
// `term.buffer.active.type`.
export function touchScrollTarget(mode: any, bufferType: any) {
  if (typeof mode === "string" && mode !== "none") return "app";
  if (bufferType === "alternate") return "app";
  return "viewport";
}

// How many fingers, whose gesture. One is the terminal's. Two are the
// CANVAS's: `touch-action: none` on the body took every browser gesture, so
// the pan is given back here through the same `scrollLeft/Top` writes the
// mouse pan makes. Under `maxlock` there is nowhere to pan. Three are the
// system's.
export function touchGesture(fingers: any, maxlock: any) {
  if (fingers === 1) return "terminal";
  if (fingers === 2 && !maxlock) return "canvas";
  return "none";
}

// The point between the fingers, which is what a two-finger pan tracks: the
// fingers can drift apart or together without the plane jumping.
export function touchCentroid(touches: any) {
  const list: any[] = Array.from(touches ?? []);
  if (!list.length) return { x: 0, y: 0 };
  let x = 0;
  let y = 0;
  for (const t of list) {
    x += t.clientX;
    y += t.clientY;
  }
  return { x: x / list.length, y: y / list.length };
}

// How far a press travels before it is a DRAG. A finger never holds still,
// and with `touch-action: none` the browser no longer tells a tap from a
// scroll for us: below the threshold the press is a click. The threshold
// DELAYS the start and never swallows the delta — placement runs from the
// grab offset taken at pointerdown. An unknown pointer type gets the
// finger's number.
const DRAG_THRESHOLD = { mouse: 4, touch: 10 };
export function dragThreshold(pointerType: any) {
  return pointerType === "mouse" ? DRAG_THRESHOLD.mouse : DRAG_THRESHOLD.touch;
}
export function dragBegins(start: any, pointer: any, threshold: any) {
  const dx = (pointer?.x || 0) - (start?.x || 0);
  const dy = (pointer?.y || 0) - (start?.y || 0);
  return Math.hypot(dx, dy) >= threshold;
}

// Two taps on a console's titlebar make a double tap when the second comes
// within DOUBLE_TAP_MS of the first and lands within a finger's drag
// threshold of it. Our own rule, not `dblclick`: WebKit on iOS does not
// turn two taps on a `touch-action: none` bar into one. `prev` and `tap`
// are `{ t, x, y }` (ms, client px); `prev` is null after a double tap.
export const DOUBLE_TAP_MS = 300;
export function isDoubleTap(prev: any, tap: any) {
  if (!prev || !tap) return false;
  const gap = tap.t - prev.t;
  return gap >= 0 && gap <= DOUBLE_TAP_MS && !dragBegins(prev, tap, DRAG_THRESHOLD.touch);
}
// A finger held this long on the console name, without moving past the
// drag threshold, renames the console.
export const HOLD_MS = 500;

// Inertia. Terminals hold thousands of lines and a strict 1:1 drag makes the
// scrollback unreachable by hand, which is the substance of xterm #594.
// `FLING_DECAY` is per 16ms frame; below `FLING_MIN` the glide has stopped
// being motion and starts being drift, so it is cut rather than eased.
const FLING_DECAY = 0.94;
export const FLING_MIN = 0.02; // px/ms
export function flingStep(velocity: any, ms: any) {
  if (!Number.isFinite(velocity) || !Number.isFinite(ms) || ms <= 0) {
    return { dy: 0, velocity: 0 };
  }
  const next = velocity * Math.pow(FLING_DECAY, ms / 16);
  return { dy: velocity * ms, velocity: Math.abs(next) < FLING_MIN ? 0 : next };
}

// THE KEY BAR (the tablet's missing row): a virtual keyboard has no Esc, no
// Ctrl and — on iOS — no arrows.
//
// The bytes each button sends. An arrow is NOT one sequence: in application
// cursor mode a full-screen program expects `ESC O A`, and `ESC [ A` there
// scrolls nothing. `appCursor` is read live off `term.modes`.
// Null-prototype: a plain literal answers `"toString"` with a function, and
// the name comes off a `data-key` attribute — a string from the DOM.
const KEY_BYTES = Object.assign(Object.create(null), {
  esc: "\x1b",
  tab: "\t",
  // CR, what a real Return key sends. Lets a menu be answered with the bar
  // alone, without opening the virtual keyboard.
  enter: "\r",
  // Opens an agent's command menu without the virtual keyboard.
  slash: "/",
  "ctrl-c": "\x03",
});
const ARROW_FINAL = Object.assign(Object.create(null), {
  up: "A",
  down: "B",
  right: "C",
  left: "D",
});
// `shift` is the bar's Shift latch. xterm's encodings: Tab becomes back-tab
// (CBT, which Claude Code cycles its modes on), and an arrow takes the
// modifier parameter 2 in both cursor modes. Esc, Enter, / and ^C have no
// Shift form here, so they are sent unchanged.
export function keySequence(name: any, appCursor: any, shift: any) {
  if (shift && name === "tab") return "\x1b[Z";
  const literal = KEY_BYTES[name];
  if (typeof literal === "string") return literal;
  const final = ARROW_FINAL[name];
  if (typeof final !== "string") return "";
  if (shift) return "\x1b[1;2" + final;
  return (appCursor ? "\x1bO" : "\x1b[") + final;
}

// One key-bar tap under the Shift latch. `shift` toggles the latch and sends
// nothing. A key that sends bytes uses the latch once and clears it; a key
// that sends nothing leaves it set.
export function barKey(name: any, appCursor: any, latched: any) {
  if (name === "shift") return { seq: "", latched: !latched };
  const seq = keySequence(name, appCursor, latched);
  return { seq, latched: seq ? false : latched };
}

// The latching Ctrl: a finger presses one key at a time, so `Ctrl` arms and
// the NEXT character is folded. Only a single printable character folds — `d`
// can be a whole paste or a bracketed-paste burst, and masking its first byte
// would corrupt it. Anything else passes through WITH the latch still set.
export function applyCtrlLatch(latched: any, d: any) {
  if (!latched || typeof d !== "string" || d.length !== 1) return { out: d, latched };
  const code = d.toUpperCase().charCodeAt(0);
  if (code < 0x40 || code > 0x5f) return { out: d, latched };
  return { out: String.fromCharCode(code & 0x1f), latched: false };
}

// Whether `d` is one of the answers xterm writes back for a terminal
// QUERY: cursor position and status (`CSI…R`, `CSI…n`), device attributes
// (`CSI…c`), mode and keyboard reports (`CSI…$y`, `CSI?…u`), window reports
// (`CSI…t`), and the DCS and OSC replies. xterm sends each answer as one
// `onData` call. A replayed backlog asks its old questions again (ConPTY's
// startup `ESC[6n` is always there), so these answers are dropped while it
// replays. A modified F3 (`CSI 1;5R`) has the same bytes as a cursor answer;
// the replay lasts a moment, so that collision is accepted.
const TERMINAL_REPLY =
  /^\x1b(?:\[(?:[?>]?[\d;]*[Rnc]|\??[\d;]+\$y|[\d;]+t|\?\d*u)|P[\s\S]*\x1b\\|\]\d+;[\s\S]*(?:\x07|\x1b\\))$/;
export function isTerminalReply(d: any) {
  return typeof d === "string" && TERMINAL_REPLY.test(d);
}

// Whether a window shows the bar. `mode` is "on", "off", or absent for auto
// (has a touch surface). `any-pointer` rather than `pointer`: an iPad with a
// Magic Keyboard reports a FINE primary pointer and is still a tablet.
export function keyBarVisible(mode: any, coarse: any) {
  if (mode === "on") return true;
  if (mode === "off") return false;
  return !!coarse;
}

// The `inputmode` of a terminal's input field. With the key bar shown, the
// virtual keyboard opens only from the bar's keyboard key: `none` keeps the
// field focused, so the bar keys, a paste and a hardware keyboard still
// type, with no virtual keyboard on screen. Without the bar the attribute
// is absent and the browser decides. Pure.
export function terminalInputMode(barShown: any, keyboardOpen: any) {
  return barShown && !keyboardOpen ? "none" : null;
}

// Whether the key bar offers a PASTE button. `readText` exists only in a
// secure context, and unlike the write there is no `execCommand` fallback
// for a read. Pure: takes the clipboard object (or `undefined`).
export function pasteOffered(clipboard: any) {
  return !!clipboard && typeof clipboard.readText === "function";
}

// THE RIGHT BUTTON is copy or paste, and the browser menu never opens over a
// console. It is never reported to the child either: a child that asked for
// mouse events gets each press as a report, and xterm clears the selection
// on every report (`SelectionService` on `onUserInput`), so the press meant
// to copy erased the text first. With a selection it copies; without one it
// pastes; where the clipboard cannot be read (an insecure origin) it does
// nothing, and Ctrl+V still pastes. Pure.
export function rightClickAction(hasSelection: any, canPaste: any) {
  if (hasSelection) return "copy";
  return canPaste ? "paste" : "none";
}

// THE LEFT BUTTON UNDER A TUI. xterm gives every press to a child that asked
// for mouse events and selects only with Shift (Option on macOS), a key no
// operator reaches for. A plain left press is held instead ("hold"): moved
// past the drag threshold it becomes a terminal selection, released in place
// it reaches the child as the click it was. A press with any modifier keeps
// xterm's own routing, so Alt+drag still gives the drag to the child. Pure:
// `mode` is `term.modes.mouseTrackingMode`.
export function pressRoute(mode: any, button: any, modified: any) {
  if (typeof mode !== "string" || mode === "none") return "pass";
  return button === 0 && !modified ? "hold" : "pass";
}

// The modifier that makes xterm select while a child owns the mouse
// (`shouldForceSelection`): Option on macOS, which needs
// `macOptionClickForcesSelection`, and Shift elsewhere. Alt is NOT set
// outside macOS: there it asks for a column selection. Pure.
export function forceSelectionKeys(platform: any) {
  return /Mac|iPhone|iPad/.test(platform || "") ? { altKey: true } : { shiftKey: true };
}

// A move with no button pressed is a report too (mode "any", DECSET 1003)
// and clears the selection the same way: moving the pointer to the right
// button would erase it. Held back while a selection exists. Pure.
export function holdMoveReport(mode: any, hasSelection: any, buttons: any) {
  return typeof mode === "string" && mode !== "none" && !!hasSelection && buttons === 0;
}

// THE PHONE BLEED. Fullscreen is withheld on WebKit (`fullscreenOffered`), so
// on a phone maximize is the ceiling and the chrome folds away below this
// width: `syncMaxLock` writes `body.console-max`, 01-base.css gates on the
// same number. Width, not pointer: an iPad keeps its chrome. 560px is the
// workbench's phone breakpoint (04-canvas.css, 11-appended.css).
export const PHONE_MAX_WIDTH = 560;
export function phoneBleed(maxed: any, viewportWidth: any) {
  return !!maxed && Number.isFinite(viewportWidth) && viewportWidth <= PHONE_MAX_WIDTH;
}

// The buffer row under a finger, for the line-selection mode. `selectLines`
// takes buffer-absolute rows, hence `viewportY`. Clamped to the screen so a
// finger that slid off the bottom selects to the last row; a zero/NaN cell
// height answers the top row, not NaN.
export function selectionRow(clientY: any, screenTop: any, cellHeight: any, rows: any, viewportY: any) {
  const base = Number.isFinite(viewportY) ? viewportY : 0;
  if (!Number.isFinite(cellHeight) || cellHeight <= 0 || !Number.isFinite(clientY)) return base;
  const last = Math.max(0, (Number.isFinite(rows) ? rows : 1) - 1);
  const row = Math.floor((clientY - (screenTop || 0)) / cellHeight);
  return base + Math.min(last, Math.max(0, row));
}

// TERMINAL FONT SIZE, per browser profile: an iPad and a desktop sharing this
// desk disagree about glyph size. FONT_DEFAULT is xterm's own default.
export const FONT_MIN = 10;
export const FONT_MAX = 28;
export const FONT_DEFAULT = 15;

export function stepFont(current: any, delta: any) {
  const from = Number.isFinite(current) ? current : FONT_DEFAULT;
  return Math.min(FONT_MAX, Math.max(FONT_MIN, Math.round(from) + delta));
}

// THE VIRTUAL KEYBOARD'S BITE out of the viewport, in px, published as
// `--kb-inset` (styles.css reads it on `.maximized` and `:fullscreen`). Pure:
// layout viewport minus what is visible.
//   iOS      PANS the visual viewport: `height` shrinks, `offsetTop` grows.
//   Android  with `interactive-widget=resizes-content` shrinks the layout
//            viewport itself, so this reads ~0 and the CSS var path is inert.
// A pinch is not a keyboard: `scale` gates it off.
const ZOOM_EPSILON = 0.01;
export function keyboardInset({ innerHeight, height, offsetTop, scale }: any) {
  if (typeof scale === "number" && Math.abs(scale - 1) > ZOOM_EPSILON) return 0;
  const inset = (innerHeight || 0) - (height || 0) - (offsetTop || 0);
  if (!Number.isFinite(inset) || inset <= 0) return 0;
  return Math.round(inset);
}

// The largest image a paste will send (ADR-0055 §4): the daemon's
// `MAX_IMAGE_BYTES`, mirrored so an oversized screenshot is refused before
// base64. The daemon remains the authority.
export const IMAGE_PASTE_MAX = 4 * 1024 * 1024;

// The image-paste rule (ADR-0055 §5), pure and tabled. `types` are the
// clipboard items' MIME types, `size` the image item's byte length. One of
//   "passthrough" — no image on the clipboard: xterm's own text paste runs;
//   "watched"     — an image, but this window only watches: refuse visibly;
//   "too-large"   — an image past the cap: refuse without sending;
//   "drop"        — an image to hand to `image.write`.
export function pasteDecision({ types, size, watching }: any) {
  const hasImage = (types || []).some(
    (t: any) => typeof t === "string" && t.startsWith("image/"),
  );
  if (!hasImage) return "passthrough";
  if (watching) return "watched";
  if (!(size >= 0) || size > IMAGE_PASTE_MAX) return "too-large";
  return "drop";
}

// What an agent put on the clipboard is pasted into a shell: a TRAILING
// NEWLINE turns a mis-paste into an execution (`curl … | sh\n`), and an
// escape sequence reaches the terminal it is pasted into. The C1 controls
// (U+0080 to U+009F) go too: U+009B is a one-character CSI, so a terminal that
// reads C1 runs it like an escape sequence. A code-point test
// so the source carries no control-character escapes of its own.
export function scrubClipboard(text: any) {
  let out = "";
  for (const ch of text.replace(/\r\n/g, "\n")) {
    const c = ch.codePointAt(0);
    // Keep tab (9) and newline (10); drop the rest of C0, DEL (127) and C1.
    if (c === 9 || c === 10 || (c >= 32 && c !== 127 && !(c >= 0x80 && c <= 0x9f))) out += ch;
  }
  return out.replace(/\n+$/, "");
}

// Pure over `ClipboardItem`s: the first image wins over text, as in the
// keyboard `paste` event (ADR-0055).
export async function clipboardContent(items: any) {
  const list: any[] = Array.from(items || []);
  for (const item of list) {
    const type = (item.types || []).find((t: any) => t.startsWith("image/"));
    if (type) return { image: await item.getType(type) };
  }
  for (const item of list) {
    if ((item.types || []).includes("text/plain")) {
      return { text: await (await item.getType("text/plain")).text() };
    }
  }
  return { text: "" };
}
