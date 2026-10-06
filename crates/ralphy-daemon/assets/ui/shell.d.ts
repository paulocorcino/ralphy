// The `shell()` members (app.js) that a component lists in `uses`, and the
// Alpine magics it calls (ADR-0073 D4, ADR-0075 D6). A component's `this` is
// typed with `Pick<Shell, …>` of its `uses`, so a name outside that list is a
// type error. A member is added here when a component first lists it.

/** A peer of `/api/fleet`, as the Hosts dialog reads it. */
type FleetPeer = {
  daemon_id: string;
  name: string;
  state: string;
  diagnosis?: string;
  destination?: string;
  identity_file?: string;
  /** Set when the peer is paired over SSH. */
  tunnel?: unknown;
  os?: string;
  environment?: string;
};

interface Shell {
  /** The `x-bind` object of a `.modal-scrim`: `path` names the open flag. */
  scrim(path: string, close: () => void): object;
  osOf(x: FleetPeer | null | undefined): string;
  peerIcon(group: object): string;
  peerFault(group: object): string;
  peerStateWord(state: string): string;
  fleetPeers: FleetPeer[];
  loadRepos(opts?: { git?: boolean }): Promise<void>;
  _flashAction(msg: string): void;
  hostRemoved(daemonId: string): void;
  /** The open project's slug, or null when no project is open. */
  openSlug: string | null;
  projectLabel(ref: string): string;
  /** Resolves true when the operator confirms. */
  askConfirm(opts: { title: string; message: string; confirmLabel: string }): Promise<boolean>;
}

interface AlpineMagics {
  $nextTick(callback?: () => void): Promise<void>;
}
