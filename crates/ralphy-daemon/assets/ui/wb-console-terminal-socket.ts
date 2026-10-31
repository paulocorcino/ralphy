/* ---------------------------------------------------------------------------
   The retirement of a console channel of the console's terminal
   (`wb-console-terminal.ts`): the detach announcement, and the close.
   --------------------------------------------------------------------------- */
import { encodeDetach } from "./wb-console-session.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { ConsoleChannel } from "./wb-console-terminal-types.d.ts";

// Announce `reason` on an open channel. A channel that waits for the page
// socket cannot send.
export function announceDetach(ws: ConsoleChannel | null, reason: DetachReason | undefined) {
  if (reason && ws && ws.readyState === 1) ws.send(encodeDetach(reason));
}

// Retire a channel. Its close ends its frames and calls no handler
// (`openConsole`); a frame that was still decoding finds a channel that is
// no longer the link's, and is dropped there.
export function detachSocket(ws: ConsoleChannel | null, reason: DetachReason | undefined) {
  if (!ws) return;
  try {
    announceDetach(ws, reason);
    ws.close();
  } catch {} // a page socket that fails a send is closing, and its close ends the channel
}
