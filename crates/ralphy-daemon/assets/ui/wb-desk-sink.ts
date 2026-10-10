/* ---------------------------------------------------------------------------
   ralphy workbench — the desk WRITE seam

   `wb-console.ts` owns the desk changes and decides WHEN to send them; this
   module owns WHERE that write goes. Two implementations, one surface:

     daemon()  the shell's real write — `PUT /api/desk` (ADR-0050)
     none()    a sink that writes nowhere

   The seam exists because the same console module runs in a second document —
   the detached-fence popup — which renders a fence's consoles but must never
   author the desk: it holds a partial view of the plane, so a PUT from there
   would replace the operator's whole layout with a fragment of it. Injecting
   the sink is what makes the popup INCAPABLE of writing, rather than merely
   guarded at each call site by a `detached` test that a later edit can forget.

   Only the WRITE is injected. The GET stays in `wb-console.ts` and DOES run in
   the popup (`reloadDesk` runs when each document creates its console), so the
   suppression rests entirely on the sink here — not on an unlifted `deskLoaded`
   permit. Reading is harmless; replacing the desk from a partial view is not.
--------------------------------------------------------------------------- */
import { apiFetch } from "./wb-api.ts";

// The tab id and `hold` stay at module scope (ADR-0075 D7 allows a value that
// is loaded once): a browser runs a module once per document, which is the
// scope both have, and the console and `app.ts` must see the same `hold`.
// A test that loads a new page clears `hold` first, as a new document would.
export const WBDeskSink = (function () {
  // One id per document. The daemon echoes it in the `desk.dirty` push its
  // PUT causes, so this tab does not read its own write again.
  const TAB =
    globalThis.crypto?.randomUUID?.() || "t" + Math.random().toString(36).slice(2) + Date.now();
  function tabId() {
    return TAB;
  }
  // While this tab runs an older build than the daemon, it writes no desk
  // (ADR-0070 D6): its JavaScript may not know the daemon's records.
  let hold = false;
  function setHold(on: boolean) {
    hold = !!on;
  }

  // The daemon-backed write. Both entry points take an already-serialised body:
  // the caller builds it from the changes it holds at that moment.
  //
  // `put` answers what happened, and never rejects:
  //   { kind: "ok", reply }               — the daemon applied the body
  //   { kind: "refused", status, reply }  — the daemon answered with an error
  //   { kind: "network" }                 — the daemon could not be reached
  //   { kind: "held" }                    — this sink writes nothing now
  // The caller decides which of them to send again (wb-console.ts `flushed`).
  function daemon() {
    // Chained on the previous write so two uploads cannot land out of order
    // over a LAN or a dev tunnel.
    let inFlight: Promise<unknown> = Promise.resolve();
    return {
      put(body: string) {
        if (hold) return Promise.resolve({ kind: "held" });
        const sent = inFlight.then(() =>
          apiFetch("PUT /api/desk", {
            query: { tab: TAB },
            headers: { "Content-Type": "application/json" },
            body,
          }).then(
            async (r) => {
              const reply = (await r.json?.().catch(() => null)) ?? null;
              return r.ok ? { kind: "ok", reply } : { kind: "refused", status: r.status, reply };
            },
            () => ({ kind: "network" }),
          ),
        );
        inFlight = sent.catch(() => {});
        return sent;
      },
      // The tab is going away: `keepalive` lets the request outlive the
      // document. Deliberately NOT chained — there is no next flush to order
      // against, and awaiting one would be awaiting past the document's life.
      putSync(body: string) {
        if (hold) return;
        try {
          apiFetch("PUT /api/desk", {
            query: { tab: TAB },
            headers: { "Content-Type": "application/json" },
            body,
            keepalive: true,
          }).catch(() => {});
        } catch {}
      },
    };
  }

  // Writes nowhere: every write is `held`, the answer of a sink that is not
  // writing now.
  function none() {
    return {
      put() {
        return Promise.resolve({ kind: "held" });
      },
      putSync() {},
    };
  }

  return { daemon, none, tabId, setHold };
})();
