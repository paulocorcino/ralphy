/* ---------------------------------------------------------------------------
   The console's terminal: xterm.js in a window body, wired to its session
   over `/ws/session` (ADR-0075 phase 5).

   `createTerminal(deps)` returns `attachTerminal`, `detachSocket` and
   `announceDetach` for one console (ADR-0075 D7). They read the console only
   through `deps`, and `TerminalDeps` lists every read, so `tsc` refuses a
   read outside it. `wb-console.ts` creates one per console and keeps the
   font size, the plane and the clipboard. Each `attachTerminal` call keeps
   the state of its own terminal and socket; the session is the daemon's.
   The touch gesture is `wb-console-touch.ts`; the image paste, OSC 52 and
   the right button are `wb-console-paste.ts`; the GPU addon is
   `terminalGpu` in `wb-console-gpu.ts` (#621).
   --------------------------------------------------------------------------- */
import { WBSessionRoute } from "./wb-session-route.ts";
import * as WBConsoleInput from "./wb-console-input.ts";
import * as WBConsoleSession from "./wb-console-session.ts";
import { wireTouch } from "./wb-console-touch.ts";
import { wirePaste } from "./wb-console-paste.ts";
import { wirePress } from "./wb-console-press.ts";
import { createFit } from "./wb-console-fit.ts";
import { frameQueue } from "./wb-console-frames.ts";
import { reportHandshake, terminalGpu } from "./wb-console-gpu.ts";
import { resumeDecision, CONNECT_TIMEOUT_MS, RESUME_DEBOUNCE_MS } from "./wb-resume.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { SessionRequest } from "./wb-session-route.ts";
import type { FramePayload } from "./wb-console-frames.ts";
import type { Group } from "./wb-fleet.ts";

const { barKey, applyCtrlLatch, isTerminalReply } = WBConsoleInput;

