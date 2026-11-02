/* ---------------------------------------------------------------------------
   The FILES panel and the peer of the open project (`openPeerDown`,
   `peerDownAct` and `filesFollowFleet` of wb-files.ts): the fleet group that
   the fleet calls down, the act that wakes it, and the read of the levels
   when it is back.

   Each step takes the files component as `files`. FILES takes the fact of the
   peer from the fleet, its owner.
   --------------------------------------------------------------------------- */
import { WBFleet } from "./wb-fleet.ts";
import type { AlpineMagics } from "./wb-alpine.ts";
import type { Group } from "./wb-fleet.ts";

/** What a peer step reads, writes or calls on the files component. */
type PeerFiles = {
  $store: AlpineMagics["$store"];
  _tree: WunderbaumTree | null;
  _filesPeerDown: string | null;
  openPeerDown(): Group | null;
  treeFresh(): void;
  revalidateLevel(rel: string): Promise<void>;
  rawTree(): WunderbaumTree | null;
  isFolder(node: WunderbaumNode | null | undefined): boolean;
  relPath(node: WunderbaumNode | null): string;
};

// The fleet group of the open project's peer while the fleet calls that
// peer down, else null.
export function peerDownGroup(files: PeerFiles) {
  const daemon = WBFleet.refDaemon(files.$store.projects.openSlug || "");
  if (!daemon) return null;
  const group = files.$store.projects.fleetGroups().find((g) => g.daemon === daemon);
  return group && !WBFleet.available(group) ? group : null;
}

export function wakePeerDown(files: PeerFiles) {
  const g = files.openPeerDown();
  if (!g) return;
  if (WBFleet.wakeable(g)) void files.$store.projects.wakePeer(g.daemon);
  else void files.$store.projects.readFleetNow();
}

// After each good fleet read: the read that calls the open project's peer
// back reads again the levels the tree shows.
export function followFleet(files: PeerFiles) {
  const down = !!files.openPeerDown();
  if (
    !down &&
    files._filesPeerDown &&
    files._filesPeerDown === files.$store.projects.openSlug &&
    files._tree
  ) {
    files.treeFresh();
    void files.revalidateLevel("");
    files.rawTree()?.root.visit((n: WunderbaumNode) => {
      if (files.isFolder(n) && n.expanded) void files.revalidateLevel(files.relPath(n));
    });
  }
  files._filesPeerDown = down ? files.$store.projects.openSlug : null;
}
