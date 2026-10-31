/* ---------------------------------------------------------------------------
   The console's terminal: xterm.js in a window body, wired to its session
   over `/ws/session` (ADR-0075 phase 5).

   `createTerminal(deps)` returns `attachTerminal`, `detachSocket` and
   `announceDetach` for one console (ADR-0075 D7). They read the console only
   through `deps`, and `TerminalDeps` lists every read, so `tsc` refuses a
   read outside it. `wb-console.ts` creates one per console and keeps the
   font size, the plane and the clipboard. Each `attachTerminal` call keeps
   the state of its own terminal and socket; the session is the daemon's.
   The socket link (connect, reconnect, close, input while down) is
   `wb-console-terminal-link.ts`. The touch gesture is `wb-console-touch.ts`;
   the image paste, OSC 52 and the right button are `wb-console-paste.ts`; the
   GPU addon is `terminalGpu` in `wb-console-gpu.ts` (#621). `TerminalDeps` and
   `TerminalOpts` are `wb-console-terminal-types.d.ts`; the retirement of a
   socket is `wb-console-terminal-socket.ts`.
   --------------------------------------------------------------------------- */
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { wireTouch } from "./wb-console-touch.ts";
import { wirePaste } from "./wb-console-paste.ts";
import { wirePress } from "./wb-console-press.ts";
import { createFit } from "./wb-console-fit.ts";
import { placeOf } from "./wb-console-probe.ts";
import { terminalGpu } from "./wb-console-gpu.ts";
import { announceDetach, detachSocket } from "./wb-console-terminal-socket.ts";
import { createLink } from "./wb-console-terminal-link.ts";
import type { TerminalLinkState } from "./wb-console-terminal-link.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { TerminalDeps, TerminalOpts } from "./wb-console-terminal-types.d.ts";

const { barKey, applyCtrlLatch, isTerminalReply } = WBConsoleInput;

const { encodeTerminal, encodeDetach, encodeShown } = WBConsoleSession;

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

// xterm 6.0.0 builds its scrollbar with VS Code's scrollable element at
// visibility Auto: the bar shows only while the mouse is over the terminal or
// just after a scroll, and a bar never revealed stays hidden. Measured
// 2026-10-24: a console with 1051 lines showed no bar. Visible (3) still hides
// a bar with nothing to scroll (the alternate screen). xterm has no option
// for it, so this goes through its private path, set by `open`; the path is
// pinned against the vendored file in `served_vendor.rs`.
const SCROLLBAR_VISIBLE = 3;
function showScrollbarWhileNeeded(term: XtermTerminal) {
  const scrollable = term._core?._viewport?._scrollableElement;
  if (!scrollable) {
    console.warn("console scrollbar: xterm has no scrollable element at the expected path");
    return;
  }
  scrollable.updateOptions({ vertical: SCROLLBAR_VISIBLE });
}

