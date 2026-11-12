/* ---------------------------------------------------------------------------
   The desk upload: the debounce, the one batch on the wire, the answer to it
   and the last batch as the tab closes. `createDeskFlush(deps)` is built by
   `createDesk` (`wb-desk.ts`), which owns the desk state it reads.
   --------------------------------------------------------------------------- */
import type { WBDeskSink } from "./wb-desk-sink.ts";
import type { WBDeskSync } from "./wb-desk-sync.ts";
import type { DeskReply } from "./wb-types.d.ts";
import { devWarn } from "./wb-log.ts";

// Where an upload goes. The sink never rejects, and a network failure is one
// more answer to `flushed`.
export type DeskSink = Pick<ReturnType<typeof WBDeskSink.daemon>, "put" | "putSync">;
// The answer to one upload, as `flushed` reads it.
export type FlushAnswer = { kind: string; status?: number; reply?: DeskReply | null };

// What the upload reads from the desk, and nothing else.
export type FlushDeps = {
  deskSink: DeskSink;
  sync: ReturnType<typeof WBDeskSync.createSync>;
  isDeskLoaded: () => boolean;
  refreshView: () => void;
  converge: () => void;
  reloadForRestoredDesk: () => void;
  // The daemon refused a write over a desk it cannot read.
  deskUnreadable: (out: FlushAnswer) => void;
};

export function createDeskFlush(deps: FlushDeps) {
  const { deskSink, sync, isDeskLoaded, refreshView, converge, reloadForRestoredDesk } = deps;

  // The upload, debounced. WHERE it goes is `deskSink`'s business
  // (wb-desk-sink.ts). One batch is on the wire at a time.
  let deskFlush: ReturnType<typeof setTimeout> | null = null;
  let flushing = false;
  function scheduleDeskFlush(ms = 250) {
    clearTimeout(deskFlush);
    // Cleared when it FIRES too: a spent timer id is still truthy.
    deskFlush = setTimeout(() => {
      deskFlush = null;
      flushDesk();
    }, ms);
  }
  function flushDesk() {
    if (flushing || sync.phase() !== "ready") return;
    const body = sync.nextBatch();
    if (!body) return;
    flushing = true;
    void deskSink
      .put(JSON.stringify(body))
      .catch(() => ({ kind: "network" }))
      .then((out) => {
        flushing = false;
        flushed(out);
        return null;
      });
  }

  // The last batch, as the tab closes, from the console's `pagehide`.
  function flushDeskOnClose() {
    if (!isDeskLoaded() || sync.phase() !== "ready") return;
    const body = sync.closingBatch();
    if (!body) return;
    clearTimeout(deskFlush);
    deskFlush = null;
    deskSink.putSync(JSON.stringify(body));
  }

  // The answer to one upload (`flushDesk`): a batch that failed in a way the
  // daemon may still accept is sent again with the same `seq`, later.
  let flushBackoff = 1000;
  function flushed(out: FlushAnswer) {
    const kind = out.kind;
    if (kind === "held") return;
    if (kind === "ok") {
      flushedOk(out);
      return;
    }
    if (kind === "refused" && flushedRefused(out)) return;
    // A network failure, a 401 before a login, a 5xx: kept, and sent again.
    scheduleDeskFlush(flushBackoff);
    flushBackoff = Math.min(flushBackoff * 2, 30000);
  }
  // The daemon took the batch: the desk it sent is the view now.
  function flushedOk(out: FlushAnswer) {
    flushBackoff = 1000;
    for (const r of out.reply?.refused ?? []) devWarn("desk change refused:", r.error);
    const verdict = sync.acked(out.reply);
    if (verdict === "reload") {
      reloadForRestoredDesk();
      return;
    }
    if (verdict === "taken") {
      refreshView();
      converge();
    }
    if (sync.hasPending()) scheduleDeskFlush();
  }
  // The daemon refused the batch. True when the refusal is handled; false
  // for one that is kept and sent again.
  function flushedRefused(out: FlushAnswer): boolean {
    const state = out.reply?.state;
    if (out.status === 409 && state === "restored") {
      reloadForRestoredDesk();
      return true;
    }
    // The daemon will never accept this batch.
    if (out.status === 400 || out.status === 422) {
      devWarn("desk changes refused:", out.reply?.error);
      sync.dropped();
      if (sync.hasPending()) scheduleDeskFlush();
      return true;
    }
    if (out.status === 409 && state === "unreadable") {
      deps.deskUnreadable(out);
      return true;
    }
    return false;
  }

  return { scheduleDeskFlush, flushDeskOnClose };
}
