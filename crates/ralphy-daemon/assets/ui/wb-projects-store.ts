/* ---------------------------------------------------------------------------
   The open project and the list of projects: the Alpine store `projects`
   (ADR-0073 amendment of 2026-10-08, decision 3).

   The store is the one owner of this fact, and of the fleet, the agent
   roster and the live sessions (#621). Its fields change only through its
   own methods. `shell()` code reads `this.$store.projects`, the markup reads
   `$store.projects`, and code outside Alpine reads `Alpine.store("projects")`.

   It reads no `shell()` member and calls no component; it reads the store
   `connection`. Its `init()` reads the projects and the agents, hears
   `workbench:repos-reread` and `workbench:sessions-reread`, and starts the
   peer read. It tells the files that the fleet was read with
   `workbench:fleet-read`, and the consoles through `WBConsole`. The peer rows
   and the update of a peer are in wb-projects-fleet.ts.

   `main.ts` registers it as `projects`. `projectsStore` builds it without
   Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { WBAgents } from "./wb-agents.ts";
import { apiFetch } from "./wb-api.ts";
import { WBFail } from "./wb-fail.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { sendWindow } from "./wb-events.ts";
import {
  carriedDaemonName,
  carriedEnv,
  carriedState,
  readFleet,
  updatePeer,
  wakeRefusal,
} from "./wb-projects-fleet.ts";
import type { Messages } from "./wb-messages.ts";
import type { Project } from "./wb-project.ts";
import type { FleetPeer, Group, ReadState, RosterRow, Timer } from "./wb-shell-types.d.ts";

/** What `main.ts` gives the store. */
export type ProjectsStoreDeps = {
  /** The door for operator messages: a failed wake says why. */
  messages: Messages;
};

