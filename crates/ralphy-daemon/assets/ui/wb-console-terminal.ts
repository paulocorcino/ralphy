/* ---------------------------------------------------------------------------
   The console's terminal: xterm.js in a window body, wired to its session
   over `/ws/session` (ADR-0075 phase 5).

   `createTerminal(deps)` returns `attachTerminal`, `detachSocket` and
   `announceDetach` for one console (ADR-0075 D7). They read the console only
   through `deps`, and `TerminalDeps` lists every read, so `tsc` refuses a
   read outside it. `wb-console.ts` creates one per console and keeps the
   font size, the plane and the clipboard. Each `attachTerminal` call keeps
   the state of its own terminal and socket; the session is the daemon's.
   --------------------------------------------------------------------------- */
import { WBSessionRoute } from "./wb-session-route.ts";
import { WBFail } from "./wb-fail.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBConsoleSession from "./wb-console-session.ts";

const {
  prefersDomRenderer,
  touchScrollLines,
  touchScrollTarget,
  touchGesture,
  touchCentroid,
  dragThreshold,
  dragBegins,
  flingStep,
  barKey,
  applyCtrlLatch,
  isTerminalReply,
  pasteOffered,
  rightClickAction,
  pressRoute,
  forceSelectionKeys,
  holdMoveReport,
  selectionRow,
  pasteDecision,
  scrubClipboard,
  FLING_MIN,
} = WBConsoleInput;

const {
  encodeTerminal,
  encodeResize,
  encodeDetach,
  resumeDecision,
  endNotice,
  reconnectDecision,
  peerGate,
  TAG_TERMINAL,
  TAG_COMMAND,
  RESUME_DEBOUNCE_MS,
  CONNECT_TIMEOUT_MS,
} = WBConsoleSession;

// The terminal's surface, ADR-0035's palette. xterm.js takes no CSS variables
// (WebGL paints the glyphs), so these mirror :root in styles.css and must
// move with it — the lockstep `wb-monaco.ts` keeps.
// Base colours ONLY: the 16 ANSI slots stay xterm's defaults, the palette
// every vendor TUI picked its colours against. The background is pure black,
// not `--log-bg` — the same exception `wb-monaco.ts` makes.
const TERMINAL_THEME = {
  background: "#000000",
  foreground: "#d4ccc0", // --text
  cursor: "#e8d9a8", // --console-text
  cursorAccent: "#000000",
  selectionBackground: "#423a31", // --surface-hi
};

// The largest OSC 52 payload accepted, measured on the BASE64 so an oversized
// string is never materialised. xterm's own OSC limit is 10MB.
const OSC52_MAX_B64 = 128 * 1024;

// What the terminal reads from the console, and nothing else.
export type TerminalDeps = {
  // The console's page: `WBDaemon` (the image paste) and `getShell` (whether
  // the shell's key listener exists).
  window: any;
  // The session socket's origin, scheme-matched to the page.
  WS_ORIGIN: string;
  // The stored font size, read when a terminal is made.
  fontSize: () => number;
  // The plane and the viewport; null before the page has them.
  stage: () => any;
  workspace: () => any;
  // Stops a jump of the viewport in flight: a two-finger pan outranks it.
  cancelSlide: () => void;
  // The system clipboard. They stay in the console: the key bar uses them too.
  writeClipboard: (text: any, term: any) => void;
  readClipboard: () => Promise<any>;
};

