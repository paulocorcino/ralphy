/* ---------------------------------------------------------------------------
   The listeners of the FILES tree: the `workbench:*` events the shell sends,
   added once by `init()` of wb-files.ts.
   --------------------------------------------------------------------------- */
import type { TreeSub } from "./wb-files-mount.ts";

// Where a Promise nobody awaits reports its failure: the console, tagged.
export const logFailure = (what: string) => (err: unknown) => {
  // oxlint-disable-next-line no-console -- the failure is for a developer
  console.error(`[workbench] ${what}`, err);
};

/** What the listeners call on the files component. */
type ListenFiles = {
  _treeSub: TreeSub | null;
  filesFollowProject(): void;
  filesFollowFleet(): void;
  filesFollowWake(daemon: string): void;
  filesFollowCheckout(): void;
  openFileSearch(): void;
  performMove(from: string, to: string): Promise<unknown>;
  onTreeDirty(rel: string): unknown;
  revealRel(rel: string): Promise<unknown>;
};

export function listenFiles(files: ListenFiles) {
  window.addEventListener("workbench:project-changed", () => {
    files.filesFollowProject();
  });
  window.addEventListener("workbench:fleet-read", () => {
    files.filesFollowFleet();
  });
  window.addEventListener("workbench:peer-woken", (e) => {
    files.filesFollowWake(e.detail.daemon);
  });
  window.addEventListener("workbench:checkout-changed", () => {
    files.filesFollowCheckout();
  });
  // The tab became visible, or a login: the socket sends what it missed.
  window.addEventListener("workbench:panels-reread", () => files._treeSub?.replay());
  // `/` and Ctrl/Cmd+Shift+F (`wire()` in app.ts).
  window.addEventListener("workbench:file-search-open", () => {
    files.openFileSearch();
  });
  window.addEventListener(
    "workbench:move-confirmed",
    (e) => void files.performMove(e.detail.from, e.detail.to),
  );
  // A create or a delete of the write seam (`wire()` in app.ts): re-list the
  // level, THEN reveal, so `setActive()` is the last write. `revealRel`
  // expands the ancestors: a nudge for a COLLAPSED dir is dropped, so a
  // nudge alone would leave a new entry invisible.
  window.addEventListener("workbench:tree-dirty", (e) => {
    Promise.resolve(files.onTreeDirty(e.detail.rel))
      .then(() => {
        const rel = e.detail.reveal ?? "";
        return rel === "" ? null : files.revealRel(rel);
      })
      .catch(logFailure("the tree did not refresh"));
  });
}
