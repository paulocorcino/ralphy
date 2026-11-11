/* ---------------------------------------------------------------------------
   The peer rows and the peer update of the Alpine store `projects`
   (wb-projects-store.ts): the rows a `/api/fleet` read makes, the sentences
   of a failed wake or fleet read, and the update of a peer.

   Each action takes the store as `store`, and calls only its own methods, so
   the store stays the one writer of its fields.
   --------------------------------------------------------------------------- */
import { apiFetch } from "./wb-api.ts";
import { sendWindow } from "./wb-events.ts";
import { WBFail } from "./wb-fail.ts";
import { WBFleet } from "./wb-fleet.ts";
import * as WBHosts from "./wb-hosts.ts";
import { WBProject } from "./wb-project.ts";
import type { Project } from "./wb-project.ts";
import type { Messages } from "./wb-messages.ts";
import type { ApiResponse, FleetRepoRow } from "./wb-api.ts";
import type { FleetPeer, ReadState } from "./wb-shell-types.d.ts";

// Run one `host` verb to its end. Resolves to null when it succeeded, else to
// the sentence that says why it failed: the CLI's own failure line when it
// printed one.
export function runHostVerb(verb: string, payload: Record<string, string>): Promise<string | null> {
  return new Promise((resolve) => {
    let s = WBHosts.initial();
    window.WBDaemon.spawn(verb, payload, (st) => {
      if (st.status === "output") {
        const fed = WBHosts.feed(s.buf, st.chunk);
        s = Object.assign({}, s, { buf: fed.rest });
        for (const event of fed.events) s = WBHosts.next(s, { type: "event", event });
      } else if (st.status === "exited") {
        resolve(
          st.code === 0
            ? null
            : (s.failure?.message ?? "") || `The command ended with code ${st.code}.`,
        );
      } else {
        resolve(WBFail.failed(st, "The daemon did not start the command."));
      }
    });
  });
}

// The live state a local row keeps when `/api/repos` is read again: the dot
// stays until `refreshLive` answers, and an offline row that answers is idle.
export function carriedState(reachable: boolean, prev: Project | undefined) {
  if (!reachable) return "offline";
  return prev?.state === "offline" ? "idle" : (prev?.state ?? "") || "idle";
}

// The environment label and the daemon name stay until `loadFleet` answers.
export function carriedEnv(prev: Project | undefined) {
  return prev?.env ?? "";
}

export function carriedDaemonName(prev: Project | undefined) {
  return prev?.daemonName ?? "";
}

// `/api/fleet` is the ONLY source of this daemon's own environment label and
// name; `mine` is its row of this daemon. The rows are written in place.
export function stampLocalRows(
  projects: Project[],
  mine: { environment?: string; os?: string; daemon_name?: string },
) {
  for (const p of projects) {
    p.env = mine.environment ?? "";
    p.os = mine.os ?? "";
    p.daemonName = mine.daemon_name ?? "";
  }
}

// The rows of a fleet read: this daemon's own (its facts stamp the local
// rows) and every peer's.
export function fleetRows(repos: FleetRepoRow[] | undefined) {
  const rows = Array.isArray(repos) ? repos : [];
  return { mine: rows.find((x) => x.local), peers: rows.filter((x) => !x.local).map(peerRow) };
}

// One peer's repo as a project row.
function peerRow(x: FleetRepoRow) {
  return {
    // `<daemon_id>/<slug>`: the same `owner/repo` on two daemons is two rows.
    key: x.key,
    slug: x.slug,
    name: x.name || x.slug,
    path: x.path || "",
    root: WBFleet.peerRoot(x),
    branch: x.branch ?? "",
    branches: [],
    // The peer's OWN working-tree facts, same classification as `loadRepos`.
    dirty: x.dirty === true,
    state: x.reachable ? "idle" : "offline",
    remote: WBProject.isGitHubRemote(x.remote) ? "github" : "local",
    remoteUrl: x.remote ?? "",
    tree: [],
    ...peerFacts(x),
  };
}

// What makes a row a peer row: the daemon that serves it and its state.
function peerFacts(x: FleetRepoRow) {
  return {
    daemon: x.daemon_id,
    daemonName: x.daemon_name || "",
    env: x.environment || "",
    os: x.os || "",
    peerState: x.peer_state || "",
  };
}

// Why a wake did not leave the peer usable: the daemon's own sentence names
// the environment and what is wrong.
export async function wakeRefusal(
  r: ApiResponse<"POST /api/fleet/nudge">,
  woke: { diagnosis?: string } | null,
) {
  const refusal = r.ok ? null : await r.json().catch(() => null);
  return WBFail.failed(
    { message: (woke?.diagnosis ?? "") || (refusal?.error ?? "") },
    "Could not wake the peer: the peer did not answer.",
  );
}

