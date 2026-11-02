// The tree memory of the FILES component (`treeMem` of wb-files.ts): the
// collections the levels, the expansion and the search filter read.
import type { TreeMemory } from "./wb-files-levels.ts";

/** What `treeMemory` creates the collections on. */
type MemoryFiles = {
  _treeCache: Map<string, TreeEntry[]> | null;
  _treeValidated: Set<string> | null;
  _treeExpanded: Map<string, string[]> | null;
  _fileHits: Map<string, number | true> | null;
};

// Lazily create the three tree-memory collections (see the note in wb-files.ts).
export function treeMemory(files: MemoryFiles): TreeMemory {
  files._treeCache ||= new Map();
  files._treeValidated ||= new Set();
  files._treeExpanded ||= new Map();
  // path → count (content) or `true` (name): what the filter predicate reads.
  files._fileHits ||= new Map();
  return {
    cache: files._treeCache,
    validated: files._treeValidated,
    expanded: files._treeExpanded,
    hits: files._fileHits,
  };
}
