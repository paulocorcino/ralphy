// Unit tests for assets/ui/wb-resume.ts — the Resume rule of the page's sockets.
// A pure function: the test calls it directly, with no socket and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { CONNECT_TIMEOUT_MS, resumeDecision } from "../assets/ui/wb-resume.ts";

// --- resumeDecision: the resume rule, shared by the daemon door and the consoles -------------
// A tablet suspends the tab: no JS runs while the link is torn down, so the
// sockets come back reporting OPEN with nothing ever arriving on them again.
// The fixed 3s retry only helps the ones that actually heard their close.

test("resumeDecision reconnects exactly the sockets the resume must replace", () => {
  const T = CONNECT_TIMEOUT_MS;
  // [case, socket, the stale verdicts it is asked under, expected]
  const rows = [
    // A socket that is gone reconnects, whatever the verdict.
    ["gone (null)", { readyState: null }, [true, false], "reconnect"],
    ["gone (undefined)", { readyState: undefined }, [true, false], "reconnect"],
    ["CLOSING", { readyState: 2 }, [true, false], "reconnect"],
    ["CLOSED", { readyState: 3 }, [true, false], "reconnect"],
    // A young CONNECTING socket is left alone — it IS the reconnect.
    ["young CONNECTING", { readyState: 0 }, [true, false], "none"],
    ["CONNECTING for 0 ms", { readyState: 0, connectingMs: 0 }, [true, false], "none"],
    [
      "CONNECTING just before the deadline",
      { readyState: 0, connectingMs: T - 1 },
      [true, false],
      "none",
    ],
    // Opened before the suspend, or onto a link that was not up yet: its
    // deadline timer froze with the tab, so the resume is what retires it.
    ["CONNECTING at the deadline", { readyState: 0, connectingMs: T }, [true, false], "reconnect"],
    // An OPEN socket churns only when the caller says it is stale.
    ["OPEN and stale", { readyState: 1 }, [true], "reconnect"],
    ["OPEN and not stale", { readyState: 1 }, [false], "none"],
  ];
  for (const [name, socket, stales, want] of rows) {
    for (const stale of stales) {
      assert.equal(resumeDecision({ ...socket, stale }), want, `${name}, stale=${stale}`);
    }
  }
});