// Why a failed `/api/fleet` read failed: the daemon's own words when it
// answered, else that it did not.
export function fleetFailReason(e: unknown) {
  const message = e instanceof Error ? e.message : "";
  return message.startsWith("the daemon") ? message : "the daemon did not answer";
}

/** What an update reads, writes or calls on the store `projects`. */
type UpdateStore = {
  fleetPeers: FleetPeer[];
  updating: Record<string, boolean>;
  loadRepos(): Promise<void>;
};

// Install this computer's Ralphy on an SSH host that speaks another
// protocol, then add it again. The click is the operator's permission to
// install on the host (ADR-0067).
export async function updatePeer(store: UpdateStore, messages: Messages, daemonId: string) {
  const peer = store.fleetPeers.find((p) => p.daemon_id === daemonId);
  const destination = peer?.destination ?? "";
  if (peer === undefined || destination === "" || store.updating[daemonId] === true) return false;
  store.updating[daemonId] = true;
  try {
    const identity = peer.identity_file ?? "";
    const payload: Record<string, string> =
      identity === ""
        ? { destination, name: peer.name }
        : { destination, name: peer.name, identity };
    for (const verb of ["host.install", "host.add"]) {
      const failure = await runHostVerb(verb, payload);
      if (failure !== null) {
        messages.flash(`Could not update Ralphy on ${peer.name}. ${failure}`);
        return false;
      }
    }
    // `loadRepos`, not `loadFleet`: the latter CONCATENATES peer rows.
    await store.loadRepos();
    return true;
  } finally {
    delete store.updating[daemonId];
  }
}

/** What a fleet read reads, writes or calls on the store `projects`. */
type FleetStore = {
  projects: Project[];
  fleetPeers: FleetPeer[];
  fleetRead: ReadState;
  fleetError: string;
  fleetRejectNote: string;
  _fleetRows: Project[];
  _fleetSeq: number;
  fleetRejectText(peers: FleetPeer[]): string;
  setProjects(list: Project[]): void;
  shareProjectNames(): void;
  shareFleet(): void;
};

// The local fleet (ADR-0052 §5, #349): append every PEER's repos after the
// local `/api/repos` pass. INVARIANT: a `/api/fleet` failure leaves the LOCAL
// list exactly as it was.
export async function readFleet(store: FleetStore) {
  // Two project reads close together (a wake fires the visible tab and the
  // socket reopen) must not both append the peers: the newest read owns
  // the peer rows, and they replace, never add to, what is there.
  const seq = ++store._fleetSeq;
  const localRows = () => store.projects.filter((p) => (p.daemon ?? "") === "");
  try {
    const r = await apiFetch("GET /api/fleet");
    if (seq !== store._fleetSeq) return;
    if (r.status === 404) {
      // A daemon older than the fleet: a fleet of one, not a failure.
      store.fleetPeers = [];
      store.fleetError = "";
      store.fleetRejectNote = "";
      return;
    }
    if (!r.ok) throw new Error(`the daemon answered ${r.status}`);
    const fleet = await r.json();
    if (seq !== store._fleetSeq) return;
    store.fleetPeers = Array.isArray(fleet.peers) ? fleet.peers : [];
    store.fleetRejectNote = store.fleetRejectText(store.fleetPeers);
    const { mine, peers } = fleetRows(fleet.repos);
    if (mine) stampLocalRows(store.projects, mine);
    store._fleetRows = peers;
    store.setProjects(localRows().concat(store._fleetRows));
    store.shareProjectNames();
    store.shareFleet();
    // The files compare the peer of the open project with this read.
    sendWindow(window, "workbench:fleet-read");
    store.fleetRead = WBFail.readFold(store.fleetRead, { ok: true, value: true, at: Date.now() });
    store.fleetError = "";
  } catch (e) {
    if (seq !== store._fleetSeq) return;
    // After a good read the peers and their rows stay, marked not current
    // (ADR-0070 D3); `loadRepos` rebuilt the list without them.
    const reason = fleetFailReason(e);
    store.fleetRead = WBFail.readFold(store.fleetRead, { ok: false, reason, at: Date.now() });
    if (store.fleetRead.goodAt) {
      store.setProjects(localRows().concat(store._fleetRows));
      store.shareProjectNames();
    } else {
      store.fleetPeers = [];
    }
    store.fleetError = WBFail.notCurrent(store.fleetRead, (ms) =>
      window.Alpine.store("connection").fmtClock(ms),
    );
  }
}
