/* ---------------------------------------------------------------------------
   The open of a FILES tree entry (`openFile` and `fileSearchFindTerm` of
   wb-files.ts): a note opens as a card, a binary file is refused, and any
   other file opens as a tab, on the first hit when the CONTENT search is on.
   The copy of its path (`copyPath`) is here too.

   Each step takes the files component as `files`.
   --------------------------------------------------------------------------- */
import { classify, fullPath } from "./wb-file-paths.ts";
import type { AlpineMagics } from "./wb-alpine.ts";
import type { Hit } from "./wb-file-search.ts";

/** What an open reads or calls on the files component. */
type OpenFiles = {
  $store: AlpineMagics["$store"];
  fileSearch: { open: boolean; mode: string; query: string; hits: Hit[] };
  relPath(node: WunderbaumNode | null): string;
  nodeDetail(node: WunderbaumNode): WorkbenchNodeDetail;
  fileSearchFindTerm(): string | null;
};

export function openEntry(files: OpenFiles, node: WunderbaumNode) {
  const path = files.relPath(node);
  const ftype = classify(node.title);
  window.WB.emit("open", { ...files.nodeDetail(node), ftype });
  // A note opens as a CARD, not as a tab (ADR-0064 §11): on the stage if it is
  // not there yet, and by a jump if it is.
  if (ftype === "note") {
    files.$store.tabs.openNote(path);
    return;
  }
  // No tree is mounted without an open project.
  const slug = files.$store.projects.openSlug;
  if (!slug) return;
  if (ftype === "binary") {
    // Flash it too: a click that silently does nothing reads as a broken
    // tree.
    window.WB.emit("open-refused", { project: slug, path, reason: "binary" });
    files.$store.modals.flash?.("Cannot open binary files.");
    return;
  }
  // Out of a CONTENT search: the tab lands on the first occurrence
  // (ADR-0036 amendment 2026-09-15).
  const find = files.fileSearchFindTerm();
  files.$store.tabs.openTab({ project: slug, path, title: node.title, ftype, find });
}

// The term to land on: the live query, only while the CONTENT filter is on.
export function searchFindTerm(files: OpenFiles) {
  const fs = files.fileSearch;
  if (!fs.open || fs.mode !== "content" || !fs.hits.length) return null;
  const q = String(fs.query ?? "").trim();
  return q || null;
}

// `full` joins the project's absolute `root` onto the rel path in the
// ROOT's own separator, so it pastes into a native shell; the rel path
// when no root is known. `navigator.clipboard` is undefined on an
// insecure non-loopback origin, so the call is optional-chained.
export function copyEntryPath(files: OpenFiles, node: WunderbaumNode, full: boolean) {
  const rel = files.relPath(node);
  const root = full
    ? files.$store.projects.projects.find((p) => p.slug === files.$store.projects.openSlug)?.root
    : "";
  const path = root ? fullPath(root, rel) : rel;
  navigator.clipboard?.writeText(path).catch(() => {});
  window.WB.emit("copy-path", { ...files.nodeDetail(node), path });
}
