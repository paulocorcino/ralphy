/* ---------------------------------------------------------------------------
   ralphy workbench shell — the page socket

   The page's one socket on `/ws` (ADR-0082) and the channel layer on it.
   `wb-daemon.ts` builds it with `createPageSocket` and gives its users the
   `join`.
--------------------------------------------------------------------------- */
import { CONNECT_TIMEOUT_MS, resumeDecision } from "./wb-resume.ts";
import { pageChannels } from "./wb-channels.ts";
import type { ChannelSocket, PageChannels } from "./wb-channels.ts";
import type { SocketUser } from "./wb-daemon-subscriptions.ts";

/** What the page socket needs from the door that builds it. */
export type PageSocketDeps = {
  /** The WebSocket origin, scheme-matched to the page. */
  wsOrigin: string;
  /** The wait before a lost page socket is opened again. */
  reconnectMs: number;
  /** The wait between two resumes of the page socket. */
  resumeDebounceMs: number;
  /** Runs on each open of the socket, after its users heard it. */
  onOpen: () => void;
};

// The channel layer's view of a WebSocket. `WebSocket.send` takes only bytes
// on an `ArrayBuffer`; every frame the layer makes is on one, and any other
// frame is copied onto one.
function channelSocket(sock: WebSocket): ChannelSocket {
  return {
    get readyState() {
      return sock.readyState;
    },
    send: (data: Uint8Array) => sock.send(onArrayBuffer(data) ? data : data.slice()),
  };
}

function onArrayBuffer(bytes: Uint8Array): bytes is Uint8Array<ArrayBuffer> {
  return bytes.buffer instanceof ArrayBuffer;
}

// Fail a handshake that has not opened by the deadline. `close()` on a
// CONNECTING socket fires `close`, so the caller's ordinary retry takes over.
function armHandshakeDeadline(ws: WebSocket) {
  setTimeout(() => {
    if (ws.readyState !== 0) return;
    try {
      ws.close();
    } catch {}
  }, CONNECT_TIMEOUT_MS);
}

// Retire a socket so its pending events cannot reach us. `onmessage` matters as
// much as `onclose`: a frame still queued on the outgoing socket would land
// after the replacement is wired and be read as the NEW connection's news.
export function detachSocket(ws: WebSocket | null) {
  if (!ws) return;
  ws.onclose = null;
  ws.onmessage = null;
  ws.onopen = null;
  ws.onerror = null;
  try {
    if (ws.readyState <= 1) ws.close();
  } catch {}
}

// The page's ONE socket on `/ws` (ADR-0082): presence and the daemon's
// pushes are channel 0, and each subscription has a channel of its own
// (wb-channels.ts). The first user opens it, and it closes when the last
// one leaves. Reconnects on `close` ONLY — the spec guarantees `error` is
// always followed by `close`, so scheduling on both would double the backoff
// into a storm. One fixed 3s timer per drop, no exponential backoff.
// A new socket starts with no channel on the daemon, so every open runs
// each user's `onOpen` again, with `reopened` true after its first open: a
// caller's catch-up read rides the reconnect with no second code path.
export function createPageSocket({
  wsOrigin,
  reconnectMs,
  resumeDebounceMs,
  onOpen,
}: PageSocketDeps) {
  const users = new Map<SocketUser, { opened: boolean }>();
  let ws: WebSocket | null = null;
  // The channel layer of the open socket; `null` while it is not open.
  let channels: PageChannels | null = null;
  let timer: number | null = null;
  let lastResumeAt = 0;
  let connectingSince = 0;
  let handshakeMs: number | null = null;
  // The page's read of its session, given before the first socket. The
  // daemon mints the device cookie only on that read, and reads the device
  // once, at the upgrade (#834): the first socket waits for the read, or
  // for the handshake deadline. `null` once the wait began.
  let sessionRead: Promise<unknown> | null = null;
  let waitingForSession = false;
  const greet = (user: SocketUser, state: { opened: boolean }, layer: PageChannels) => {
    const reopened = state.opened;
    state.opened = true;
    user.onOpen(layer, reopened);
  };
  const bare = (bytes: Uint8Array) => {
    for (const user of [...users.keys()]) user.onBare?.(bytes);
  };
  // The channels of a socket that ended are over; the next open opens them again.
  const lose = () => {
    const layer = channels;
    channels = null;
    layer?.socketLost();
  };
  const waitForSession = (read: Promise<unknown>) => {
    sessionRead = null;
    waitingForSession = true;
    const go = () => {
      if (!waitingForSession) return;
      waitingForSession = false;
      if (users.size && !ws) connect();
    };
    void read.then(go, go);
    setTimeout(go, CONNECT_TIMEOUT_MS);
  };
  const connect = () => {
    if (sessionRead) {
      waitForSession(sessionRead);
      return;
    }
    const sock = new WebSocket(wsOrigin + "/ws");
    ws = sock;
    connectingSince = Date.now();
    armHandshakeDeadline(sock);
    sock.binaryType = "arraybuffer";
    sock.onopen = () => {
      handshakeMs = Date.now() - connectingSince;
      const layer = pageChannels(channelSocket(sock), bare);
      channels = layer;
      for (const [user, state] of [...users]) greet(user, state, layer);
      onOpen();
    };
    sock.onmessage = (ev: MessageEvent<ArrayBuffer>) => channels?.receive(new Uint8Array(ev.data));
    sock.onclose = () => {
      lose();
      // The last user left as its channel ended: no user, no socket. A
      // timer here would open a second socket beside the one the next
      // user opens.
      if (ws !== sock) return;
      timer = setTimeout(connect, reconnectMs);
    };
  };
  const leave = (user: SocketUser) => {
    if (!users.delete(user) || users.size) return;
    clearTimeout(timer);
    timer = null;
    lose();
    detachSocket(ws);
    ws = null;
  };
  return {
    // Add a user; it returns the user's leave.
    join: (user: SocketUser) => {
      const state = { opened: false };
      users.set(user, state);
      if (ws) {
        if (channels) greet(user, state, channels);
      } else if (!waitingForSession) {
        connect();
      }
      return () => leave(user);
    },
    // Hold the first socket until `read`, the page's read of its session,
    // settles. A page that already opened a socket ignores it.
    afterSessionRead: (read: Promise<unknown>) => {
      if (connectingSince === 0 && !waitingForSession) sessionRead = read;
    },
    // Send a bare frame on channel 0 if the socket is open; `false` means
    // the frame was not sent, and the caller's open hook re-sends its state.
    sendIfOpen: (frame: Uint8Array<ArrayBuffer>) => {
      if (!channels || !ws) return false;
      ws.send(frame);
      return true;
    },
    resume: (stale: boolean) => {
      if (!ws) return false;
      const now = Date.now();
      if (now - lastResumeAt < resumeDebounceMs) return false;
      const connectingMs = now - connectingSince;
      if (resumeDecision({ readyState: ws.readyState, stale, connectingMs }) === "none") {
        return false;
      }
      lastResumeAt = now;
      clearTimeout(timer);
      timer = null;
      // The daemon reads the frames sent before the close (#834).
      for (const user of [...users.keys()]) user.onResume?.();
      const old = ws;
      detachSocket(old);
      lose();
      // A console that reopens as its channel ends joins inside `lose`. When
      // it was the only user, its leave cleared `ws` and its join opened the
      // next socket; a second one here would open the console twice.
      if (ws === old) connect();
      return true;
    },
    // The milliseconds from the construction of the last socket that opened
    // to its `open`, or `null` before the first open.
    handshakeMs: () => handshakeMs,
  };
}
