/* ---------------------------------------------------------------------------
   Opening a file into a tab, for the Alpine store `tabs` (`fetchContent`,
   `openLink` and `openTab` of wb-tabs-store.ts): the `file.read` or
   `file.image` read and its refusals, a markdown link to another repo file,
   and the tab a file opens into.

   Each step takes the store as `store`, and calls only its own methods, so
   the store stays the one writer of its tabs.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { classify, fileTabId, tabIcon } from "./wb-file-paths.ts";
import type { CanvasTab, TabBody, TabOpen } from "./wb-shell-types.d.ts";

/** What opening a file reads or calls on the store `tabs`. */
type OpenTabs = {
  tabs: CanvasTab[];
  activate(id: string): void;
  showTab(id: string): void;
  closeTab(id: string): void;
  persistView(): void;
  syncViewer(): void;
  openNote(path: string, project?: string | null, checkout?: string | null): void;
  openTab(open: TabOpen): void;
  fetchContent(
    project: string,
    path: string,
    ftype: string,
    checkout: string | null | undefined,
  ): Promise<TabBody>;
};

// `file.read`, resolving to what the pane opens with: `{content, encoding,
// bom}` (ADR-0036 amendment 2026-09-22), or `{refused: reason}` when the
// daemon served nothing for a reason the pane can state (`binary`, `too
// large`, …) — the tab KEEPS its place and says so — or `null` when there
// is nothing to hold a tab open for (`not found`, a transport drop): the
// tab closes. `checkout` is the tab's PINNED checkout (#406).
export function fetchContent(
  store: OpenTabs,
  project: string,
  path: string,
  ftype: string,
  checkout: string | null | undefined,
): Promise<TabBody> {
  const refuse = (reason: string) => {
    window.WB.emit("open-refused", { project, path, reason });
    window.Alpine.store("modals").flash?.(
      WBFail.failed({ reason }, "Could not open the file: the daemon gave no reason."),
    );
    if (reason === "not found" || reason === "transport") {
      store.closeTab(fileTabId(project, path, checkout));
      return null;
    }
    return { refused: reason };
  };
  // An image is `file.image` (ADR-0049): a `data:` URL. Same refusal shape.
  if (ftype === "image") {
    let refusal: TabBody = null;
    return WBDaemon.readImage(project, path, (reason) => (refusal = refuse(reason)), checkout)
      .then((url) => (url == null ? refusal : { content: url }))
      .catch(() => refuse("transport"));
  }
  return WBDaemon.observe("file.read", WBDaemon.withCheckout({ repo: project, path }, checkout))
    .then((reply) => {
      if (reply.status === "ok") {
        return { content: reply.content, encoding: reply.encoding, bom: reply.bom };
      }
      return refuse(WBFail.message(reply, "refused"));
    })
    .catch(() => refuse("transport"));
}

// A markdown link to another repo file: same viewer choice and binary
// refusal as a tree click. `checkout` is the SOURCE pane's pin (#406): a
// link in a worktree's document names that worktree's file.
// `as: "bytes"` is the ONE caller that refuses the note routing below: a
// `.note` whose bytes are not a container (ADR-0064 §11) must not become a
// card, and classifying it again would be the loop the card just escaped.
// The viewer serves text and images and refuses anything else, so what is
// honest here is the refusal, named — not a pane that would show nothing.
export function openLink(store: OpenTabs, { project, path, fragment, checkout, as }: OpenRequest) {
  const title = path.split("/").pop() ?? path;
  if (as === "bytes") {
    window.WB.emit("open-refused", { project, path, reason: "not a note" });
    window.Alpine.store("modals").flash?.(`${path} is not a note.`);
    return;
  }
  const ftype = classify(title);
  // A note linking to a note lands on the plane, exactly as a double-click
  // in the explorer does.
  if (ftype === "note") {
    store.openNote(path, project, checkout);
    return;
  }
  if (ftype === "binary") {
    window.WB.emit("open-refused", { project, path, reason: "binary" });
    window.Alpine.store("modals").flash?.("Cannot open binary files.");
    return;
  }
  if (project === null) return;
  store.openTab({ project, path, title, ftype, fragment, checkout });
}

// `content`: a re-attached popup passes its (possibly edited) bytes back.
// `fragment`/`find`: a `#heading` or search term to land on once the bytes
// are shown. `checkout` PINS the tab to the tree it was opened in (#406):
// a Save from a tab showing worktree bytes must never land on the primary.
// `encoding`/`bom` come back with a re-attached popup's bytes, so the
// reattached pane saves the way the detached one would have.
export function openTab(
  store: OpenTabs,
  { project, path, title, ftype, content, fragment, find, checkout, encoding, bom }: TabOpen,
) {
  const ck =
    checkout !== undefined ? checkout : window.Alpine.store("checkouts").checkoutOf(project);
  const id = fileTabId(project, path, ck);
  if (store.tabs.some((t) => t.id === id)) {
    store.activate(id);
    if (fragment) WBViewer.jumpTo(id, fragment);
    if (find) WBViewer.find(id, find);
    return;
  }
  store.tabs.push({
    id,
    kind: ftype,
    title,
    path,
    project,
    icon: tabIcon(ftype),
    closable: true,
    checkout: ck,
  });
  store.showTab(id);
  store.persistView();
  void window.Alpine.nextTick(() => {
    // A re-attach passes bytes in; a fresh open reads `file.read`.
    const bytes: Promise<TabBody> =
      content != null
        ? Promise.resolve({ content, encoding, bom })
        : store.fetchContent(project, path, ftype, ck);
    void bytes.then((body) => {
      if (body == null) return; // nothing to show: fetchContent closed the tab
      WBViewer.open({
        id,
        project,
        label: window.Alpine.store("projects").projectLabel(project),
        path,
        ftype,
        checkout: ck,
        ...body,
      });
      // NOT `setActive(id)`: `restoreView` opens N tabs in one burst and
      // THEN activates the stored one, so the last read to answer must not
      // own the screen.
      store.syncViewer();
      if (fragment) WBViewer.jumpTo(id, fragment);
      if (find) WBViewer.find(id, find);
    });
  });
}
