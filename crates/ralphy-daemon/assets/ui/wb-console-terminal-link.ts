/* ---------------------------------------------------------------------------
   The socket link of one console's terminal (`wb-console-terminal.ts`): the
   connect, the close path, the backoff and the resume, and the input while
   the socket is down. The session is the daemon's, and a dropped socket
   reattaches to the same id (ADR-0079 D8).

   `createLink(state, env)` works on one `TerminalLinkState`. The terminal
   makes that state and reads it for its input and its output; only this
   module's functions change the link fields.
   --------------------------------------------------------------------------- */
import * as WBConsoleSession from "./wb-console-session.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { frameQueue } from "./wb-console-frames.ts";
import { reportStall } from "./wb-stall.ts";
import { resumeDecision, CONNECT_TIMEOUT_MS, RESUME_DEBOUNCE_MS } from "./wb-resume.ts";
import { announceDetach, detachSocket } from "./wb-console-terminal-socket.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { SessionRequest } from "./wb-session-route.ts";
import type { FramePayload, SocketEvent } from "./wb-console-frames.ts";
import type { Group } from "./wb-fleet.ts";
import type { ScrollWatch } from "./wb-console-probe.ts";
import type { createFit } from "./wb-console-fit.ts";
import type { LinkState, TerminalOpts } from "./wb-console-terminal-types.d.ts";

const { endNotice, reconnectDecision, peerGate } = WBConsoleSession;

const RECONNECT_BASE = 1000;
const RECONNECT_MAX = 15000;

// The state of one terminal's socket link. The terminal makes it, and the
// functions of `createLink` change it.
export type TerminalLinkState = {
  // The current socket.
  ws: WebSocket | null;
  opened: boolean; // has the CURRENT socket opened
  everOpened: boolean; // has ANY socket of this window opened
  // True on EVERY path into the watcher role (the park below, or `{id,
  // watch}` from the start). The `term.onData` gate reads this flag.
  watching: boolean;
  announced: string | null; // the daemon's reason, when it named one before closing
  refusal: string | null; // the daemon's words when that reason is "refused"
  switching: boolean; // an intentional close on the way to a takeover
  firstConnect: boolean;
  // The count of the session's output bytes this terminal shows (ADR-0079
  // D8), sent as `since` so that a reconnect gets only the bytes it misses.
  // `null` when it is not known: a peer of an older build sends no count,
  // and a terminal that shows a notice of this page no longer matches the
  // session's bytes.
  shown: number | null;
  // The next terminal frame is the replay, which `shown` already counts.
  replayNext: boolean;
  // True while the session's output is written. Every other write is a
  // notice of this page (here, or in the paste and close modules).
  writingOutput: boolean;
  // True while the scrollback replay is being parsed. The replay is RAW BYTES
  // (one terminal frame), so every escape sequence in the backlog runs again
  // (OSC 52 is refused meanwhile). Cleared by the write callback, not after
  // `term.write` returns: xterm parses ASYNCHRONOUSLY.
  replaying: boolean;
  retryDelay: number;
  retryTimer: ReturnType<typeof setTimeout> | null;
  failedReopens: number;
  connectingSince: number;
  // This window is done. Without the latch a resume would reconnect a dead
  // id and print a second "[session closed]".
  ended: boolean;
  lastResumeAt: number;
  // True while the fleet calls this window's peer down: no socket, no timer.
  // The session lives on the peer, so a hold never gives up.
  held: boolean;
  linkLost: boolean;
  leaving: boolean;
  currentSessionId: number | null;
  currentRepo: string | null;
  currentDaemonId: string | null;
  currentEnvironment: string | null;
};

// What the link reads from the terminal around it: the xterm surface, the
// fit and its stop, the scroll probe's hooks, and the options of this window.
export type TerminalLinkEnv = {
  term: XtermTerminal;
  opts: TerminalOpts;
  WS_ORIGIN: string;
  sizing: ReturnType<typeof createFit>;
  stopFit: () => void;
  probe: ScrollWatch | undefined;
};

// The count of the session's output bytes a `session-open` says the terminal
// shows, or null when the daemon sent none that is a whole count.
function offsetOf(payload: FramePayload | undefined): number | null {
  const offset = payload?.offset;
  return typeof offset === "number" && Number.isSafeInteger(offset) && offset >= 0 ? offset : null;
}

