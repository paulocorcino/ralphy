/* ---------------------------------------------------------------------------
   wb-resume.ts — the Resume rule of the page's sockets (CONTEXT.md: Resume).
   Pure: the daemon door (`wb-daemon.ts`) and the console terminal import it, so
   the tree, presence, changes and console sockets reconnect by one rule.
   --------------------------------------------------------------------------- */

// RESUME — the tablet case. A suspended tab runs no JS while the link is torn
// down, so it comes back holding sockets that report OPEN and never deliver
// another byte. Named `resume`, never `wake`: waking is for a peer daemon
// (CONTEXT.md).
//
// `stale` is the caller's verdict: the shell feeds it from the presence
// heartbeat (`setStaleProbe`); the popup, with none, falls back to how long
// it was hidden — so an ordinary desktop tab switch churns nothing.

// Two resume triggers (`visibilitychange` and `online`) land within the same
// millisecond on an iOS resume. Without a debounce the second one tears down
// the socket the first one just opened.
export const RESUME_DEBOUNCE_MS = 1500;

// A handshake gets this long to open. Without a deadline a reattach opened
// onto a link that is not up yet (an iPhone back from a call) sits in
// CONNECTING until the OS abandons TCP/TLS, under a
// "[connection lost — reconnecting…]" that no resume will touch.
export const CONNECT_TIMEOUT_MS = 8000;

export type ResumeVerdict = "reconnect" | "none";

// `readyState` is the WebSocket's, or null/undefined when the socket is gone.
// `connectingMs` is how long a CONNECTING socket has been so.
export interface ResumeSocket {
  readyState: number | null | undefined;
  stale?: boolean;
  connectingMs?: number;
}

// Pure, tabled like `reconnectDecision`. CONNECTING is already the reconnect —
// closing it only restarts the handshake a round-trip later — until it
// outlives the handshake deadline, whose timer froze along with the tab.
export function resumeDecision({ readyState, stale, connectingMs }: ResumeSocket): ResumeVerdict {
  if (readyState == null) return "reconnect";
  if (readyState === 0) return (connectingMs ?? 0) >= CONNECT_TIMEOUT_MS ? "reconnect" : "none";
  if (readyState === 1) return stale ? "reconnect" : "none";
  return "reconnect";
}
