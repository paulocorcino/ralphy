/* ---------------------------------------------------------------------------
   What one message of the session socket says (`/ws/session`).

   `readSocketEvent(bytes)` folds the bytes of one message into a closed
   `SocketEvent`. The terminal module matches it exhaustively and does the
   side effects; this module has none.

   `frameQueue(apply)` gives each message to `apply` in arrival order. When
   `session-open` says the replay is gzipped, the next terminal frame is
   decoded first, and the frames that arrive meanwhile wait behind it.
   `whenIdle(run)` runs `run` once no frame waits, so a socket's close is
   decided after a `session-end` that waited behind a decode.
   --------------------------------------------------------------------------- */
import { socketFrame } from "./wb-api.ts";
import { TAG_COMMAND, TAG_TERMINAL } from "./wb-console-session.ts";
import { WBSessionRoute } from "./wb-session-route.ts";

// The payload of the daemon's `session-end`, `session-open` and
// `session-size` frames. A peer of an older build sends no `session-size`.
export type FramePayload = NonNullable<Parameters<typeof WBSessionRoute.announcement>[1]> & {
  reason?: string;
  message?: unknown;
  watch?: boolean;
  rows?: number;
  cols?: number;
};

// A command frame the daemon pushes on the session socket.
type SessionFrame = { verb?: string; payload?: FramePayload };

export type SocketEvent =
  // Terminal output: the session id the frame names, and the bytes after it.
  | { kind: "output"; sessionId: number; bytes: Uint8Array }
  | { kind: "open"; payload: FramePayload | undefined }
  | { kind: "end"; reason: string; message: string | null }
  // The session size: the size of the writer's console.
  | { kind: "size"; rows: number; cols: number }
  // The gzip replay did not decode: the terminal does not show it.
  | { kind: "replay-lost" }
  // A frame this page does not act on: another verb, a bad body, a short or
  // unknown frame.
  | { kind: "other" };

export function readSocketEvent(a: Uint8Array): SocketEvent {
  if (a[0] === TAG_TERMINAL) {
    // A terminal frame is a tag, an 8-byte session id, then the output.
    if (a.length < 9) return { kind: "other" };
    const sessionId = Number(new DataView(a.buffer, a.byteOffset + 1, 8).getBigUint64(0));
    return { kind: "output", sessionId, bytes: a.subarray(9) };
  }
  if (a[0] === TAG_COMMAND) {
    // The daemon's deliberate-end announcement, sent as DATA before the
    // Close frame: the close metadata does not survive the trip (#334).
    let c: SessionFrame | null = null;
    try {
      c = socketFrame<SessionFrame>(a);
    } catch {}
    if (c && c.verb === "session-open") return { kind: "open", payload: c.payload };
    if (c && c.verb === "session-size") {
      const rows = c.payload?.rows;
      const cols = c.payload?.cols;
      if (isCount(rows) && isCount(cols)) return { kind: "size", rows, cols };
      return { kind: "other" };
    }
    if (c && c.verb === "session-end") {
      return {
        kind: "end",
        reason: c.payload?.reason ?? "child-exited",
        message: typeof c.payload?.message === "string" ? c.payload.message : null,
      };
    }
  }
  return { kind: "other" };
}

function isCount(n: unknown): n is number {
  return typeof n === "number" && Number.isInteger(n) && n > 0;
}

// The decoded bytes of a gzip stream; rejects on a stream that is not gzip.
async function gunzip(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([new Uint8Array(bytes)]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

export type FrameQueue = {
  take: (bytes: Uint8Array) => void;
  whenIdle: (run: () => void) => void;
};

// A replay that has not decoded by now is lost; a decode that never ends
// would hold every later frame of the console behind it.
export const DECODE_DEADLINE_MS = 10000;

export function frameQueue(
  apply: (event: SocketEvent) => void,
  decode: (bytes: Uint8Array) => Promise<Uint8Array> = gunzip,
): FrameQueue {
  // Set by `session-open`: the next terminal frame is a gzip replay.
  let gzipNext = false;
  // The messages that arrived while the replay decodes, in order; null when
  // no decode is running.
  let waiting: Uint8Array[] | null = null;
  // What waits for the queue to be idle, in order.
  let idle: (() => void)[] = [];

  function take(bytes: Uint8Array) {
    if (waiting) {
      waiting.push(bytes);
      return;
    }
    const event = readSocketEvent(bytes);
    if (event.kind === "open") gzipNext = event.payload?.replay_encoding === "gzip";
    if (event.kind !== "output" || !gzipNext) {
      apply(event);
      return;
    }
    gzipNext = false;
    waiting = [];
    // The first of the decode and the timer ends the wait; the other is dropped.
    let ended = false;
    const end = (event: SocketEvent) => {
      if (ended) return;
      ended = true;
      clearTimeout(timer);
      try {
        apply(event);
      } finally {
        drain();
      }
    };
    const timer = setTimeout(() => {
      console.warn("console replay did not decode in time");
      end({ kind: "replay-lost" });
    }, DECODE_DEADLINE_MS);
    decode(event.bytes).then(
      (decoded) => end({ ...event, bytes: decoded }),
      // The replay is lost, not the console: the live stream goes on.
      (error: unknown) => {
        console.warn("console replay did not decode:", error);
        end({ kind: "replay-lost" });
      },
    );
  }

  // A frame that starts another decode sends the rest of `queued` into the
  // new wait, still in order.
  // One frame that throws costs that frame only: the later frames and the
  // idle list still run, so a waiting socket close is never stranded.
  function drain() {
    const queued = waiting ?? [];
    waiting = null;
    try {
      for (const bytes of queued) {
        try {
          take(bytes);
        } catch (error: unknown) {
          console.warn("console frame was not applied:", error);
        }
      }
    } finally {
      if (!waiting) {
        const runs = idle;
        idle = [];
        for (const run of runs) run();
      }
    }
  }

  function whenIdle(run: () => void) {
    if (waiting) idle.push(run);
    else run();
  }

  return { take, whenIdle };
}