export function createTerminal(deps: TerminalDeps) {
  const { window, WS_ORIGIN, fontSize, stage, workspace, cancelSlide, writeClipboard, readClipboard } =
    deps;

  // Announce `reason` on an open socket. A socket still connecting cannot send.
  function announceDetach(ws: any, reason: any) {
    if (reason && ws && ws.readyState === 1) ws.send(encodeDetach(reason));
  }

  // Retire a socket so its pending events cannot reach us. `onmessage` matters
  // as much as `onclose`: a frame still queued lands AFTER this returns, when
  // `ws` names the replacement. Local, not `WBDaemon`'s: this module loads on
  // its own in the node harness and the popup.
  function detachSocket(ws: any, reason: any) {
    if (!ws) return;
    ws.onclose = null;
    ws.onmessage = null;
    ws.onopen = null;
    ws.onerror = null;
    try {
      announceDetach(ws, reason);
      if (ws.readyState <= 1) ws.close();
    } catch {}
  }

  // Attach a real xterm.js terminal into `body`, wired to a PTY over `/ws/session`.
  // `opts` is one of: {repo, agent} (a NEW agent launch), {console:true[, repo]}
  // (a NEW free-console launch — home dir when `repo` absent), or
  // {id[, takeover][, watch]} (a REATTACH; `watch` is read-only). Returns a
  // handle so the window chrome can refit, take the baton, and close it.
  function attachTerminal(body: any, opts: any) {
    const term = new Terminal({ convertEol: false, theme: TERMINAL_THEME });
    // Set rather than passed: the constructor literal is pinned in lib.rs as
    // the theme contract; the size is a per-profile preference.
    term.options.fontSize = fontSize();
    // Option+drag selects on macOS while a TUI owns the mouse; Shift+drag is
    // xterm's default elsewhere (`shouldForceSelection`).
    term.options.macOptionClickForcesSelection = true;
    const fit = new FitAddon.FitAddon();
    term.loadAddon(fit);
    term.open(body);
    // GPU glyph rendering with a DOM fallback: on a lost context the addon is
    // disposed and xterm falls back to DOM without dropping the session.
    // NOT on WebKit: the addon renders scrolled rows twice there (xterm.js
    // #3357, #5816; reproduced with the scrollbar, so the renderer, not our
    // gesture). Every browser on iPadOS is WebKit.
    // The terminal starts on the DOM renderer; the page decides which windows
    // hold a context (`rebalanceGpu`, wb-console-gpu.ts) and calls `useGpu`/`dropGpu`.
    let webgl: any = null;
    // The canvases the addon added, so `dropGpu` asks only them for a context:
    // `getContext` on a canvas that has none would create one.
    let gpuCanvases: any = [];
    // A browser that cannot give a context is not asked again.
    let gpuBroken = false;
    function useGpu() {
      if (webgl || gpuBroken || prefersDomRenderer(navigator.vendor)) return;
      try {
        const addon = new WebglAddon.WebglAddon();
        // Lost to the browser: back to the DOM renderer without dropping the
        // session. The slot goes back to the page at its next rebalance.
        addon.onContextLoss(() => {
          if (webgl === addon) dropGpu();
        });
        const before = new Set(term.element.querySelectorAll("canvas"));
        term.loadAddon(addon);
        webgl = addon;
        gpuCanvases = [...term.element.querySelectorAll("canvas")].filter((c) => !before.has(c));
      } catch {
        gpuBroken = true;
      }
    }
    // MEASURED: disposing the addon does not free the browser's slot until the
    // context is collected; an explicit `loseContext` does (14 contexts, 8
    // disposed, 8 new: 7 "Too many active WebGL contexts" warnings without it,
    // 0 with it; Chromium headless, @xterm/addon-webgl 0.19.0, 2026-10-04).
    function dropGpu() {
      const addon = webgl;
      if (!addon) return;
      const canvases = gpuCanvases;
      webgl = null;
      gpuCanvases = [];
      addon.dispose();
      for (const c of canvases) c.getContext("webgl2")?.getExtension("WEBGL_lose_context")?.loseContext();
    }
    term.loadAddon(new WebLinksAddon.WebLinksAddon());

    // The touch gesture this terminal owns (`touchScrollLines`). Single finger
    // only; the stylesheet's `touch-action: none` already told the browser the
    // console is not a pan surface, and A+/A− is a terminal's zoom.
    let touchY: any = null;
    let touchX = 0;
    let touchLastY = 0;
    let touchAccum = 0;
    let touchLastAt = 0;
    let touchVelocity = 0;
    let fling = 0;
    // THE LINE-SELECTION MODE: xterm selects only through mouse events and the
    // touch handlers below spend the finger on scrolling. Armed by the key
    // bar's `sel` button for ONE gesture: a single-finger drag selects whole
    // buffer lines (`selectLines` is the public API; cell-precise is not), and
    // lifting the finger disarms it, leaving the selection for the copy button.
    let selecting = false;
    let selStart: any = null;
    const setSelecting = (on: any) => {
      selecting = !!on;
      selStart = null;
      if (typeof opts.onSelecting === "function") opts.onSelecting(selecting);
    };
    const rowAt = (clientY: any) => {
      const screen = term.element?.querySelector(".xterm-screen");
      const top = screen ? screen.getBoundingClientRect().top : 0;
      return selectionRow(clientY, top, cellHeight(), term.rows, term.buffer.active.viewportY);
    };
    const cellHeight = () => {
      const el = term.element;
      const rows = term.rows;
      return el && rows > 0 ? el.clientHeight / rows : 0;
    };
    // Scroll by a fractional number of lines, carrying the remainder (a slow
    // drag moves less than one row per event). Coalesced to ONE scroll per
    // frame: `touchmove` fires faster than the display refreshes, and several
    // paints in one frame show up as a half-updated screen on a slow renderer.
    let scrollRaf = 0;
    // The app's share goes in through xterm's OWN wheel listener as line-mode
    // wheel events — one per line, so `consumeWheelEvent` neither dampens nor
    // batches them — at the finger's coordinates (a mouse report carries the
    // cell). xterm then does what it does for the trackpad.
    const wheelToApp = (lines: any) => {
      const el = term.element;
      if (!el) return;
      const deltaY = Math.sign(lines);
      for (let n = Math.abs(lines); n > 0; n--) {
        el.dispatchEvent(
          new WheelEvent("wheel", {
            deltaY,
            deltaMode: WheelEvent.DOM_DELTA_LINE,
            clientX: touchX,
            clientY: touchY ?? touchLastY,
            bubbles: true,
            cancelable: true,
          }),
        );
      }
    };
    const flushScroll = () => {
      scrollRaf = 0;
      const whole = Math.trunc(touchAccum);
      if (whole === 0) return;
      touchAccum -= whole;
      // Decided per flush, not per gesture: an app can take the mouse or drop
      // into the alternate buffer while a finger is still down.
      if (touchScrollTarget(term.modes.mouseTrackingMode, term.buffer.active.type) === "app") wheelToApp(whole);
      else term.scrollLines(whole);
    };
    const scrollByPixels = (dy: any) => {
      touchAccum += touchScrollLines(dy, cellHeight());
      if (!scrollRaf) scrollRaf = requestAnimationFrame(flushScroll);
    };
    const stopScroll = () => {
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
      scrollRaf = 0;
    };
    const stopFling = () => {
      if (fling) cancelAnimationFrame(fling);
      fling = 0;
    };
    // Repaint every row: a renderer that left a row half-drawn mid-gesture is
    // corrected once at the end rather than every frame.
    const refreshScreen = () => {
      try {
        term.refresh(0, term.rows - 1);
      } catch {}
    };
    // A two-finger pan of the plane, live until either finger lifts. The
    // remaining finger does NOT resume a scroll: it never had a `touchstart`.
    let pan: any = null;
    const stopPan = () => {
      if (!pan) return;
      pan = null;
      stage()?.classList.remove("panning");
    };
    body.addEventListener(
      "touchstart",
      (e: any) => {
        stopFling();
        // Armed selection. Prevented so the synthesized click never reaches
        // xterm's mousedown, which would clear the selection; the textarea
        // keeps its focus, so the keyboard stays up.
        if (selecting && e.touches.length === 1) {
          e.preventDefault();
          touchY = null;
          selStart = rowAt(e.touches[0].clientY);
          try {
            term.selectLines(selStart, selStart);
          } catch {}
          return;
        }
        const ws = workspace();
        const gesture = touchGesture(e.touches.length, !!ws?.classList.contains("maxlock"));
        if (gesture === "canvas") {
          // A second finger ends the terminal's gesture: the plane owns the touch.
          touchY = null;
          stopScroll();
          touchAccum = 0;
          // The operator's hand outranks a jump in flight (as `onFloorDown`).
          cancelSlide();
          const c = touchCentroid(e.touches);
          pan = { x: c.x, y: c.y, left: ws.scrollLeft, top: ws.scrollTop };
          stage()?.classList.add("panning");
          return;
        }
        stopPan();
        if (gesture !== "terminal") {
          touchY = null;
          return;
        }
        touchY = e.touches[0].clientY;
        touchX = e.touches[0].clientX;
        touchAccum = 0;
        touchVelocity = 0;
        touchLastAt = e.timeStamp;
        // NOT prevented: the tap must reach xterm, or the terminal never takes
        // focus and the on-screen keyboard never opens.
      },
      // Non-passive ONLY for the armed-selection branch above.
      { passive: false },
    );
    body.addEventListener(
      "touchmove",
      (e: any) => {
        if (selecting && selStart != null) {
          if (e.touches.length === 1) {
            const row = rowAt(e.touches[0].clientY);
            try {
              term.selectLines(Math.min(selStart, row), Math.max(selStart, row));
            } catch {}
          }
          e.preventDefault();
          return;
        }
        if (pan) {
          if (e.touches.length !== 2) return;
          const ws = workspace();
          const c = touchCentroid(e.touches);
          if (ws) {
            ws.scrollLeft = pan.left - (c.x - pan.x);
            ws.scrollTop = pan.top - (c.y - pan.y);
          }
          e.preventDefault();
          return;
        }
        if (touchY == null || e.touches.length !== 1) return;
        const y = e.touches[0].clientY;
        touchX = e.touches[0].clientX;
        const dy = y - touchY;
        touchY = y;
        const dt = e.timeStamp - touchLastAt;
        touchLastAt = e.timeStamp;
        if (dt > 0) touchVelocity = dy / dt;
        scrollByPixels(dy);
        // Without this the canvas underneath pans instead.
        e.preventDefault();
      },
      { passive: false },
    );
    const endTouch = (e: any) => {
      // The selection stays (the copy button reads it); the arming does not.
      if (selecting) {
        if (e.touches.length === 0) setSelecting(false);
        return;
      }
      if (pan) {
        if (e.touches.length < 2) stopPan();
        return;
      }
      if (touchY == null) return;
      touchLastY = touchY;
      touchY = null;
      // A finger lifted long after it stopped moving is a hold, not a flick.
      if (e.timeStamp - touchLastAt > 80 || Math.abs(touchVelocity) < FLING_MIN) {
        refreshScreen();
        return;
      }
      let v = touchVelocity;
      let last = performance.now();
      const glide = (now: any) => {
        const step = flingStep(v, now - last);
        last = now;
        v = step.velocity;
        // Already inside a frame: flush HERE, not through `scrollByPixels`.
        touchAccum += touchScrollLines(step.dy, cellHeight());
        stopScroll();
        flushScroll();
        fling = v ? requestAnimationFrame(glide) : 0;
        // The glide has stopped: correct a dropped partial paint once.
        if (!fling) refreshScreen();
      };
      fling = requestAnimationFrame(glide);
    };
    body.addEventListener("touchend", endTouch, { passive: true });
    body.addEventListener("touchcancel", endTouch, { passive: true });
    fit.fit();

    // A pasted IMAGE is not text (ADR-0055): xterm forwards only `text/plain`.
    // The bytes become a clipboard drop (`image.write`) and the drop's PATH is
    // pasted through `term.paste` — bracketed when the child asked, NO trailing
    // newline either way. Text falls through untouched. The gate mirrors
    // `onData`: a watcher SEES the refusal, and nothing leaves its window.
    // Returns whether the paste was taken (false = let the text through).
    const dropImage = (types: any, file: any) => {
      const decision = pasteDecision({ types, size: file ? file.size : -1, watching });
      if (decision === "passthrough") return false;
      if (decision === "watched") {
        if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
        return true;
      }
      if (decision === "too-large") {
        term.write("\r\n[paste refused — too large]\r\n");
        return true;
      }
      const daemon = window.WBDaemon;
      if (!daemon) {
        // The popup forgot its bridge: say so rather than swallow the paste.
        term.write("\r\n[paste refused — daemon not connected]\r\n");
        return true;
      }
      const reader = new FileReader();
      reader.onerror = () => term.write("\r\n[paste refused — unreadable]\r\n");
      reader.onload = () => {
        const base64 = String(reader.result).replace(/^data:[^,]*,/, "");
        daemon
          .write("image.write", { repo: currentRepo, base64 })
          .then((reply: any) => {
            if (WBFail.isError(reply) || !reply.path) {
              const why = WBFail.why(reply, "the daemon refused it");
              term.write(`\r\n[paste refused — ${why}]\r\n`);
              return;
            }
            term.paste(reply.path);
            term.focus();
          })
          // The socket closed with no reply: say what the browser saw.
          .catch((err: any) => {
            const why = (err && err.message) || "connection unavailable";
            term.write(`\r\n[paste refused — ${why}]\r\n`);
          });
      };
      reader.readAsDataURL(file);
      return true;
    };
    term.textarea.addEventListener("paste", (e: any) => {
      const items: any[] = Array.from(e.clipboardData?.items ?? []);
      const image = items.find((i) => i.type.startsWith("image/"));
      const file = image ? image.getAsFile() : null;
      if (dropImage(items.map((i) => i.type), file)) e.preventDefault();
    });

    // OSC 52 — "put this on the clipboard". xterm's core does not implement
    // it, and OSC 52 has no reply, so an agent would announce a copy it never
    // got. WRITE ONLY:
    // - The READ form (`52;c;?`) is never answered: it would let an agent read
    //   the operator's clipboard.
    // - Refused while `replaying`: the scrollback replays as raw bytes, and a
    //   copy from an hour ago would rewrite the clipboard on every reattach.
    // - Refused in a watcher: the same bytes reach EVERY attached window, and
    //   the window whose operator asked owns the clipboard. (Copying BY HAND
    //   in a watcher stays allowed — the #335 gate is about writing to the
    //   child.)
    term.parser.registerOscHandler(52, (data: any) => {
      // NEVER return the clipboard promise: `OscHandler.end` PAUSES the parser
      // on a promise, and a rejected write (unfocused document) would stall
      // the terminal.
      if (replaying || watching) return true;
      const semi = data.indexOf(";");
      if (semi < 0) return true;
      const payload = data.slice(semi + 1);
      // `?` reads and `!` clears; both are no-ops.
      if (payload === "?" || payload === "!") return true;
      if (payload.length > OSC52_MAX_B64) return true;
      let text;
      try {
        const bin = atob(payload);
        text = new TextDecoder().decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
      } catch {
        // base64url or bad padding: `atob` THROWS into xterm's parser.
        return true;
      }
      writeClipboard(scrubClipboard(text), term);
      return true;
    });

    // Ctrl+Insert copies the selection. NOT Ctrl+Shift+C: on Chrome/Edge that
    // is the DevTools accelerator and a page cannot take it back. Ctrl+C
    // belongs to the child.
    term.attachCustomKeyEventHandler((e: any) => {
      // Alt+Shift+arrows in a column walk the columns and their rows (ADR-0051
      // §5): xterm must not send them to the child, and the document listener
      // of the columns (wb-consoles-tab.ts) takes them.
      if (
        e.altKey &&
        e.shiftKey &&
        !e.ctrlKey &&
        !e.metaKey &&
        /^Arrow(Left|Right|Up|Down)$/.test(e.code) &&
        body.closest(".session-window")?.classList.contains("column")
      ) {
        return false;
      }
      // Alt+Shift+R and Alt+Shift+<digit> open a console from inside a
      // terminal too. Only where the console menus' document listener exists
      // (wb-consoles-tab.ts, on the page with a shell): a
      // detached popup has none, so its terminal keeps the key.
      if (
        e.altKey &&
        e.shiftKey &&
        !e.ctrlKey &&
        !e.metaKey &&
        /^(?:Digit\d|KeyR)$/.test(e.code) &&
        typeof window.getShell === "function"
      ) {
        return false;
      }
      if (e.type !== "keydown" || !e.ctrlKey || e.shiftKey || e.altKey) return true;
      if (e.key !== "Insert" || !term.hasSelection()) return true;
      writeClipboard(term.getSelection(), term);
      return false;
    });
    // `rightClickAction` and `holdMoveReport`, applied. CAPTURE phase on
    // `body`: xterm binds its listeners on `term.element`, a child, so a stop
    // here means neither xterm nor the child gets the event. The decision is
    // made on `mousedown`, while the selection still exists. Both clipboard
    // calls run inside the press, a user gesture.
    body.addEventListener(
      "mousedown",
      (e: any) => {
        if (e.button !== 2) return;
        const rightTaken = rightClickAction(term.hasSelection(), pasteOffered(navigator.clipboard));
        e.stopPropagation();
        // xterm's own mousedown focused the terminal; it no longer runs.
        e.preventDefault();
        term.focus();
        if (rightTaken === "copy") {
          writeClipboard(term.getSelection(), term);
          term.clearSelection();
        } else if (rightTaken === "paste") {
          readClipboard()
            .then(({ image, text }: any) => {
              if (image) dropImage([image.type], image);
              else if (text) term.paste(text);
            })
            .catch(() => {});
        }
      },
      true,
    );
    body.addEventListener(
      "contextmenu",
      (e: any) => {
        e.preventDefault();
        e.stopPropagation();
      },
      true,
    );
    // `pressRoute`, applied. The held press is REPLAYED to xterm as a
    // synthetic event: with the force-selection key once it turns into a
    // drag, or as itself (then the release) once it ends in place. The
    // replays are marked so this listener lets them through. The real release
    // is stopped: xterm adds its `mouseup` listener to the document during the
    // replayed press, and the real release would report a second time.
    const replayed = new WeakSet();
    const replay = (target: any, type: any, from: any, keys: any) => {
      const ev = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        view: from.view,
        clientX: from.clientX,
        clientY: from.clientY,
        screenX: from.screenX,
        screenY: from.screenY,
        button: 0,
        buttons: type === "mousedown" ? 1 : 0,
        // xterm's selection starts only on `detail === 1` (a single click).
        detail: 1,
        ...keys,
      });
      replayed.add(ev);
      target.dispatchEvent(ev);
    };
    body.addEventListener(
      "mousedown",
      (e: any) => {
        if (replayed.has(e) || !term.element?.contains(e.target)) return;
        const modified = e.shiftKey || e.altKey || e.ctrlKey || e.metaKey;
        if (pressRoute(term.modes.mouseTrackingMode, e.button, modified) !== "hold") return;
        e.stopPropagation();
        e.preventDefault();
        term.focus();
        const doc = body.ownerDocument;
        const start = { x: e.clientX, y: e.clientY };
        const end = () => {
          doc.removeEventListener("mousemove", onMove, true);
          doc.removeEventListener("mouseup", onUp, true);
        };
        const onMove = (m: any) => {
          if (!dragBegins(start, { x: m.clientX, y: m.clientY }, dragThreshold("mouse"))) return;
          end();
          // The selection service is disabled under tracking, so it does not
          // extend an old selection; a new drag replaces it.
          term.clearSelection();
          replay(e.target, "mousedown", e, forceSelectionKeys(navigator.platform || navigator.userAgent));
        };
        const onUp = (u: any) => {
          end();
          u.stopPropagation();
          replay(e.target, "mousedown", e, {});
          replay(e.target, "mouseup", u, {});
        };
        doc.addEventListener("mousemove", onMove, true);
        doc.addEventListener("mouseup", onUp, true);
      },
      true,
    );
    body.addEventListener(
      "mousemove",
      (e: any) => {
        if (holdMoveReport(term.modes.mouseTrackingMode, term.hasSelection(), e.buttons)) {
          e.stopPropagation();
        }
      },
      true,
    );
    // Refit whenever THIS window's body changes size. The only ResizeObserver
    // in the file; it resizes a TERMINAL, never a window rect (#336).
    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {}
    });
    ro.observe(body);

    let currentSessionId = opts.id ?? null;
    let currentRepo = opts.repo ?? null;
    let currentDaemonId: any = null;
    let currentEnvironment: any = null;
    let leaving = false;

    // A dropped socket does NOT end the session: the daemon keeps the child
    // alive (session_ws's teardown invariant), so a close is recovered by
    // reattaching to the SAME id. The daemon replays scrollback on reattach,
    // so the terminal is reset on a reconnecting open. Backoff is exponential
    // with jitter, capped.
    //
    // NO RECONNECT EVER CARRIES `takeover` (#334): two open workbenches
    // reclaiming the writer slot on a timer evicted each other ~1.1s per flip,
    // indefinitely. The baton moves only on `takeOver()`. `reconnectDecision`
    // owns the choice; this carries it out.
    const RECONNECT_BASE = 1000;
    const RECONNECT_MAX = 15000;
    let ws: any = null;
    let opened = false; // has the CURRENT socket opened
    let everOpened = false; // has ANY socket of this window opened
    // True on EVERY path into the watcher role (the park below, or `{id,
    // watch}` from the start). The `term.onData` gate reads this flag.
    let watching = !!opts.watch;
    let announced: any = null; // the daemon's reason, when it named one before closing
    let refusal: any = null; // the daemon's words when that reason is "refused"
    let switching = false; // an intentional close on the way to a takeover
    let firstConnect = true;
    // True while the scrollback replay is being parsed. The replay is RAW BYTES
    // (one terminal frame), so every escape sequence in the backlog runs again
    // (OSC 52 is refused meanwhile). Cleared by the write callback, not after
    // `term.write` returns: xterm parses ASYNCHRONOUSLY.
    let replaying = false;
    let retryDelay = 0;
    let retryTimer: any = null;
    let failedReopens = 0;
    let connectingSince = 0;
    // This window is done. Without the latch a resume would reconnect a dead
    // id and print a second "[session closed]".
    let ended = false;
    let lastResumeAt = 0;
    // True while the fleet calls this window's peer down: no socket, no timer.
    // The session lives on the peer, so a hold never gives up.
    let held = false;
    const peerGroup = () => (typeof opts.peerGroup === "function" ? opts.peerGroup() : null);

    function hold(group: any) {
      held = true;
      if (retryTimer) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (typeof opts.onPeerHold === "function") opts.onPeerHold(group);
    }

    function release() {
      held = false;
      retryDelay = 0;
      failedReopens = 0;
      if (typeof opts.onPeerBack === "function") opts.onPeerBack();
      connect({ id: currentSessionId, repo: currentRepo, watch: watching });
    }

    function giveUp() {
      ended = true;
      // Stop observing so a dead-ws terminal doesn't keep firing fit() until the
      // window is closed.
      ro.disconnect();
      term.write("\r\n" + endNotice(announced, refusal) + "\r\n");
      if (typeof opts.onEnded === "function") opts.onEnded(announced, refusal);
    }

    function scheduleReconnect() {
      retryDelay = Math.min(
        retryDelay ? retryDelay * 2 : RECONNECT_BASE,
        RECONNECT_MAX,
      );
      const wait = retryDelay + Math.random() * 0.3 * retryDelay; // jitter
      retryTimer = setTimeout(() => {
        retryTimer = null;
        connect({ id: currentSessionId, repo: currentRepo, watch: watching });
      }, wait);
    }

    function connect(connOpts: any) {
      opened = false;
      announced = null;
      refusal = null;
      // A reattach (`id`) gets the backlog replayed; a fresh launch has none.
      // An empty scrollback sends no replay frame, so the flag rides until the
      // first LIVE frame clears it.
      replaying = connOpts.id != null;
      ws = new WebSocket(
        WBSessionRoute.url(WS_ORIGIN, {
          ...connOpts,
          holder: WBSessionRoute.tabHolder(),
        }),
      );
      ws.binaryType = "arraybuffer";
      connectingSince = Date.now();
      // `close()` on a CONNECTING socket fires `onclose` with `opened` false:
      // the ordinary failed-reopen path, backoff and give-up count included.
      const handshake = ws;
      setTimeout(() => {
        if (handshake.readyState !== 0) return;
        try {
          handshake.close();
        } catch {}
      }, CONNECT_TIMEOUT_MS);
      ws.onopen = () => {
        opened = true;
        everOpened = true;
        retryDelay = 0;
        failedReopens = 0;
        // A reconnect reattaches and the daemon replays the whole backlog; clear
        // what's on screen first so the replay repaints instead of duplicating.
        if (!firstConnect) term.reset();
        firstConnect = false;
        // HERE, not in `onPark`: the reset above would wipe a line written
        // before the socket opened.
        if (watching) {
          term.write("\r\n[read-only: another window has control]\r\n");
        }
        fit.fit();
        ws.send(encodeResize(term.rows, term.cols));
      };
      ws.onmessage = (ev: any) => {
        const a = new Uint8Array(ev.data);
        if (a[0] === TAG_TERMINAL) {
          if (currentSessionId == null) {
            currentSessionId = Number(
              new DataView(a.buffer, a.byteOffset + 1, 8).getBigUint64(0),
            );
            // A fresh launch's id is only known now; the chrome records under it.
            if (typeof opts.onSession === "function") opts.onSession(currentSessionId);
          }
          // The callback tells the OSC 52 handler the replay is behind us.
          term.write(
            a.subarray(9),
            replaying
              ? () => {
                  replaying = false;
                }
              : undefined,
          );
        } else if (a[0] === TAG_COMMAND) {
          // The daemon's deliberate-end announcement, sent as DATA before the
          // Close frame: the close metadata does not survive the trip (#334).
          let c = null;
          try {
            c = JSON.parse(new TextDecoder().decode(a.subarray(1)));
          } catch {}
          if (c && c.verb === "session-open") {
            const owner = WBSessionRoute.announcement(
              {
                sessionId: currentSessionId,
                daemonId: currentDaemonId,
                environment: currentEnvironment,
              },
              c.payload,
            );
            currentSessionId = owner.sessionId;
            currentDaemonId = owner.daemonId;
            currentEnvironment = owner.environment;
            if (typeof opts.onSession === "function")
              opts.onSession(currentSessionId, c.payload);
            // A launch that joined a session another page drives is read-only
            // from the start; the daemon says so here.
            if (c.payload?.watch === true && !watching) {
              watching = true;
              term.write("\r\n[read-only: another window has control]\r\n");
              if (typeof opts.onPark === "function") opts.onPark("joined");
            }
          } else if (c && c.verb === "session-end") {
            announced = c.payload?.reason ?? "child-exited";
            refusal = typeof c.payload?.message === "string" ? c.payload.message : null;
          }
        }
      };
      // Swallow the error event; onclose drives recovery in every case.
      ws.onerror = () => {};
      ws.onclose = (event: any) => {
        if (leaving || switching) return;
        if (!opened) failedReopens += 1;
        const decision = reconnectDecision({
          everOpened,
          announced,
          idKnown: currentSessionId != null,
          failedReopens,
        });
        switch (peerGate({ decision, group: peerGroup(), id: currentSessionId })) {
          case "hold":
            if (!opened) failedReopens -= 1;
            hold(peerGroup());
            return;
          case "give-up":
            giveUp();
            return;
          case "park-as-watcher":
            // Park ONCE, immediately. A watch socket that itself drops falls
            // back to the backoff, so a refused watch cannot busy-loop.
            if (!watching) {
              watching = true;
              if (typeof opts.onPark === "function") opts.onPark(announced);
              connect({ id: currentSessionId, repo: currentRepo, watch: true });
            } else {
              scheduleReconnect();
            }
            return;
          default:
            if (retryDelay === 0) {
              term.write("\r\n[connection lost — reconnecting…]\r\n");
              // The fleet may already know why; its answer can hold this window.
              if (typeof opts.readFleet === "function") opts.readFleet();
            }
            scheduleReconnect();
        }
      };
    }

    // Every byte this window sends to the child goes through here (keyboard,
    // key bar, paste), so the watched gate and the Ctrl latch apply to all.
    let ctrlLatched = false;
    let shiftLatched = false;
    function sendInput(raw: any) {
      const folded = applyCtrlLatch(ctrlLatched, raw);
      ctrlLatched = folded.latched;
      if (typeof opts.onCtrlLatch === "function") opts.onCtrlLatch(ctrlLatched);
      const d = folded.out;
      // The daemon-side drop in `Attachment::write` (session.rs) stays as
      // defence in depth; this gate makes the refusal VISIBLE (#335).
      if (watching) {
        if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
        return false;
      }
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(encodeTerminal(d));
        return true;
      }
      return false;
    }

    // The backlog's old queries are answered again during a replay; those
    // answers would reach the child as typed input.
    term.onData((d: any) => {
      if (replaying && isTerminalReply(d)) return;
      sendInput(d);
    });
    term.onResize(({ rows, cols }: any) => {
      if (ws && ws.readyState === WebSocket.OPEN)
        ws.send(encodeResize(rows, cols));
    });

    if (peerGate({ decision: "connect", group: peerGroup(), id: opts.id }) === "hold") hold(peerGroup());
    else connect(opts);

    return {
      term,
      fit,
      useGpu,
      dropGpu,
      get ws() {
        return ws;
      },
      get sessionId() {
        return currentSessionId;
      },
      get daemonId() {
        return currentDaemonId;
      },
      get environment() {
        return currentEnvironment;
      },
      get watching() {
        return watching;
      },
      // A key-bar tap, through `sendInput`: refused for a watcher like a
      // keystroke, and `Ctrl` then `c` folds through the same latch.
      // The Shift latch lives here, not in `sendInput`: the virtual keyboard
      // has its own Shift, so only the next BAR key consumes it.
      sendKey(name: any) {
        if (name === "ctrl") {
          ctrlLatched = !ctrlLatched;
          if (typeof opts.onCtrlLatch === "function") opts.onCtrlLatch(ctrlLatched);
          return ctrlLatched;
        }
        const step = barKey(name, !!term.modes?.applicationCursorKeysMode, shiftLatched);
        if (step.latched !== shiftLatched) {
          shiftLatched = step.latched;
          if (typeof opts.onShiftLatch === "function") opts.onShiftLatch(shiftLatched);
        }
        if (name === "shift") return shiftLatched;
        return step.seq ? sendInput(step.seq) : false;
      },
      get ctrlLatched() {
        return ctrlLatched;
      },
      get shiftLatched() {
        return shiftLatched;
      },
      // Arm (or disarm) the line-selection gesture. NOT gated on `watching`:
      // a selection is a read, and a watcher may copy what it sees.
      setSelecting,
      // The paste key's image: the same drop as a keyboard paste, same gate.
      pasteImage(blob: any) {
        return dropImage([blob.type], blob);
      },
      get selecting() {
        return selecting;
      },
      // The page came back from a suspend (or the network did). Returns whether
      // it acted. The `currentSessionId == null` bail is load-bearing: a window
      // not yet told its id would compose a LAUNCH url and spawn a second
      // vendor CLI (`reconnectDecision` R1, `takeOver`).
      resume(stale: any) {
        if (leaving || ended || currentSessionId == null) return false;
        // Held: the fleet decides, so ask it rather than dial a peer it calls
        // down. Its answer reaches `peerRefresh`.
        if (held) {
          if (typeof opts.readFleet === "function") opts.readFleet();
          return false;
        }
        const now = Date.now();
        if (now - lastResumeAt < RESUME_DEBOUNCE_MS) return false;
        // A pending backoff is brought forward. `retryDelay` is kept: it stops
        // "[connection lost]" printing twice for one drop.
        if (retryTimer) {
          lastResumeAt = now;
          clearTimeout(retryTimer);
          retryTimer = null;
          connect({ id: currentSessionId, repo: currentRepo, watch: watching });
          return true;
        }
        const readyState = ws ? ws.readyState : null;
        const connectingMs = now - connectingSince;
        if (resumeDecision({ readyState, stale, connectingMs }) === "none") return false;
        lastResumeAt = now;
        detachSocket(ws, "reconnect");
        connect({ id: currentSessionId, repo: currentRepo, watch: watching });
        return true;
      },
      // The ONLY place in this file that sets `takeover` — from the parked
      // banner's button. `switching` makes the current socket's onclose a no-op.
      takeOver() {
        if (currentSessionId == null) return;
        switching = true;
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        // Detach EVERY handler before closing: the events land AFTER this
        // returns, when `switching` is false again and `ws` names the new
        // socket. A queued `session-end` would otherwise attach a stale reason
        // to the NEW connection and turn its next drop into a give-up.
        detachSocket(ws, "reconnect");
        watching = false;
        announced = null;
        refusal = null;
        failedReopens = 0;
        retryDelay = 0;
        switching = false;
        if (typeof opts.onResume === "function") opts.onResume();
        connect({ id: currentSessionId, repo: currentRepo, takeover: true });
      },
      // A fleet read arrived. A held window goes back when its peer is
      // available, or when the fleet no longer lists it (the ordinary retry
      // decides then). A window in its backoff holds when the peer is down.
      peerRefresh() {
        if (leaving || ended) return;
        const group = peerGroup();
        const gate = peerGate({ decision: "reconnect", group, id: currentSessionId });
        if (held) {
          if (gate === "hold") opts.onPeerHold?.(group);
          else release();
          return;
        }
        if (retryTimer && gate === "hold") hold(group);
      },
      // `reason` (see `encodeDetach`) tells the daemon why the socket closes.
      dispose(reason: any) {
        leaving = true;
        if (held) {
          held = false;
          if (typeof opts.onPeerBack === "function") opts.onPeerBack();
        }
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        ro.disconnect();
        // A glide or a pending scroll would call `scrollLines` on a disposed
        // terminal.
        stopFling();
        stopScroll();
        if (ws && ws.readyState <= 1) {
          try {
            announceDetach(ws, reason);
          } catch {}
          ws.close();
        }
        dropGpu();
        term.dispose();
      },
    };
  }

  return { attachTerminal, detachSocket, announceDetach };
}
