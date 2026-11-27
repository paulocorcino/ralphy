/* ---------------------------------------------------------------------------
   The selection of the file tree: Ctrl+click adds or removes a row (the
   tree does it), a plain click on a row and Escape drop it.
   --------------------------------------------------------------------------- */

function dropSelection(tree: WunderbaumTree): boolean {
  const selected = tree.getSelectedNodes();
  for (const node of selected) node.setSelected(false);
  return selected.length > 0;
}

/** A click on a row without Ctrl ends the selection that Ctrl+click built. */
export function selectionClick(e: WunderbaumClickEvent): void {
  if (!e.event.ctrlKey) dropSelection(e.tree);
}

/** Escape drops the selection; with nothing selected, the tree keeps the key. */
export function selectionKey(e: WunderbaumKeyEvent): false | undefined {
  if (e.event.key !== "Escape") return undefined;
  return dropSelection(e.tree) ? false : undefined;
}
