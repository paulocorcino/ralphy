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
  /** The update can wake it through `wsl.exe`. */
  nudgeable?: boolean;
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
  /** The security fact; the Security dialog changes it only through `securityChanged`. */
  security: {
    tokenSet: boolean;
    passwordSet: boolean;
    totpEnrolled: boolean;
    requireLogin: boolean;
    remoteImages: boolean;
    policy: string;
  };
  securityChanged(patch: Partial<Shell["security"]>): void;
  probeSession(): Promise<void>;
  logOff(): Promise<void>;
  /** The release view (`WBRelease.read`). */
  release: any;
  readonly releaseSummary: string;
  releaseStale(): boolean;
  markReleaseSeen(): void;
  releaseWatchChanged(enable: boolean): void;
  refreshLive(): Promise<void>;
  /** A row of `/api/sessions`. */
  localSessions(): any[];
  peerSessions(daemonId: string): any[];
  identityMark(): string;
  loadFleet(): Promise<void>;
  /** The rows of `/api/repos`. */
  projects: any[];
  repoRef(p: any): string;
  toggle(ref: string, row?: any): void;
}

interface AlpineMagics {
  $nextTick(callback?: () => void): Promise<void>;
  $refs: Record<string, HTMLElement | undefined>;
}
