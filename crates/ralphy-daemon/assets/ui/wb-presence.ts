/* ---------------------------------------------------------------------------
   The operator's presence at this page (ADR-0085 D14): whether the tab is
   visible, and when the operator's last key, click or touch in it was. The
   daemon reads it to decide whether a console that waits is told on
   Telegram: an operator at a page is not sent a message.

   The page sends `presence.report {visible, lastInput, now, origin}` on the
   `/ws` socket (`WBDaemon.reportPresence`): when the visibility changes, on
   the first input after a quiet minute, every minute while the tab is
   visible, and each time the socket opens. Not on every key: the daemon's
   window is five minutes, so a minute late changes nothing. `lastInput` and
   `now` are this page's clock in ms; the daemon uses only their difference.
   `origin` is where a link in a Telegram message opens.

   `main.ts` starts it once, after `WBDaemon`.
   --------------------------------------------------------------------------- */

/** One report, as the daemon reads it (`presence_board.rs`). */
export type PresenceReport = { visible: boolean; lastInput: number | null; now: number; origin: string };

/** What the module needs: the send on the `/ws` socket, the hook on each of
 * its opens, and a clock. */
export type PresenceDeps = {
  send: (report: PresenceReport) => boolean;
  onOpen: (fn: () => void) => void;
  now?: () => number;
};

/** How long the page waits after an input before the next input is
 * reported, and how often a visible page reports. */
export const QUIET_MS = 60000;

/** The inputs that count as the operator being here. */
const INPUTS = ["keydown", "pointerdown", "touchstart"] as const;

export function startPresence(window: Window, document: Document, deps: PresenceDeps) {
  const now = deps.now ?? (() => Date.now());
  let lastInput: number | null = null;
  // The time of the last report; an input after a quiet minute is news.
  let reportedAt = 0;

  const report = () => {
    const at = now();
    const sent = deps.send({
      visible: document.visibilityState === "visible",
      lastInput,
      now: at,
      origin: window.location.origin,
    });
    if (sent) reportedAt = at;
  };

  const input = () => {
    const at = now();
    const quiet = lastInput === null || at - lastInput >= QUIET_MS || at - reportedAt >= QUIET_MS;
    lastInput = at;
    if (quiet) report();
  };

  for (const name of INPUTS) document.addEventListener(name, input, { capture: true, passive: true });
  document.addEventListener("visibilitychange", report);
  deps.onOpen(report);
  window.setInterval(() => {
    if (document.visibilityState === "visible") report();
  }, QUIET_MS);
  report();
}