export function projectsStore(deps: ProjectsStoreDeps) {
  return {
    openSlug: null as string | null,
    projects: [] as Project[],
    // The open project's ref as the key of a per-project record. With no
    // project open the key is "null", which no project ref is, so a read
    // finds nothing.
    openKey() {
      return String(this.openSlug);
    },
    setOpen(slug: string | null) {
      this.openSlug = slug;
    },
    setProjects(list: Project[]) {
      this.projects = list;
    },
    // The git fields of one row: the branch acts (and their revert) and the
    // sync read set them. It takes the row the caller found, not its ref: a
    // revert after a new repo read lands on the old row, not on the new one.
    setRowGit(
      p: Project,
      git: { branch?: string | undefined; branches?: string[] | undefined; head?: Project["head"] },
    ) {
      if (git.branches !== undefined) p.branches = git.branches;
      if (git.branch !== undefined) p.branch = git.branch;
      if (git.head !== undefined) p.head = git.head;
    },
    repoRef(p: Project) {
      return WBFleet.repoRef(p);
    },
    // The open project's row, for the panels that reuse a per-row control.
    openProject() {
      return this.projects.find((p) => this.repoRef(p) === this.openSlug) ?? null;
    },
    openProjectBranch() {
      const row = this.projects.find((p) => this.repoRef(p) === this.openSlug);
      return (row?.branch ?? "") || "current";
    },
    // The branch chip lives on the Files bar (#332), which only the OPEN
    // project renders.
    rowOpen(p: Project) {
      return this.openSlug === this.repoRef(p);
    },
    // What every surface OUTSIDE the sidebar prints for a repo ref. A peer ref
    // is `<daemon_id>/<owner>/<repo>`: the ULID is how the fleet ROUTES
    // (ADR-0052 §5), so the environment is printed in its place. The ref itself
    // is untouched on the wire, the desk and the tab ids. Row lookup by
    // `repoRef`, not slug: the same `owner/repo` on two daemons is two rows.
    projectLabel(ref: string | null | undefined) {
      if (ref == null || ref === "") return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return WBFleet.refLabel(ref);
      const name = WBProject.projectName(row);
      return (row.daemon ?? "") !== "" && row.env ? `${name} · ${row.env}` : name;
    },
    // The tooltip twin of `projectLabel`: `owner/repo` or the full folder of
    // a remoteless repo, plus the environment of a peer.
    projectTitle(ref: string | null | undefined) {
      if (ref == null || ref === "") return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return WBFleet.refLabel(ref);
      const who = WBProject.projectTitle(row);
      return (row.daemon ?? "") !== "" && row.env ? `${who} · ${row.env}` : who;
    },

    // The adapter roster comes from `/api/agents`, never a list here (#304).
    agents: [] as ReturnType<typeof WBAgents.runRows>,
    roster: [] as RosterRow[],
    // A failed `/api/repos` (#202): a visible error. Cleared on login
    // (`clearReposError`), before the read again.
    reposError: "",
    // The read state of the shown facts this sidebar shows (ADR-0070 D3):
    // `WBFail.readFold` results, `null` before the first read.
    reposRead: null as ReadState,
    fleetRead: null as ReadState,
    fleetError: "",
    // The peers the daemon could not read: they list no project, so the
    // sidebar says so instead of showing an empty fleet (ADR-0070 D4).
    fleetRejectNote: "",
    sessionsRead: null as ReadState,
    liveSessions: [] as HostedSession[],
    // The peer rows of the last good `/api/fleet`, kept when a read fails.
    _fleetRows: [] as Project[],
    // The local fleet's peers (ADR-0052 §5, #349), from `/api/fleet`. Empty: a
    // fleet of one, or a daemon too old to serve the route.
    fleetPeers: [] as FleetPeer[],
    // Peers with a wake in flight, keyed by daemon_id: a second click sends no
    // second nudge.
    waking: {} as Record<string, boolean>,
    // SSH hosts with an update in flight, keyed by daemon_id.
    updating: {} as Record<string, boolean>,
    // A repo refresh in flight. The list does not refresh by itself; the
    // button picks up a new repo or a branch/dirty change.
    reposLoading: false,

    // Alpine calls this once, when `main.ts` registers the store.
    init() {
      void this.loadRepos();
      void this.loadAgents();
      // A push, a reopened socket or the tab coming back (the store
      // `connection`) asks to read these facts again.
      window.addEventListener(
        "workbench:repos-reread",
        (e) => void this.loadRepos({ git: e.detail.git }),
      );
      window.addEventListener("workbench:sessions-reread", () => void this.refreshLive());
      // A peer pushes neither its reachability nor its sessions, so they are
      // read every 30 s while a peer is listed and the tab is visible
      // (ADR-0070 D2 event 6). `loadRepos` reads the fleet and the sessions.
      this._peerTick = setInterval(() => {
        this.peerTick();
      }, this.PEER_READ_MS);
    },

    // A peer reads no git fact: the change set and the branch have their own
    // triggers.
    peerTick() {
      if (!window.Alpine.store("connection").tabHidden() && this.fleetPeers.length > 0) {
        void this.loadRepos({ git: false });
      }
    },
    PEER_READ_MS: 30000,
    _peerTick: null as Timer,

    // The daemon's adapter roster (#304). A failed fetch leaves the roster
    // EMPTY rather than showing adapters this daemon may not have.
    _agentsSeq: 0,
    async loadAgents(repo?: string | null) {
      if (repo === undefined) repo = this.openSlug;
      const seq = ++this._agentsSeq;
      try {
        const r = await apiFetch("GET /api/agents", {
          query: repo != null && repo !== "" ? { repo } : {},
        });
        if (!r.ok) throw new Error(`/api/agents ${r.status}`);
        const state = WBAgents.rosterState(await r.json(), repo);
        if (seq !== this._agentsSeq) return;
        this.roster = state.roster;
        this.agents = state.agents;
      } catch {
        if (seq !== this._agentsSeq) return;
        const state = WBAgents.rosterState([], repo);
        this.roster = state.roster;
        this.agents = state.agents;
      }
    },
    // Hydrate the accordion from the daemon's repo registry. `git: false`
    // skips the change set and the branch: a peer tick or a project-list push
    // says nothing about them (fact index: their own push, and a periodic
    // read only while the Changes panel is open).
    async loadRepos({ git = true } = {}) {
      this.reposLoading = true;
      try {
        const r = await apiFetch("GET /api/repos");
        if (r.ok) {
          const repos = await r.json();
          // The rows this read replaces: their live dot and their environment
          // stay until `refreshLive` and `loadFleet` answer, and the peer rows
          // stay until the fleet read replaces them.
          const before = new Map(
            this.projects.filter((p) => (p.daemon ?? "") === "").map((p) => [p.slug, p]),
          );
          const local: Project[] = repos.map((x) => {
            const prev = before.get(x.slug);
            return {
              slug: x.slug,
              // What the operator calls the project; the SLUG stays the identity
              // (ADR-0008 D7), and for a remoteless repo it is a hash key.
              name: x.name || x.slug,
              // The on-disk path: the tooltip of a remoteless repo shows it.
              path: x.path || "",
              // The ABSOLUTE native root (#362), distinct from `path` (git's
              // forward-slashed `--show-toplevel` that peers parse).
              root: x.root ?? "",
              branch: x.branch ?? "",
              branches: x.branch !== null && x.branch !== "" ? [x.branch] : [],
              // `{kind:"branch",name}` or `{kind:"detached",sha}`; `null` when
              // the daemon has no answer or is older than this field.
              head: x.head ?? null,
              // `remote` is the github|local classification the dot binds to; the
              // raw origin url rides in `remoteUrl` for `githubUrl()`.
              dirty: x.dirty,
              state: carriedState(x.reachable, prev),
              env: carriedEnv(prev),
              daemonName: carriedDaemonName(prev),
              remote: WBProject.isGitHubRemote(x.remote) ? "github" : "local",
              remoteUrl: x.remote ?? "",
              tree: [],
            };
          });
          this.setProjects(local.concat(this._fleetRows));
          this.shareProjectNames();
          this.reposError = "";
          this.reposRead = WBFail.readFold(this.reposRead, {
            ok: true,
            value: true,
            at: Date.now(),
          });
          // NOT awaited: a down peer costs `/api/fleet` its 2 s timeout, and a
          // peer's absence must not stall the LOCAL sidebar's spinner.
          void this.loadFleet();
        } else {
          this.reposFailed(`the daemon answered ${r.status}`);
        }
      } catch {
        this.reposFailed("the daemon did not answer");
      } finally {
        this.reposLoading = false;
        // The sessions are read with the projects, whatever the projects read
        // answered: the live dots and the console menu come from them.
        void this.refreshLive();
        // The sidebar refresh button is the Changes count's manual reload (#307).
        if (git && this.openSlug !== null) {
          sendWindow(window, "workbench:changes-read", { ref: this.openSlug });
        }
      }
    },

    // A failed `/api/repos` (ADR-0070 D3). After a good read the list stays,
    // marked not current. Before one, the list is empty and says why.
    reposFailed(reason: string) {
      this.reposRead = WBFail.readFold(this.reposRead, { ok: false, reason, at: Date.now() });
      if (this.reposRead.goodAt) {
        this.reposError = WBFail.notCurrent(this.reposRead, (ms) =>
          window.Alpine.store("connection").fmtClock(ms),
        );
        return;
      }
      this.setProjects([]);
      this.reposError = `Could not load the projects from the daemon: ${reason}.`;
    },

    // The fleet read is in wb-projects-fleet.ts.
    fleetRejectText(peers: FleetPeer[]) {
      const bad = peers.filter((p) => p.state === "malformed");
      if (bad.length === 0) return "";
      const what = bad.length === 1 ? "a peer" : `${bad.length} peers`;
      return `Could not read ${what}: ${bad.map((p) => p.diagnosis || p.name).join("; ")}`;
    },
    async loadFleet() {
      await readFleet(this);
    },
    _fleetSeq: 0,

    // One fleet read now, for a panel that saw a peer fail before the 30 s
    // read could. Calls close together share the read in flight: every
    // console of a peer that drops asks at the same moment.
    readFleetNow() {
      this._fleetNow ??= this.loadFleet().finally(() => {
        this._fleetNow = null;
      });
      return this._fleetNow;
    },
    _fleetNow: null as Promise<void> | null,

    // The fleet read that confirms a removed host waits up to 2 s for the
    // peer that is now down: until then its row would come back.
    hostRemoved(daemon: string) {
      this.fleetPeers = this.fleetPeers.filter((p) => p.daemon_id !== daemon);
      this._fleetRows = this._fleetRows.filter((r) => r.daemon !== daemon);
      this.setProjects(this.projects.filter((r) => r.daemon !== daemon));
    },

    // Wake a sleeping peer. The operator's action is the consent (ADR-0052 §4).
    // `/api/fleet/nudge` resolves when the environment is USABLE.
    async wakePeer(daemonId: string) {
      if (!daemonId || this.waking[daemonId] === true) return false;
      this.waking[daemonId] = true;
      try {
        const r = await apiFetch("POST /api/fleet/nudge", { query: { daemon_id: daemonId } });
        const woke = r.ok ? await r.json().catch(() => null) : null;
        if (woke?.ready !== true) {
          deps.messages.flash(await wakeRefusal(r, woke));
          return false;
        }
        // `loadRepos`, not `loadFleet`: the latter CONCATENATES peer rows.
        await this.loadRepos();
        sendWindow(window, "workbench:peer-woken", { daemon: daemonId });
        return true;
      } catch {
        deps.messages.flash("Could not wake the peer: the daemon is not connected.");
        return false;
      } finally {
        delete this.waking[daemonId];
      }
    },

    // Install this computer's Ralphy on an SSH host, then add it again
    // (wb-projects-fleet.ts).
    updatePeer(daemonId: string) {
      return updatePeer(this, deps.messages, daemonId);
    },
    peerUpdatable(g: Group) {
      return WBFleet.updatable(g);
    },

    // Opening a row on a sleeping peer wakes it. A no-op for every other row.
    wakePeerFor(ref: string | null | undefined) {
      const daemon = WBFleet.refDaemon(ref);
      if (!daemon) return;
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      if (WBFleet.wakeable(group)) void this.wakePeer(daemon);
    },

    peerWakeable(g: Group) {
      return WBFleet.wakeable(g);
    },
    peerAvailable(g: Group) {
      return WBFleet.available(g);
    },
    refAvailable(ref: string | null | undefined) {
      const daemon = WBFleet.refDaemon(ref);
      if (!daemon) return true;
      return WBFleet.available(this.fleetGroups().find((g) => g.daemon === daemon));
    },
    peerIcon(g: Group) {
      return WBFleet.stateIcon(g);
    },
    peerFault(g: Group) {
      return WBFleet.stateFault(g);
    },
    groupTitle(g: Group) {
      return WBFleet.groupTitle(g);
    },
    groupLabel(g: Group) {
      return WBFleet.groupLabel(g);
    },
    groupHost(g: Group) {
      return WBFleet.groupHost(g);
    },
    // `x` is a fleet group or a peer of `/api/fleet`: both carry `os` and
    // `environment`.
    osOf(x: { os?: string; environment?: string } | null | undefined) {
      return WBFleet.system(x && x.os, x && x.environment);
    },

    // Local rows first, then one group per peer environment (wb-fleet.ts).
    // The sidebar gives its filtered rows; a group's state comes from the
    // peer list, and the open row always passes the filter.
    fleetGroups(rows?: Project[]) {
      return WBFleet.fleetGroups(rows ?? this.projects, this.fleetPeers);
    },
    // Each project's `live` dot from `/api/sessions` (#204). Never overrides
    // `offline`; a transport throw leaves the states untouched.
    async refreshLive() {
      // Reads close together can answer out of order: the newest owns the list.
      const seq = ++this._liveSeq;
      try {
        const r = await apiFetch("GET /api/sessions");
        if (seq !== this._liveSeq) return;
        if (!r.ok) {
          this.sessionsFailed(`the daemon answered ${r.status}`);
          return;
        }
        const sessions = await r.json();
        if (seq !== this._liveSeq) return;
        this.sessionsRead = WBFail.readFold(this.sessionsRead, {
          ok: true,
          value: true,
          at: Date.now(),
        });
        // The console menu's fold reads this (#304).
        this.liveSessions = sessions;
        // The console windows read their own row off the same poll (ADR-0059).
        window.WBConsole.ingestSessions(sessions);
        for (const p of this.projects) {
          if (p.state === "offline") continue;
          const mine = sessions.filter((s) => WBSessionRoute.matchesRepo(s, this.repoRef(p)));
          const dot = WBProject.projectDotOf(mine);
          p.state = dot.state;
          p.dotDetail = dot.detail;
        }
      } catch {
        if (seq === this._liveSeq) this.sessionsFailed("the daemon did not answer");
      }
    },
    _liveSeq: 0,
    // A failed `/api/sessions` keeps the last list and the live dots, marked
    // not current in the console menu (ADR-0070 D3).
    sessionsFailed(reason: string) {
      this.sessionsRead = WBFail.readFold(this.sessionsRead, { ok: false, reason, at: Date.now() });
    },
    sessionsError() {
      return WBFail.notCurrent(this.sessionsRead, (ms) =>
        window.Alpine.store("connection").fmtClock(ms),
      );
    },

    // The consoles this daemon hosts. A peer's rows in `/api/sessions` carry the
    // peer's `daemon_id`; they end only when that peer restarts.
    localSessions() {
      const peers = new Set(this.fleetPeers.map((p) => p.daemon_id));
      return this.liveSessions.filter((s) => !peers.has(s.daemon_id));
    },
    // The consoles a peer hosts, by the peer's `daemon_id`.
    peerSessions(daemonId: string) {
      return this.liveSessions.filter((s) => s.daemon_id === daemonId);
    },

    // The consoles name their project too (title, tooltip, default name), and
    // `wb-console.ts` has no project list of its own.
    shareProjectNames() {
      window.WBConsole.ingestProjects(
        this.projects.map((p) => ({
          ref: this.repoRef(p),
          name: WBProject.projectName(p),
          title: WBProject.projectTitle(p),
        })),
      );
    },
    // A console of a peer project says what its peer's state is, and comes
    // back when the peer does. Every project, not the filtered list: a search
    // in the sidebar must not change what a console says.
    shareFleet() {
      window.WBConsole.ingestFleet(WBFleet.fleetGroups(this.projects, this.fleetPeers), {
        wake: (daemonId) => this.wakePeer(daemonId),
        read: () => void this.readFleetNow(),
      });
    },
    clearReposError() {
      this.reposError = "";
    },
  };
}

/** The Alpine store `projects`. */
export type ProjectsStore = ReturnType<typeof projectsStore>;
