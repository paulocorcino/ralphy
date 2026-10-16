/* ---------------------------------------------------------------------------
   What the file tree does to open the Properties dialog: the menu row and the
   `Alt+Enter` key. Both send `workbench:properties-open` with the rel path of
   the row; the dialog (wb-properties-dialog.ts) does the rest.

   They take the two rules of the tree that name a row (`relPath`, `isFolder`),
   so `wb-files.ts` stays the one place that knows how a row is named.
   --------------------------------------------------------------------------- */
import { sendWindow } from "./wb-events.ts";

/** The tree's rules for naming a row. */
type Rows = {
  relPath(node: WunderbaumNode | null): string;
  isFolder(node: WunderbaumNode | null | undefined): boolean;
};

/** Show the properties of `node`; `null` is the empty space of the tree, which
 * is the project root, and its rel path is empty. */
function showProperties(rows: Rows, node: WunderbaumNode | null) {
  sendWindow(window, "workbench:properties-open", { path: rows.relPath(node), isFolder: rows.isFolder(node) });
}

/** The context menu row. It only reads, so the menu offers it everywhere,
 * inside `.git` and `.ralphy` too. */
export function propertiesItem(rows: Rows, node: WunderbaumNode | null) {
  return { label: "Properties", icon: "bi-info-circle", run: () => showProperties(rows, node) };
}

/** `Alt+Enter` on a row, as the file managers of the desktop have it. `false`
 * stops the tree's own handling of the key; `undefined` leaves every other key
 * to the tree. */
export function propertiesKey(rows: Rows, e: WunderbaumKeyEvent) {
  if (!e.node || !e.event.altKey || e.event.key !== "Enter") return undefined;
  showProperties(rows, e.node);
  return false;
}