export function createLink(s: TerminalLinkState, env: TerminalLinkEnv) {
  const { term, opts, WS_ORIGIN, sizing, stopFit, probe } = env;

  const peerGroup = () => (typeof opts.peerGroup === "function" ? opts.peerGroup() : null);
  function link(state: LinkState) {
    const lost = state === "lost";
    if (lost === s.linkLost) return;
    s.linkLost = lost;
    if (typeof opts.onLink === "function") opts.onLink(state);
  }

  function hold(group: Group | null) {
    s.held = true;
    // The peer's own strip says why.
    link("back");
    if (s.retryTimer) {
      clearTimeout(s.retryTimer);
      s.retryTimer = null;
    }
    if (typeof opts.onPeerHold === "function") opts.onPeerHold(group);
  }

  function release() {
    s.held = false;
    s.retryDelay = 0;
    s.failedReopens = 0;
    if (typeof opts.onPeerBack === "function") opts.onPeerBack();
    connect({ id: s.currentSessionId, repo: s.currentRepo, watch: s.watching });
  }

  function giveUp() {
    s.ended = true;
    link("back");
    // Stop observing so a dead-ws terminal doesn't keep firing fit() until the
    // window is closed.
    stopFit();
    term.write("\r\n" + endNotice(s.announced, s.refusal) + "\r\n");
    void reportStall("console-gave-up", s.announced ?? "closed", 0);
    if (typeof opts.onEnded === "function") opts.onEnded(s.announced, s.refusal);
  }

  function scheduleReconnect() {
    s.retryDelay = Math.min(s.retryDelay ? s.retryDelay * 2 : RECONNECT_BASE, RECONNECT_MAX);
    const wait = s.retryDelay + Math.random() * 0.3 * s.retryDelay; // jitter
    s.retryTimer = setTimeout(() => {
      s.retryTimer = null;
      connect({ id: s.currentSessionId, repo: s.currentRepo, watch: s.watching });
    }, wait);
  }

  function connect(connOpts: SessionRequest) {
    s.opened = false;
    s.announced = null;
    s.refusal = null;
    // A reattach (`id`) gets the backlog replayed; a fresh launch has none.
    // An empty scrollback sends no replay frame, so the flag rides until the
    // first LIVE frame clears it.
    s.replaying = connOpts.id != null;
    s.replayNext = false;
    // A terminal that no `session-open` of this window has written to yet.
    let fresh = s.firstConnect;
    const holder = WBSessionRoute.tabHolder();
    const request = WBSessionRoute.requestOpts(
      connOpts,
      holder,
      WBSessionRoute.canGunzip(),
      WBSessionRoute.tabAttempt(),
      s.shown ?? undefined,
    );
    const socket = new WebSocket(
      WBSessionRoute.url(WS_ORIGIN, { ...request, link: WBSessionRoute.tabLink() }),
    );
    s.ws = socket;
    socket.binaryType = "arraybuffer";
    s.connectingSince = Date.now();
    // `close()` on a CONNECTING socket fires `onclose` with `opened` false:
    // the ordinary failed-reopen path, backoff and give-up count included.
    const handshake = socket;
    setTimeout(() => {
      if (handshake.readyState !== 0) return;
      try {
        handshake.close();
      } catch {}
    }, CONNECT_TIMEOUT_MS);
    socket.onopen = () => {
      s.opened = true;
      s.everOpened = true;
      s.retryDelay = 0;
      s.failedReopens = 0;
      s.firstConnect = false;
      link("back");
      // Never xterm's default 80×24: only a size the body measured.
      sizing.opened();
    };
    // The daemon's `session-open`: who owns the session, what the terminal
    // shows of it, and whether this window only watches.
    function takeOpen(event: Extract<SocketEvent, { kind: "open" }>) {
      const resumed = event.payload?.resume === true;
      // A resume writes only the missing bytes after what the terminal
      // shows. Any other replay repaints the screen, so the terminal is
      // cleared first.
      if (!fresh && !resumed) term.reset();
      fresh = false;
      // HERE, not in `onPark`: the reset above would wipe a line written
      // before the socket opened.
      if (s.watching && !resumed) term.write("\r\n[read-only: another window has control]\r\n");
      const owner = WBSessionRoute.announcement(
        {
          sessionId: s.currentSessionId,
          daemonId: s.currentDaemonId,
          environment: s.currentEnvironment,
        },
        event.payload,
      );
      s.currentSessionId = owner.sessionId;
      s.currentDaemonId = owner.daemonId;
      s.currentEnvironment = owner.environment;
      if (typeof opts.onSession === "function") opts.onSession(s.currentSessionId, event.payload);
      joinWatch(event.payload);
      // Last: the notices above are not session output.
      s.shown = offsetOf(event.payload);
      s.replayNext = event.payload?.replay === true;
    }
    // A launch that joined a session another page drives is read-only
    // from the start; the daemon says so here.
    function joinWatch(payload: FramePayload | undefined) {
      if (payload?.watch !== true || s.watching) return;
      s.watching = true;
      term.write("\r\n[read-only: another window has control]\r\n");
      if (typeof opts.onPark === "function") opts.onPark("joined");
    }
    const frames = frameQueue((event) => {
      // A replay decoded after its socket was retired belongs to no one.
      if (s.leaving || s.ws !== socket) return;
      switch (event.kind) {
        case "output":
          if (s.currentSessionId == null) {
            s.currentSessionId = event.sessionId;
            // A fresh launch's id is only known now; the chrome records under it.
            if (typeof opts.onSession === "function") opts.onSession(s.currentSessionId);
          }
          if (s.replayNext) s.replayNext = false;
          else if (s.shown != null) s.shown += event.bytes.length;
          // The callback tells the OSC 52 handler the replay is behind us.
          s.writingOutput = true;
          probe?.wrote(event.bytes.length);
          try {
            term.write(
              event.bytes,
              s.replaying
                ? () => {
                    s.replaying = false;
                  }
                : undefined,
            );
          } finally {
            s.writingOutput = false;
          }
          return;
        case "open":
          takeOpen(event);
          return;
        // A watcher paints at the session size; the writer ignores it.
        case "size":
          sizing.sessionSize(event.rows, event.cols);
          return;
        case "replay-lost":
          s.shown = null;
          s.replayNext = false;
          return;
        case "end":
          s.announced = event.reason;
          s.refusal = event.message;
          return;
        case "other":
          return;
        default: {
          const unreachable: never = event;
          return unreachable;
        }
      }
    });
    socket.onmessage = (ev: MessageEvent<ArrayBuffer>) => frames.take(new Uint8Array(ev.data));
    // Swallow the error event; onclose drives recovery in every case.
    socket.onerror = () => {};
    const closed = () => {
      if (s.leaving || s.switching || s.ws !== socket) return;
      if (!s.opened) s.failedReopens += 1;
      const decision = reconnectDecision({
        everOpened: s.everOpened,
        announced: s.announced,
        idKnown: s.currentSessionId != null,
        failedReopens: s.failedReopens,
      });
      switch (peerGate({ decision, group: peerGroup(), id: s.currentSessionId })) {
        case "hold":
          if (!s.opened) s.failedReopens -= 1;
          hold(peerGroup());
          return;
        case "give-up":
          giveUp();
          return;
        case "park-as-watcher":
          parkAsWatcher();
          return;
        case "connect":
        case "reconnect":
          if (s.retryDelay === 0) {
            link("lost");
            // The fleet may already know why; its answer can hold this window.
            if (typeof opts.readFleet === "function") opts.readFleet();
          }
          scheduleReconnect();
      }
    };
    // Park ONCE, immediately. A watch socket that itself drops falls back
    // to the backoff, so a refused watch cannot busy-loop.
    function parkAsWatcher() {
      if (s.watching) {
        scheduleReconnect();
        return;
      }
      s.watching = true;
      // The watcher repaints from the snapshot, under its banner.
      s.shown = null;
      if (typeof opts.onPark === "function") opts.onPark(s.announced);
      connect({ id: s.currentSessionId, repo: s.currentRepo, watch: true });
    }
    // After the frames that wait behind a replay decode: a `session-end`
    // among them names the reason this close is read with.
    socket.onclose = () => frames.whenIdle(closed);
  }

  // A key typed while the socket is down is not sent. The link strip says
  // so, and the reconnect comes now instead of after the backoff (#751).
  // A CONNECTING socket is already the reconnect.
  function inputWhileDown() {
    if (s.leaving || s.ended || s.held || s.currentSessionId == null) return;
    link("lost");
    if (s.ws && s.ws.readyState === 0) return;
    const now = Date.now();
    if (now - s.lastResumeAt < RESUME_DEBOUNCE_MS) return;
    s.lastResumeAt = now;
    reconnectNow();
  }

  // Reattach now: a pending backoff is dropped, else the old socket is.
  function reconnectNow() {
    if (s.retryTimer) {
      clearTimeout(s.retryTimer);
      s.retryTimer = null;
    } else {
      detachSocket(s.ws, "reconnect");
    }
    connect({ id: s.currentSessionId, repo: s.currentRepo, watch: s.watching });
  }

  // The page came back from a suspend (or the network did). Returns whether
  // it acted. The `currentSessionId == null` bail is load-bearing: a window
  // not yet told its id would compose a LAUNCH url and spawn a second
  // vendor CLI (`reconnectDecision` R1, `takeOver`).
  function resume(stale: boolean) {
    if (s.leaving || s.ended || s.currentSessionId == null) return false;
    // Held: the fleet decides, so ask it rather than dial a peer it calls
    // down. Its answer reaches `peerRefresh`.
    if (s.held) {
      if (typeof opts.readFleet === "function") opts.readFleet();
      return false;
    }
    const now = Date.now();
    if (now - s.lastResumeAt < RESUME_DEBOUNCE_MS) return false;
    // A pending backoff is brought forward. `retryDelay` is kept: it stops
    // a second fleet read for one drop.
    if (!s.retryTimer) {
      const readyState = s.ws ? s.ws.readyState : null;
      const connectingMs = now - s.connectingSince;
      if (resumeDecision({ readyState, stale, connectingMs }) === "none") return false;
    }
    s.lastResumeAt = now;
    reconnectNow();
    return true;
  }

  // The ONLY place in this module that sets `takeover` — from the parked
  // banner's button. `switching` makes the current socket's onclose a no-op.
  function takeOver() {
    if (s.currentSessionId == null) return;
    s.switching = true;
    if (s.retryTimer) {
      clearTimeout(s.retryTimer);
      s.retryTimer = null;
    }
    // Detach EVERY handler before closing: the events land AFTER this
    // returns, when `switching` is false again and `ws` names the new
    // socket. A queued `session-end` would otherwise attach a stale reason
    // to the NEW connection and turn its next drop into a give-up.
    detachSocket(s.ws, "reconnect");
    s.watching = false;
    s.announced = null;
    s.refusal = null;
    s.failedReopens = 0;
    s.retryDelay = 0;
    s.switching = false;
    if (typeof opts.onResume === "function") opts.onResume();
    connect({ id: s.currentSessionId, repo: s.currentRepo, takeover: true });
  }

  // A fleet read arrived. A held window goes back when its peer is
  // available, or when the fleet no longer lists it (the ordinary retry
  // decides then). A window in its backoff holds when the peer is down.
  function peerRefresh() {
    if (s.leaving || s.ended) return;
    const group = peerGroup();
    const gate = peerGate({ decision: "reconnect", group, id: s.currentSessionId });
    if (s.held) {
      if (gate === "hold") opts.onPeerHold?.(group);
      else release();
      return;
    }
    if (s.retryTimer && gate === "hold") hold(group);
  }

  // The first half of a window's close: no reconnect from here on, the link
  // strip and the peer's hold are released, and a pending backoff is dropped.
  function leave() {
    s.leaving = true;
    link("back");
    if (s.held) {
      s.held = false;
      if (typeof opts.onPeerBack === "function") opts.onPeerBack();
    }
    if (s.retryTimer) {
      clearTimeout(s.retryTimer);
      s.retryTimer = null;
    }
  }

  // The second half: `reason` (see `encodeDetach`) tells the daemon why the
  // socket closes.
  function closeSocket(reason: DetachReason) {
    const ws = s.ws;
    if (ws && ws.readyState <= 1) {
      try {
        announceDetach(ws, reason);
      } catch {}
      ws.close();
    }
  }

  // The window's first connect, or a hold when its peer is down.
  function start() {
    if (peerGate({ decision: "connect", group: peerGroup(), id: opts.id }) === "hold")
      hold(peerGroup());
    else connect(opts);
  }

  return { start, inputWhileDown, resume, takeOver, peerRefresh, leave, closeSocket };
}
