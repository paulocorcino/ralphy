/* ---------------------------------------------------------------------------
   Stall report (CONTEXT.md → *Stall report*, ADR-0082): the page tells its
   daemon that a read, a replay decode or a console waited past its deadline,
   and the daemon writes one line in `daemon.log`.

   The report goes by a plain `fetch`, not `apiFetch`: a report that passed
   its own deadline would report itself. It is not sent on a socket: on iOS
   the screen lock closes every socket of the page, at the moment there is
   something to report.
   --------------------------------------------------------------------------- */
import { WBSessionRoute } from "./wb-session-route.ts";

/** What waited past its deadline. */
export type StallKind = "read-deadline" | "decode-deadline" | "console-gave-up";

/** A pair `(kind, what)` is reported at most once in this time. */
export const STALL_REPEAT_MS = 60000;

/** How long the report itself may wait for its answer. */
const STALL_SEND_DEADLINE_MS = 5000;

const lastSent = new Map<string, number>();

/** Forget every report sent, so the next one is sent. For the tests. */
export function forgetStalls() {
  lastSent.clear();
}

/** Report a stall to the daemon. It never throws and never rejects. */
export async function reportStall(kind: StallKind, what: string, waitedMs: number): Promise<void> {
  const key = `${kind} ${what}`;
  const now = Date.now();
  const last = lastSent.get(key);
  if (last !== undefined && now - last < STALL_REPEAT_MS) return;
  lastSent.set(key, now);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), STALL_SEND_DEADLINE_MS);
  try {
    const body = JSON.stringify({
      kind,
      what,
      waited_ms: Math.round(waitedMs),
      holder: WBSessionRoute.tabHolder(),
    });
    await globalThis.fetch("/api/device/stall", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body,
      signal: controller.signal,
    });
  } catch {
    // Nothing reads the result: a report that fails is lost, as the stall it names.
  } finally {
    clearTimeout(timer);
  }
}
