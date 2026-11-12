/* ---------------------------------------------------------------------------
   The move of a FILES tree entry (`performMove` of wb-files.ts): the rename
   goes through the write seam, and the tree, the open tabs and the selection
   follow it.

   It takes the files component as `files`.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { parentRel } from "./wb-file-paths.ts";
import type { AlpineMagics } from "./wb-alpine.ts";
import { devError } from "./wb-log.ts";

/** What a move reads or calls on the files component. */
type MoveFiles = {
  $store: AlpineMagics["$store"];
  onTreeDirty(rel: string): unknown;
  revealRel(rel: string): Promise<unknown>;
};

// Through `WBDaemon.write`, not the fire-and-forget `window.WB.emit("rename")`:
// the reveal, the flash and the tab re-path need the reply. INVARIANT: no
// tab is re-pathed and no reveal happens on a refusal.
export async function moveEntry(files: MoveFiles, from: string, to: string) {
  const reply = await WBDaemon.write(
    "file.rename",
    WBDaemon.withCheckout(
      { repo: files.$store.projects.openSlug, path: from, to },
      files.$store.checkouts.checkoutOf(files.$store.projects.openSlug),
    ),
  ).catch(() => null);
  if (!reply) {
    files.$store.modals.flash("Could not move: the daemon did not answer.");
    return;
  }
  if (WBFail.isError(reply)) {
    files.$store.modals.flash(WBFail.failed(reply, "Could not move: the daemon gave no reason."));
    return;
  }
  try {
    await files.onTreeDirty(parentRel(from));
    await files.onTreeDirty(parentRel(to));
    files.$store.tabs.repathTabs(from, to);
    // LAST, so its `setActive()` is the final write.
    await files.revealRel(to);
  } catch (err) {
    devError("[workbench] the move did not finish", err);
  }
}
