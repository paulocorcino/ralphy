/* ---------------------------------------------------------------------------
   The remote acts of the Changes view (`syncFetch`, `syncPull` and `syncPush`
   of wb-changes-panel.ts): the operator's act against the upstream, never a
   timer's.

   Each act takes the changes component as `changes`.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import type { AlpineMagics } from "./wb-alpine.ts";

/** What a remote act reads, writes or calls on the changes component. */
type SyncActs = {
  $store: AlpineMagics["$store"];
  syncBusy: "fetch" | "pull" | "push" | null;
  changesError: string;
  _changesRefused(msg: string): void;
  loadSync(slug: string | null | undefined): Promise<void>;
  loadChanges(slug: string | null | undefined): Promise<void>;
};

// Fetch from the upstream — the operator's act, never a timer's. A refusal
// is `{status:"error"}` whose message IS the core's prose.
export async function syncFetch(changes: SyncActs, slug: string) {
  if (changes.syncBusy || changes.$store.connection.writeLocked()) return;
  changes.syncBusy = "fetch";
  changes.changesError = "";
  try {
    const reply = await window.WBDaemon.observe(
      "sync.fetch",
      window.WBDaemon.withCheckout({ repo: slug }, changes.$store.checkouts.checkoutOf(slug)),
    );
    if (WBFail.isError(reply)) {
      changes._changesRefused(WBFail.failed(reply, "Could not fetch: the daemon gave no reason."));
    }
  } catch {
    // A transport throw is NOT a refusal: the repo never answered.
    changes._changesRefused("Could not fetch: the daemon did not answer.");
  } finally {
    changes.syncBusy = null;
  }
  void changes.loadSync(slug);
}

// Fast-forward from the upstream. A successful pull moves the working tree,
// so the change set is reloaded beside the counts.
export async function syncPull(changes: SyncActs, slug: string) {
  if (changes.syncBusy || changes.$store.connection.writeLocked()) return;
  changes.syncBusy = "pull";
  changes.changesError = "";
  let moved = false;
  try {
    const reply = await window.WBDaemon.observe(
      "sync.pull",
      window.WBDaemon.withCheckout({ repo: slug }, changes.$store.checkouts.checkoutOf(slug)),
    );
    if (WBFail.isError(reply)) {
      changes._changesRefused(WBFail.failed(reply, "Could not pull: the daemon gave no reason."));
    } else {
      moved = true;
    }
  } catch {
    changes._changesRefused("Could not pull: the daemon did not answer.");
  } finally {
    changes.syncBusy = null;
  }
  void changes.loadSync(slug);
  if (moved) void changes.loadChanges(slug);
}

// Publish the branch (#320). The OPERATOR's click is the whole consent (no
// opt-in flag, ADR-0046 amendment); a refusal's message IS the core's
// prose. No credential UI, by decision. Push moves no file, so only the
// counts reload.
export async function syncPush(changes: SyncActs, slug: string) {
  if (changes.syncBusy || changes.$store.connection.writeLocked()) return;
  changes.syncBusy = "push";
  changes.changesError = "";
  try {
    const reply = await window.WBDaemon.observe(
      "sync.push",
      window.WBDaemon.withCheckout({ repo: slug }, changes.$store.checkouts.checkoutOf(slug)),
    );
    if (WBFail.isError(reply)) {
      changes._changesRefused(WBFail.failed(reply, "Could not push: the daemon gave no reason."));
    }
  } catch {
    changes._changesRefused("Could not push: the daemon did not answer.");
  } finally {
    changes.syncBusy = null;
  }
  void changes.loadSync(slug);
}
