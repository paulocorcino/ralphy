/* ---------------------------------------------------------------------------
   The retirement of a session socket of the console's terminal
   (`wb-console-terminal.ts`): the detach announcement, and the close that
   leaves no handler behind.
   --------------------------------------------------------------------------- */
import { encodeDetach } from "./wb-console-session.ts";
import type { DetachReason } from "./wb-console-session.ts";

// Announce `reason` on an open socket. A socket still connecting cannot send.
export function announceDetach(ws: WebSocket | null, reason: DetachReason | undefined) {
  if (reason && ws && ws.readyState === 1) ws.send(encodeDetach(reason));
}

// Retire a socket so its pending events cannot reach us. `onmessage` matters
// as much as `onclose`: a frame still queued lands AFTER this returns, when
// `ws` names the replacement. Local, not `WBDaemon`'s: this module loads on
// its own in the node harness and the popup.
export function detachSocket(ws: WebSocket | null, reason: DetachReason | undefined) {
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
