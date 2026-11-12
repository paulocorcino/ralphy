/* ---------------------------------------------------------------------------
   What the FILES tree does when the daemon nudges (`onHeadMoved`,
   `onTreeDirty` and `refreshOpenViewers` of wb-files.ts): the head moved, a
   directory changed, and the open tabs in that directory read their bytes
   again.

   Each step takes the files component as `files`.
   --------------------------------------------------------------------------- */
import { sendWindow } from "./wb-events.ts";
import type { AlpineMagics } from "./wb-alpine.ts";
import type { CanvasTab } from "./wb-shell-types.d.ts";

/** What a nudge step reads, writes or calls on the files component. */
type NudgeFiles = {
  $store: AlpineMagics["$store"];
  _treeCheckout: string | null;
  _headTimer: ReturnType<typeof setTimeout> | null;
  HEAD_SETTLE_MS: number;
  rawTree(): WunderbaumTree | null;
  findFolderByRel(rel: string): WunderbaumNode | null;
  reconcileLevel(node: WunderbaumNode, rel: string): Promise<void>;
  treeWentStale(err: unknown): void;
  refreshOpenViewers(rel: string): Promise<void | void[]>;
  useDaemonTree(): boolean;
};

// The bytes of the open tab `t` of `project` as they are on disk now, or null
// when the read is refused. An image tab re-reads through its OWN verb
// (ADR-0049 §1). A text tab re-reads with ITS encoding as the hint: the
// encoding is sticky once opened, so a nudge never silently re-detects it.
function freshBytes(t: CanvasTab, project: string): Promise<string | null | undefined> {
  const path = t.path;
  if (path === undefined) return Promise.resolve(null);
  if (t.kind === "image") return WBDaemon.readImage(project, path, undefined, t.checkout);
  const payload = WBDaemon.withCheckout({ repo: project, path }, t.checkout);
  const enc = WBViewer.encodingOf(t.id)?.encoding ?? "";
  if (enc !== "") payload["encoding"] = enc;
  return WBDaemon.observe("file.read", payload).then((reply) =>
    reply.status === "ok" ? reply.content : null,
  );
}

async function showFreshBytes(t: CanvasTab, project: string) {
  const content = await freshBytes(t, project);
  if (content != null) WBViewer.externalChange(t.id, content);
}

// A `head.dirty` push: the open checkout's HEAD moved (a switch, a commit).
// Git re-reads what the branch drives on `workbench:head-moved`. The gitdir
// and its `logs/` push together for one move, so a short trailing timer
// makes them one read.
export function headMoved(files: NudgeFiles) {
  clearTimeout(files._headTimer);
  files._headTimer = setTimeout(() => {
    const ref = files.$store.projects.openSlug;
    if (ref == null || ref === "") return;
    sendWindow(window, "workbench:head-moved", { ref });
  }, files.HEAD_SETTLE_MS);
}

// A `tree.dirty` nudge for `rel`: refetch that level IF it is on screen. A
// nudge for a collapsed/absent dir is DROPPED (ADR-0036 §4).
export function treeDirty(files: NudgeFiles, rel: string) {
  const tree = files.rawTree();
  if (!tree) return Promise.resolve();
  const node = rel === "" ? tree.root : files.findFolderByRel(rel);
  if (!node) return Promise.resolve(); // not in the tree → invisible, drop
  if (rel !== "" && !node.expanded) return Promise.resolve(); // collapsed → invisible, drop
  // Reconcile in place, then freshen the open tabs in this directory. A
  // reconcile failure leaves every row in place (`_reconcileOnce` resolves
  // the listing BEFORE touching the tree) and must still refresh viewers.
  return files
    .reconcileLevel(node, rel)
    .catch((err: unknown) => {
      files.treeWentStale(err);
    })
    .then(() => files.refreshOpenViewers(rel));
}

const dirOf = (p: string | undefined) => {
  if (typeof p !== "string") return null;
  const i = p.lastIndexOf("/");
  return i < 0 ? "" : p.slice(0, i);
};

// After a directory nudge, re-read any open tab whose file lives in `rel`
// and push the bytes to its viewer. A failure keeps the tab's bytes.
export function openViewersRefresh(files: NudgeFiles, rel: string): Promise<void | void[]> {
  if (!files.useDaemonTree()) return Promise.resolve();
  const reads: Promise<void>[] = [];
  for (const t of files.$store.tabs.tabs) {
    if (t.project !== files.$store.projects.openSlug || dirOf(t.path) !== rel) continue;
    // A tab pinned to another checkout (#406) is not this nudge's.
    if ((t.checkout ?? null) !== (files._treeCheckout ?? null)) continue;
    reads.push(showFreshBytes(t, t.project).catch(() => {}));
  }
  // The settled batch, so a caller can await a fully-refreshed set.
  return Promise.all(reads);
}
