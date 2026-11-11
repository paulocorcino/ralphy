/* ---------------------------------------------------------------------------
   The reveal of a FILES tree entry (`revealRel` of wb-files.ts): every
   ancestor folder opens, shallow-first, and the entry becomes the active node.

   It takes the files component as `files` and reads the tree only through
   `files.rawTree()`.
   --------------------------------------------------------------------------- */

/** What a reveal reads or writes on the files component. */
type RevealFiles = {
  _revealSeq: number;
  _revealedRel: string | null;
  rawTree(): WunderbaumTree | null;
  relPath(node: WunderbaumNode | null): string;
};

// Expand every ancestor folder of `rel`, shallow-first. A revealed FILE has
// nothing to expand. False when an ancestor is not mounted: nothing to reveal.
async function expandAncestors(
  tree: WunderbaumTree,
  rel: string,
  relPath: (n: WunderbaumNode) => string,
) {
  const parts = rel.split("/");
  for (let i = 1; i < parts.length; i++) {
    const prefix = parts.slice(0, i).join("/");
    const f = tree.findFirst((n: WunderbaumNode) => relPath(n) === prefix);
    if (!f) return false;
    if (!f.expanded) await f.setExpanded(true);
  }
  return true;
}

export async function reveal(files: RevealFiles, rel: string, opts: { restore?: boolean }) {
  const tree = files.rawTree();
  if (!tree || typeof rel !== "string" || rel === "") return null;
  if (opts.restore !== true) {
    files._revealSeq = (files._revealSeq || 0) + 1;
    files._revealedRel = rel;
  }
  if (!(await expandAncestors(tree, rel, (n) => files.relPath(n)))) return null;
  const node = tree.findFirst((n: WunderbaumNode) => files.relPath(n) === rel);
  if (!node) return null;
  node.setActive();
  return node;
}
