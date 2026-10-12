/* ---------------------------------------------------------------------------
   The open project and the list of projects: the Alpine store `projects`
   (ADR-0073 amendment of 2026-10-08, decision 3).

   The store is the one owner of this fact, and of the fleet, the agent
   roster and the live sessions (#621). Its fields change only through its
   own methods. `shell()` code reads `this.$store.projects`,
   the markup reads `$store.projects`, a component reads it through the
   `$store` magic, and code outside Alpine reads `Alpine.store("projects")`.
   A helper that needs other `shell()` state too stays in `shell()`.

   It reads no `shell()` member and calls no component; it reads the store
   `connection`. Its `init()` reads the projects and the agents, hears
   `workbench:repos-reread` and `workbench:sessions-reread`, and starts the
   peer read. It tells the files that the fleet was read with
   `workbench:fleet-read`, and the consoles through `WBConsole`.

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
    // The ref of the open project (`repoRef`), or null.
    openSlug: null as string | null,
    // `loadRepos()` fills this at init.
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
    setRowGit(p: Project, git: { branch?: string; branches?: string[]; head?: Project["head"] }) {
      if (git.branches !== undefined) p.branches = git.branches;
      if (git.branch !== undefined) p.branch = git.branch;
      if (git.head !== undefined) p.head = git.head;
    },
    repoRef(p: Project) {
      return WBFleet.repoRef(p);
    },
    // The open project's row, for the panels scoped to `openSlug` that reuse a
    // per-row control (the Changes head's checkout chip).
    openProject() {
      return this.projects.find((p) => this.repoRef(p) === this.openSlug) || null;
    },
    // The current git branch of the open project (for the "current" mode blurb).
    openProjectBranch() {
      return this.projects.find((p) => this.repoRef(p) === this.openSlug)?.branch || "current";
    },
    // The branch chip lives on the Files bar (#332), which only the OPEN
    // project renders. `.project-slug` carries the ADR-0008 D7 identity in
    // `data-slug`, which is how the browser tests find a row.
    rowOpen(p: Project) {
      return this.openSlug === this.repoRef(p);
    },
    // What every surface OUTSIDE the sidebar prints for a repo ref. A peer ref
    // is `<daemon_id>/<owner>/<repo>`: the ULID is how the fleet ROUTES
    // (ADR-0052 §5), so the environment is printed in its place. The ref itself
    // is untouched on the wire, the desk and the tab ids. Row lookup by
    // `repoRef`, not slug: the same `owner/repo` on two daemons is two rows.
    projectLabel(ref: string | null | undefined) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return WBFleet.refLabel(ref);
      const name = WBProject.projectName(row);
      return row.daemon && row.env ? `${name} · ${row.env}` : name;
    },
    // The tooltip twin of `projectLabel`: `owner/repo`, or the full folder of
    // a remoteless repo, plus the environment of a peer. Never a hash key or
    // a daemon id.
    projectTitle(ref: string | null | undefined) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return WBFleet.refLabel(ref);
      const who = WBProject.projectTitle(row);
      return row.daemon && row.env ? `${who} · ${row.env}` : who;
    },

    // The adapter roster comes from `/api/agents`, never a list here:
    // onboarding a vendor must not need a frontend change (#304).
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
    // The peers the daemon could not read (a peer file it cannot parse, or a
    // peer store it cannot list): they list no project, so the sidebar says
    // so instead of showing an empty fleet (ADR-0070 D4).
    fleetRejectNote: "",
    sessionsRead: null as ReadState,
    // The live sessions: the New-console menu, the release dialogs and the
    // checkout chip read them.
    liveSessions: [] as HostedSession[],
    // The peer rows of the last good `/api/fleet`, kept when a read fails.
    _fleetRows: [] as Project[],
    // The local fleet's peers (ADR-0052 §5, #349), from `/api/fleet`. Empty: a
    // fleet of one, or a daemon too old to serve the route.
    fleetPeers: [] as FleetPeer[],
    // Peers with a wake in flight, keyed by daemon_id: a cold WSL boot takes
    // seconds, and the key stops a second click sending a second nudge.
    waking: {} as Record<string, boolean>,
    // A repo refresh in flight. The list does NOT auto-refresh (only the live
    // dots do, via the heartbeat); the button picks up a new repo or a
    // branch/dirty change.
    reposLoading: false,

    // Alpine calls this once, when `main.ts` registers the store, before
    // the shell. `main.ts` builds `WBConsole` first.
    init() {
      this.loadRepos();
      this.loadAgents();
      // A push, a reopened socket or the tab coming back (the store
      // `connection`) asks to read these facts again.
      window.addEventListener("workbench:repos-reread", (e) => this.loadRepos({ git: e.detail.git }));
      window.addEventListener("workbench:sessions-reread", () => this.refreshLive());
      // A peer pushes neither its reachability nor its sessions, so they are
      // read every 30 s while a peer is listed and the tab is visible
      // (ADR-0070 D2 event 6). `loadRepos` reads the fleet and the sessions.
      this._peerTick = setInterval(() => this.peerTick(), this.PEER_READ_MS);
    },

    // A peer reads no git fact: the change set and the branch have their own
    // triggers.
    peerTick() {
      if (!window.Alpine.store("connection").tabHidden() && this.fleetPeers.length) this.loadRepos({ git: false });
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
        const r = await apiFetch("GET /api/agents", { query: repo ? { repo } : {} });
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
    // Hydrate the accordion from the daemon's repo registry. `remote` is
    // inferred from the slug shape
    // (`git::project_slug`'s `path-<hash>` fallback is a remoteless repo).
    // `git: false` skips the change set and the branch: a peer tick or a
    // project-list push says nothing about them (fact index: their own push,
    // and a periodic read only while the Changes panel is open).
    async loadRepos({ git = true } = {}) {
      this.reposLoading = true;
      try {
        const r = await apiFetch("GET /api/repos");
        if (r.ok) {
          const repos = await r.json();
          // The rows this read replaces: their live dot and their environment
          // stay until `refreshLive` and `loadFleet` answer, and the peer rows
          // stay until the fleet read replaces them.
          const before = new Map(this.projects.filter((p) => !p.daemon).map((p) => [p.slug, p]));
          const local: Project[] = repos.map((x) => ({
            slug: x.slug,
            // What the operator calls the project; the SLUG stays the identity
            // (ADR-0008 D7), and for a remoteless repo it is a hash key.
            name: x.name || x.slug,
            // The on-disk path: the tooltip of a remoteless repo shows it.
            path: x.path || "",
            // The ABSOLUTE native root (#362), distinct from `path` (git's
            // forward-slashed `--show-toplevel` that peers parse).
            root: x.root || "",
            branch: x.branch || "",
            branches: x.branch ? [x.branch] : [],
            // `{kind:"branch",name}` or `{kind:"detached",sha}`; `null` when
            // the daemon has no answer or is older than this field.
            head: x.head || null,
            // `remote` is the github|local classification the dot binds to; the
            // raw origin url rides in `remoteUrl` for `githubUrl()`.
            dirty: !!x.dirty,
            state: !x.reachable ? "offline" : before.get(x.slug)?.state === "offline" ? "idle" : before.get(x.slug)?.state || "idle",
            env: before.get(x.slug)?.env || "",
            daemonName: before.get(x.slug)?.daemonName || "",
            remote: WBProject.isGitHubRemote(x.remote) ? "github" : "local",
            remoteUrl: x.remote || "",
            tree: [],
          }));
          this.setProjects(local.concat(this._fleetRows));
          this.shareProjectNames();
          this.reposError = "";
          this.reposRead = WBFail.readFold(this.reposRead, { ok: true, value: true, at: Date.now() });
          // Deliberately NOT awaited: a down peer costs `/api/fleet` its 2 s
          // per-peer timeout, and holding `reposLoading` open for that would make
          // a peer's absence stall the LOCAL sidebar's spinner and live dots.
          // Federation is additive in latency too.
          this.loadFleet();
        } else {
          this.reposFailed(`the daemon answered ${r.status}`);
        }
      } catch {
        this.reposFailed("the daemon did not answer");
      } finally {
        this.reposLoading = false;
        // The sessions are read with the projects, whatever the projects read
        // answered: the live dots and the console menu come from them.
        this.refreshLive();
        // The sidebar refresh button is the Changes count's manual reload (#307).
        if (git && this.openSlug) sendWindow(window, "workbench:changes-read", { ref: this.openSlug });
      }
    },

    // A failed `/api/repos` (ADR-0070 D3). After a good read the list stays,
    // marked not current. Before one, the list is empty and says why.
    reposFailed(reason: string) {
      this.reposRead = WBFail.readFold(this.reposRead, { ok: false, reason, at: Date.now() });
      if (this.reposRead.goodAt) {
        this.reposError = WBFail.notCurrent(this.reposRead, (ms) => window.Alpine.store("connection").fmtClock(ms));
        return;
      }
      this.setProjects([]);
      this.reposError = `Could not load the projects from the daemon: ${reason}.`;
    },

    // The local fleet (ADR-0052 §5, #349): append every PEER's repos after the
    // local `/api/repos` pass, plus the peer list the group headers render.
    // INVARIANT: a `/api/fleet` failure leaves the LOCAL list exactly as it was.
    fleetRejectText(peers: FleetPeer[]) {
      const bad = peers.filter((p) => p.state === "malformed");
      if (!bad.length) return "";
      const what = bad.length === 1 ? "a peer" : `${bad.length} peers`;
      return `Could not read ${what}: ${bad.map((p) => p.diagnosis || p.name).join("; ")}`;
    },
    async loadFleet() {
      // Two project reads close together (a wake fires the visible tab and the
      // socket reopen) must not both append the peers: the newest read owns
      // the peer rows, and they replace, never add to, what is there.
      const seq = ++this._fleetSeq;
      const localRows = () => this.projects.filter((p) => !p.daemon);
      try {
        const r = await apiFetch("GET /api/fleet");
        if (seq !== this._fleetSeq) return;
        if (r.status === 404) {
          // A daemon older than the fleet: a fleet of one, not a failure.
          this.fleetPeers = [];
          this.fleetError = "";
          this.fleetRejectNote = "";
          return;
        }
        if (!r.ok) throw new Error(`the daemon answered ${r.status}`);
        const fleet = await r.json();
        if (seq !== this._fleetSeq) return;
        this.fleetPeers = Array.isArray(fleet.peers) ? fleet.peers : [];
        this.fleetRejectNote = this.fleetRejectText(this.fleetPeers);
        const rows = Array.isArray(fleet.repos) ? fleet.repos : [];
        // `/api/fleet` is the ONLY source of this daemon's own environment label
        // and name; the local rows are stamped with it here.
        const mine = rows.find((x) => x.local);
        if (mine) {
          for (const p of this.projects) {
            p.env = mine.environment || "";
            p.os = mine.os || "";
            p.daemonName = mine.daemon_name || "";
          }
        }
        const peerRows = rows.filter((x) => !x.local);
        this._fleetRows = peerRows.map((x) => ({
            // `<daemon_id>/<slug>`: the same `owner/repo` on two daemons is two rows.
            key: x.key,
            slug: x.slug,
            name: x.name || x.slug,
            path: x.path || "",
            branch: x.branch || "",
            branches: [],
            // The peer's OWN working-tree facts, same classification as `loadRepos`.
            dirty: !!x.dirty,
            state: x.reachable ? "idle" : "offline",
            remote: WBProject.isGitHubRemote(x.remote) ? "github" : "local",
            remoteUrl: x.remote || "",
            tree: [],
            // What makes this a peer row.
            daemon: x.daemon_id,
            daemonName: x.daemon_name || "",
            env: x.environment || "",
            os: x.os || "",
            peerState: x.peer_state || "",
          }));
        this.setProjects(localRows().concat(this._fleetRows));
        this.shareProjectNames();
        this.shareFleet();
        // The files compare the peer of the open project with this read.
        sendWindow(window, "workbench:fleet-read");
        this.fleetRead = WBFail.readFold(this.fleetRead, { ok: true, value: true, at: Date.now() });
        this.fleetError = "";
      } catch (e) {
        if (seq !== this._fleetSeq) return;
        // After a good read the peers and their rows stay, marked not current
        // (ADR-0070 D3); `loadRepos` rebuilt the list without them.
        const reason = String((e as Error)?.message || "").startsWith("the daemon") ? (e as Error).message : "the daemon did not answer";
        this.fleetRead = WBFail.readFold(this.fleetRead, { ok: false, reason, at: Date.now() });
        if (this.fleetRead.goodAt) {
          this.setProjects(localRows().concat(this._fleetRows));
          this.shareProjectNames();
        } else {
          this.fleetPeers = [];
        }
        this.fleetError = WBFail.notCurrent(this.fleetRead, (ms) => window.Alpine.store("connection").fmtClock(ms));
      }
    },
    _fleetSeq: 0,

    // One fleet read now, for a panel that saw a peer fail before the 30 s
    // read could. Calls close together share the read in flight: every
    // console of a peer that drops asks at the same moment.
    readFleetNow() {
      if (!this._fleetNow) {
        this._fleetNow = this.loadFleet().finally(() => {
          this._fleetNow = null;
        });
      }
      return this._fleetNow;
    },
    _fleetNow: null as Promise<void> | null,

    // The fleet read that confirms a removed host waits up to 2 s for the
    // peer that is now down: until then its row would come back.
    hostRemoved(daemon: string) {
      this.fleetPeers = (this.fleetPeers || []).filter((p) => p.daemon_id !== daemon);
      this._fleetRows = this._fleetRows.filter((r) => r.daemon !== daemon);
      this.setProjects(this.projects.filter((r) => r.daemon !== daemon));
    },

    // Wake a sleeping peer. The operator's action is the consent (as push,
    // ADR-0046), which is why this lives in the workbench: a daemon nudging on
    // every probe would be supervising by accident (ADR-0052 §4).
    // `/api/fleet/nudge` resolves when the environment is USABLE.
    async wakePeer(daemonId: string) {
      if (!daemonId || this.waking[daemonId]) return false;
      this.waking[daemonId] = true;
      try {
        const r = await apiFetch("POST /api/fleet/nudge", { query: { daemon_id: daemonId } });
        const woke = r.ok ? await r.json().catch(() => null) : null;
        if (!woke?.ready) {
          // The daemon's own sentence names the environment and what is wrong.
          const refusal = r.ok ? null : await r.json().catch(() => null);
          deps.messages.flash(
            WBFail.failed({ message: woke?.diagnosis || refusal?.error }, "Could not wake the peer: the peer did not answer."),
          );
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

    // Opening a row on a sleeping peer wakes it. A no-op for every other row.
    wakePeerFor(ref: string | null | undefined) {
      const daemon = WBFleet.refDaemon(ref);
      if (!daemon) return;
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      if (WBFleet.wakeable(group)) this.wakePeer(daemon);
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
        this.sessionsRead = WBFail.readFold(this.sessionsRead, { ok: true, value: true, at: Date.now() });
        // The console menu's fold reads this (#304).
        this.liveSessions = sessions;
        // The console windows read their own row off the same poll (ADR-0059).
        window.WBConsole?.ingestSessions?.(sessions);
        for (const p of this.projects) {
          if (p.state === "offline") continue;
          const mine = sessions.filter((s) =>
            WBSessionRoute.matchesRepo(s, this.repoRef(p)),
          );
          // A `waiting` agent outranks `live` on the dot (ADR-0059).
          p.state = !mine.length
            ? "idle"
            : WBProject.agentStateOf(mine) === "waiting"
              ? "waiting"
              : "live";
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
      return WBFail.notCurrent(this.sessionsRead, (ms) => window.Alpine.store("connection").fmtClock(ms));
    },

    // Every console on this daemon is its child, so the update's restart ends
    // them all, the agents inside included. `/api/sessions` lists them.
    // The consoles this daemon hosts. A peer's rows in `/api/sessions` carry the
    // peer's `daemon_id`; they end only when that peer restarts.
    localSessions() {
      const peers = new Set((this.fleetPeers || []).map((p) => p.daemon_id));
      return (this.liveSessions || []).filter((s) => !peers.has(s.daemon_id));
    },
    // The consoles a peer hosts, by the peer's `daemon_id`.
    peerSessions(daemonId: string) {
      return (this.liveSessions || []).filter((s) => s.daemon_id === daemonId);
    },

    // The consoles name their project too (title, tooltip, default name), and
    // `wb-console.ts` has no project list of its own.
    shareProjectNames() {
      window.WBConsole?.ingestProjects?.(
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
      window.WBConsole?.ingestFleet?.(WBFleet.fleetGroups(this.projects, this.fleetPeers), {
        wake: (daemonId) => this.wakePeer(daemonId),
        read: () => this.readFleetNow(),
      });
    },
    clearReposError() {
      this.reposError = "";
    },
  };
}

/** The Alpine store `projects`. */
export type ProjectsStore = ReturnType<typeof projectsStore>;
