// The typing in the FILES search field (`fileSearchTyped` of wb-files.ts):
// a keystroke arms the debounce, and a query under the floor clears instead.
import { WBFileSearch } from "./wb-file-search.ts";

/** What the typing reads, writes or calls on the files component. */
type TypingFiles = {
  fileSearch: { query: string; seq: number };
  _fileSearchTimer: ReturnType<typeof setTimeout> | null;
  clearFileSearch(): Promise<void>;
  fileSearchNow(): Promise<void>;
};

export function typedSearch(files: TypingFiles) {
  clearTimeout(files._fileSearchTimer);
  if (!WBFileSearch.worthSearching(files.fileSearch.query)) {
    files.fileSearch.seq++;
    void files.clearFileSearch();
    return;
  }
  files._fileSearchTimer = setTimeout(() => files.fileSearchNow(), WBFileSearch.DEBOUNCE_MS);
}
