/* ---------------------------------------------------------------------------
   The level cache of the FILES tree (`loadTreeLevel`, `fetchTreeLevel`,
   `revalidateLevel` and their helpers in wb-files.ts): one directory level
   read from `tree.list`, painted from memory when it was read before and
   read again in the background, and the rules that keep the memory honest.

   Each step takes the files component as `files`. The tree is read and
   changed only through `files.rawTree()`: `this` of the component is Alpine's
   Proxy, and a change made through the Proxy leaves rows at stale offsets.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBReadRetry } from "./wb-read-retry.ts";
import type { AlpineMagics } from "./wb-alpine.ts";

/** The collections `treeMem` creates on the files component. */
export type TreeMemory = {
  cache: Map<string, TreeEntry[]>;
  validated: Set<string>;
  expanded: Map<string, string[]>;
  hits: Map<string, number | true>;
};

/** What a level step reads, writes or calls on the files component. */
type LevelFiles = {
  $store: AlpineMagics["$store"];
  _tree: WunderbaumTree | null;
  _treeCache: Map<string, TreeEntry[]> | null;
  _treeValidated: Set<string> | null;
  rawTree(): WunderbaumTree | null;
  relPath(node: WunderbaumNode | null): string;
  isFolder(node: WunderbaumNode | null | undefined): boolean;
  fileIcon(title: string): string;
  treeMem(): TreeMemory;
  treeKey(rel: string): string;
  treeFresh(): void;
  treeWentStale(err: unknown): void;
  useDaemonTree(): boolean;
  findFolderByRel(rel: string): WunderbaumNode | null;
  fetchTreeLevel(rel: string): Promise<WunderbaumSource[]>;
  pruneTreeCache(rel: string, entries: TreeEntry[]): void;
  forgetValidatedBelow(rel: string): void;
  revalidateLevel(rel: string): Promise<unknown>;
  reconcileLevel(node: WunderbaumNode, rel: string): Promise<void>;
};

// One directory level from `tree.list`, folders lazy. Cache-FIRST
// (stale-while-revalidate): a level read once is painted from memory and
// re-read in the BACKGROUND, touching the DOM only when the directory
// changed. Nothing is cached on the daemon (ADR-0036); the revalidation
// is not optional, since the watch is dropped when the project closes.
export function loadTreeLevel(files: LevelFiles, rel: string): Promise<WunderbaumSource[]> {
  const { cache, validated } = files.treeMem();
  const key = files.treeKey(rel);
  const hit = cache.get(key);
  if (!hit) return files.fetchTreeLevel(rel);
  // Revalidate once per level per open, deferred so the paint happens first.
  if (!validated.has(key)) {
    validated.add(key);
    setTimeout(() => files.revalidateLevel(rel), 0);
  }
  return Promise.resolve(treeNodes(files, hit));
}

// The un-cached read, always fresh: a reconcile served from the cache it
// corrects would validate itself. INVARIANT: a read that FAILED throws; it
// does NOT resolve `[]`, which is the real statement "no entries" every
// caller acts on by replacing what is on screen.
export function fetchTreeLevel(files: LevelFiles, rel: string): Promise<WunderbaumSource[]> {
  const { cache, validated } = files.treeMem();
  const key = files.treeKey(rel);
  const payload = WBDaemon.withCheckout(
    { repo: files.$store.projects.openSlug, path: rel },
    files.$store.checkouts.checkoutOf(files.$store.projects.openSlug),
  );
  return WBDaemon.observe("tree.list", payload).then((reply) => {
    if (!reply || reply.status !== "ok" || !Array.isArray(reply.entries)) {
      throw new WBReadRetry.Refused(WBFail.message(reply, ""));
    }
    files.treeFresh();
    files.pruneTreeCache(rel, reply.entries);
    cache.set(key, reply.entries);
    validated.add(key);
    return treeNodes(files, reply.entries);
  });
}