const {
  encodeTerminal,
  encodeDetach,
  endNotice,
  reconnectDecision,
  peerGate,
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

// What the terminal reads from the console, and nothing else.
export type TerminalDeps = {
  // The console's page: `WBDaemon` (the image paste) and `getShell` (whether
  // the shell's key listener exists).
  window: Pick<Window, "WBDaemon" | "getShell">;
  // The session socket's origin, scheme-matched to the page.
  WS_ORIGIN: string;
  // The stored font size, read when a terminal is made.
  fontSize: () => number;
  // The plane and the viewport; null before the page has them.
  stage: () => HTMLElement | null;
  workspace: () => HTMLElement | null;
  // Stops a jump of the viewport in flight: a two-finger pan outranks it.
  cancelSlide: () => void;
  // The system clipboard. They are made in `wb-console-keys.ts`: the key bar uses them too.
  writeClipboard: (text: string, term: XtermTerminal) => void;
  readClipboard: () => Promise<ClipboardRead>;
};

// What a clipboard read gives: an image, or else text (`clipboardContent`).
type ClipboardRead = { image?: Blob; text?: string };

// What `attachTerminal` is built with, and what the chrome hands back to
// rebuild a terminal after sleep (`TermWiring`).
export type TerminalOpts = SessionRequest & {
  // Arms or disarms the key bar's line-selection button.
  onSelecting?: (on: boolean) => void;
  // A key or a paste was refused because this window only watches.
  onWatchedInput?: () => void;
  onCtrlLatch?: (on: boolean) => void;
  onShiftLatch?: (on: boolean) => void;
  // The session's id, and the daemon's announcement when it came with one.
  onSession?: (id: number | null, announced?: FramePayload) => void;
  // This window parked as a watcher: `announced` is the daemon's reason.
  onPark?: (announced: string | null) => void;
  onEnded?: (announced: string | null, refusal: string | null) => void;
  // The baton came back to this window.
  onResume?: () => void;
  // Whether a full-bleed console covers this window, and whether it has the
  // focus: the size rule of wb-console-fit.ts.
  covered?: () => boolean;
  focused?: () => boolean;
  // The peer's fleet group, or null for a local project.
  peerGroup?: () => Group | null;
  onPeerHold?: (group: Group | null) => void;
  onPeerBack?: () => void;
  readFleet?: () => void;
};

export function createTerminal(deps: TerminalDeps) {
  const { window, WS_ORIGIN, fontSize, stage, workspace, cancelSlide, writeClipboard, readClipboard } =
    deps;

  // Announce `reason` on an open socket. A socket still connecting cannot send.
  function announceDetach(ws: WebSocket | null, reason: DetachReason | undefined) {
    if (reason && ws && ws.readyState === 1) ws.send(encodeDetach(reason));
  }

  // Retire a socket so its pending events cannot reach us. `onmessage` matters
  // as much as `onclose`: a frame still queued lands AFTER this returns, when
  // `ws` names the replacement. Local, not `WBDaemon`'s: this module loads on
  // its own in the node harness and the popup.
  function detachSocket(ws: WebSocket | null, reason: DetachReason | undefined) {
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
  function attachTerminal(body: HTMLElement, opts: TerminalOpts) {
    const term = new Terminal({ convertEol: false, theme: TERMINAL_THEME });
    // Set rather than passed: the constructor literal is pinned in lib.rs as
    // the theme contract; the size is a per-profile preference.
    term.options.fontSize = fontSize();
    // Option+drag selects on macOS while a TUI owns the mouse; Shift+drag is
    // xterm's default elsewhere (`shouldForceSelection`).
    term.options.macOptionClickForcesSelection = true;
    const fitAddon = new FitAddon.FitAddon();
    term.loadAddon(fitAddon);
    term.open(body);
    // The GPU addon: `terminalGpu`, wb-console-gpu.ts.
    const { useGpu, dropGpu } = terminalGpu(term);
    term.loadAddon(new WebLinksAddon.WebLinksAddon());

    // The touch gesture and the line selection: wb-console-touch.ts.
    const touch = wireTouch({ term, body, stage, workspace, cancelSlide, opts });
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
      watchingNow: () => watching,
      replayingNow: () => replaying,
      currentRepoNow: () => currentRepo,
    });

    // The keyboard and mouse rules: wb-console-press.ts.
    wirePress({ term, body, window, writeClipboard });
    // Refit whenever THIS window's body changes size, and send the size: wb-console-fit.ts.
    const sizing = createFit({
      term,
      body,
      fitAddon,
      socket: () => ws,
      covered: opts.covered,
      focused: opts.focused,
      watching: () => watching,
    });
    const { fit, dispose: stopFit } = sizing;

    let currentSessionId = opts.id ?? null;
    let currentRepo = opts.repo ?? null;
    let currentDaemonId: string | null = null;
    let currentEnvironment: string | null = null;
    let leaving = false;

    // A dropped socket does NOT end the session: the daemon keeps the child
    // alive (session_ws's teardown invariant), so a close is recovered by
    // reattaching to the SAME id. The daemon replays the screen on reattach,
    // so the terminal is reset on a reconnecting open, unless the daemon
    // sends only the bytes after `shown` (a resume). Backoff is exponential
    // with jitter, capped.
    //
    // NO RECONNECT EVER CARRIES `takeover` (#334): two open workbenches
    // reclaiming the writer slot on a timer evicted each other ~1.1s per flip,
    // indefinitely. The baton moves only on `takeOver()`. `reconnectDecision`
    // owns the choice; this carries it out.
    const RECONNECT_BASE = 1000;
    const RECONNECT_MAX = 15000;
    let ws: WebSocket | null = null;
    let opened = false; // has the CURRENT socket opened
    let everOpened = false; // has ANY socket of this window opened
    // True on EVERY path into the watcher role (the park below, or `{id,
    // watch}` from the start). The `term.onData` gate reads this flag.
    let watching = !!opts.watch;
    let announced: string | null = null; // the daemon's reason, when it named one before closing
    let refusal: string | null = null; // the daemon's words when that reason is "refused"
    let switching = false; // an intentional close on the way to a takeover
    let firstConnect = true;
    // The count of the session's output bytes this terminal shows (ADR-0079
    // D8), sent as `since` so that a reconnect gets only the bytes it misses.
    // `null` when it is not known: a peer of an older build sends no count,
    // and a terminal that shows a notice of this page no longer matches the
    // session's bytes.
    let shown: number | null = null;
    // The next terminal frame is the replay, which `shown` already counts.
    let replayNext = false;
    // True while the session's output is written. Every other write is a
    // notice of this page (here, or in the paste and close modules).
    let writingOutput = false;
    const write = term.write.bind(term);
    term.write = (data, done) => {
      if (!writingOutput) shown = null;
      write(data, done);
    };
    // True while the scrollback replay is being parsed. The replay is RAW BYTES
    // (one terminal frame), so every escape sequence in the backlog runs again
    // (OSC 52 is refused meanwhile). Cleared by the write callback, not after
    // `term.write` returns: xterm parses ASYNCHRONOUSLY.
    let replaying = false;
    let retryDelay = 0;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
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

    function hold(group: Group | null) {
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
      stopFit();
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

    function connect(connOpts: SessionRequest) {
      opened = false;
      announced = null;
      refusal = null;
      // A reattach (`id`) gets the backlog replayed; a fresh launch has none.
      // An empty scrollback sends no replay frame, so the flag rides until the
      // first LIVE frame clears it.
      replaying = connOpts.id != null;
      replayNext = false;
      // A terminal that no `session-open` of this window has written to yet.
      let fresh = firstConnect;
      const holder = WBSessionRoute.tabHolder();
      const request = WBSessionRoute.requestOpts(
        connOpts,
        holder,
        WBSessionRoute.canGunzip(),
        WBSessionRoute.tabAttempt(),
        shown ?? undefined,
      );
      ws = new WebSocket(WBSessionRoute.url(WS_ORIGIN, request));
      const socket = ws;
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
        firstConnect = false;
        reportHandshake(Date.now() - connectingSince);
        // Never xterm's default 80×24: only a size the body measured.
        sizing.opened();
      };
      const frames = frameQueue((event) => {
        // A replay decoded after its socket was retired belongs to no one.
        if (leaving || ws !== socket) return;
        switch (event.kind) {
          case "output":
            if (currentSessionId == null) {
              currentSessionId = event.sessionId;
              // A fresh launch's id is only known now; the chrome records under it.
              if (typeof opts.onSession === "function") opts.onSession(currentSessionId);
            }
            if (replayNext) replayNext = false;
            else if (shown != null) shown += event.bytes.length;
            // The callback tells the OSC 52 handler the replay is behind us.
            writingOutput = true;
            try {
              term.write(
                event.bytes,
                replaying
                  ? () => {
                      replaying = false;
                    }
                  : undefined,
              );
            } finally {
              writingOutput = false;
            }
            return;
          case "open": {
            const resumed = event.payload?.resume === true;
            // A resume writes only the missing bytes after what the terminal
            // shows. Any other replay repaints the screen, so the terminal is
            // cleared first.
            if (!fresh && !resumed) term.reset();
            fresh = false;
            // HERE, not in `onPark`: the reset above would wipe a line written
            // before the socket opened.
            if (watching && !resumed) term.write("\r\n[read-only: another window has control]\r\n");
            const owner = WBSessionRoute.announcement(
              {
                sessionId: currentSessionId,
                daemonId: currentDaemonId,
                environment: currentEnvironment,
              },
              event.payload,
            );
            currentSessionId = owner.sessionId;
            currentDaemonId = owner.daemonId;
            currentEnvironment = owner.environment;
            if (typeof opts.onSession === "function")
              opts.onSession(currentSessionId, event.payload);
            // A launch that joined a session another page drives is read-only
            // from the start; the daemon says so here.
            if (event.payload?.watch === true && !watching) {
              watching = true;
              term.write("\r\n[read-only: another window has control]\r\n");
              if (typeof opts.onPark === "function") opts.onPark("joined");
            }
            // Last: the notices above are not session output.
            const offset = event.payload?.offset;
            shown = typeof offset === "number" && Number.isSafeInteger(offset) && offset >= 0 ? offset : null;
            replayNext = event.payload?.replay === true;
            return;
          }
          // A watcher paints at the session size; the writer ignores it.
          case "size":
            sizing.sessionSize(event.rows, event.cols);
            return;
          case "replay-lost":
            shown = null;
            replayNext = false;
            return;
          case "end":
            announced = event.reason;
            refusal = event.message;
            return;
          case "other":
            return;
          default: {
            const unreachable: never = event;
            return unreachable;
          }
        }
      });
      ws.onmessage = (ev: MessageEvent) => frames.take(new Uint8Array(ev.data));
      // Swallow the error event; onclose drives recovery in every case.
      ws.onerror = () => {};
      const closed = () => {
        if (leaving || switching || ws !== socket) return;
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
              // The watcher repaints from the snapshot, under its banner.
              shown = null;
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
      // After the frames that wait behind a replay decode: a `session-end`
      // among them names the reason this close is read with.
      ws.onclose = () => frames.whenIdle(closed);
    }

    // Every byte this window sends to the child goes through here (keyboard,
    // key bar, paste), so the watched gate and the Ctrl latch apply to all.
    let ctrlLatched = false;
    let shiftLatched = false;
    function sendInput(raw: string) {
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
    term.onData((d) => {
      if (replaying && isTerminalReply(d)) return;
      sendInput(d);
    });

    if (peerGate({ decision: "connect", group: peerGroup(), id: opts.id }) === "hold") hold(peerGroup());
    else connect(opts);

    return {
      term,
      fit,
      // The cover over this window went (`refreshCover`).
      uncovered: sizing.uncovered,
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
      // The page came back from a suspend (or the network did). Returns whether
      // it acted. The `currentSessionId == null` bail is load-bearing: a window
      // not yet told its id would compose a LAUNCH url and spawn a second
      // vendor CLI (`reconnectDecision` R1, `takeOver`).
      resume(stale: boolean) {
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
      dispose(reason: DetachReason) {
        leaving = true;
        if (held) {
          held = false;
          if (typeof opts.onPeerBack === "function") opts.onPeerBack();
        }
        if (retryTimer) {
          clearTimeout(retryTimer);
          retryTimer = null;
        }
        stopFit();
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
