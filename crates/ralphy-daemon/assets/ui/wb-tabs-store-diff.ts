/* ---------------------------------------------------------------------------
   Opening a Changes row into a diff tab, for the Alpine store `tabs`
   (`openDiff` of wb-tabs-store.ts): the refusal of a new folder, the image
   or binary row that opens as a file tab or is refused, and the two sides
   of the diff read from the daemon.

   Each step takes the store as `store`, and calls only its own methods, so
   the store stays the one writer of its tabs.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBChanges } from "./wb-changes.ts";
import { classify } from "./wb-file-paths.ts";
import type { Messages } from "./wb-messages.ts";
import type { CanvasTab, ChangeEntry, DiffTarget, TabOpen } from "./wb-shell-types.d.ts";

/** What opening a diff reads or calls on the store `tabs`. */
type DiffTabs = {
  tabs: CanvasTab[];
  activate(id: string): void;
  showTab(id: string): void;
  closeTab(id: string): void;
  syncViewer(): void;
  openTab(open: TabOpen): void;
};

// HEAD on one side, the working tree on the other (#311). Read-only;
// Monaco computes the diff, nothing produces a patch.
export function openDiff(store: DiffTabs, messages: Messages, project: string, entry: ChangeEntry) {
  // git lists a new folder as one row whose path ends with "/": it has
  // no text to compare, and a diff of it was an empty tab.
  if (entry.path.endsWith("/")) {
    messages.flash(`${entry.path} is a new folder. Open its files in Files.`);
    return;
  }
  // Both diff sides read text, and a binary side closes the tab again: an
  // image or other binary path never reaches the diff.
  const ftype = classify(entry.path.split("/").pop() ?? entry.path);
  if (ftype === "image" || ftype === "binary") {
    openChangedBinary(store, project, entry, ftype);
    return;
  }
  // Pinned to the selection at open (#407): both sides read `t.checkout`.
  const t = WBChanges.diffTarget(
    entry,
    project,
    window.Alpine.store("checkouts").checkoutOf(project),
  );
  if (store.tabs.some((x) => x.id === t.id)) {
    store.activate(t.id);
    return;
  }
  store.tabs.push({
    id: t.id,
    kind: "diff",
    title: t.title,
    path: t.workingPath,
    project,
    checkout: t.checkout,
    icon: "bi bi-file-earmark-diff",
    closable: true,
  });
  store.showTab(t.id);
  window.WB.emit("open-diff", { project, path: t.workingPath, checkout: t.checkout });
  void window.Alpine.nextTick(() => {
    // Latched: a path refused on BOTH sides would flash twice.
    let refused = false;
    const refuse = (reason: string) => {
      if (refused) return null;
      refused = true;
      window.Alpine.store("modals").flash?.(
        WBFail.failed({ message: reason }, "Could not open the diff: the daemon gave no reason."),
      );
      store.closeTab(t.id);
      return null;
    };
    Promise.all([diffHeadSide(project, t, refuse), diffWorkSide(project, t, refuse)])
      .then(([head, work]) => {
        // A refusal on EITHER side aborts: half a diff reads as "no changes".
        if (head == null || work == null) return;
        // Closed during the round trips: mounting now would leak a pane
        // with no tab to close it.
        if (!store.tabs.some((x) => x.id === t.id)) return;
        WBViewer.open({
          id: t.id,
          project,
          checkout: t.checkout,
          label: window.Alpine.store("projects").projectLabel(project),
          path: t.workingPath,
          ftype: "diff",
          content: work,
          original: head,
        });
        // As `openTab`: the pane follows the CURRENT active tab.
        store.syncViewer();
      })
      .catch(() => refuse("transport"));
  });
}

// A Changes row that names an image or other binary. An image shows its
// working copy in the image pane (ADR-0049); a deleted one has no working
// copy, and `blob.read` serves text only, so it is refused like a binary.
function openChangedBinary(store: DiffTabs, project: string, entry: ChangeEntry, ftype: string) {
  const path = entry.path;
  if (ftype === "binary") {
    window.WB.emit("open-refused", { project, path, reason: "binary" });
    window.Alpine.store("modals").flash?.("Cannot open binary files.");
    return;
  }
  if (entry.status === "deleted") {
    window.WB.emit("open-refused", { project, path, reason: "deleted" });
    window.Alpine.store("modals").flash?.("This image was deleted, so there is nothing to show.");
    return;
  }
  const title = path.split("/").pop() ?? path;
  store.openTab({
    project,
    path,
    title,
    ftype,
    checkout: window.Alpine.store("checkouts").checkoutOf(project),
  });
}

// The diff's HEAD side; an added/untracked path diffs against emptiness.
function diffHeadSide(project: string, t: DiffTarget, refuse: (reason: string) => null) {
  if (t.headAbsent) return Promise.resolve("");
  return WBDaemon.observe(
    "blob.read",
    WBDaemon.withCheckout({ repo: project, revision: "head", path: t.headPath }, t.checkout),
  ).then((reply) => {
    if (WBFail.isError(reply)) return refuse(WBFail.message(reply, "refused"));
    const blob = reply.blob || {};
    if (blob.status === "present") return blob.content;
    if (blob.status === "absent") return "";
    return refuse(blob.reason || "refused");
  });
}

// The diff's working side. `not found` is NOT a refusal: a stale row
// (deleted between the list and the click) diffs against emptiness.
function diffWorkSide(project: string, t: DiffTarget, refuse: (reason: string) => null) {
  if (t.workingAbsent) return Promise.resolve("");
  // NOT `fetchContent`: it collapses every refusal to `null` and closes
  // the `file:` tab id rather than this diff's.
  return WBDaemon.observe(
    "file.read",
    WBDaemon.withCheckout({ repo: project, path: t.workingPath }, t.checkout),
  ).then((reply) => {
    if (reply.status === "ok") return reply.content;
    const reason = WBFail.message(reply, "refused");
    return reason === "not found" ? "" : refuse(reason);
  });
}
