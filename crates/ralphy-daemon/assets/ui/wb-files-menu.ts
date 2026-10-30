/* ---------------------------------------------------------------------------
   The context menu of the file tree: the rows it offers for one row of the
   tree, or for its empty space, which is the project root.

   It takes the tree's own acts (open, copy, move, create, delete), so
   `wb-files.ts` stays the one place that does each of them, and this module
   only decides which rows a node gets.
   --------------------------------------------------------------------------- */
import { underProtectedDir } from "./wb-file-paths.ts";
import { propertiesItem } from "./wb-files-properties.ts";
import { pickFiles } from "./wb-files-upload.ts";
import type { MenuItem } from "./wb-shell-types.d.ts";

/** The acts of the tree the menu rows run. */
type MenuTree = {
  isFolder(node: WunderbaumNode | null | undefined): boolean;
  relPath(node: WunderbaumNode | null): string;
  openFile(node: WunderbaumNode): void;
  copyPath(node: WunderbaumNode, full?: boolean): void;
  duplicateNode(node: WunderbaumNode): Promise<void>;
  moveNode(node: WunderbaumNode): void;
  emitCreate(node: WunderbaumNode | null, kind: "file" | "folder"): void;
  nodeDetail(node: WunderbaumNode): WorkbenchNodeDetail;
} & Parameters<typeof pickFiles>[0];

/** The rows for `node`. `node` is null for empty tree space, which addresses
 * the repo root: the create items apply, the per-node items drop out. */
export function treeMenu(tree: MenuTree, node: WunderbaumNode | null): MenuItem[] {
  return [
    ...(node ? nodeRows(tree, node) : []),
    propertiesItem(tree, node),
    { sep: true as const },
    // Creating targets the folder itself, or the folder CONTAINING the
    // clicked file.
    { label: "New file…", icon: "bi-file-earmark-plus", run: () => tree.emitCreate(node, "file") },
    { label: "New folder…", icon: "bi-folder-plus", run: () => tree.emitCreate(node, "folder") },
    uploadRow(tree, node),
    node && { sep: true as const },
    node && {
      label: "Delete",
      icon: "bi-trash",
      danger: true,
      run: () => window.WB.emit("delete", tree.nodeDetail(node)),
    },
  ].filter(Boolean);
}

/** "Upload…" on a folder or on the project root, never inside `.git` or
 * `.ralphy`, where the daemon refuses every write. */
function uploadRow(tree: MenuTree, node: WunderbaumNode | null): MenuItem | false {
  if (node && !tree.isFolder(node)) return false;
  if (underProtectedDir(tree.relPath(node))) return false;
  return { label: "Upload…", icon: "bi-cloud-upload", run: () => pickFiles(tree, node) };
}

/** The rows that act on one row of the tree, above Properties. */
function nodeRows(tree: MenuTree, node: WunderbaumNode): MenuItem[] {
  const isFolder = tree.isFolder(node);
  return [
    !isFolder && { label: "Open", icon: "bi-box-arrow-up-right", run: () => tree.openFile(node) },
    { label: "Rename…", icon: "bi-pencil", run: () => node.startEditTitle() },
    // FLAT rows, not a submenu, for a two-item choice made constantly.
    { label: "Copy full path", icon: "bi-clipboard", run: () => tree.copyPath(node, true) },
    { label: "Copy relative path", icon: "bi-clipboard", run: () => tree.copyPath(node, false) },
    !isFolder && { label: "Duplicate", icon: "bi-files", run: () => tree.duplicateNode(node) },
    // Folders too. Dropped for a node inside `.git`/`.ralphy`, whose every
    // move the daemon refuses on the SOURCE.
    !underProtectedDir(tree.relPath(node)) && {
      label: "Move to…",
      icon: "bi-arrow-right-square",
      run: () => tree.moveNode(node),
    },
  ].filter(Boolean);
}
