/* ---------------------------------------------------------------------------
   wb-resume.ts — the Resume rule of the page's sockets (CONTEXT.md: Resume).
   Pure: the daemon door (`wb-daemon.ts`) and the console terminal import it, so
   the page socket and every console reconnect by one rule.
   --------------------------------------------------------------------------- */

// RESUME — the tablet case. A suspended tab runs no JS while the link is torn
// down, so it comes back holding sockets that report OPEN and never deliver
// another byte. Named `resume`, never `wake`: waking is for a peer daemon
// (CONTEXT.md).
//
// The daemon door (`wb-daemon.ts`) hears each trigger once and judges
// `stale` once (`pageIsStale`), then resumes the page socket, then lets each
// console run its own resume (ADR-0082, amendment of 2026-11-01).
export type ResumeTrigger = "visible" | "online" | "heartbeat";

// Three triggers resume the page: the page shown again (`visibilitychange`),
// `online`, and the heartbeat that stops while the page stays visible (#751).
// The first two land within the same millisecond on an iOS resume. Without a
// debounce the second one tears down the socket the first one just opened.
export const RESUME_DEBOUNCE_MS = 1500;

// The daemon sends a heartbeat every ~2 s: three missed ones mean this
// page's link is gone. A suspended tablet runs no JS, so on return the last
// heartbeat is old; a desktop tab switch leaves it fresh.
export const HEARTBEAT_LOST_MS = 6000;

// How long a page with no heartbeat (the detached fence page, which
// subscribes to no presence) counts as hidden before its socket is taken as
// stale when it is shown. An ordinary desktop tab switch stays below it and
// churns nothing.
export const RESUME_HIDDEN_MS = 60000;

// Whether the page socket is stale on `trigger`. `heartbeatAgeMs` is the age
// of the last heartbeat (`Infinity` before the first), or `null` on a page
// with no heartbeat; `hiddenMs` is how long the page was hidden. `online`
// fires because the link the socket ran over is a different link now.
export function pageIsStale(
  trigger: ResumeTrigger,
  heartbeatAgeMs: number | null,
  hiddenMs: number,
): boolean {
  switch (trigger) {
    case "online":
    case "heartbeat":
      return true;
    case "visible":
      return heartbeatAgeMs === null
        ? hiddenMs > RESUME_HIDDEN_MS
        : heartbeatAgeMs > HEARTBEAT_LOST_MS;
    default: {
      const unreachable: never = trigger;
      return unreachable;
    }
  }
}

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
