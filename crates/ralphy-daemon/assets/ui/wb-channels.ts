/* ---------------------------------------------------------------------------
   The page's channels over its one socket (`/ws`).

   `pageChannels(socket, onBare)` gives the channel layer of one socket. A
   channel frame is `[0x04][channel: u32 BE][flags: u8][piece]` (the mirror of
   `src/protocol/channel.rs`): the pieces of one channel, joined in order, are
   one inner frame in the codec of `wb-console-session.ts`. Bit 0 of `flags`
   is MORE, and no other bit is set.

   Channel 0 is the socket itself. Its frames are bare (no `0x04` tag) and go
   to `onBare`, except `channel.closed`, which goes to its channel. The page
   picks each channel id, from 1 up, and never reuses one on a socket.

   The layer has no WebSocket. The module that owns the socket (`wb-daemon.ts`)
   gives each message to `receive`, and calls `socketLost` when the socket
   ends. A new socket gets a new layer.
   --------------------------------------------------------------------------- */
import { isRecord, socketFrame } from "./wb-api.ts";
import { encodeCommand } from "./wb-console-session.ts";

export const TAG_CHANNEL = 0x04;

/** The most bytes of one piece, in both directions. */
export const MAX_PIECE = 16 * 1024;

const FLAG_MORE = 0x01;
const HEADER_BYTES = 6;
const MAX_CHANNEL_ID = 0xffffffff;
// `WebSocket.OPEN`: the layer does not read the DOM global.
const SOCKET_OPEN = 1;

/** What a channel carries; the same set as `ChannelKind` in the daemon. */
export type ChannelKind = "console" | "tree" | "runs" | "changes" | "spawn";

/** Why a channel ended: a word of the daemon's `channel.closed`, or
 * `socket-lost` when the socket under it ended. */
export type CloseReason = "refused" | "too-large" | "done" | "socket-lost";

/** The part of a WebSocket the layer uses. */
export type ChannelSocket = {
  readonly readyState: number;
  send(data: Uint8Array): void;
};

export type Channel = {
  readonly id: number;
  readonly readyState: "open" | "closed";
  /** Send one inner frame, cut into pieces. `false` when the channel or the
   * socket is closed. */
  send(inner: Uint8Array): boolean;
  /** End the channel: the daemon is told, and `onClosed` is not called. */
  close(): void;
  /** One whole inner frame, joined from its pieces. */
  onFrame: ((inner: Uint8Array) => void) | null;
  /** The daemon ended the channel, or the socket was lost. Called once. */
  onClosed: ((reason: CloseReason) => void) | null;
};

export type PageChannels = {
  /** Open a channel: sends `channel.open` on channel 0. */
  open(kind: ChannelKind, payload: { [field: string]: JsonValue | undefined }): Channel;
  /** Take one binary message of the socket. */
  receive(bytes: Uint8Array): void;
  /** The socket ended: every open channel is closed with `socket-lost`. */
  socketLost(): void;
};

type Entry = {
  channel: Channel;
  pending: Uint8Array[];
  /** The daemon or the socket ends the channel. */
  end(reason: CloseReason): void;
};