// Evict every remembered level the fresh listing of `rel` CONTRADICTS: a
// cache key is a path, not an identity, so a renamed-then-recreated
// directory would inherit the old one's children. A name no longer among
// the subdirectories cannot have children, so its remembered subtree goes.
export function pruneTreeCache(files: LevelFiles, rel: string, entries: TreeEntry[]) {
  const { cache, validated } = files.treeMem();
  const dirs = new Set(entries.filter((en) => en.dir).map((en) => en.name));
  // The keys DESCENDING from `rel`; its own key (empty `child`) is what
  // this listing replaces.
  const prefix = files.treeKey(rel === "" ? "" : `${rel}/`);
  for (const key of [...cache.keys()]) {
    if (!key.startsWith(prefix)) continue;
    const child = key.slice(prefix.length).split("/")[0];
    if (!child || dirs.has(child)) continue;
    cache.delete(key);
    validated.delete(key);
  }
}

// Mark every cached level BELOW `rel` as not validated, so the next
// `loadTreeLevel` of each paints from the cache and re-reads it.
export function forgetValidatedBelow(files: LevelFiles, rel: string) {
  const { validated } = files.treeMem();
  const prefix = files.treeKey(rel === "" ? "" : `${rel}/`);
  const own = files.treeKey(rel);
  for (const key of [...validated]) {
    if (key !== own && key.startsWith(prefix)) validated.delete(key);
  }
}

// Daemon entries → fresh Wunderbaum node specs, rebuilt on every call: the
// tree OWNS and mutates the objects it is given. An ignored entry carries
// `wb-ignored`, set once when the row is created. An older peer sends no
// `ignored`, and its rows are simply not dimmed.
function treeNodes(files: LevelFiles, entries: TreeEntry[]): WunderbaumSource[] {
  return entries.map((en) => {
    const node: WunderbaumSource = en.dir
      ? { title: en.name, folder: true, lazy: true }
      : { title: en.name, icon: files.fileIcon(en.name) };
    if (en.ignored) node.classes = "wb-ignored";
    return node;
  });
}

// The node whose children are the level `rel`: the root for "", else its folder.
function levelNode(files: LevelFiles, rel: string): WunderbaumNode | null {
  if (rel !== "") return files.findFolderByRel(rel);
  return files.rawTree()?.root ?? null;
}

// Re-read a level painted from cache and reconcile ONLY if the directory
// changed: the common case costs one read and zero DOM work.
export function revalidateLevel(files: LevelFiles, rel: string) {
  if (!files._tree || !files.useDaemonTree()) return Promise.resolve();
  const { cache } = files.treeMem();
  const key = files.treeKey(rel);
  const before = JSON.stringify(cache.get(key) ?? null);
  const slug = files.$store.projects.openSlug;
  return files.fetchTreeLevel(rel)
    .then(() => {
      // A project switch in flight: another project's tree.
      if (slug !== files.$store.projects.openSlug || !files._tree) return;
      if (JSON.stringify(cache.get(key) ?? null) === before) return;
      const node = levelNode(files, rel);
      if (!node) return;
      return files.reconcileLevel(node, rel);
    })
    // A dropped read leaves the cached level on screen, but says so.
    .catch((err: unknown) => files.treeWentStale(err));
}

// Whether the rows under `node` already show `specs` (from `treeNodes`):
// the same names, kinds and ignore marks, in the same order. A level still
// loading has a status row, so it never matches.
export function levelShows(files: LevelFiles, node: WunderbaumNode, specs: WunderbaumSource[]) {
  const rows = node.children;
  if (!Array.isArray(rows) || rows.length !== specs.length) return false;
  return specs.every((spec, i) => {
    const row = rows[i];
    return (
      row !== undefined &&
      row.title === spec.title &&
      files.isFolder(row) === !!spec.folder &&
      !!row.hasClass?.("wb-ignored") === (spec.classes === "wb-ignored")
    );
  });
}

// An unchanged level with a `.gitignore` may still have changed the ignore
// marks of the levels below it: re-read each expanded one, which repaints
// only a level whose listing changed.
export function revalidateBelow(files: LevelFiles, node: WunderbaumNode, rel: string) {
  files.forgetValidatedBelow(rel);
  node.visit((n: WunderbaumNode) => {
    if (files.isFolder(n) && n.expanded) void files.revalidateLevel(files.relPath(n));
  });
}