export function createTerminal(deps: TerminalDeps) {
  const {
    window,
    WS_ORIGIN,
    fontSize,
    stage,
    workspace,
    cancelSlide,
    writeClipboard,
    readClipboard,
  } = deps;

  // Attach a real xterm.js terminal into `body`, wired to a PTY over `/ws/session`.
  // `opts` is one of: {repo, agent} (a NEW agent launch), {console:true[, repo]}
  // (a NEW free-console launch — home dir when `repo` absent), or
  // {id[, takeover][, watch]} (a REATTACH; `watch` is read-only). Returns a
  // handle so the window chrome can refit, take the baton, and close it.
  function attachTerminal(body: HTMLElement, opts: TerminalOpts) {
    const term = new Terminal({ convertEol: false, theme: TERMINAL_THEME });
    // Set rather than passed: the constructor literal is pinned in lib.rs as
    // the theme contract; the size is a per-profile preference.
    term.options.fontSize = fontSize();
    // Option+drag selects on macOS while a TUI owns the mouse; Shift+drag is
    // xterm's default elsewhere (`shouldForceSelection`).
    term.options.macOptionClickForcesSelection = true;
    // The width table of Unicode 11, as the agents and the daemon's screen
    // count it: with xterm's default table (Unicode 6) an emoji such as U+2705
    // takes one column instead of two, and every later write on its line lands
    // one column off. The `unicode` API is proposed in @xterm/xterm 6.0.0.
    term.options.allowProposedApi = true;
    term.loadAddon(new Unicode11Addon.Unicode11Addon());
    term.unicode.activeVersion = "11";
    const fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);
    term.open(body);
    showScrollbarWhileNeeded(term);
    // The GPU addon: `terminalGpu`, wb-console-gpu.ts. A change of renderer
    // refits; the page changes it only after this function returns.
    const { useGpu, dropGpu } = terminalGpu(term, () => sizing.fit.fit());
    term.loadAddon(new WebLinksAddon.WebLinksAddon());

    // The touch gesture and the line selection: wb-console-touch.ts.
    const probe = deps.scrollProbe?.watch(term, () => placeOf(body));
    // A wheel the page made for a touch (`wheelToApp`) is not trusted. In the
    // capture phase: xterm stops the wheel events it takes.
    if (probe)
      body.addEventListener("wheel", (e) => e.isTrusted && probe.moved("wheel"), {
        passive: true,
        capture: true,
      });
    const touch = wireTouch({ term, body, stage, workspace, cancelSlide, opts, probe });
    const { setSelecting, stopFling, stopScroll } = touch;
    fitAddon.fit();

    // The image paste, OSC 52 and the right button: wb-console-paste.ts.
    const { dropImage } = wirePaste({
      term,
      body,
      window,
      opts,
      writeClipboard,
      readClipboard,
      watchingNow: () => linkState.watching,
      replayingNow: () => linkState.replaying,
      currentRepoNow: () => linkState.currentRepo,
    });

    // The keyboard and mouse rules: wb-console-press.ts.
    wirePress({ term, body, window, writeClipboard });
    // Refit whenever THIS window's body changes size, and send the size: wb-console-fit.ts.
    const sizing = createFit({
      term,
      body,
      fitAddon,
      socket: () => linkState.ws,
      covered: opts.covered,
      focused: opts.focused,
      watching: () => linkState.watching,
    });
    const { fit, dispose: stopFit } = sizing;

    // The state of the socket link, and the functions that change it:
    // wb-console-terminal-link.ts. The terminal reads the state below.
    const linkState: TerminalLinkState = {
      ws: null,
      opened: false,
      everOpened: false,
      // A launch that is a watch starts read-only.
      watching: !!opts.watch,
      announced: null,
      refusal: null,
      switching: false,
      firstConnect: true,
      shown: null,
      replayNext: false,
      writingOutput: false,
      replaying: false,
      retryDelay: 0,
      retryTimer: null,
      failedReopens: 0,
      connectingSince: 0,
      ended: false,
      lastResumeAt: 0,
      held: false,
      linkLost: false,
      leaving: false,
      currentSessionId: opts.id ?? null,
      currentRepo: opts.repo ?? null,
      currentDaemonId: null,
      currentEnvironment: null,
    };
    const link = createLink(linkState, { term, opts, WS_ORIGIN, sizing, stopFit, probe });

    // Output is written through this wrapper. A write that is not session
    // output (a notice of this page) forgets the byte count.
    const write = term.write.bind(term);
    term.write = (data, done) => {
      if (!linkState.writingOutput) linkState.shown = null;
      write(data, done);
    };

    let ctrlLatched = false;
    let shiftLatched = false;

    // Every byte this window sends to the child goes through here (keyboard,
    // key bar, paste), so the watched gate and the Ctrl latch apply to all.
    function sendInput(raw: string) {
      const folded = applyCtrlLatch(ctrlLatched, raw);
      ctrlLatched = folded.latched;
      if (typeof opts.onCtrlLatch === "function") opts.onCtrlLatch(ctrlLatched);
      const d = folded.out;
      // The daemon-side drop in `Attachment::write` (session.rs) stays as
      // defence in depth; this gate makes the refusal VISIBLE (#335).
      if (linkState.watching) {
        if (typeof opts.onWatchedInput === "function") opts.onWatchedInput();
        return false;
      }
      const ws = linkState.ws;
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(encodeTerminal(d));
        return true;
      }
      link.inputWhileDown();
      return false;
    }

    // The backlog's old queries are answered again during a replay; those
    // answers would reach the child as typed input.
    term.onData((d) => {
      if (linkState.replaying && isTerminalReply(d)) return;
      sendInput(d);
    });

    link.start();

    return {
      term,
      fit,
      // The cover over this window went (`refreshCover`).
      uncovered: sizing.uncovered,
      useGpu,
      dropGpu,
      get ws() {
        return linkState.ws;
      },
      get sessionId() {
        return linkState.currentSessionId;
      },
      get daemonId() {
        return linkState.currentDaemonId;
      },
      get environment() {
        return linkState.currentEnvironment;
      },
      get watching() {
        return linkState.watching;
      },
      // A key-bar tap, through `sendInput`: refused for a watcher like a
      // keystroke, and `Ctrl` then `c` folds through the same latch.
      // The Shift latch lives here, not in `sendInput`: the virtual keyboard
      // has its own Shift, so only the next BAR key consumes it.
      sendKey(name: string) {
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
      pasteImage(blob: Blob) {
        return dropImage([blob.type], blob);
      },
      get selecting() {
        return touch.selecting;
      },
      // The page came back from a suspend, or the network did: see `resume`
      // in wb-console-terminal-link.ts. Returns whether it acted.
      resume: link.resume,
      // The baton of the parked banner's button: see `takeOver` in
      // wb-console-terminal-link.ts.
      takeOver: link.takeOver,
      // A fleet read arrived: see `peerRefresh` in wb-console-terminal-link.ts.
      peerRefresh: link.peerRefresh,
      // The page went hidden, or is visible again. The socket stays open;
      // only the daemon's log reads this.
      notePage(hidden: boolean) {
        const ws = linkState.ws;
        if (!ws || ws.readyState !== 1) return;
        try {
          ws.send(hidden ? encodeDetach("hidden") : encodeShown());
        } catch {} // a socket that fails here is closing, and its close tells
      },
      // `reason` (see `encodeDetach`) tells the daemon why the socket closes.
      dispose(reason: DetachReason) {
        link.leave();
        stopFit();
        // A glide or a pending scroll would call `scrollLines` on a disposed
        // terminal.
        stopFling();
        stopScroll();
        link.closeSocket(reason);
        dropGpu();
        term.dispose();
      },
    };
  }

  return { attachTerminal, detachSocket, announceDetach };
}
