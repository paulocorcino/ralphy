/* ---------------------------------------------------------------------------
   The duplicate of a FILES tree entry (`duplicateNode` of wb-files.ts): the
   free name beside the file is found from a fresh listing of its folder, the
   copy goes through `file.copy`, and the tree shows and selects the copy.

   It takes the files component as `files`, as the other steps of the tree do.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { duplicateRel, parentRel } from "./wb-file-paths.ts";
import type { AlpineMagics } from "./wb-alpine.ts";

/** What a duplicate reads or calls on the files component. */
type DuplicateFiles = {
  $store: AlpineMagics["$store"];
  relPath(node: WunderbaumNode | null): string;
  onTreeDirty(rel: string): unknown;
  revealRel(rel: string): Promise<unknown>;
};

// Duplicate a file beside itself, NO prompt. `file.copy` refuses an
// existing dst, so the free name is found HERE (`duplicateRel`).
export async function duplicateNode(files: DuplicateFiles, node: WunderbaumNode) {
  const rel = files.relPath(node);
  if (!rel) return;
  const parent = parentRel(rel);

  // The tree's gestures speak the tree's checkout (#406); a refusal beats
  // a primary-aimed copy that silently misfires.
  const checkout = files.$store.checkouts.checkoutOf(files.$store.projects.openSlug);
  const listing = await WBDaemon.observe(
    "tree.list",
    WBDaemon.withCheckout({ repo: files.$store.projects.openSlug, path: parent }, checkout),
  ).catch(() => null);
  // A refused listing must NOT degrade to an empty `taken` set.
  if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
    files.$store.modals.flash?.("couldn't list the folder");
    return;
  }
  const to = duplicateRel(rel, new Set(listing.entries.map((e) => e.name)));

  const reply = await WBDaemon.write(
    "file.copy",
    WBDaemon.withCheckout({ repo: files.$store.projects.openSlug, path: rel, to }, checkout),
  ).catch(() => null);
  if (!reply || WBFail.isError(reply)) {
    files.$store.modals.flash?.(
      reply
        ? WBFail.failed(reply, "Could not duplicate the file: the daemon gave no reason.")
        : "Could not duplicate the file: the daemon did not answer.",
    );
    return;
  }
  await files.onTreeDirty(parent);
  await files.revealRel(to);
}