export function pageChannels(
  socket: ChannelSocket,
  onBare: (bytes: Uint8Array) => void,
): PageChannels {
  const table = new Map<number, Entry>();
  let lastId = 0;

  function open(kind: ChannelKind, payload: { [field: string]: JsonValue | undefined }): Channel {
    const id = lastId + 1;
    const usable = socket.readyState === SOCKET_OPEN && id <= MAX_CHANNEL_ID;
    const entry = newEntry(id, usable);
    if (!usable) return entry.channel;
    lastId = id;
    table.set(id, entry);
    socket.send(encodeCommand("channel.open", { ...payload, channel: id, kind }));
    return entry.channel;
  }

  function newEntry(id: number, usable: boolean): Entry {
    let live = usable;
    const stop = () => {
      live = false;
      table.delete(id);
      entry.pending = [];
    };
    const entry: Entry = {
      pending: [],
      end(reason) {
        if (!live) return;
        stop();
        entry.channel.onClosed?.(reason);
      },
      channel: {
        id,
        get readyState() {
          return live ? "open" : "closed";
        },
        send(inner) {
          if (!live || socket.readyState !== SOCKET_OPEN) return false;
          for (const frame of cut(id, inner)) socket.send(frame);
          return true;
        },
        close() {
          if (!live) return;
          stop();
          if (socket.readyState === SOCKET_OPEN) {
            socket.send(encodeCommand("channel.close", { channel: id }));
          }
        },
        onFrame: null,
        onClosed: null,
      },
    };
    return entry;
  }

  function receive(bytes: Uint8Array): void {
    if (bytes[0] === TAG_CHANNEL) {
      takePiece(bytes);
    } else if (!takeClosed(bytes)) {
      onBare(bytes);
    }
  }

  function takePiece(bytes: Uint8Array): void {
    if (bytes.length < HEADER_BYTES) return;
    const flags = bytes[5] ?? 0;
    if ((flags & ~FLAG_MORE) !== 0) return;
    const id = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(1);
    const entry = table.get(id);
    if (!entry) return;
    entry.pending.push(bytes.subarray(HEADER_BYTES));
    if ((flags & FLAG_MORE) !== 0) return;
    const inner = join(entry.pending);
    entry.pending = [];
    entry.channel.onFrame?.(inner);
  }

  // Whether `bytes` was a `channel.closed`, which channel 0 does not show.
  function takeClosed(bytes: Uint8Array): boolean {
    const closed = readClosed(bytes);
    if (!closed) return false;
    table.get(closed.channel)?.end(closed.reason);
    return true;
  }

  function socketLost(): void {
    for (const entry of [...table.values()]) entry.end("socket-lost");
  }

  return { open, receive, socketLost };
}

/** The channel frames that carry `inner`: pieces of at most `MAX_PIECE`
 * bytes, MORE on every piece but the last. An empty frame is one empty piece. */
export function cut(channel: number, inner: Uint8Array): Uint8Array[] {
  const frames: Uint8Array[] = [];
  let start = 0;
  do {
    const piece = inner.subarray(start, start + MAX_PIECE);
    start += piece.length;
    frames.push(channelFrame(channel, start < inner.length, piece));
  } while (start < inner.length);
  return frames;
}

function channelFrame(channel: number, more: boolean, piece: Uint8Array): Uint8Array {
  const out = new Uint8Array(HEADER_BYTES + piece.length);
  out[0] = TAG_CHANNEL;
  new DataView(out.buffer).setUint32(1, channel);
  out[5] = more ? FLAG_MORE : 0;
  out.set(piece, HEADER_BYTES);
  return out;
}

function join(pieces: Uint8Array[]): Uint8Array {
  if (pieces.length === 1 && pieces[0]) return pieces[0];
  const out = new Uint8Array(pieces.reduce((sum, piece) => sum + piece.length, 0));
  let at = 0;
  for (const piece of pieces) {
    out.set(piece, at);
    at += piece.length;
  }
  return out;
}

type ClosedFrame = { channel: number; reason: CloseReason };

// The `channel.closed` a bare frame says, or `null` for any other frame.
function readClosed(bytes: Uint8Array): ClosedFrame | null {
  if (bytes[0] !== 0x02) return null;
  let frame: { verb?: unknown; payload?: unknown };
  try {
    frame = socketFrame<{ verb?: unknown; payload?: unknown }>(bytes);
  } catch {
    // Not JSON: channel 0 gets it, and decides what it is.
    return null;
  }
  if (!isRecord(frame) || frame.verb !== "channel.closed" || !isRecord(frame.payload)) return null;
  const { channel, reason } = frame.payload;
  if (typeof channel !== "number") return null;
  return { channel, reason: closeReason(reason) };
}

function closeReason(reason: unknown): CloseReason {
  switch (reason) {
    case "refused":
    case "too-large":
    case "done":
      return reason;
    default:
      // A word this page does not know: the channel is over all the same.
      return "refused";
  }
}
