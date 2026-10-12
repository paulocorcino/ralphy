/* ---------------------------------------------------------------------------
   What one message of the session socket says (`/ws/session`).

   `readSocketEvent(bytes)` folds the bytes of one message into a closed
   `SocketEvent`. The terminal module matches it exhaustively and does the
   side effects; this module has none.
   --------------------------------------------------------------------------- */
import { socketFrame } from "./wb-api.ts";
import { TAG_COMMAND, TAG_TERMINAL } from "./wb-console-session.ts";
import { WBSessionRoute } from "./wb-session-route.ts";

// The payload of the daemon's `session-end` and `session-open` frames.
export type FramePayload = NonNullable<Parameters<typeof WBSessionRoute.announcement>[1]> & {
  reason?: string;
  message?: unknown;
  watch?: boolean;
};

// A command frame the daemon pushes on the session socket.
type SessionFrame = { verb?: string; payload?: FramePayload };

export type SocketEvent =
  // Terminal output: the session id the frame names, and the bytes after it.
  | { kind: "output"; sessionId: number; bytes: Uint8Array }
  | { kind: "open"; payload: FramePayload | undefined }
  | { kind: "end"; reason: string; message: string | null }
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
