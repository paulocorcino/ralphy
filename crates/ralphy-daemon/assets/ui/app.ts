/* ---------------------------------------------------------------------------
   ralphy workbench shell — shell behaviour

   The sidebar is a project accordion (Alpine); the file tree is a Wunderbaum
   instance. The canvas is a tabbed workspace: "Consoles" is fixed and hosts
   the floating console windows (wb-console.ts); every opened file is its own
   closable tab rendered by a viewer (wb-viewer.ts).

   Every user gesture becomes one CustomEvent, `workbench:action`, on
   `document`. That event IS the seam: a backend subscribes and does the work.
--------------------------------------------------------------------------- */
import type { AlpineMagics } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { WBAgents } from "./wb-agents.ts";
import { WBChanges } from "./wb-changes.ts";
import { WBDeskSink } from "./wb-desk-sink.ts";
import { classify, fileTabId, newEntryTitle, parentRel } from "./wb-file-paths.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import { WBRelease } from "./wb-release.ts";
import { WBReleaseDialogs } from "./wb-release-dialogs.ts";
import { WBRun } from "./wb-runs.ts";
import { WBSecurityDialog } from "./wb-security-dialog.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { WBSettingsDialog } from "./wb-settings-dialog.ts";
import { WBSplit } from "./wb-split.ts";
import { sendDocument, sendWindow } from "./wb-events.ts";

/** A peer of `/api/fleet`. */
export type FleetPeer = {
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

// A phone in either orientation: its SHORT side is under the workbench's phone
// breakpoint (560px). Landscape iPhone is ~750 wide but ~340 tall; an iPad's
// short side is 744+ and keeps its desktop chrome. 01-base.css and
// 05-workspace.css gate on the same query.
const PHONE_QUERY = "(max-width: 560px), (pointer: coarse) and (max-height: 560px)";


// The `this` of `shell()`'s members: its own members, the Alpine magics, and
// any other name as `any` until its fields are typed one by one. The type a
// component checks its `uses` against is the literal itself (`Shell`).
function shellData<T extends object>(data: T & ThisType<T & AlpineMagics & Record<string, any>>): T {
  return data;
}

// The popups `wire` opened, as `shell()` reaches them. `wire` sets this, so
// each call starts with no popup.
let detached = { watch: (_win: any, _desc: any) => {}, dirty: () => false, close: () => {} };

export function shell() {
  return shellData({
    // The slow backstop: the `/ws/tree` nudge (#310) only reports what a RUN
    // did; an operator's own editor produces no event. Two git subprocesses per
    // minute, only while the panel is open and the tab in front.
    CHANGES_POLL_MS: 50000,
    // The build this page was served with (`<meta name="ralphy-build">`);
    // "" with no such tag, and then the page never reloads for a build.
    pageBuild: document.querySelector<HTMLMetaElement>('meta[name="ralphy-build"]')?.content || "",
    _boardBackstop: null as any,
    _changesBackstop: null as any,
    // The adapter roster comes from `/api/agents`, never a list here:
    // onboarding a vendor must not need a frontend change (#304).
    agents: [] as any[],
    roster: [] as any[],
    // A failed `/api/repos` (#202): a visible error.
    reposError: "",
    // The read state of the shown facts this sidebar shows (ADR-0070 D3):
    // `WBFail.readFold` results, `null` before the first read.
    reposRead: null as any,
    fleetRead: null as any,
    fleetError: "",
    // The peers the daemon could not read (a peer file it cannot parse, or a
    // peer store it cannot list): they list no project, so the sidebar says
    // so instead of showing an empty fleet (ADR-0070 D4).
    fleetRejectNote: "",
    sessionsRead: null as any,
    // The live sessions: the New-console menu, the release dialogs and the
    // checkout chip read them.
    liveSessions: [] as any[],
    // Why the daemon cannot read the saved desk, or "" (ADR-0070 D4); a copy
    // of `WBConsole.deskFailure()` so the page can show it.
    deskFailure: "",
    // The peer rows of the last good `/api/fleet`, kept when a read fails.
    _fleetRows: [] as any[],
    // The local fleet's peers (ADR-0052 §5, #349), from `/api/fleet`. Empty: a
    // fleet of one, or a daemon too old to serve the route.
    fleetPeers: [] as FleetPeer[],
    // Peers with a wake in flight, keyed by daemon_id: a cold WSL boot takes
    // seconds, and the key stops a second click sending a second nudge.
    waking: {} as Record<string, any>,
    // The last refusal from the Changes panel, held until the next act. NOT
    // `runsActionMsg`: that renders only inside `aside.runs`, which is closed
    // by default. One string, not per-project: switching projects is itself
    // the next act.
    changesError: "",
    // A repo refresh in flight. The list does NOT auto-refresh (only the live
    // dots do, via the heartbeat); the button picks up a new repo or a
    // branch/dirty change.
    reposLoading: false,
    // Live presence + identity (#204): uptime from the `/ws` heartbeat,
    // name/avatar from `/api/identity`.
    uptimeText: "",
    identityName: "",
    identityAvatar: "",
    _lastHeartbeat: 0,
    // Staleness is DERIVED on a clock (`_clockTick`), not in the binding: the
    // event that makes the daemon stale — ticks STOPPING — never re-renders a
    // binding on `_lastHeartbeat`. Writing the same boolean is inert under
    // Alpine, so the steady state costs one comparison a second.
    presenceStale: false,
    _presenceSub: null as any, // the `/ws` heartbeat subscription, kept so a resume can re-open it

    // Alpine lifecycle.
    init() {
      this.probeSession();
      this.loadRepos();
      this.loadAgents();
      this.subscribePresence();
      this.loadIdentity();
      // Read at load and again on every return to the tab (below): the daemon
      // polls releases on its own six-hour clock, and a tab left open for days
      // would otherwise never show what that poll found.
      this.loadRelease();
      // The board's two time-driven refresh triggers (#301), registered ONCE;
      // the predicate (wb-kanban.ts) decides.
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible") this.onTabVisible();
      });
      // A tablet resumes on a different link; its sockets died without a close.
      window.addEventListener("online", () => this.resumeSockets(true));
      // The open project changed (ADR-0073 amendment of 2026-10-08, decision 4).
      // Each part resets only its own state, so the order is not a contract;
      // the files hear it in `wb-files.ts`.
      window.addEventListener("workbench:project-changed", () => this.boardFollowProject());
      window.addEventListener("workbench:project-changed", (e) => this.gitFollowProject(e.detail.slug));
      // The open checkout's HEAD moved (the files' `/ws/tree` socket says so).
      window.addEventListener("workbench:head-moved", (e) => this.gitFollowHead(e.detail.ref));
      // The console module cannot know whether THIS document's connection is
      // alive; hand it the heartbeat verdict, or its hidden-time fallback
      // resets every console after a minute on another tab.
      window.WBConsole?.setStaleProbe?.(() => this.socketsAreStale());
      // The selected checkouts (#406): the ONE hook for `unknown checkout`, and
      // the copy of the desk mirror once the boot desk lands.
      window.WBDaemon?.onUnknownCheckout?.((repo: any, name: any) => this.checkoutGone(repo, name));
      window.WBConsole?.whenDeskLoaded?.().then(() => {
        this.adoptDeskCheckouts();
        this.syncDeskFailure();
      });
      // A flush can be the read that finds the desk unreadable; no push says so.
      window.WBConsole?.setDeskFailureHook?.(() => this.syncDeskFailure());
      window.addEventListener("workbench:menus-close", () => this.closeAvatarMenu());
      // Anchor the clock at page load: `_boardLoadedAt` at 0 would clear the
      // 120s floor on the first tick.
      this._boardLoadedAt = Date.now();
      this._boardBackstop = setInterval(() => this.boardBackstopTick(), 30000);
      // Registered once; the tick asks whether the panel is open.
      this._changesBackstop = setInterval(() => this.refreshChanges(), this.CHANGES_POLL_MS);
      // A peer pushes neither its reachability nor its sessions, so they are
      // read every 30 s while a peer is listed and the tab is visible
      // (ADR-0070 D2 event 6). `loadRepos` reads the fleet and the sessions.
      this._peerTick = setInterval(() => this.peerTick(), this.PEER_READ_MS);
      // The phase clock's tick: one assignment a second while the panel is
      // open; the run document is NOT re-read.
      this._clockTick = setInterval(() => {
        if (this.runsOpen) this.nowMs = Date.now();
        // Unconditional: the flag must already be true when the menu opens.
        this.presenceStale = this.socketsAreStale();
      }, 1000);
    },

    // The daemon's mark, in ONE place (rail puck, account menu, About card). The
    // fallback is a picture too: a blank 26px circle reads as a failed load.
    identityMark() {
      return this.identityAvatar || "🤖";
    },
    // Name + avatar for the account menu. A 404 (un-baptized) or a thrown
    // fetch leaves the fields empty.
    async loadIdentity() {
      try {
        const r = await fetch("/api/identity");
        if (r.ok) {
          const id = await r.json();
          this.identityName = id.name || "";
          this.identityAvatar = id.avatar || "";
        }
      } catch {}
    },

    // Three missed ~2s heartbeats: this document's connection is gone. The
    // shell's whole staleness signal — a suspended tablet runs no JS, so on
    // return the stamp is old; a desktop tab switch leaves it fresh.
    socketsAreStale() {
      return !this._lastHeartbeat || Date.now() - this._lastHeartbeat > 6000;
    },

    // Bring the long-lived subscriptions back after a suspend. Each decides for
    // itself (`resumeDecision`) and debounces.
    resumeSockets(stale?: any) {
      const verdict = stale === undefined ? this.socketsAreStale() : stale;
      this._runsSub?.resume?.(verdict);
      this._changesSub?.resume?.(verdict);
      this._presenceSub?.resume?.(verdict);
      // The files' `/ws/tree` socket (wb-files.ts).
      sendWindow(window, "workbench:sockets-resume", { stale: verdict });
    },

    // The `/ws` presence heartbeat (daemon mode). Each tick stamps
    // `_lastHeartbeat`, refreshes uptime and carries name/avatar once
    // baptized. It reads nothing: the shown facts are read again on the
    // daemon's pushes, which ride the same socket (ADR-0070 D2).
    subscribePresence() {
      if (!window.WBDaemon?.subscribePresence) return;
      this._presenceSub = window.WBDaemon.subscribePresence(
        (p: any) => {
          this._lastHeartbeat = Date.now();
          this.uptimeText = "Running for " + this.fmtUptime(p.uptime_secs);
          if (p.name) this.identityName = p.name;
          if (p.avatar) this.identityAvatar = p.avatar;
          if (p.build && this.pageBuild && p.build !== this.pageBuild) this.onBuildSkew();
        },
        {
          onPush: (verb: any, payload: any) => this.onPresencePush(verb, payload),
          onOpen: (reopened: any) => this.onPresenceOpen(reopened),
        },
      );
    },

    // A hidden tab reads no shown fact; `onTabVisible` reads them all again
    // (ADR-0070 D2), so a push dropped here is not lost.
    tabHidden() {
      return document.visibilityState === "hidden";
    },

    // A push from the daemon for a fact it owns (ADR-0070 D2 event 1).
    onPresencePush(verb: any, payload: any) {
      if (this.tabHidden()) return;
      if (verb === "sessions.dirty") {
        // A spawn and its first agent state arrive together: one read.
        clearTimeout(this._liveTimer);
        this._liveTimer = setTimeout(() => {
          if (!this.tabHidden()) this.refreshLive();
        }, this.LIVE_SETTLE_MS);
      } else if (verb === "desk.dirty") {
        // This tab's own write: it already holds the result.
        if (payload?.tab && payload.tab === WBDeskSink?.tabId?.()) return;
        this.rereadDesk();
      } else if (verb === "repos.dirty" || verb === "peers.dirty") {
        // The change set and the branch have their own pushes.
        this.loadRepos({ git: false });
      }
    },
    // A peer reads no git fact: the change set and the branch have their own
    // triggers.
    peerTick() {
      if (!this.tabHidden() && this.fleetPeers.length) this.loadRepos({ git: false });
    },
    LIVE_SETTLE_MS: 250,
    PEER_READ_MS: 30000,
    _liveTimer: null as any,
    _peerTick: null as any,

    // The presence socket opened again: a push may have been lost while it
    // was down (ADR-0070 D2 event 2). The first open reads nothing: `init`
    // already did.
    onPresenceOpen(reopened: any) {
      if (!reopened || this.tabHidden()) return;
      this.loadRepos();
      this.rereadDesk();
      this.maybeRefreshBoard("reopen");
      if (this.$store.projects.openSlug) this.hydrateRuns();
    },

    // Read the desk again. The console module puts it on the stage, and tells
    // the columns (wb-consoles-tab.ts) which consoles left it; the selected
    // checkouts are copied, since another client may have picked a tree.
    rereadDesk() {
      const read = window.WBConsole?.reloadDesk?.();
      if (!read?.then) return;
      read.then(() => {
        this.syncDeskFailure();
        this.adoptDeskCheckouts();
      });
    },
    syncDeskFailure() {
      this.deskFailure = window.WBConsole?.deskFailure?.() || "";
    },
    // The desk failure's one action (ADR-0070 D4).
    startNewDesk() {
      window.WBConsole?.startNewDesk?.()
        .then(() => this.syncDeskFailure())
        .catch((e: any) => {
          const why = String(e?.message || "").startsWith("the daemon") ? e.message : "the daemon did not answer";
          this._flashAction(`Could not start a new desk: ${why}.`);
        });
    },

    // Seconds → a compact `1d 2h`, `2h 14m`, `5m`, `12s` uptime string.
    fmtUptime(secs: any) {
      const s = Math.max(0, Math.floor(secs || 0));
      const d = Math.floor(s / 86400);
      const h = Math.floor((s % 86400) / 3600);
      const m = Math.floor((s % 3600) / 60);
      if (d) return `${d}d ${h}h`;
      if (h) return `${h}h ${m}m`;
      if (m) return `${m}m`;
      return `${s}s`;
    },

    // Ask the daemon whether this browser is authorized. A thrown fetch
    // keeps `authed` at its default.
    async probeSession() {
      try {
        const r = await fetch("/api/session");
        if (r.ok) {
          const s = await r.json();
          this.authed = s.authed;
          this.login.passwordRequired = s.password;
          this.security.policy = s.policy;
          // The login card's mark: `/api/identity` is gated, so this pre-login
          // leg is the only source. `loadIdentity` overwrites it after login.
          if (s.avatar) this.identityAvatar = s.avatar;
          // Gated on `authed`: a pre-login restore would have every tab refused
          // and closed, persisting the loss (#339). `rehydrateAfterAuth` is the
          // other end.
          if (s.authed) this.restoreView();
        }
      } catch {
        // No restore: `authed` is still at its `true` default, and a restore
        // against a dead daemon closes every tab whose read fails — and
        // `closeTab` persists.
      }
    },

    // The daemon's adapter roster (#304). A failed fetch leaves the roster
    // EMPTY rather than showing adapters this daemon may not have.
    _agentsSeq: 0,
    async loadAgents(repo?: any) {
      if (repo === undefined) repo = this.$store.projects.openSlug;
      const seq = ++this._agentsSeq;
      try {
        const r = await fetch(WBAgents.rosterUrl(repo));
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
        const r = await fetch("/api/repos");
        if (r.ok) {
          const repos = await r.json();
          // The rows this read replaces: their live dot and their environment
          // stay until `refreshLive` and `loadFleet` answer, and the peer rows
          // stay until the fleet read replaces them.
          const before = new Map(this.$store.projects.projects.filter((p) => !p.daemon).map((p) => [p.slug, p]));
          const local = repos.map((x: any) => ({
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
          this.$store.projects.setProjects(local.concat(this._fleetRows));
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
        if (git && this.$store.projects.openSlug) this.loadChanges(this.$store.projects.openSlug);
        if (git && this.$store.projects.openSlug) this.loadSync(this.$store.projects.openSlug);
      }
    },

    // A failed `/api/repos` (ADR-0070 D3). After a good read the list stays,
    // marked not current. Before one, the list is empty and says why.
    reposFailed(reason: any) {
      this.reposRead = WBFail.readFold(this.reposRead, { ok: false, reason, at: Date.now() });
      if (this.reposRead.goodAt) {
        this.reposError = WBFail.notCurrent(this.reposRead, (ms) => this.fmtClock(ms));
        return;
      }
      this.$store.projects.setProjects([]);
      this.reposError = `Could not load the projects from the daemon: ${reason}.`;
    },

    // A time of day, `14:02`, for "Read at …".
    fmtClock(ms: any) {
      return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    },

    // The local fleet (ADR-0052 §5, #349): append every PEER's repos after the
    // local `/api/repos` pass, plus the peer list the group headers render.
    // INVARIANT: a `/api/fleet` failure leaves the LOCAL list exactly as it was.
    fleetRejectText(peers: any) {
      const bad = peers.filter((p: any) => p.state === "malformed");
      if (!bad.length) return "";
      const what = bad.length === 1 ? "a peer" : `${bad.length} peers`;
      return `Could not read ${what}: ${bad.map((p: any) => p.diagnosis || p.name).join("; ")}`;
    },
    async loadFleet() {
      // Two project reads close together (a wake fires the visible tab and the
      // socket reopen) must not both append the peers: the newest read owns
      // the peer rows, and they replace, never add to, what is there.
      const seq = ++this._fleetSeq;
      const localRows = () => this.$store.projects.projects.filter((p) => !p.daemon);
      try {
        const r = await fetch("/api/fleet");
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
        const mine = rows.find((x: any) => x.local);
        if (mine) {
          for (const p of this.$store.projects.projects) {
            p.env = mine.environment || "";
            p.os = mine.os || "";
            p.daemonName = mine.daemon_name || "";
          }
        }
        const peerRows = rows.filter((x: any) => !x.local);
        this._fleetRows = peerRows.map((x: any) => ({
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
        this.$store.projects.setProjects(localRows().concat(this._fleetRows));
        this.shareProjectNames();
        this.shareFleet();
        // The files compare the peer of the open project with this read.
        sendWindow(window, "workbench:fleet-read");
        this.fleetRead = WBFail.readFold(this.fleetRead, { ok: true, value: true, at: Date.now() });
        this.fleetError = "";
      } catch (e: any) {
        if (seq !== this._fleetSeq) return;
        // After a good read the peers and their rows stay, marked not current
        // (ADR-0070 D3); `loadRepos` rebuilt the list without them.
        const reason = String(e?.message || "").startsWith("the daemon") ? e.message : "the daemon did not answer";
        this.fleetRead = WBFail.readFold(this.fleetRead, { ok: false, reason, at: Date.now() });
        if (this.fleetRead.goodAt) {
          this.$store.projects.setProjects(localRows().concat(this._fleetRows));
          this.shareProjectNames();
        } else {
          this.fleetPeers = [];
        }
        this.fleetError = WBFail.notCurrent(this.fleetRead, (ms) => this.fmtClock(ms));
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
    _fleetNow: null as any,

    // The fleet read that confirms a removed host waits up to 2 s for the
    // peer that is now down: until then its row would come back.
    hostRemoved(daemon: any) {
      this.fleetPeers = (this.fleetPeers || []).filter((p) => p.daemon_id !== daemon);
      this._fleetRows = this._fleetRows.filter((r) => r.daemon !== daemon);
      this.$store.projects.setProjects(this.$store.projects.projects.filter((r) => r.daemon !== daemon));
    },

    // Wake a sleeping peer. The operator's action is the consent (as push,
    // ADR-0046), which is why this lives in the workbench: a daemon nudging on
    // every probe would be supervising by accident (ADR-0052 §4).
    // `/api/fleet/nudge` resolves when the environment is USABLE.
    async wakePeer(daemonId: any) {
      if (!daemonId || this.waking[daemonId]) return false;
      this.waking[daemonId] = true;
      try {
        const r = await fetch(`/api/fleet/nudge?daemon_id=${encodeURIComponent(daemonId)}`, {
          method: "POST",
        });
        const reply = await r.json().catch(() => ({}));
        if (!r.ok || !reply.ready) {
          // The daemon's own sentence names the environment and what is wrong.
          this._flashAction(
            WBFail.failed({ message: reply.diagnosis || reply.error }, "Could not wake the peer: the peer did not answer."),
          );
          return false;
        }
        // `loadRepos`, not `loadFleet`: the latter CONCATENATES peer rows.
        await this.loadRepos();
        sendWindow(window, "workbench:peer-woken", { daemon: daemonId });
        return true;
      } catch {
        this._flashAction("Could not wake the peer: the daemon is not connected.");
        return false;
      } finally {
        delete this.waking[daemonId];
      }
    },

    // Opening a row on a sleeping peer wakes it. A no-op for every other row.
    wakePeerFor(ref: any) {
      const daemon = WBFleet.refDaemon(ref);
      if (!daemon) return;
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      if (WBFleet.wakeable(group)) this.wakePeer(daemon);
    },

    peerWakeable(g: any) {
      return WBFleet.wakeable(g);
    },
    peerAvailable(g: any) {
      return WBFleet.available(g);
    },
    refAvailable(ref: any) {
      const daemon = WBFleet.refDaemon(ref);
      if (!daemon) return true;
      return WBFleet.available(this.fleetGroups().find((g) => g.daemon === daemon));
    },
    peerIcon(g: any) {
      return WBFleet.stateIcon(g);
    },
    peerFault(g: any) {
      return WBFleet.stateFault(g);
    },
    groupTitle(g: any) {
      return WBFleet.groupTitle(g);
    },
    groupLabel(g: any) {
      return WBFleet.groupLabel(g);
    },
    groupHost(g: any) {
      return WBFleet.groupHost(g);
    },
    // `x` is a fleet group or a peer of `/api/fleet`: both carry `os` and
    // `environment`.
    osOf(x: any) {
      return WBFleet.system(x && x.os, x && x.environment);
    },

    // Local rows first, then one group per peer environment (wb-fleet.ts).
    fleetGroups() {
      return WBFleet.fleetGroups(this.filteredProjects(), this.fleetPeers);
    },
    // Each project's `live` dot from `/api/sessions` (#204). Never overrides
    // `offline`; a transport throw leaves the states untouched.
    async refreshLive() {
      // Reads close together can answer out of order: the newest owns the list.
      const seq = ++this._liveSeq;
      try {
        const r = await fetch("/api/sessions");
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
        for (const p of this.$store.projects.projects) {
          if (p.state === "offline") continue;
          const mine = sessions.filter((s: any) =>
            WBSessionRoute.matchesRepo(s, this.$store.projects.repoRef(p)),
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
    sessionsFailed(reason: any) {
      this.sessionsRead = WBFail.readFold(this.sessionsRead, { ok: false, reason, at: Date.now() });
    },
    sessionsError() {
      return WBFail.notCurrent(this.sessionsRead, (ms) => this.fmtClock(ms));
    },

    _flashAction(msg: any) {
      this.runsActionMsg = msg;
      clearTimeout(this._actionTimer);
      this._actionTimer = setTimeout(() => (this.runsActionMsg = ""), 2600);
    },

    // --- modal stack ------------------------------------------------------
    // The open modals, oldest first: `{ path, opener }`. Only the last one
    // answers Escape, and each returns focus to its opener on close.
    _modalStack: [] as any[],
    // The confirm dialog (replaces window.confirm); `askConfirm` opens it.
    confirmModal: {
      open: false,
      title: "",
      message: "",
      confirmLabel: "Confirm",
      cancelLabel: "Cancel",
      danger: false,
    },
    _confirmResolve: null as any,
    // The prompt dialog (replaces window.prompt, which is suppressible
    // per-origin and never appears in an unfocused popup). `askPrompt`
    // resolves the typed string, or null.
    promptModal: {
      open: false,
      title: "",
      message: "",
      value: "",
      placeholder: "",
      confirmLabel: "Create",
      error: "",
    },
    _promptResolve: null as any,
    // The Escape keydown a modal has already answered.
    _escapeEvent: null,
    // The one binding every `.modal-scrim` in index.html uses:
    // `x-bind="scrim('runOpen', () => closeRunModal())"`. `path` names the open
    // flag, dotted for a nested one (`confirmModal.open`). Alpine evaluates the
    // object once per scrim, so `was` lives as long as the element.
    // A click on the scrim closes nothing: a stray click must not throw away
    // what a modal holds. Only its own buttons and Escape close it.
    scrim(path: any, close: any) {
      const isOpen = () => path.split(".").reduce((o: any, k: any) => o?.[k], this);
      let was = false;
      const self = this;
      return {
        "x-show": () => isOpen(),
        // Every open scrim hears the same window keydown; only the top one acts,
        // so a confirm raised over another modal closes alone. The event is
        // marked because the browser runs Alpine's effects between two
        // listeners: the close pops the stack before the next scrim is asked,
        // and the modal under it would read as the top.
        "@keydown.escape.window": (e: any) => {
          if (self._escapeEvent === e) return;
          if (isOpen() && self.isTopModal(path)) {
            self._escapeEvent = e;
            close();
          }
        },
        // Watches the flag, not the close methods: `logOff()` clears flags
        // directly, and that close must still pop the stack.
        "x-effect"(this: any) {
          const open = !!isOpen();
          if (open === was) return;
          was = open;
          if (open) self.modalOpened(path, this.$el);
          else self.modalClosed(path);
        },
      };
    },
    isTopModal(path: any) {
      return this._modalStack.at(-1)?.path === path;
    },
    // Whether the modal with this open-flag path is open, at any depth. Code
    // outside a dialog's component asks this, never the flag (ADR-0073 D5).
    modalOpen(path: any) {
      return this._modalStack.some((m) => m.path === path);
    },
    modalOpened(path: any, scrimEl: any) {
      this._modalStack.push({ path, opener: document.activeElement });
      // One frame later: `x-show` has flipped by then, and a modal that focuses
      // its own field on open (Branch, Prompt) has already done so.
      window.requestAnimationFrame(() => {
        const dialog = scrimEl.querySelector('[role="dialog"], [role="alertdialog"]') || scrimEl;
        if (dialog.contains(document.activeElement)) return;
        const controls = Array.from<any>(
          dialog.querySelectorAll(
            'button, input, select, textarea, [href], [tabindex]:not([tabindex="-1"])',
          ),
        ).filter((el) => !el.disabled && el.getClientRects().length > 0);
        // The header ✕ is the last resort: it leads every modal, and the
        // operator came for the content.
        (controls.find((el) => !el.classList.contains("modal-x")) || controls[0])?.focus();
      });
    },
    modalClosed(path: any) {
      const i = this._modalStack.findLastIndex((m) => m.path === path);
      if (i < 0) return;
      const [{ opener }] = this._modalStack.splice(i, 1);
      if (opener?.isConnected) opener.focus();
    },
    // Accelerators are ignored while typing or while a modal is up.
    // `allowTerminal`: a key that must also work from inside a terminal (the
    // column walk); xterm's input is a TEXTAREA.
    consoleShortcutsBlocked(allowTerminal = false) {
      if (!this.authed) return true;
      if (this.modalOpen(WBSettingsDialog.openFlag) || this.modalOpen(WBSecurityDialog.openFlag) || this.modalOpen("runOpen") || this.modalOpen("branchOpen")) return true;
      if (this.modalOpen(WBReleaseDialogs.whatsNewFlag)) return true;
      const el: any = document.activeElement;
      if (allowTerminal && el?.closest?.(".xterm")) return false;
      return !!(
        el &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable || el.closest(".monaco-editor"))
      );
    },
    // Resolve `true`/`false` on the operator's choice. A pending dialog is
    // settled `false` first so a second call never strands its promise.
    askConfirm(opts: any = {}) {
      if (this._confirmResolve) this.confirmRespond(false);
      this.confirmModal = {
        open: true,
        title: opts.title || "Confirm",
        message: opts.message || "",
        confirmLabel: opts.confirmLabel || "Confirm",
        cancelLabel: opts.cancelLabel || "Cancel",
        danger: opts.danger || false,
      };
      return new Promise((resolve) => {
        this._confirmResolve = resolve;
      });
    },
    // Close the dialog and settle its promise with the choice.
    confirmRespond(ok: any) {
      this.confirmModal.open = false;
      const resolve = this._confirmResolve;
      this._confirmResolve = null;
      if (resolve) resolve(ok);
    },
    // Resolve the typed string, or `null`. Mirrors askConfirm.
    askPrompt(opts: any = {}) {
      if (this._promptResolve) this.promptRespond(null);
      this.promptModal = {
        open: true,
        title: opts.title || "Name",
        message: opts.message || "",
        value: opts.value || "",
        placeholder: opts.placeholder || "",
        confirmLabel: opts.confirmLabel || "Create",
        error: "",
      };
      // Focus after Alpine has painted; caret at the end, not selected.
      queueMicrotask(() => {
        const el = document.getElementById("prompt-input") as HTMLInputElement | null;
        if (!el) return;
        el.focus();
        el.setSelectionRange(el.value.length, el.value.length);
      });
      return new Promise<string | null>((resolve) => {
        this._promptResolve = resolve;
      });
    },
    // A name that cannot be a single directory entry is refused HERE, dialog
    // open. The daemon confines every path regardless (`confine_write`); this
    // says *which* character was wrong.
    promptSubmit() {
      const name = this.promptModal.value.trim();
      const bad = !name
        ? "name is required"
        : /[\\/]/.test(name)
          ? "name cannot contain / or \\"
          : name === "." || name === ".."
            ? "name cannot be . or .."
            : "";
      if (bad) {
        this.promptModal.error = bad;
        return;
      }
      this.promptRespond(name);
    },
    // Close the dialog and settle its promise with `name` (null = cancelled).
    promptRespond(name: any) {
      this.promptModal.open = false;
      const resolve = this._promptResolve;
      this._promptResolve = null;
      if (resolve) resolve(name);
    },

    // --- the open project -------------------------------------------------
    // The fact itself is the Alpine store `projects` (wb-projects-store.ts).
    toggle(ref: any, row?: any) {
      const previous = this.$store.projects.openSlug;
      // A row on a host that cannot answer stays closed. The click still wakes
      // a sleeping host: that is the act the operator asked for.
      if (previous !== ref && !this.refAvailable(ref)) {
        this.wakePeerFor(ref);
        return;
      }
      this.$store.projects.setOpen(previous === ref ? null : ref);
      const slug = this.$store.projects.openSlug;
      // NOT awaited: the accordion must not sit behind a cold WSL boot.
      if (slug === ref) this.wakePeerFor(ref);
      this.loadAgents(slug);
      // The chip's `<branch> · <name>` needs the listing (#406).
      if (slug === ref) this.ensureWorktreeListing(ref);
      this.refreshSpend();
      this.projectChanged(previous);
    },
    // The files, the board and the git part each drop what was scoped to the
    // project that WAS open (ADR-0073 amendment of 2026-10-08, decision 4).
    projectChanged(previous: any) {
      const slug = this.$store.projects.openSlug;
      sendWindow(window, "workbench:project-changed", { slug, previous });
    },
    // A sleeping peer's wake button. Its two sentences keep their order here,
    // not in a `+` chain inside the markup (ADR-0065 §9).
    wakeTitle(g: any) {
      if (this.waking[g.daemon]) return `Waking ${g.environment}…`;
      return `Wake ${g.environment}. ${g.diagnosis}`;
    },
    // A row on a host that cannot answer: why it does not open.
    unavailableTitle(g: any) {
      const host = WBFleet.peerName(g);
      const head = `${host} is not available (${this.peerStateWord(g.state)}).`;
      if (WBFleet.wakeable(g)) return `${head} Click to wake it.`;
      return `${head} Its projects open when it connects again.`;
    },
    rowTitle(p: any) {
      return WBProject.rowTitle(p);
    },
    // Drop a project from the daemon's registry (#363); the disk is NOT
    // touched. The confirm is awaited BEFORE any `WBDaemon` call: cancel must
    // open no socket.
    async removeProject(p: any) {
      const ref = this.$store.projects.repoRef(p);
      const ok = await this.askConfirm({
        title: "Remove project",
        message: `Remove “${WBProject.projectName(p)}” from Ralphy? Files on disk are kept.`,
        confirmLabel: "Remove",
        danger: true,
      });
      if (!ok) return;
      try {
        const reply = await window.WBDaemon.observe("project.remove", {
          // The envelope routes on a REGISTERED repo before dispatch: `repo` is
          // the cwd, `slug` what is unregistered (the split keeps the peer
          // proxy working).
          repo: ref,
          slug: p.slug,
        });
        // `unknown repo` means already gone: the state this click asks for.
        const gone =
          !WBFail.isError(reply) || WBFail.message(reply, "") === "unknown repo";
        if (!gone) {
          this._flashAction(WBFail.failed(reply, "Could not remove the project: the daemon gave no reason."));
          return;
        }
        // Identity is `repoRef`, not the slug: a peer can list the same slug.
        this.$store.projects.setProjects(this.$store.projects.projects.filter((x) => this.$store.projects.repoRef(x) !== ref));
        if (this.$store.projects.openSlug === ref) {
          this.$store.projects.setOpen(null);
          this.projectChanged(ref);
        }
        this.loadRepos();
      } catch {
        this._flashAction("remove unavailable: no daemon");
      }
    },
    // The status dot: live → green, idle → grey, offline → red (unreachable
    // path), waiting → yellow (an agent is asking for you, ADR-0059).
    // Orthogonal to `remote`.
    // The project dot's tooltip, in words; the class keeps the state code.
    dotTitle(state: any) {
      return (
        ({
          live: "A console is open",
          waiting: "An agent is waiting for you",
          offline: "The folder cannot be reached",
        } as Record<string, string>)[state] || "No console is open"
      );
    },
    dotClass(state: any) {
      return state === "live"
        ? "live"
        : state === "waiting"
          ? "waiting"
          : state === "offline"
            ? "offline"
            : "";
    },
    peerStateWord(state: any) {
      return WBFleet.stateWord(state);
    },

    // --- chrome panels ----------------------------------------------------
    // Sidebar, Runs panel and Kanban board: each a layout flip on a body class.
    // A phone opens with the sidebar closed: there it floats over the canvas.
    sideOpen: !window.matchMedia?.(PHONE_QUERY)?.matches,
    // `projects` or `changes` (#317). Changes is a VIEW scoped to `openSlug`.
    sideView: "projects",
    runsOpen: false,
    kanbanOpen: false,
    projectQuery: "",

    phone() {
      return !!window.matchMedia?.(PHONE_QUERY)?.matches;
    },

    // Clicking the rail button of the view already showing collapses the sidebar.
    showSideView(view: any) {
      if (this.sideOpen && this.sideView === view) {
        this.sideOpen = false;
        return;
      }
      this.sideView = view;
      this.sideOpen = true;
      // Opening Changes IS a read trigger: the rows were last read when the
      // project was opened.
      this.refreshChanges();
    },

    // Re-read the working tree, only while the Changes panel is on screen: both
    // reads are local but each is a subprocess.
    refreshChanges() {
      if (!this.sideOpen || this.sideView !== "changes" || !this.$store.projects.openSlug) return;
      if (document.visibilityState !== "visible") return;
      this.loadChanges(this.$store.projects.openSlug);
      this.loadSync(this.$store.projects.openSlug);
    },

    // The change indicator for one row. Only slugs whose count was READ render
    // one: a `changes.list` per repo would be N git subprocesses on open.
    projectBadge(slug: any) {
      return WBChanges.projectBadge(this.changesCount, slug);
    },

    // Case-insensitive slug/branch/label filter. The sidebar count keeps
    // showing `projects.length`.
    filteredProjects() {
      const q = this.projectQuery.trim().toLowerCase();
      if (!q) return this.$store.projects.projects;
      // The filter must never fail to match what the row DOES print (the
      // label, #332); the raw `path` is not matched.
      // INVARIANT: the OPEN row always passes. The open project's files
      // (wb-files.ts) sit under its row, and a list without that row leaves
      // them under no name.
      return this.$store.projects.projects.filter(
        (p) =>
          this.$store.projects.rowOpen(p) ||
          p.slug.toLowerCase().includes(q) ||
          p.branch.toLowerCase().includes(q) ||
          this.repoLabel(p).toLowerCase().includes(q)
      );
    },

    // Sidebar row label: the repo name, UPPERCASED (wb-project.ts).
    repoLabel(p: any) {
      return WBProject.repoLabel(p);
    },

    // The consoles name their project too (title, tooltip, default name), and
    // `wb-console.ts` has no project list of its own.
    shareProjectNames() {
      window.WBConsole?.ingestProjects?.(
        this.$store.projects.projects.map((p) => ({
          ref: this.$store.projects.repoRef(p),
          name: WBProject.projectName(p),
          title: WBProject.projectTitle(p),
        })),
      );
    },
    // A console of a peer project says what its peer's state is, and comes
    // back when the peer does. Every project, not the filtered list: a search
    // in the sidebar must not change what a console says.
    shareFleet() {
      window.WBConsole?.ingestFleet?.(WBFleet.fleetGroups(this.$store.projects.projects, this.fleetPeers), {
        wake: (daemonId: any) => this.wakePeer(daemonId),
        read: () => this.readFleetNow(),
      });
    },

    // The global `/` shortcut.
    focusProjectSearch() {
      // `/` must never focus an input the Changes view is hiding (#317).
      this.sideView = "projects";
      this.sideOpen = true;
      this.$nextTick(() => this.$refs.projectSearch?.focus());
    },
    toggleRuns() {
      this.runsOpen = !this.runsOpen;
      // Closing drops the board-arrival marker: a same-numbered issue in
      // another run would inherit it.
      if (!this.runsOpen) this.trailFocus = null;
      if (this.runsOpen) {
        // `nowMs` is as stale as the panel has been closed; re-anchor before
        // the first paint.
        this.nowMs = Date.now();
        this.hydrateRuns();
      }
    },
    toggleKanban() {
      // The tasks board (wb-kanban.ts): an overlay flip over the canvas.
      this.kanbanOpen = !this.kanbanOpen;
      if (this.kanbanOpen) {
        this.kanbanSel = null;
        // Lazy-load the tracker for the open project when the board opens.
        this.loadBoard();
      }
      window.WB.emit("kanban-toggle", { open: this.kanbanOpen });
    },

    // --- branch switcher --------------------------------------------------
    // Per slug, named apart from the shell-wide `changesError` below: a
    // duplicate key in this literal is a silent no-op.
    changesReadError: {} as Record<string, any>,
    // Per slug, the read state of the change set, the branch (sync), the board
    // and the runs (`WBFail.readFold`, ADR-0070 D3). A write is locked while
    // its fact is not current.
    changesRead: {} as Record<string, any>,
    syncRead: {} as Record<string, any>,
    // The two rendered groups (#315). INVARIANT: every path that sets one must
    // set the OTHER in the SAME statement — a stale group left behind renders
    // rows under a headline while the badge already reads `—`.
    changesStaged: {} as Record<string, any>,
    changesUnstaged: {} as Record<string, any>,
    // The sync row per project (#316): the fold of `sync.status`. Same three
    // triggers as the change set, never a timer.
    syncByProject: {} as Record<string, any>,
    // The remote act in flight (`"fetch"` | `"pull"` | `"push"`), or null. One
    // slot for the whole bar: the three acts share the upstream, so a second
    // click while one is out would race it against the first — and a push's
    // round trip is long enough that a silent button reads as a dead one.
    syncBusy: null as any,
    // Same token for the Changes count (#310's nudge overlaps the open's read)
    // and for the sync row.
    _changesSeq: 0,
    _syncSeq: 0,
    // The branch chip opens a filtered picker; switching or creating goes
    // through the daemon's `branch.*` verbs. The header reflects the pick
    // optimistically.
    branchOpen: false,
    branchModal: {
      slug: null as string | null,
      filter: "",
      branches: [] as string[],
      current: "",
      primaryBranch: "",
      dirty: false,
      checkoutDirty: false,
    },
    worktreeListings: {} as Record<string, any>,

    // Only when the daemon can reach the repo on disk. NOT gated on `remote`:
    // a local-only repo still has branches.
    canSwitchBranch(p: any) {
      return WBProject.canSwitchBranch(p);
    },

    // The checkout chip of a project row: which tree Files, Changes and
    // search read, and that a click chooses another.
    checkoutTitle(p: any) {
      const name = this.checkoutOf(this.$store.projects.repoRef(p));
      return name
        ? `Files, changes and search show worktree “${name}”. Click to choose another one.`
        : "Files, changes and search show the primary tree. Click to choose a worktree.";
    },

    branchChipTitle(p: any) {
      const ref = this.$store.projects.repoRef(p);
      return WBProject.branchChipTitle(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    chipDirty(p: any) {
      const ref = this.$store.projects.repoRef(p);
      return WBProject.chipDirty(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    // The row's branch chip. Collapsed it is only the change count, and the
    // click falls through to `.project-head`'s toggle; open it is the switcher,
    // and must not ALSO collapse the row it sits on. Switching is gated on
    // reachability, not on remote (`canSwitchBranch`): an unreachable chip
    // stays inert — informational — but still swallows the click.
    branchChipClick(p: any, ev: any) {
      if (!this.$store.projects.rowOpen(p)) return;
      ev.stopPropagation();
      this.openBranchModal(p);
    },

    openBranchModal(p: any) {
      if (!this.canSwitchBranch(p)) return;
      // Reaching for the picker IS the next branch act.
      this.branchError = "";
      const ref = this.$store.projects.repoRef(p);
      // Under a selection "current" is the WORKTREE's branch (#407).
      const ck = this.checkoutOf(ref);
      const wt = ck ? (this.worktreeListings[ref]?.worktrees || []).find((w: any) => w && w.name === ck) : null;
      this.branchModal = {
        slug: ref,
        filter: "",
        branches: [...(p.branches || [p.branch])],
        current: wt ? wt.branch || "HEAD" : p.branch,
        primaryBranch: p.branch,
        dirty: !!p.dirty,
        // The dirty warning is about the tree the switch will hit (#407);
        // `dirty` stays the primary's for its picker row.
        checkoutDirty: wt ? wt.dirty === true : !!p.dirty,
      };
      this.branchOpen = true;
      this.loadBranches(ref);
      // The chip's dirty dot and the `current` seed above read the listing.
      this.ensureWorktreeListing(ref);
      this.$nextTick(() => {
        this.$refs.branchFilter?.focus();
      });
    },

    // The repo's real local branches via `branch.list` (#199). A failed read
    // empties the list (M5).
    async loadBranches(slug: any) {
      try {
        const reply = await window.WBDaemon.observe(
          "branch.list",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (this.branchModal.slug !== slug) return; // modal moved on — leave it
        if (!reply || reply.status !== "ok") {
          this.branchModal.branches = [];
          this._flashAction?.("Could not load the branches.");
          return;
        }
        // The daemon nests the CLI's `{current, branches:[]}` JSON under the
        // `branches` field (lib.rs Query reply), same as `reply.board.*` /
        // `reply.issue.*` — read one level deeper, not the top level.
        const data = reply.branches || {};
        if (Array.isArray(data.branches)) this.branchModal.branches = data.branches;
        if (data.current) this.branchModal.current = data.current;
      } catch {
        if (this.branchModal.slug === slug) {
          this.branchModal.branches = [];
          this._flashAction?.("Could not load the branches.");
        }
      }
    },

    closeBranchModal() {
      this.branchOpen = false;
    },

    // The open project's change count (#307) via `changes.list`: reloads on
    // open, sidebar refresh and run-completion nudge (#310), never on a
    // repo-wide watch. The SELECTED checkout's (#407, ADR-0063 §2).
    async loadChanges(slug: any) {
      if (!slug) return;
      // Overlapping reads can return OUT OF ORDER (as `_runsSeq`).
      const seq = ++this._changesSeq;
      try {
        const reply = await window.WBDaemon.observe(
          "changes.list",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (seq !== this._changesSeq) return; // superseded → the newer read owns it
        if (!reply || reply.status !== "ok") {
          this.changesFailed(slug, WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        const folded = WBChanges.fold(reply);
        this.changesCount[slug] = folded.count;
        this.changesStaged[slug] = folded.staged;
        this.changesUnstaged[slug] = folded.unstaged;
        this.changesReadError[slug] = "";
        this.changesRead[slug] = WBFail.readFold(this.changesRead[slug], { ok: true, value: true, at: Date.now() });
      } catch {
        if (seq === this._changesSeq) {
          this.changesFailed(slug, "the daemon did not answer");
        }
      }
    },
    // A failed `changes.list` (ADR-0070 D3). After a good read the groups and
    // the count stay, marked not current; before one, the count is absent
    // (`—`), never another repo's number or a clean tree.
    changesFailed(slug: any, reason: any) {
      const read = WBFail.readFold(this.changesRead[slug], { ok: false, reason, at: Date.now() });
      this.changesRead[slug] = read;
      if (read.goodAt) {
        this.changesReadError[slug] = WBFail.notCurrent(read, (ms) => this.fmtClock(ms));
        return;
      }
      this.changesCount[slug] = null;
      this.changesReadError[slug] = `Could not read the changes: ${reason}`;
      this.changesStaged[slug] = [];
      this.changesUnstaged[slug] = [];
    },

    // The open project's sync state (#316) via `sync.status`, which makes NO
    // network call. No timer: a launcher holding N repos must never become a
    // scheduled network client.
    async loadSync(slug: any) {
      if (!slug) return;
      const seq = ++this._syncSeq;
      try {
        const reply = await window.WBDaemon.observe(
          "sync.status",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (seq !== this._syncSeq) return; // superseded → the newer read owns it
        if (!reply || reply.status !== "ok") {
          this.syncFailed(slug, WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        const sync = WBChanges.foldSync(reply);
        this.syncByProject[slug] = sync;
        this.syncRead[slug] = WBFail.readFold(this.syncRead[slug], { ok: true, value: true, at: Date.now() });
        // Under a selected worktree the read is THAT tree's HEAD, and
        // `p.branch` is the primary's (#407), so only a primary read moves it.
        const branch = WBChanges.headBranch(sync);
        const p = this.checkoutOf(slug) ? null : this.$store.projects.projects.find((x) => this.$store.projects.repoRef(x) === slug);
        if (p && branch !== null && p.branch !== branch) p.branch = branch;
        const head = WBChanges.headOf(sync);
        if (p && head !== null) p.head = head;
      } catch {
        if (seq === this._syncSeq) {
          this.syncFailed(slug, "the daemon did not answer");
        }
      }
    },
    // A failed `sync.status` (ADR-0070 D3). After a good read the row stays,
    // and its note says it is not current; before one, the state is unknown.
    syncFailed(slug: any, reason: any) {
      const read = WBFail.readFold(this.syncRead[slug], { ok: false, reason, at: Date.now() });
      this.syncRead[slug] = read;
      const prev = this.syncByProject[slug];
      if (read.goodAt && prev && prev.state !== "unknown") {
        this.syncByProject[slug] = { ...prev, note: WBFail.notCurrent(read, (ms) => this.fmtClock(ms)) };
        return;
      }
      this.syncByProject[slug] = { ...WBChanges.foldSync(null), note: `Could not read the branch: ${reason}` };
    },

    // Fetch from the upstream — the operator's act, never a timer's. A refusal
    // is `{status:"error"}` whose message IS the core's prose.
    async syncFetch(slug: any) {
      if (this.syncBusy || this.writeLocked()) return;
      this.syncBusy = "fetch";
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "sync.fetch",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not fetch: the daemon gave no reason."),
          );
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered.
        this._changesRefused("Could not fetch: the daemon did not answer.");
      } finally {
        this.syncBusy = null;
      }
      this.loadSync(slug);
    },

    // Fast-forward from the upstream. A successful pull moves the working tree,
    // so the change set is reloaded beside the counts.
    async syncPull(slug: any) {
      if (this.syncBusy || this.writeLocked()) return;
      this.syncBusy = "pull";
      this.changesError = "";
      let moved = false;
      try {
        const reply = await window.WBDaemon.observe(
          "sync.pull",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not pull: the daemon gave no reason."),
          );
        } else {
          moved = true;
        }
      } catch {
        this._changesRefused("Could not pull: the daemon did not answer.");
      } finally {
        this.syncBusy = null;
      }
      this.loadSync(slug);
      if (moved) this.loadChanges(slug);
    },

    // Publish the branch (#320). The OPERATOR's click is the whole consent (no
    // opt-in flag, ADR-0046 amendment); a refusal's message IS the core's
    // prose. No credential UI, by decision. Push moves no file, so only the
    // counts reload.
    async syncPush(slug: any) {
      if (this.syncBusy || this.writeLocked()) return;
      this.syncBusy = "push";
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "sync.push",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not push: the daemon gave no reason."),
          );
        }
      } catch {
        this._changesRefused("Could not push: the daemon did not answer.");
      } finally {
        this.syncBusy = null;
      }
      this.loadSync(slug);
    },

    // Working-tree change count per slug (#307). `null` until a load succeeds,
    // so a failed read never reads like a clean tree; `changesReadError`
    // carries the reason into the Changes view's title.
    changesCount: {} as Record<string, any>,
    // A refused `branch.switch`/`branch.create`, held until the next branch act
    // or a project switch. Not `treeError` (the tree is fine) and not
    // `changesError` (the chip lives in THIS panel).
    branchError: "",
    _changesSub: null as any, // the run-completion nudge subscription for the open project (#310)
    // The run-completion subscription (#310, ADR-0036 amendment). The socket
    // carries EVERY repo's nudge, so the filter is here.
    mountChangesSub() {
      if (!window.WBDaemon?.subscribeChanges || !this.$store.projects.openSlug) return;
      this._changesSub = window.WBDaemon.subscribeChanges(this.$store.projects.openSlug, (frame: any) => {
        if (this.tabHidden()) return;
        // Optional-chained: a frame without wb-changes.ts must not throw
        // inside `onmessage`.
        if (WBChanges?.shouldReload?.(frame, this.$store.projects.openSlug)) {
          this.loadChanges(this.$store.projects.openSlug);
          this.loadSync(this.$store.projects.openSlug);
        }
      });
    },
    destroyChangesSub() {
      try {
        this._changesSub?.close();
      } catch {}
      this._changesSub = null;
    },
    // On `workbench:project-changed`. Refusal notes name an act against the
    // project that WAS open, and an unsent commit message (#318) is dropped.
    // After the paint, the changes-nudge socket (#310) follows the tree's
    // open/close path, and the open project's changes are read.
    gitFollowProject(slug: any) {
      this.changesError = "";
      this.branchError = "";
      if (this.commitMsgSlug !== slug) {
        this.commitMsg = "";
        this.commitMsgSlug = slug;
      }
      this.$nextTick(() => {
        this.destroyChangesSub();
        this.mountChangesSub();
        if (this.$store.projects.openSlug) this.loadChanges(this.$store.projects.openSlug);
        if (this.$store.projects.openSlug) this.loadSync(this.$store.projects.openSlug);
      });
    },
    // On `workbench:head-moved` (the files' socket heard `head.dirty`): re-read
    // what the branch drives: the chip, the sync row and the Changes count.
    gitFollowHead(ref: any) {
      this.loadChanges(ref);
      this.loadSync(ref);
      // Under a selection the chip reads the worktree's branch from the
      // listing, not `p.branch`.
      if (this.checkoutOf(ref)) this.ensureWorktreeListing(ref, true);
    },

    // ---- write controls (#318) ------------------------------------------
    // Disabled from the open repo's LIVE RUN list (`runs.list`, ADR-0047 §9).
    // A HINT, not the authority: the CLI's `guard_run_lock` refuses
    // unconditionally, and a `ralphy triage` holding the lock writes no run
    // snapshot, so a click can still be refused while these look enabled.
    //
    // A write is also locked while the fact it acts on is not current
    // (ADR-0070 D3): a failed read after a good one, or a failed first read.
    // A fact never read yet is not "not current": the panel is loading.
    writeLocked() {
      return !!this.writeLockReason();
    },
    writeLockReason() {
      const slug = this.$store.projects.openSlug;
      if (this.buildSkew) return this.BUILD_SKEW_LOCK;
      if (this.changesRead[slug]?.current === false || this.syncRead[slug]?.current === false) {
        return "The changes shown are not current. Wait for the next read, or reload the page.";
      }
      if (this.runsRead[slug]?.current === false) {
        return "The runs shown are not current. Wait for the next read, or reload the page.";
      }
      return WBChanges.writeLockReason(this.runsByProject[slug]);
    },
    BUILD_SKEW_LOCK: "This page is older than Ralphy. Save your work, and the page loads the new version.",
    // The daemon runs another build than this page (ADR-0070 D6). With no
    // unsaved work the tab reloads. With unsaved work it waits: the notice
    // stays, only saving that work is allowed, and each heartbeat asks again.
    // A console never holds the reload back: the daemon owns its PTY.
    onBuildSkew() {
      // A commit draft is not counted: the skew lock refuses the commit, so
      // it is work the tab could never save. A detached file window is: it
      // saves through this tab, and a reloaded tab no longer hears it.
      const unsaved = !!(window.WBViewer?.anyDirty?.() || window.WBNotes?.anyDirty?.() || detached.dirty());
      // A hidden tab waits: reloaded now, it would read every fact unseen.
      // The next heartbeat after it becomes visible asks again.
      if (!unsaved && !this.tabHidden()) {
        // The desk changes held back while the work was saved (a new note's
        // path among them) go out with the page's last write; the daemon
        // merges them per record.
        WBDeskSink?.setHold?.(false);
        // A detached file window would outlive the reload with no tab that
        // hears its Save; closing it sends the file home as a tab.
        detached.close();
        window.location.reload();
        return;
      }
      // A hidden tab writes no desk until it reloads: its JavaScript may not
      // know the daemon's desk.
      if (!unsaved) {
        WBDeskSink?.setHold?.(true);
        return;
      }
      if (!this.buildSkew) {
        this.buildSkew = true;
        WBDeskSink?.setHold?.(true);
      }
    },
    // True while this tab runs an older build than the daemon and holds unsaved
    // work (ADR-0070 D6); set by `onBuildSkew`.
    buildSkew: false,
    // The board's label editor, under the SAME lock: `label set` is a
    // run-lock-aware Mutate (mutate.rs).
    labelsLocked() {
      return !!this.labelLockReason();
    },
    labelLockReason() {
      if (this.buildSkew) return this.BUILD_SKEW_LOCK;
      if (this.boardRead[this.$store.projects.openSlug]?.current === false) {
        return "The board shown is not current. Wait for the next read.";
      }
      return WBChanges.writeLockReason(
        this.runsByProject[this.$store.projects.openSlug],
        "You can edit labels again when it finishes.",
      );
    },
    // The run verbs reuse the Changes derivation LITERALLY (#331). CAVEAT:
    // `guard_run_lock` is called by changes/config/mutate/sync only; `ralphy
    // run`, `ralphy triage` and `push` do NOT refuse on a live lock (runlock.rs:
    // "a signal, never a mutex"), so for those this `disabled` is the only gate.
    verbLocked() {
      return this.writeLocked();
    },
    verbTitle(verb: any) {
      return WBRun.verbLockTitle(verb, this.writeLockReason());
    },
    // The flash after a no-arg verb is sent: `Triage requested.`
    verbRequestedText(verb: any) {
      return `${verb.charAt(0).toUpperCase()}${verb.slice(1)} requested.`;
    },
    // `all` is the group head's button, which acts on every row of the group.
    rowActTitle(verb: any, all = false) {
      const locked = this.writeLockReason();
      if (locked) return locked;
      if (verb === "stage") return all ? "Stage all changes" : "Stage changes";
      if (verb === "discard") return "Discard changes";
      return all ? "Unstage all changes" : "Unstage changes";
    },
    // Push's title (#320) states the run-lock reason, as `rowActTitle` does.
    pushTitle() {
      return (
        this.syncBusyTitle("push") ||
        this.writeLockReason() ||
        this.pushAct().title
      );
    },
    pushAct() {
      return WBChanges.pushAct(this.syncByProject[this.$store.projects.openSlug]);
    },
    pullBlocked() {
      return WBChanges.pullBlocked(this.syncByProject[this.$store.projects.openSlug]);
    },
    // The remote bar's title while an act is out: the busy act names itself,
    // the other two name what they are waiting on.
    syncBusyTitle(verb: any) {
      if (!this.syncBusy) return "";
      if (this.syncBusy !== verb) return `Waiting for the ${this.syncBusy} to finish`;
      return ({ fetch: "Fetching…", pull: "Pulling…", push: "Pushing…" } as Record<string, string>)[verb] || "";
    },
    groupNote(group: any) {
      return WBChanges.groupDiscardNote(group);
    },
    commitTarget() {
      return WBChanges.commitTarget(this.syncByProject[this.$store.projects.openSlug]);
    },
    // `withOriginal` only on the UNSTAGE direction — see `wb-changes.ts`.
    groupPaths(list: any, withOriginal: any) {
      return WBChanges.groupPaths(list, withOriginal);
    },
    commitTitle() {
      const locked = this.writeLockReason();
      if (locked) return locked;
      if (!(this.changesStaged[this.$store.projects.openSlug] || []).length) {
        return "Stage a change first";
      }
      if (!this.commitMsg.trim()) return "Write a commit message first";
      return this.commitTarget().label;
    },
    canCommit() {
      return (
        !this.writeLocked() &&
        this.commitMsgSlug === this.$store.projects.openSlug &&
        !!this.commitMsg.trim() &&
        !!(this.changesStaged[this.$store.projects.openSlug] || []).length
      );
    },

    // Stage / unstage / commit, each in `syncFetch`'s shape, re-reading the
    // list on EVERY path. The list is never moved optimistically.
    async stagePaths(slug: any, paths: any) {
      if (!slug || !paths || !paths.length || this.writeLocked()) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.stage",
          window.WBDaemon.withCheckout({ repo: slug, paths }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not stage: the daemon gave no reason."),
          );
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered.
        this._changesRefused("Could not stage: the daemon did not answer.");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    async unstagePaths(slug: any, paths: any) {
      if (!slug || !paths || !paths.length || this.writeLocked()) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.unstage",
          window.WBDaemon.withCheckout({ repo: slug, paths }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not unstage: the daemon gave no reason."),
          );
        }
      } catch {
        this._changesRefused("Could not unstage: the daemon did not answer.");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    // Discard ONE row's changes (#319) — the only irreversible act here, so the
    // only one confirmed (`discardConfirm`). A cancel makes NO daemon call.
    async discardRow(slug: any, entry: any) {
      if (!slug || !entry || !entry.path || this.writeLocked()) return;
      const c = WBChanges.discardConfirm(entry);
      const ok = await this.askConfirm({
        title: c.title,
        message: c.message,
        confirmLabel: c.confirmLabel,
        danger: true,
      });
      // A read may have failed while the dialog was open: a discard cannot
      // be undone, so the lock is asked again.
      if (!ok || this.writeLocked()) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.discard",
          window.WBDaemon.withCheckout({ repo: slug, paths: [entry.path] }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not discard: the daemon gave no reason."),
          );
        }
      } catch {
        this._changesRefused("Could not discard: the daemon did not answer.");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    async commitStaged(slug: any) {
      // Never commit a draft composed for another project.
      if (this.commitMsgSlug !== slug || this.writeLocked()) return;
      const message = this.commitMsg.trim();
      if (!slug || !message) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.commit",
          window.WBDaemon.withCheckout({ repo: slug, message }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not commit: the daemon gave no reason."),
          );
        } else {
          // Cleared on success ONLY: a refused commit must not eat the message.
          this.commitMsg = "";
        }
      } catch {
        this._changesRefused("Could not commit: the daemon did not answer.");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    // Filtered (case-insensitive substring), current pinned to the top.
    branchList() {
      const q = this.branchModal.filter.trim().toLowerCase();
      const all = this.branchModal.branches;
      const hit = q ? all.filter((b) => b.toLowerCase().includes(q)) : all.slice();
      const cur = this.branchModal.current;
      return hit.sort((a, b) => (a === cur ? -1 : b === cur ? 1 : a.localeCompare(b)));
    },

    // The Files bar's checkout chip (ADR-0063 amendment 2026-09-16 b): shown
    // once the repo has a worktree; a pick sets the #406 selection (what
    // Files, Changes, diff and Find show), never where a console is launched.
    hasWorktrees(p: any) {
      return WBProject.hasWorktrees(this.worktreeListings[this.$store.projects.repoRef(p)] || null);
    },
    openCheckoutChip(p: any, anchor: any) {
      const ref = this.$store.projects.repoRef(p);
      const listing = this.worktreeListings[ref] || null;
      const mine = (this.liveSessions || []).filter((s) => WBSessionRoute.matchesRepo(s, ref));
      window.WBConsole.checkoutMenu({
        anchor,
        host: document.body,
        rows: window.WBConsole.checkoutMenuRows(listing, this.checkoutOf(ref), mine, p.branch, !!p.dirty),
        onPick: (row: any) => this.setCheckout(ref, row.primary ? null : row.name),
        onRemove: (row: any) => this.removeWorktree(ref, row),
      });
    },

    // The commit message being composed (#318). One box, but it belongs to
    // `commitMsgSlug` ONLY: a message typed for repo A must never land as repo
    // B's commit. Cleared on success only.
    commitMsg: "",
    commitMsgSlug: null,

    // --- the selected checkout (#406, ADR-0063 §4) ----------------------------
    // A `worktree.remove` in flight, per repo ref: the chip's menu greys the
    // row and a second click is ignored until the re-read lands.
    worktreeRemoving: {} as Record<string, any>,
    // The selected checkout per repo ref (#406, ADR-0063 §4): the REACTIVE copy
    // of `WBConsole`'s desk mirror (a closure variable there is invisible to
    // Alpine). `worktreeListings` is the last `worktree.list` reply per ref.
    checkouts: {} as Record<string, any>,
    checkoutOf(ref: any) {
      return this.checkouts[ref] || null;
    },
    chipLabel(p: any) {
      const ref = this.$store.projects.repoRef(p);
      return WBProject.chipLabel(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    // The reactive map is REPLACED so Alpine sees it; persistence goes to the
    // desk mirror; the files remount an open tree built for another checkout.
    setCheckout(ref: any, name: any) {
      const next = { ...this.checkouts };
      if (name) next[ref] = String(name);
      else delete next[ref];
      this.checkouts = next;
      window.WBConsole?.setCheckout?.(ref, name || null);
      if (this.$store.projects.openSlug === ref) sendWindow(window, "workbench:checkout-changed");
      // The Changes panel and the sync row are the SELECTED checkout's (#407).
      // Another tree's change set is not this tree's "last value" (ADR-0070
      // D3): its reads start over.
      delete this.changesRead[ref];
      delete this.syncRead[ref];
      if (this.$store.projects.openSlug === ref) {
        this.loadChanges(ref);
        this.loadSync(ref);
      }
    },
    // The daemon answered `unknown checkout` for `name`: drop the selection —
    // unless it already moved on, in which case a late reply says nothing.
    checkoutGone(ref: any, name: any) {
      if (this.checkoutOf(ref) !== name) return;
      this.setCheckout(ref, null);
      this._flashAction(`Worktree ${name} no longer exists. Showing the primary tree.`);
    },
    // The chip needs the worktree's BRANCH, which only `worktree.list` knows:
    // one read per ref, `force` re-reads (after a branch act the chip
    // converges from this reply, #407). A forced re-read that fails DROPS the
    // cached entry rather than showing the pre-act branch. Newest read wins.
    async ensureWorktreeListing(ref: any, force = false) {
      if (!ref || ref === "~" || (this.worktreeListings[ref] && !force)) return;
      const seq = (this._listingSeq = (this._listingSeq || 0) + 1);
      let listing: any = null;
      try {
        const reply = await window.WBDaemon.observe("worktree.list", { repo: ref });
        if (reply && reply.status === "ok") listing = reply.checkouts || null;
      } catch {}
      if (seq !== this._listingSeq) return; // superseded → the newer read owns it
      if (listing || force) {
        this.worktreeListings = { ...this.worktreeListings, [ref]: listing };
        window.WBConsole?.ingestWorktrees?.(ref, listing);
      }
    },
    // Copy the desk mirror's selections into the reactive map once the desk
    // has landed (boot, and again after a login under the `Session` policy),
    // and bring an already-open tree in line with what it now says.
    adoptDeskCheckouts() {
      const before = this.$store.projects.openSlug ? this.checkoutOf(this.$store.projects.openSlug) : null;
      this.checkouts = window.WBConsole?.checkouts?.() || {};
      if (this.$store.projects.openSlug) sendWindow(window, "workbench:checkout-changed");
      if (this.$store.projects.openSlug) {
        this.ensureWorktreeListing(this.$store.projects.openSlug);
        // The desk can land AFTER the open's own reads: re-read under the
        // restored selection, only when it differs (two git spawns otherwise).
        if (this.checkoutOf(this.$store.projects.openSlug) !== before) {
          this.loadChanges(this.$store.projects.openSlug);
          this.loadSync(this.$store.projects.openSlug);
        }
      }
    },
    // The create row shows only when the typed name matches no existing branch.
    canCreateBranch() {
      const name = this.branchModal.filter.trim();
      if (!name) return false;
      return !this.branchModal.branches.some((b) => b.toLowerCase() === name.toLowerCase());
    },

    // Enter = act on the top match, else create the typed branch (quick-pick).
    branchEnter() {
      const list = this.branchList();
      if (list.length) this.switchBranch(list[0]);
      else if (this.canCreateBranch()) this.createBranch();
    },

    // Under a selected worktree (#407) the act lands on THAT tree's HEAD:
    // `p.branch` is the primary's and must not move, so no optimistic update —
    // the chip converges from `_mutateBranch`'s forced `worktree.list` re-read.
    switchBranch(name: any) {
      if (this.writeLocked()) {
        this._flashAction(this.writeLockReason());
        this.closeBranchModal();
        return;
      }
      if (name !== this.branchModal.current) {
        const slug = this.branchModal.slug;
        const checkout = this.checkoutOf(slug);
        const p = checkout ? null : this.$store.projects.projects.find((x) => this.$store.projects.repoRef(x) === slug);
        const prev = p ? p.branch : null;
        if (p) p.branch = name; // optimistic — the chip updates immediately
        window.WB.emit("branch-switch", { project: slug, branch: name, checkout });
        // The run-lock-aware `branch.switch` Mutate (#199): refusal → revert.
        this._mutateBranch("branch.switch", slug, name, () => {
          if (p) p.branch = prev!;
        });
      }
      this.closeBranchModal();
    },

    createBranch() {
      if (!this.canCreateBranch()) return;
      if (this.writeLocked()) {
        this._flashAction(this.writeLockReason());
        return;
      }
      const name = this.branchModal.filter.trim();
      const from = this.branchModal.current;
      const slug = this.branchModal.slug;
      const checkout = this.checkoutOf(slug);
      const p = checkout ? null : this.$store.projects.projects.find((x) => this.$store.projects.repoRef(x) === slug);
      const prevBranch = p ? p.branch : null;
      const prevBranches = p ? [...(p.branches || [])] : null;
      if (p) {
        p.branches = [...(p.branches || []), name];
        p.branch = name; // a fresh branch is checked out onto
      }
      window.WB.emit("branch-create", { project: slug, name, from, checkout });
      this._mutateBranch("branch.create", slug, name, () => {
        if (p) {
          p.branch = prevBranch!;
          p.branches = prevBranches!;
        }
      });
      this.closeBranchModal();
    },

    // The chip menu's trash action: `worktree.remove` (#409). The listing is
    // the truth on every path (a `branch kept` reply is an error whose
    // directory is gone), so the selection resets from the re-read
    // (`checkoutAfterListing`), never from the reply's status. A refusal lands
    // verbatim in a notice with one OK: the menu it came from has closed.
    // A cancel makes NO daemon call.
    async removeWorktree(slug: any, w: any) {
      if (!slug || !w || w.primary || this.worktreeRemoving[slug]) return;
      const ok = await this.askConfirm({
        title: `Delete worktree ${w.name}?`,
        message:
          "The worktree folder is deleted from the disk. " +
          "Its branch is deleted too, unless it has commits that are not on the base branch.",
        confirmLabel: "Delete",
        danger: true,
      });
      if (!ok || this.worktreeRemoving[slug]) return;
      this.worktreeRemoving = { ...this.worktreeRemoving, [slug]: w.name };
      const refused = (message: any) =>
        window.WBConsole.askNotice({ title: `Could not delete worktree ${w.name}`, message });
      try {
        const reply = await window.WBDaemon.observe("worktree.remove", { repo: slug, name: w.name });
        if (WBFail.isError(reply)) {
          refused(WBFail.cause(reply, "The daemon refused to delete the worktree."));
        } else {
          this._flashAction(`Worktree ${w.name} deleted`);
        }
      } catch {
        refused("Could not reach the daemon. Check whether the worktree was deleted.");
      } finally {
        await this.ensureWorktreeListing(slug, true);
        const ck = this.checkoutOf(slug);
        if (ck && WBProject.checkoutAfterListing(ck, this.worktreeListings[slug]) === null) {
          this.checkoutGone(slug, ck);
        }
        // Cleared last: no entry means the re-read landed too.
        const next = { ...this.worktreeRemoving };
        delete next[slug];
        this.worktreeRemoving = next;
      }
    },

    // Await a `branch.*` Mutate; on a `{status:"error"}` refusal (a held
    // run.lock, ADR-0036 §6) run `revert` and report the verbatim message under
    // the chip AND in the runs flash (the aside is closed by default). Carries
    // the selected checkout (#407): the worktree's HEAD moves, never the
    // primary's.
    async _mutateBranch(verb: any, slug: any, name: any, revert: any) {
      try {
        const reply = await window.WBDaemon.observe(
          verb,
          WBDaemon.withCheckout({ repo: slug, name }, this.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          revert();
          this._branchRefused(
            WBFail.failed(
              reply,
              verb === "branch.create"
                ? "Could not create the branch: the daemon gave no reason."
                : "Could not switch branch: the daemon gave no reason.",
            ),
          );
        }
      } catch {
        // A transport throw is NOT a refusal, so the optimistic update STAYS —
        // the verb may have landed.
        this._branchRefused("Could not reach the daemon. Check whether the branch changed.");
      } finally {
        // Re-read the listing on every path (an unconfirmed switch may have
        // landed); a moved HEAD also changes the working tree and sync row.
        this.ensureWorktreeListing(slug, true);
        this.loadChanges(slug);
        this.loadSync(slug);
      }
    },
    // The Projects panel's counterpart to `_changesRefused`.
    _branchRefused(msg: any) {
      this.branchError = msg || "";
      this._flashAction(msg);
    },

    // --- Runs panel -------------------------------------------------------
    // Monotonic hydration token: overlapping `runs.list` replies can land OUT
    // OF ORDER; only the newest hydration commits.
    _runsSeq: 0,
    // One entry per `runid`: issue queue + per-issue status, live phase, the
    // current issue's plan.md (helpers in wb-runs.ts, `WBRun`).
    runsByProject: {} as Record<string, any>,
    // An error must never render as "No active runs": an empty project and an
    // unreadable one are different facts (ADR-0047 §6).
    runsError: "",
    currentRunId: null as string | null,
    // The clock's "now", advanced by the tick in `init` while the panel is
    // open. STATE, not `Date.now()` in the getter: Alpine only re-renders what
    // it can observe changing.
    nowMs: Date.now(),
    // The trail node the operator arrived at from the board (#301): a marker,
    // not a selection.
    trailFocus: null,
    runMenu: false,
    planSection: "",

    // Hydrate from `runs.list` (ADR-0047 §9), by REPLACEMENT — a snapshot is
    // state, not a log. On panel open and project change.
    async hydrateRuns() {
      const slug = this.$store.projects.openSlug;
      // Clear FIRST: a stale error must not outlive its project.
      this.runsError = "";
      if (!slug) return;
      const prevRuns = this.runsByProject[slug] || [];
      const seq = ++this._runsSeq;
      try {
        const reply = await window.WBDaemon.observe("runs.list", { repo: slug });
        // Superseded while in flight: the newer hydration owns the state.
        if (seq !== this._runsSeq || this.$store.projects.openSlug !== slug) return;
        if (reply?.status !== "ok") {
          this.runsFailed(slug, WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        this.runsRead[slug] = WBFail.readFold(this.runsRead[slug], { ok: true, value: true, at: Date.now() });
        this.runsByProject[slug] = (reply.runs || []).map((d: any) => {
          const run = WBRun.fromSnapshot(d);
          // A push arrives on every snapshot write (~every few hundred ms);
          // re-fetching an unchanged plan would blank the viewer each time.
          const prev = prevRuns.find((p: any) => p.runid === run.runid);
          if (prev && prev.planPath === run.planPath) {
            run.planMd = prev.planMd;
            run.planReadFailed = prev.planReadFailed;
          }
          return run;
        });
        // A run's id is a key, not a name: the line counts the runs instead.
        const bad = reply.unreadable || [];
        this.runsError = bad.length
          ? `Could not read ${bad.length} saved run${bad.length > 1 ? "s" : ""}. ` +
            "The file is damaged or from another version of Ralphy."
          : "";
        // Keep the selected run while it is still listed.
        const listed = this.projectRuns();
        this.currentRunId = listed.some((r: any) => r.runid === this.currentRunId)
          ? this.currentRunId
          : listed[0]?.runid || null;
        // Only while showing: a whole-plan `file.read` nobody can see is cost.
        if (this.runsOpen) await this.loadRunPlan();
      } catch (err: any) {
        if (seq !== this._runsSeq || this.$store.projects.openSlug !== slug) return;
        // A transport failure is a read failure, not an idle project.
        this.runsFailed(slug, WBFail.why({ message: err?.message }, "the daemon did not answer"));
      }
    },
    // A failed `runs.list` (ADR-0070 D3). After a good read the runs stay,
    // marked not current; before one, there are none and the panel says why.
    runsFailed(slug: any, reason: any) {
      const read = WBFail.readFold(this.runsRead[slug], { ok: false, reason, at: Date.now() });
      this.runsRead[slug] = read;
      if (read.goodAt) {
        this.runsError = WBFail.notCurrent(read, (ms) => this.fmtClock(ms));
        return;
      }
      this.runsByProject[slug] = [];
      this.runsError = `Could not read the runs: ${reason}.`;
    },

    // Read the selected run's plan via `file.read` (the document carries its
    // PATH, never its text). A refusal KEEPS the last good text and only flags
    // it (#330); `runsError` is untouched. Reads the PRIMARY tree — no
    // `checkout` (#406): a run takes the primary tree (ADR-0063 §7). Same for
    // `loadPlan`, `diffWorkSide` and the git-backed panels.
    async loadRunPlan() {
      const run = this.currentRun();
      if (!run?.planPath) return;
      try {
        const reply = await window.WBDaemon.observe("file.read", {
          repo: this.$store.projects.openSlug,
          path: run.planPath,
        });
        if (reply?.status === "ok") {
          run.planMd = reply.content || "";
          run.planReadFailed = false;
        } else {
          run.planReadFailed = true;
        }
      } catch {
        run.planReadFailed = true;
      }
      // A replacement mints new run objects: a `run` no longer current belongs
      // to a superseded hydration.
      if (this.currentRun() !== run) return;
      // Reassign the section only when the chosen heading is gone.
      const hs = this.planHeadings(run);
      if (!hs.includes(this.planSection)) this.planSection = hs[0] || "";
    },

    // The open project's runs (the panel is project-scoped).
    projectRuns() {
      return this.runsByProject[this.$store.projects.openSlug] || [];
    },
    // The selected run, falling back to the first when the id is stale (e.g. the
    // project changed).
    currentRun() {
      const runs = this.projectRuns();
      return runs.find((r: any) => r.runid === this.currentRunId) || runs[0] || null;
    },
    selectRun(runid: any) {
      this.currentRunId = runid;
      this.trailFocus = null; // the arrival marker belonged to the run we left
      // reset the section dropdown to the new run's first non-Steps heading
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      // each run has its own plan; the viewer follows the selection.
      this.loadRunPlan();
    },

    // Thin delegations to the faithful helpers in wb-runs.ts.
    runPhaseLabel(run: any) {
      return run ? WBRun.runPhaseLabel(run) : "";
    },
    runTitle(run: any) {
      return WBRun.runTitle(run);
    },
    runIdentity(run: any) {
      return WBRun.runIdentity(run);
    },
    // Reading `nowMs` subscribes this binding to the 1 s tick.
    runClock(run: any) {
      return WBRun.phaseClock(run, this.nowMs);
    },
    // When this phase began, and the run's whole elapsed time.
    clockTitle(run: any) {
      if (!run) return "";
      const parts: any[] = [];
      const at = (iso: any) =>
        new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      if (run.since) parts.push(`phase since ${at(run.since)}`);
      if (run.startedAt) {
        const ms = Math.max(0, this.nowMs - Date.parse(run.startedAt));
        const h = Math.floor(ms / 3_600_000);
        const m = Math.floor((ms % 3_600_000) / 60_000);
        parts.push(`run started ${at(run.startedAt)} (${h > 0 ? `${h}h ${m}m` : `${m}m`})`);
      }
      return parts.join(" · ");
    },
    issueState(run: any, iss: any) {
      return WBRun.issueState(run, iss);
    },
    issueGlyph(run: any, iss: any) {
      return WBRun.glyph(run, iss);
    },
    sleepLabel(run: any) {
      return WBRun.sleepText(run?.sleep);
    },
    nodeTitle(run: any, iss: any) {
      if (!run || !iss) return "";
      const st = WBRun.issueState(run, iss);
      let t = `#${iss.number} — ${iss.title} · ${WBRun.LABEL[st] || st}`;
      // Per-issue: tier routing gives two issues of one run different models.
      const seg = WBRun.modelEffort(iss.model, iss.effort);
      if (seg) t += ` · ${seg}`;
      if (iss.blockedBy?.length) t += ` (blocked by ${iss.blockedBy.map((n: any) => "#" + n).join(", ")})`;
      return t;
    },
    // Run → board (#301): a trail node opens that issue's detail. The Runs
    // panel closes first (`z-index: 150`, sharing the drawer's right edge).
    // `toggleKanban()` resets `kanbanSel`, so it runs BEFORE `openIssue`.
    focusIssue(number: any) {
      window.WB.emit("run-issue-focus", { project: this.$store.projects.openSlug, runid: this.currentRun()?.runid, issue: number });
      this.runsOpen = false;
      this.trailFocus = null;
      if (!this.kanbanOpen) this.toggleKanban();
      this.openIssue(number);
    },

    // Board → run (#301): the card's run pill opens the Runs panel on THAT run,
    // marking the issue in the trail. The board stays open behind it.
    openRunFor(number: any) {
      const hit = window.WBKanban.runningFor(number, this.projectRuns());
      if (!hit) return;
      this.currentRunId = hit.runid;
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      this.loadRunPlan();
      // `toggleRuns()` would CLOSE an already-open panel — only open it.
      if (!this.runsOpen) this.toggleRuns();
      this.trailFocus = number;
      this.$nextTick(() =>
        document
          .querySelector('.trail-node[data-issue="' + number + '"]')
          ?.scrollIntoView({ block: "nearest", inline: "nearest" }),
      );
    },

    runsRead: {} as Record<string, any>,
    _runsSub: null as any, // the live run-snapshot subscription for the open project, if any
    // The runid whose stop is in flight: a double-click must not dispatch two
    // `ralphy stop` children.
    runStopping: null,
    // Whether the open project has a live run: flips the toolbar between `run`
    // and `stop` (ADR-0054). Derived from the SAME list `writeLockReason`
    // reads, so the two agree, and self-clearing via `runs.dirty`.
    runIsLive() {
      return this.projectRuns().length > 0;
    },
    // Ask a live run to stop (ADR-0054). This does NOT kill anything: it
    // dispatches `ralphy stop`, which writes a request the run acts on; the
    // daemon never signals a dispatched child (ADR-0032 §5/§6). No wait: the
    // reply says the request was written; the run leaves the panel via
    // `runs.dirty` when it exits.
    async stopRun(runid: any) {
      // No runid: the run left the panel between the render and the click.
      if (!runid || this.runStopping) return;
      // ADR-0032 §6 asks for a strong confirmation. The shell's OWN dialog,
      // not `window.confirm` (blocks the page, ignores the theme).
      const ok = await this.askConfirm({
        title: "Stop this run?",
        message:
          "Stops the current issue. Commits already made are kept.",
        confirmLabel: "Stop",
        danger: true,
      });
      if (!ok) return;
      this.runStopping = runid;
      try {
        const reply = await window.WBDaemon.observe("run.stop", {
          repo: this.$store.projects.openSlug,
          runid,
        });
        if (WBFail.isError(reply)) {
          this.runVerbFailed(WBFail.failed(reply, "Could not stop the run."));
        } else {
          this._flashAction("Stop requested. The run is stopping.");
        }
      } catch {
        this._flashAction("Could not stop the run: the daemon is not connected.");
      } finally {
        this.runStopping = null;
      }
    },
    // The open project's run-snapshot subscription (#300, ADR-0047 §9).
    mountRunsSub() {
      if (!window.WBDaemon?.subscribeRuns || !this.$store.projects.openSlug) return;
      // A snapshot change means the tracker may have moved, so the same push
      // nudges the board (#301); the predicate coalesces it.
      this._runsSub = window.WBDaemon.subscribeRuns(this.$store.projects.openSlug, () => {
        if (this.tabHidden()) return;
        this.hydrateRuns();
        this.maybeRefreshBoard("runs");
      });
    },
    destroyRunsSub() {
      try {
        this._runsSub?.close();
      } catch {}
      this._runsSub = null;
    },
    // On `workbench:project-changed`. The drawer selection, the trail marker
    // and a verb refusal (#331, whose terminal frame can land long after the
    // click) of the project that WAS open are dropped. After the paint, the
    // runs socket (#300) follows the tree's open/close path.
    boardFollowProject() {
      this.kanbanSel = null;
      this.trailFocus = null;
      this.verbError = "";
      this.$nextTick(() => {
        this.destroyRunsSub();
        this.mountRunsSub();
        // Only when the board is OPEN (#301): the fold spawns a tracker CLI.
        if (this.$store.projects.openSlug && this.kanbanOpen) this.loadBoard();
        this.currentRunId = this.projectRuns()[0]?.runid || null;
        this.planSection = this.planHeadings(this.currentRun())[0] || "";
        if (this.$store.projects.openSlug) this.hydrateRuns();
      });
    },

    // --- plan viewer ------------------------------------------------------
    // The issue whose plan this panel is showing: the snapshot's `plan.issue`
    // when it has one, else the run's active issue.
    planIssueWanted(run: any) {
      return run?.planIssue ?? run?.active ?? null;
    },
    // The issue the PROSE belongs to, from the plan's own trailer. The steps
    // are keyed by issue (ADR-0047 A1) but the prose is a `file.read` of
    // `.ralphy/plan.md`, and a failed read KEEPS the last text (#330): without
    // the key the block would render the PREVIOUS issue's plan. Unkeyed prose
    // (a half-written plan) stays empty.
    planProseIssue(run: any) {
      return WBRun.planTrailerIssue(run?.planMd);
    },
    planProseIsCurrent(run: any) {
      return WBRun.planBelongsTo(run?.planMd, this.planIssueWanted(run));
    },
    // Every `##` section except Steps (its own block); none while the prose
    // belongs to another issue.
    planHeadings(run: any) {
      if (!this.planProseIsCurrent(run)) return [];
      return WBRun.headings(run?.planMd).filter((h) => h.toLowerCase() !== "steps");
    },
    // Render one `##` section as sanitized HTML. Steps render from the
    // snapshot document, not from here (#330).
    renderPlanSection(run: any, name: any) {
      if (!run || !name || !this.planProseIsCurrent(run)) return "";
      const body = WBRun.section(run?.planMd, name);
      return DOMPurify.sanitize(marked.parse(body || "_(empty)_"));
    },

    // --- the step list (the plan block is state, #330) ---------------------
    planSteps() {
      return this.currentRun()?.steps || [];
    },
    stepGlyph(status: any) {
      return WBRun.stepGlyph(status);
    },
    stepLabel(status: any) {
      return WBRun.stepLabel(status);
    },
    stepClass(status: any) {
      return WBRun.stepClass(status);
    },
    // Why the step list is empty — an unexplained blank block reads as a bug.
    stepsNote() {
      const run = this.currentRun();
      if (this.planSteps().length) return "";
      if (run?.phase === "planning") return "Writing the plan…";
      if (run?.planIssue != null) return "This plan has no steps.";
      return "No plan for this issue yet.";
    },
    // Why the prose block is empty: unreadable, not written yet, or written
    // for another issue are DIFFERENT facts.
    proseNote() {
      const run = this.currentRun();
      if (!run) return "";
      const wanted = this.planIssueWanted(run);
      if (this.planProseIsCurrent(run)) {
        // The prose IS this issue's, but may be the last good copy, not live.
        return run.planReadFailed ? "Could not read the plan. Showing the last version loaded." : "";
      }
      const theirs = this.planProseIssue(run);
      if (theirs != null) {
        // The plan on disk is the PREVIOUS issue's; name both numbers.
        return wanted != null
          ? `This plan is for #${theirs}. Waiting for the plan for #${wanted}.`
          : `This plan is for #${theirs}.`;
      }
      if (run.planReadFailed) return "Could not read plan.md.";
      if (run.phase === "planning") return "Writing the plan…";
      if (run.planMd) return "The plan is still being written.";
      return wanted != null ? `No plan for #${wanted} yet.` : "No plan for this issue yet.";
    },

    // --- run / triage / push (the daemon verbs) ---------------------------
    // The remote-trigger verbs (dispatch.rs), scoped to the open project.
    // `triage`/`push` are no-arg; `run` opens a modal for --agent,
    // the --plan-agent and --branch-mode new|current flags.
    runOpen: false,
    runsActionMsg: "",
    // A CLI refusal, held until the next verb click (#331). Distinct from the
    // 2.6 s `runsActionMsg` flash.
    verbError: "",
    // Phase 1 raw merged output of the last daemon-spawned run (wb-daemon.ts).
    rawFeed: "",
    // COLLAPSED by default and re-collapsed by every reset: a feed that opens
    // itself takes up to 30vh on every verb click. Opt-IN, not hidden.
    rawFeedOpen: false,
    runCfg: { agent: "claude", split: false, planAgent: "claude", branchMode: "new" },

    openRunModal() {
      // seed the planner to mirror the executor so an un-split run is coherent
      this.runCfg = { agent: "claude", split: false, planAgent: "claude", branchMode: "new" };
      this.runOpen = true;
    },
    closeRunModal() {
      this.runOpen = false;
    },
    // The faithful `ralphy run …` line the chosen options map to.
    runCommandPreview() {
      const c = this.runCfg;
      let s = `run --agent ${c.agent}`;
      if (c.split && c.planAgent !== c.agent) s += ` --plan-agent ${c.planAgent}`;
      s += ` --branch-mode ${c.branchMode}`;
      return s;
    },
    // Dismiss drops the buffer AND returns the box to its collapsed default.
    dismissFeed() {
      this.rawFeed = "";
      this.rawFeedOpen = false;
    },
    // What every verb click resets. The feed is cleared, not appended to, and
    // re-collapsed: one expansion is a decision about THAT output.
    _resetVerbSurface() {
      this.verbError = "";
      this.rawFeed = "";
      this.rawFeedOpen = false;
    },
    startRun() {
      this._resetVerbSurface();
      const c = this.runCfg;
      const planAgent = c.split && c.planAgent !== c.agent ? c.planAgent : null;
      window.WB.emit("run-start", {
        project: this.$store.projects.openSlug,
        agent: c.agent,
        planAgent,
        branchMode: c.branchMode,
        command: this.runCommandPreview(),
      });
      this._flashAction("Run started.");
      this.closeRunModal();
    },
    // triage / push: the verb name is the whole intent; the client never
    // composes a command line.
    fireVerb(verb: any) {
      this._resetVerbSurface();
      window.WB.emit("command", { project: this.$store.projects.openSlug, verb });
      this._flashAction(this.verbRequestedText(verb));
    },
    // From wb-daemon.ts on a TERMINAL frame only; an empty note is a no-op.
    runVerbFailed(msg: any) {
      if (msg) this.verbError = msg;
    },
    // A refusal from the CHANGES panel lands in that panel and STAYS; the flash
    // is kept beside it. `runs-verb-error`'s counterpart (#331).
    _changesRefused(msg: any) {
      this.changesError = msg || "";
      this._flashAction(msg);
    },

    // --- Kanban board -----------------------------------------------------
    // The open project's issues in four columns (window.WBKanban). Read-only
    // except labels, the one mutation that moves a card.
    KANBAN: window.WBKanban,
    // Fed by `board.list` (#198): rows adapted to the issue shape, and the
    // repo's name→color label map. Empty until `loadBoard()` resolves.
    boardIssues: {} as Record<string, any>,
    // Refresh bookkeeping (#301). `_boardLoadedAt` is stamped at fold START,
    // before any await, so the min-gap measures spacing between STARTS and an
    // erroring board throttles like a healthy one. `_boardPending` COALESCES a
    // trigger that arrived mid-fold into one follow-up load.
    _boardLoadedAt: 0,
    kanbanSel: null, // the selected issue number → opens the detail drawer

    // --- the repo's ready plan, on the board -------------------------------
    boardLabels: {} as Record<string, Map<string, string>>,
    // A `board.list` failure (#207): a broken tracker connection must never
    // read as "no work to do".
    boardError: {} as Record<string, any>,
    _boardPending: false,
    boardRefreshing: false,
    // The daemon awaits the board CLI with no timeout of its own; a wedged `gh`
    // must not disable the board for the page's life. Generous: a real fold
    // makes several calls.
    BOARD_FOLD_TIMEOUT_MS: 90000,
    kanbanFilter: "", // search box (title / #num / body / label)
    kanbanLabel: "__all", // label filter: __all | __none | <label>
    kanbanSort: "num-desc", // Backlog sort (Ready columns keep graph order)
    // A FINALIZED `.ralphy/plan.md` is executed by the next run (the trailer is
    // the resume signal, `WBRun.planTrailerIssue`), so the board shows it and
    // can throw it away. `planByProject[slug] = { md, summary }`, replaced on
    // every board load via the same confined `file.read`. Daemon-only (#300).
    planByProject: {} as Record<string, any>,
    planModal: { open: false, issue: null },

    async loadPlan(slug: any) {
      if (!slug) return;
      try {
        const reply = await window.WBDaemon.observe("file.read", {
          repo: slug,
          path: ".ralphy/plan.md",
        });
        // A refusal is the ORDINARY case (no plan sitting around): it clears
        // the entry rather than raising an error.
        const md = reply?.status === "ok" ? reply.content || "" : "";
        this.planByProject[slug] = md ? { md, summary: WBRun.planSummary(md) } : null;
      } catch {
        this.planByProject[slug] = null;
      }
    },
    // The open project's plan, or null. No trailer (`summary.issue` null) is a
    // plan still being written: not offered.
    openPlan() {
      const held = this.planByProject[this.$store.projects.openSlug];
      return held && held.summary.issue != null ? held : null;
    },
    // The plan for ONE card: only ever shown against the issue it names.
    planFor(number: any) {
      const held = this.openPlan();
      return held && held.summary.issue === number ? held : null;
    },
    // A plan left over from a closed issue is residue, and the pill says so.
    planIssueIsOpen() {
      const held = this.openPlan();
      if (!held) return true;
      const iss = this.projectIssues().find((i: any) => i.number === held.summary.issue);
      // Absent from the fold (filtered or cold): assume open.
      return !iss || iss.state !== "closed";
    },
    planPillLabel(number: any) {
      const held = this.planFor(number);
      return held ? WBRun.planPillLabel(held.summary, this.planIssueIsOpen()) : "";
    },
    planPillWarns(number: any) {
      const held = this.planFor(number);
      return !!held && WBRun.planPillWarns(held.summary, this.planIssueIsOpen());
    },
    // The head chip: for a plan whose issue is filtered out of the board or
    // absent from the fold, which would otherwise be invisible AND
    // undiscardable.
    planChipLabel() {
      const held = this.openPlan();
      if (!held) return "";
      return `#${held.summary.issue} · ${WBRun.planPillLabel(held.summary, this.planIssueIsOpen())}`;
    },
    openPlanModal() {
      const held = this.openPlan();
      if (!held) return;
      this.planModal = { open: true, issue: held.summary.issue };
    },
    closePlanModal() {
      this.planModal.open = false;
    },
    // The WHOLE document, through the Runs panel's sanitize→markdown pipeline.
    renderPlanDoc() {
      const held = this.openPlan();
      if (!held) return "";
      return DOMPurify.sanitize(marked.parse(held.md));
    },
    // The verdict is the runner's own test — zero open steps — not the
    // heading's claim.
    planVerdict() {
      const s = this.openPlan()?.summary;
      if (!s) return null;
      return {
        heading: s.heading || (s.infeasible ? "Feasible: no" : "Feasible"),
        reason: s.reason,
        needsSplit: s.needsSplit,
        infeasible: s.infeasible,
        steps: s.steps,
        openSteps: s.openSteps,
      };
    },
    discardTitle() {
      return (
        this.writeLockReason() ||
        "Delete this plan. The next run plans this issue again."
      );
    },
    // `plan.discard` carries no path (the daemon fixes the target). Gated
    // while a run holds the repo: a run owns the plan it is executing.
    async discardPlan() {
      const held = this.openPlan();
      if (!held || this.writeLocked()) return;
      const ok = await this.askConfirm({
        title: "Discard this plan?",
        message:
          `Deletes the plan for #${held.summary.issue}. The next run plans it again.`,
        confirmLabel: "Discard",
        danger: true,
      });
      if (!ok) return;
      const slug = this.$store.projects.openSlug;
      try {
        const reply = await window.WBDaemon.write("plan.discard", { repo: slug });
        if (WBFail.isError(reply)) {
          this._flashAction(WBFail.failed(reply, "Could not discard the plan."));
          return;
        }
        this._flashAction(`Plan for #${held.summary.issue} discarded.`);
        this.closePlanModal();
      } catch {
        this._flashAction("Could not discard the plan: the daemon is not connected.");
      } finally {
        // Re-read on EVERY path: the panel shows what is on disk now.
        await this.loadPlan(slug);
      }
    },

    // The open project's issues (#198). Empty until `loadBoard()` populates it.
    projectIssues() {
      return this.boardIssues[this.$store.projects.openSlug] || [];
    },

    // The whole-tracker board fold via `board.list`, cached under the slug. No
    // daemon or a transport error leaves the board empty.
    async loadBoard() {
      const slug = this.$store.projects.openSlug;
      if (!slug) return;
      // A fold in flight: remember the trigger. Dropping it loses a project
      // switch and a label write (an older fold reverts the optimistic edit).
      if (this.boardRefreshing) {
        this._boardPending = true;
        return;
      }
      this.boardRefreshing = true;
      this._boardPending = false;
      this._boardLoadedAt = Date.now();
      // The ready plan rides every board trigger. NOT awaited: a plan read
      // must never delay the rows.
      this.loadPlan(this.$store.projects.openSlug);
      try {
        const reply: any = await Promise.race([
          window.WBDaemon.observe("board.list", { repo: slug }),
          new Promise((_, rej) =>
            setTimeout(() => rej(new Error("board fold timed out")), this.BOARD_FOLD_TIMEOUT_MS),
          ),
        ]);
        if (WBFail.isError(reply)) {
          const msg = WBFail.failed(reply, "Could not load the board.");
          this.boardFailed(slug, msg);
          this._flashAction?.(msg);
          return;
        }
        const board = reply.board || {};
        this.boardIssues[slug] = (board.issues || []).map((r: any) => this.boardRowToIssue(r));
        // A Map, so a label named `constructor` reads as missing. A blank
        // color is skipped: a bare "#" is truthy and masks `labelColor`'s fallback.
        const colors = new Map<string, string>();
        for (const l of board.labels || []) {
          if (!l.color) continue;
          colors.set(l.name, "#" + String(l.color).replace(/^#/, ""));
        }
        this.boardLabels[slug] = colors;
        this.boardError[slug] = null;
        this.boardRead[slug] = WBFail.readFold(this.boardRead[slug], { ok: true, value: true, at: Date.now() });
        // Fold rows carry `body: ""`: re-merge the open drawer's detail or it
        // goes blank on every refresh.
        if (this.kanbanSel != null) this.loadIssueDetail(this.kanbanSel);
      } catch {
        this.boardFailed(slug, "Could not load the board.");
        this._flashAction?.("Could not load the board.");
      } finally {
        this.boardRefreshing = false;
        // Exactly ONE follow-up for whatever was coalesced away, or for a
        // project that changed underneath this fold. `_boardPending` is cleared
        // by the recursive call before it awaits, so this settles.
        if (this._boardPending || this.$store.projects.openSlug !== slug) {
          this._boardPending = false;
          if (this.$store.projects.openSlug && this.kanbanOpen) this.loadBoard();
        }
      }
    },

    // A failed `board.list` (ADR-0070 D3). After a good read the cards stay,
    // under a banner that says they are not current; before one, there are no
    // cards and the banner says why. Moving a card is locked meanwhile.
    boardFailed(slug: any, msg: any) {
      const read = WBFail.readFold(this.boardRead[slug], { ok: false, reason: msg, at: Date.now() });
      this.boardRead[slug] = read;
      if (read.goodAt) {
        this.boardError[slug] = WBFail.notCurrent(read, (ms) => this.fmtClock(ms));
        return;
      }
      this.boardIssues[slug] = [];
      this.boardError[slug] = msg;
    },

    // The one door every refresh trigger goes through (#301): the predicate
    // (wb-kanban.ts) decides.
    maybeRefreshBoard(trigger: any) {
      const ok = window.WBKanban.shouldRefresh({
        trigger,
        sinceMs: Date.now() - this._boardLoadedAt,
        boardOpen: this.kanbanOpen,
        docVisible: document.visibilityState === "visible",
        focused: document.hasFocus(),
      });
      if (ok) this.loadBoard();
    },
    // The board head's refresh control.
    refreshBoard() {
      this.maybeRefreshBoard("manual");
    },
    // The slow backstop: the PREDICATE, not the timer, decides.
    boardBackstopTick() {
      this.maybeRefreshBoard("backstop");
    },

    // A CLI fold row → the issue shape `wb-kanban.ts` expects. Body + comments
    // are absent from the fold (`issue.show` fills them on open).
    boardRowToIssue(row: any) {
      return {
        number: row.number,
        title: row.title || "",
        state: row.state || "open",
        reason: row.reason ?? row.state_reason ?? null,
        labels: row.labels || [],
        assignees: row.assignees || [],
        blockedBy: row.blocked_by || row.blockedBy || [],
        created: row.created || "",
        updated: row.updated || "",
        body: "",
        comments: [],
      };
    },

    // The four columns after search + label filter: Backlog by the chosen
    // sort; the Ready columns in graph order; Closed newest-first.
    kanbanColumns() {
      const all = this.projectIssues();
      const K = window.WBKanban;
      const shown = all.filter((i: any) => K.matches(i, this.kanbanFilter) && K.hasLabelFilter(i, this.kanbanLabel));
      const bucket: Record<string, any[]> = { backlog: [], agent: [], human: [], closed: [] };
      for (const i of shown) bucket[K.columnOf(i)].push(i);
      return {
        backlog: K.sortBacklog(bucket.backlog, this.kanbanSort),
        // The Ready columns keep the SERVER's graph order (#198): the fold emits
        // them in `sort_queue_in_graph` order and bucketing preserves it, so
        // board order == core queue order. `K.orderGraph` would diverge — it
        // lacks the full open-set + `## Parent` context.
        agent: bucket.agent,
        human: bucket.human,
        closed: bucket.closed.sort((a, b) => (b.updated || "").localeCompare(a.updated || "")),
      };
    },
    // Per-column live count (post-filter), for the column header badge.
    kanbanCount(colId: any) {
      return (this.kanbanColumns() as Record<string, any[]>)[colId].length;
    },
    // The label set present in the project, for the filter dropdown.
    kanbanLabelOptions() {
      const seen = new Set();
      for (const i of this.projectIssues()) for (const l of i.labels || []) seen.add(l);
      return [...seen].sort();
    },

    // The run pill for a card (the actively-worked issue of a live run).
    issueRunning(number: any) {
      return window.WBKanban.runningFor(number, this.projectRuns());
    },
    // The issue drawer's run line: `Running · executing (claude)`.
    issueRunningLabel(run: any) {
      return `Running · ${this.runStateWord(run?.state)} (${run?.agent || ""})`;
    },
    // A run state as the Runs panel words it (`sleep` → "usage limit — sleeping").
    runStateWord(state: any) {
      return (state && WBRun?.LABEL?.[state]) || state || "";
    },

    // Thin delegations to the faithful helpers (used in the template).
    kanbanColumnOf(i: any) {
      return window.WBKanban.columnOf(i);
    },
    labelColor(l: any) {
      // The repo's real label hex, else the seed vocabulary.
      return this.boardLabels[this.$store.projects.openSlug]?.get(l) || window.WBKanban.labelColor(l);
    },
    labelInk(l: any) {
      return window.WBKanban.labelInk(l);
    },
    labelShort(l: any) {
      return window.WBKanban.labelMeta(l).short;
    },
    closeLabel(i: any) {
      return window.WBKanban.closeLabel(i);
    },
    kanbanColumnTitle(i: any) {
      const id = window.WBKanban.columnOf(i);
      return (window.WBKanban.COLUMNS.find((c) => c.id === id) || {}).title || id;
    },
    kfmtDate(iso: any) {
      return window.WBKanban.fmtDate(iso);
    },

    boardRead: {} as Record<string, any>,

    // --- detail drawer ----------------------------------------------------
    // The open drawer's detail-fetch failure (#302). One string: exactly one
    // drawer is open at a time.
    issueError: null,
    issueLoading: false,
    // Selection is by number, so a label move (which can change the card's
    // column) keeps the drawer pointed at the same issue.
    selectedIssue() {
      if (this.kanbanSel == null) return null;
      return this.projectIssues().find((i: any) => i.number === this.kanbanSel) || null;
    },
    openIssue(number: any) {
      this.kanbanSel = number;
      // `issue.show` merges body + comments + blockers into the cached row.
      this.loadIssueDetail(number);
    },

    async loadIssueDetail(number: any) {
      const slug = this.$store.projects.openSlug;
      this.issueError = null;
      // Set BEFORE the first await so the markup never paints `_(empty)_` for
      // a body still on the wire; cleared only by the NEWEST fetch.
      this.issueLoading = true;
      // INVARIANT (#302): a reply — content OR error — is applied only by the
      // NEWEST fetch, and only while its project+issue is still the open
      // drawer. The number alone is not enough: the board fold re-fires this
      // on every refresh, and two projects routinely carry the same number.
      const gen = (this._issueDetailGen = (this._issueDetailGen || 0) + 1);
      const stale = () =>
        gen !== this._issueDetailGen || this.$store.projects.openSlug !== slug || this.kanbanSel !== number;
      const fail = (msg: any) => {
        if (stale()) return;
        this.issueError = msg;
        this._flashAction?.(msg);
      };
      try {
        const reply = await window.WBDaemon.observe("issue.show", { repo: slug, number });
        if (WBFail.isError(reply)) {
          fail(WBFail.failed(reply, "Could not load the issue."));
          return;
        }
        if (!reply || reply.status !== "ok" || !reply.issue || typeof reply.issue !== "object") {
          fail("Could not load the issue.");
          return;
        }
        const detail = reply.issue;
        const iss = (this.boardIssues[slug] || []).find((i: any) => i.number === number);
        if (!iss || stale()) return;
        if (typeof detail.body === "string") iss.body = detail.body;
        if (Array.isArray(detail.comments)) iss.comments = detail.comments;
        if (Array.isArray(detail.blocked_by)) iss.blockedBy = detail.blocked_by;
        // Success owns the banner too: this one may land second.
        this.issueError = null;
      } catch {
        // Transport error: the drawer says so rather than reading as empty.
        fail("Could not load the issue.");
      } finally {
        if (!stale()) this.issueLoading = false;
      }
    },
    closeIssue() {
      this.kanbanSel = null;
      this.issueError = null;
      this.issueLoading = false;
    },
    // The GitHub URL of an issue on the OPEN project, from its `remoteUrl`
    // (#204); `null` with no GitHub remote. Only the lookup stays here.
    githubUrl(number: any) {
      const p = this.$store.projects.projects.find((x) => this.$store.projects.repoRef(x) === this.$store.projects.openSlug);
      return WBProject.issueUrl(p && p.remoteUrl, number);
    },

    // The selected issue's blockers, each with its live open/closed state.
    issueBlockers(iss: any) {
      if (!iss || !iss.blockedBy?.length) return [];
      const all = this.projectIssues();
      return iss.blockedBy.map((n: any) => {
        const b = all.find((x: any) => x.number === n);
        return { number: n, open: b ? b.state === "open" : false, known: !!b, title: b?.title || "" };
      });
    },

    // An issue body / comment as sanitized markdown.
    renderIssueMd(src: any) {
      return DOMPurify.sanitize(marked.parse(src || "_(empty)_"));
    },

    // --- the one allowed mutation: labels ---------------------------------
    // Toggling a label is the sole write the board permits; reflected
    // optimistically, the daemon does the real `gh` call.
    KANBAN_LABELS: Object.keys(window.WBKanban.LABELS),
    labelMenuOpen: false,
    // Opening the menu scrolls it into the drawer's viewport: on a card near
    // the bottom it can open below the fold.
    toggleLabelMenu() {
      this.labelMenuOpen = !this.labelMenuOpen;
      if (!this.labelMenuOpen) return;
      this.$nextTick(() =>
        document.querySelector(".kd-label-menu")?.scrollIntoView({ block: "nearest", inline: "nearest" }),
      );
    },
    hasLabel(iss: any, label: any) {
      return !!iss && (iss.labels || []).includes(label);
    },
    toggleLabel(iss: any, label: any) {
      if (!iss) return;
      // Defence in depth: a `:disabled` button is still reachable by keyboard
      // in some browsers.
      if (this.labelsLocked()) return;
      const has = this.hasLabel(iss, label);
      const op = has ? "remove" : "add";
      const prev = [...(iss.labels || [])];
      iss.labels = has ? iss.labels.filter((l: any) => l !== label) : [...(iss.labels || []), label];
      const slug = this.$store.projects.openSlug;
      window.WB.emit("issue-label-change", { project: slug, number: iss.number, label, op });
      // The run-lock-aware `label.set` Mutate (#199): refusal → revert + flash.
      (async () => {
        try {
          const reply = await window.WBDaemon.observe("label.set", {
            repo: slug,
            number: iss.number,
            label,
            op,
          });
          if (WBFail.isError(reply)) {
            iss.labels = prev;
            this._flashAction(WBFail.failed(reply, "Could not change the labels: the daemon gave no reason."));
            return; // a refused write changed nothing to re-read
          }
          // Re-fold so the column reflects the server, not the optimistic
          // edit (#301).
          this.maybeRefreshBoard("label");
        } catch {
          // No daemon reachable — leave the optimistic edit in place.
        }
      })();
    },

    // --- spend (the Spend tab, #358) ---------------------------------------
    // A canvas tab (ADR-0037 amendment: a closable tab may be a daemon view),
    // scoped to the open project. Everything numeric is rendered by the daemon
    // (`/api/spend`); `WBSpend` folds only which state the pane is in.
    spend: { loading: false, error: "", doc: null, slug: null },
    // The window the operator picked, echoed back by the daemon; the pane
    // renders the document's own key, never this one.
    spendPeriod: "all",
    // The tab's view model. `openSlug` is read HERE and not stashed, so closing
    // a project drops the pane to its empty state.
    spendView() {
      return window.WBSpend.state({
        project: this.$store.projects.openSlug,
        loading: this.spend.loading,
        error: this.spend.error,
        // A document for a project no longer open is stale by definition.
        doc: this.spend.slug === this.$store.projects.openSlug ? this.spend.doc : null,
        period: this.spendPeriod,
        // Titles ride whatever the board ALREADY holds; never a load
        // (`loadBoard` spawns a throttled tracker CLI).
        issues: this.boardIssues[this.$store.projects.openSlug] || [],
      });
    },
    // The window is a server-side filter: assign, then re-read.
    setSpendPeriod(key: any) {
      if (this.spendPeriod === key) return;
      this.spendPeriod = key;
      this.loadSpend();
      // The Ledger is scoped to the SAME window: a period change invalidates
      // its rows too.
      this.ledger.slug = null;
      if (this.spendPane === "ledger") this.loadLedger();
    },
    openSpend() {
      if (!this.tabs.some((t) => t.id === "spend")) {
        // Bootstrap's cash-stack: the strip renders `t.icon` as a class.
        this.tabs.push({
          id: "spend",
          kind: "spend",
          title: "Spend",
          icon: "bi bi-cash-stack",
          closable: true,
        });
      }
      this.activate("spend");
      this.loadSpend();
    },
    async loadSpend() {
      const slug = this.$store.projects.openSlug;
      // No project open is not a failure: the pane says so itself.
      if (!slug) {
        this.spend = { loading: false, error: "", doc: null, slug: null };
        return;
      }
      this.spend = { ...this.spend, loading: true, error: "" };
      let doc: any = null;
      let error = "";
      try {
        const r = await fetch(
          "/api/spend?project=" +
            encodeURIComponent(slug) +
            "&period=" +
            encodeURIComponent(this.spendPeriod || "all"),
        );
        if (r.ok) doc = await r.json();
        else error = "Could not load spend: the daemon did not answer.";
      } catch {
        error = "Could not load spend: the daemon did not answer.";
      }
      // The project changed while in flight: one cost under another's name.
      if (this.$store.projects.openSlug !== slug) return;
      this.spend = { loading: false, error, doc, slug };
    },
    // Re-read on activation and when the accordion opens or closes a project.
    refreshSpend() {
      if (!this.tabs.some((t) => t.id === "spend")) return;
      // Dropping the slug makes the next switch to the Ledger re-read. The
      // unpriced filter goes with it: it answered the PREVIOUS project's gap.
      this.ledger.slug = null;
      this.spendLedgerUnpricedOnly = false;
      this.loadSpend();
      if (this.spendPane === "ledger") this.loadLedger();
    },

    // --- the Ledger pane (#360) --------------------------------------------
    // `Overview | Ledger` inside the ONE Spend tab: two readings of the same
    // project. The Ledger is the raw per-phase grid with the daemon's unpriced
    // verdict per row (`/api/usage?project=`).
    spendPane: "overview",
    // Set by the click-through from the Overview's unpriced figure.
    spendLedgerUnpricedOnly: false,
    ledger: {
      loading: false,
      error: "",
      records: [],
      interactive: [],
      missing: [],
      daemonId: null,
      slug: null,
    },
    // What the Spend tab shows, computed ONCE per change by the `x-effect` on
    // `.spend-tab` and on `.ledger-pane`; the markup reads these, never the
    // methods. Alpine caches nothing: each of the ~40 reads ran the whole view
    // again, and the tab is `x-show`, so a board refresh paid it with the tab
    // hidden (wb_perf_487.py: 43 calls, 12 ms per refresh on a real ledger).
    spendModel: window.WBSpend.state(),
    ledgerModel: window.WBSpend.ledger(),
    ledgerView() {
      // Rows for a project no longer open are stale (as `spendView()`); the
      // peer banner is gated with them.
      const fresh = this.ledger.slug === this.$store.projects.openSlug;
      return window.WBSpend.ledger({
        project: this.$store.projects.openSlug,
        loading: this.ledger.loading,
        error: this.ledger.error,
        records: fresh ? this.ledger.records : [],
        interactive: fresh ? this.ledger.interactive : [],
        missing: fresh ? this.ledger.missing : [],
        daemonId: this.ledger.daemonId,
        unpricedOnly: this.spendLedgerUnpricedOnly,
      });
    },
    // Deferred to the first switch: the ledger is the one response that grows
    // with the project's history.
    setSpendPane(key: any) {
      this.spendPane = key;
      if (key === "ledger") this.loadLedger();
    },
    // The Overview's unpriced figure drills into the offending rows (#355).
    showUnpricedLedger() {
      this.spendLedgerUnpricedOnly = true;
      this.setSpendPane("ledger");
    },
    async loadLedger() {
      const slug = this.$store.projects.openSlug;
      // With no project open there are still PEERS to report: an empty
      // `project=` scopes the rows to none while the daemon answers `missing`.
      const want = slug || "";
      // `loading` stops a second click duplicating the heaviest request on the
      // page; `slug` is written only when the fetch lands.
      if (this.ledger.loading) return;
      if (this.ledger.slug === want) return;
      this.ledger = { ...this.ledger, loading: true, error: "" };
      let records = [];
      let interactive = [];
      let missing = [];
      let daemonId: any = null;
      let error = "";
      try {
        const r = await fetch(
          "/api/usage?project=" +
            encodeURIComponent(want) +
            "&period=" +
            encodeURIComponent(this.spendPeriod || "all"),
        );
        if (r.ok) {
          const data = await r.json();
          records = Array.isArray(data.records) ? data.records : [];
          interactive = Array.isArray(data.interactive) ? data.interactive : [];
          missing = Array.isArray(data.missing) ? data.missing : [];
          daemonId = data.daemon_id || null;
        } else error = "Could not load the ledger: the daemon did not answer.";
      } catch {
        error = "Could not load the ledger: the daemon did not answer.";
      }
      // The operator switched projects while this was in flight.
      if ((this.$store.projects.openSlug || "") !== want) {
        this.ledger = { ...this.ledger, loading: false };
        return;
      }
      this.ledger = {
        loading: false,
        error,
        records,
        interactive,
        missing,
        daemonId,
        slug: slug || null,
      };
    },

    // --- release ----------------------------------------------------------
    // The release fact stays here, because the sidebar reads it. The About,
    // What's new and update dialogs are the component in
    // wb-release-dialogs.ts, and change it only through the two methods
    // after `loadRelease` (ADR-0073 D4).
    // The release view the daemon computed (ADR-0056 §7), seeded empty.
    release: (WBRelease && WBRelease.EMPTY) || {
      current: "",
      channel: "rc",
      standing: "unknown",
      severity: "none",
      latest: null,
      gap: [],
      disabled: false,
      can_update: false,
    },
    // Dismissed by opening the panel — except urgent news.
    releaseSeen: false,

    get releaseHasNews() {
      return !!WBRelease && WBRelease.hasNews(this.release);
    },
    // What the rail draws: an urgent release ignores the dismissal.
    get releaseUnread() {
      if (!this.releaseHasNews) return false;
      return !this.releaseSeen || WBRelease.isSticky(this.release);
    },
    get releaseSummary() {
      return WBRelease ? WBRelease.gapSummary(this.release) : "";
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
    peerSessions(daemonId: any) {
      return (this.liveSessions || []).filter((s) => s.daemon_id === daemonId);
    },

    // The tab became visible (ADR-0070 D2 event 3): every open panel's facts
    // are read again, because a hidden tab read nothing. `loadRepos` also
    // reads the sessions, the fleet, the change set and the branch.
    onTabVisible() {
      this.maybeRefreshBoard("visible");
      this.loadRepos();
      this.rereadDesk();
      // Not only with the Runs panel open: the runs lock the writes.
      this.hydrateRuns();
      this.rereadOpenPanels();
      this.resumeSockets();
      this.loadRelease();
    },
    // The panels whose facts have no push: read again on visible and after
    // login while they are open (fact index: settings 3, 4; usage 3, 4). The
    // Settings dialog hears the event and reads again only when it is open
    // (ADR-0073 D5).
    rereadOpenPanels() {
      sendWindow(window, "workbench:panels-reread");
      if (this.tabs.some((t) => t.id === "spend")) this.loadSpend();
    },
    releaseRead: null as any,
    releaseStale() {
      return WBFail.notCurrent(this.releaseRead, (ms) => this.fmtClock(ms));
    },
    async loadRelease() {
      if (!WBRelease) return;
      const view = await WBRelease.read();
      // A failed read keeps what the page last knew (ADR-0056 §6), marked
      // not current (ADR-0070 D3).
      this.releaseRead = WBFail.readFold(this.releaseRead, {
        ok: !!view,
        value: true,
        reason: "the daemon did not answer",
        at: Date.now(),
      });
      if (!view) return;
      // A dismissal is for the release that was shown. A newer one is news again.
      if (view.latest !== this.release.latest) this.releaseSeen = false;
      this.release = view;
    },
    // The What's new dialog was opened: the release it shows is dismissed.
    markReleaseSeen() {
      this.releaseSeen = true;
    },
    // The operator turned the release watch on or off.
    releaseWatchChanged(enable: any) {
      this.release = { ...this.release, disabled: !enable };
    },

    // --- account menu + security -----------------------------------------
    // The daemon auth model (ADR-0032): an opt-in access token, an optional
    // password, and TOTP whose secret is shown exactly once.
    avatarMenu: false,
    security: {
      tokenSet: true, // a networked daemon always has one; localhost needs none
      passwordSet: false,
      totpEnrolled: false,
      requireLogin: false, // opt-in: mimics a non-loopback bind with TOTP
      // Opt-in: markdown may load images from other websites (the CSP
      // `img-src https:`, ADR-0032 amendment §F).
      remoteImages: false,
      policy: "session", // overwritten by probeSession()
    },
    // The Security dialog (wb-security-dialog.ts) changes the security fact
    // only here (ADR-0073 D4). `patch` holds fields of `security`.
    securityChanged(patch: any) {
      Object.assign(this.security, patch);
    },

    // ONE dropdown at a time: the shell hears `workbench:menus-close` (sent by
    // every menu trigger, this one too) and closes the account menu here.
    closeAvatarMenu() {
      this.avatarMenu = false;
    },
    toggleAvatarMenu() {
      const was = this.avatarMenu;
      sendWindow(window, "workbench:menus-close");
      this.avatarMenu = !was;
    },

    // --- login gate -------------------------------------------------------
    // An opaque overlay covers the shell while locked (`body.locked`).
    authed: true,
    // `remember` is "keep me signed in" (ADR-0032 amendment 2026-09-16):
    // opt-in, reset on every log-off.
    login: { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: false },

    async logOff() {
      this.avatarMenu = false;
      sendWindow(window, "workbench:log-off");
      // The session cookie is HttpOnly — only the server can clear it. The
      // route needs a live session (audit F5): a 401 here means the cookie
      // was already invalid, which is the same place this lands anyway.
      try {
        await fetch("/api/logout", { method: "POST" });
      } catch {}
      // Localhost/Bearer have no login gate to drop to (#205).
      if (this.security.policy === "session") {
        this.authed = false;
        this.login = { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: this.login.passwordRequired };
      }
      window.WB.emit("logoff", {});
    },

    // Re-fetch the endpoints that returned 401 while gated; the presence
    // socket self-reconnects.
    rehydrateAfterAuth() {
      this.reposError = "";
      this.loadRepos();
      this.loadIdentity();
      // `/api/agents` is gated too.
      this.loadAgents();
      // `/api/desk` is gated too: unread AND unwritable until re-read (#327);
      // the selected checkouts ride it (#406).
      window.WBConsole?.afterLogin()?.then(() => {
        this.adoptDeskCheckouts();
        this.syncDeskFailure();
      });
      // Only now is `file.read` allowed (#339).
      this.restoreView();
      // Every shown fact reads again after login (ADR-0070 D2 event 4);
      // `loadRepos` above covers the sessions, the change set and the branch.
      this.maybeRefreshBoard("login");
      // Not only with the Runs panel open: the runs lock the writes.
      this.hydrateRuns();
      this.rereadOpenPanels();
    },

    // --- TOTP digit boxes -------------------------------------------------
    // One input per digit; a paste or OTP autofill landing all 6 in the first
    // box is spread. `login.code` is the joined string.

    _otpBoxes(el: any) {
      return el.closest(".login-otp").querySelectorAll("input");
    },

    _otpSync() {
      this.login.code = this.login.digits.join("");
    },

    // Once the 6th digit lands: the password field if required, else submit.
    _otpAdvancePastLast(el: any) {
      if (this.security.passwordSet || this.login.passwordRequired) {
        this.$refs.loginPassword?.focus();
      } else {
        el.closest("form")?.querySelector(".login-btn")?.focus();
      }
    },

    _otpFill(text: any, el: any) {
      const chars = text.replace(/\D/g, "").slice(0, 6).split("");
      for (let j = 0; j < 6; j++) this.login.digits[j] = chars[j] || "";
      this._otpSync();
      const boxes = this._otpBoxes(el);
      if (chars.length >= 6) this._otpAdvancePastLast(el);
      else boxes[chars.length].focus();
    },

    otpInput(i: any, e: any) {
      const v = e.target.value.replace(/\D/g, "");
      if (v.length > 1) {
        this._otpFill(v, e.target);
        return;
      }
      e.target.value = v;
      this.login.digits[i] = v;
      this._otpSync();
      if (!v) return;
      if (i < 5) this._otpBoxes(e.target)[i + 1].focus();
      else this._otpAdvancePastLast(e.target);
    },

    otpKeydown(i: any, e: any) {
      const boxes = this._otpBoxes(e.target);
      if (e.key === "Backspace" && !e.target.value && i > 0) {
        e.preventDefault();
        this.login.digits[i - 1] = "";
        this._otpSync();
        boxes[i - 1].focus();
      } else if (e.key === "ArrowLeft" && i > 0) {
        e.preventDefault();
        boxes[i - 1].focus();
      } else if (e.key === "ArrowRight" && i < 5) {
        e.preventDefault();
        boxes[i + 1].focus();
      }
    },

    otpPaste(e: any) {
      const text = e.clipboardData?.getData("text") || "";
      this._otpFill(text, e.target);
    },

    // The url-encoded `POST /api/login` body; `remember=true` only when
    // checked (an absent field is a standard session). Pure.
    loginBody() {
      const code = (this.login.code || "").trim();
      const body = new URLSearchParams({ code });
      if (this.login.passwordRequired || this.security.passwordSet) {
        body.set("password", this.login.password || "");
      }
      if (this.login.remember) body.set("remember", "true");
      return body.toString();
    },

    async submitLogin() {
      try {
        const res = await fetch("/api/login", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.loginBody(),
        });
        if (res.ok) {
          this.login.error = "";
          this.authed = true;
          this.forgetLoginSecrets();
          this.rehydrateAfterAuth();
          window.WB.emit("login", {});
        } else {
          this.login.error = "Invalid code or password.";
        }
      } catch {
        // Only the daemon authenticates: a thrown fetch never logs in.
        this.login.error = "Cannot reach the daemon. Try again.";
      }
    },

    // The code and the password are SPENT the moment the daemon accepts them:
    // the session is the cookie now, and neither is ever replayed. Dropping
    // them keeps a plaintext password out of a live `<input>` for the rest of
    // the session — which is also what stopped the browser's password manager
    // from pairing it with the next text field the operator typed into and
    // offering to save a note's title as a username (measured 2026-09-22).
    forgetLoginSecrets() {
      this.login.code = "";
      this.login.password = "";
      this.login.digits = ["", "", "", "", "", ""];
    },

    // --- canvas tabs ------------------------------------------------------
    // The SAME terminal glyph as the New-console button and its menu rows.
    tabs: [{ id: "consoles", kind: "consoles", title: "Consoles", icon: "bi bi-terminal", closable: false }] as any[],
    active: "consoles",
    // The secondary pane (ADR-0037 §3c): `{ kind: "pin", id }` shows that tab
    // beside whichever is active, `{ kind: "mirror" }` shows the active code
    // tab twice. One slot, ever; `WBSplit.resolve` folds it against `active`
    // in `syncViewer`. `splitRatio` is the left column's share (null = half);
    // `lastLeft` the tab last read on the left, so activating the pinned tab
    // itself keeps its neighbour rather than emptying the canvas.
    slot: null as any,
    splitRatio: null as number | null,
    lastLeft: null as any,

    // Open a `.note` as a card (ADR-0064 §11). The module decides whether this
    // is a jump to a card already on the plane or a new one; this layer only
    // puts the operator on the tab that holds the stage.
    openNote(path: any, project?: any, checkout?: any) {
      const repo = project || this.$store.projects.openSlug;
      if (!repo) return;
      if (this.active !== "consoles") this.activate("consoles");
      const tree = checkout === undefined ? window.WBConsole.checkoutOf(repo) : checkout;
      this.$nextTick(() =>
        window.WBNotes.openFromExplorer({
          repo,
          checkout: tree,
          path,
          viewport: {
            width: document.getElementById("workspace")?.clientWidth || 0,
            height: document.getElementById("workspace")?.clientHeight || 0,
          },
          offset: {
            left: document.getElementById("workspace")?.scrollLeft || 0,
            top: document.getElementById("workspace")?.scrollTop || 0,
          },
        }),
      );
    },

    // `file.read`, resolving to what the pane opens with: `{content, encoding,
    // bom}` (ADR-0036 amendment 2026-09-22), or `{refused: reason}` when the
    // daemon served nothing for a reason the pane can state (`binary`, `too
    // large`, …) — the tab KEEPS its place and says so — or `null` when there
    // is nothing to hold a tab open for (`not found`, a transport drop): the
    // tab closes. `checkout` is the tab's PINNED checkout (#406).
    fetchContent(project: any, path: any, ftype: any, checkout: any) {
      const refuse = (reason: any) => {
        window.WB.emit("open-refused", { project, path, reason });
        this._flashAction?.(WBFail.failed({ reason }, "Could not open the file: the daemon gave no reason."));
        if (reason === "not found" || reason === "transport") {
          this.closeTab(fileTabId(project, path, checkout));
          return null;
        }
        return { refused: reason };
      };
      // An image is `file.image` (ADR-0049): a `data:` URL. Same refusal shape.
      if (ftype === "image") {
        let refusal: any = null;
        return WBDaemon.readImage(project, path, (reason: any) => (refusal = refuse(reason)), checkout)
          .then((url: any) => (url == null ? refusal : { content: url }))
          .catch(() => refuse("transport"));
      }
      return WBDaemon.observe("file.read", WBDaemon.withCheckout({ repo: project, path }, checkout))
        .then((reply) => {
          if (!WBFail.isError(reply)) {
            return { content: reply.content, encoding: reply.encoding, bom: !!reply.bom };
          }
          return refuse(WBFail.message(reply, "refused"));
        })
        .catch(() => refuse("transport"));
    },

    // --- opening a file into a tab ----------------------------------------
    // A markdown link to another repo file: same viewer choice and binary
    // refusal as a tree click. `checkout` is the SOURCE pane's pin (#406): a
    // link in a worktree's document names that worktree's file.
    // `as: "bytes"` is the ONE caller that refuses the note routing below: a
    // `.note` whose bytes are not a container (ADR-0064 §11) must not become a
    // card, and classifying it again would be the loop the card just escaped.
    // The viewer serves text and images and refuses anything else, so what is
    // honest here is the refusal, named — not a pane that would show nothing.
    openLink({ project, path, fragment, checkout, as }: any) {
      const title = path.split("/").pop();
      if (as === "bytes") {
        window.WB.emit("open-refused", { project, path, reason: "not a note" });
        this._flashAction?.(`${path} is not a note.`);
        return;
      }
      const ftype = classify(title);
      // A note linking to a note lands on the plane, exactly as a double-click
      // in the explorer does.
      if (ftype === "note") {
        this.openNote(path, project, checkout);
        return;
      }
      if (ftype === "binary") {
        window.WB.emit("open-refused", { project, path, reason: "binary" });
        this._flashAction?.("Cannot open binary files.");
        return;
      }
      this.openTab({ project, path, title, ftype, fragment, checkout });
    },

    // `content`: a re-attached popup passes its (possibly edited) bytes back.
    // `fragment`/`find`: a `#heading` or search term to land on once the bytes
    // are shown. `checkout` PINS the tab to the tree it was opened in (#406):
    // a Save from a tab showing worktree bytes must never land on the primary.
    // `encoding`/`bom` come back with a re-attached popup's bytes, so the
    // reattached pane saves the way the detached one would have.
    openTab({ project, path, title, ftype, content, fragment, find, checkout, encoding, bom }: any) {
      const ck = checkout !== undefined ? checkout : this.checkoutOf(project);
      const id = fileTabId(project, path, ck);
      if (this.tabs.some((t) => t.id === id)) {
        this.activate(id);
        if (fragment) WBViewer.jumpTo(id, fragment);
        if (find) WBViewer.find(id, find);
        return;
      }
      const icon =
        ftype === "markdown"
          ? "bi bi-file-earmark-text"
          : ftype === "image"
            ? "bi bi-file-earmark-image"
            : "bi bi-file-earmark-code";
      this.tabs.push({ id, kind: ftype, title, path, project, icon, closable: true, checkout: ck });
      this.active = id;
      this.persistView();
      this.$nextTick(() => {
        // A re-attach passes bytes in; a fresh open reads `file.read`.
        const bytes =
          content != null
            ? Promise.resolve({ content, encoding, bom })
            : this.fetchContent(project, path, ftype, ck);
        bytes.then((body: any) => {
          if (body == null) return; // nothing to show: fetchContent closed the tab
          WBViewer.open({
            id,
            project,
            label: this.$store.projects.projectLabel(project),
            path,
            ftype,
            content: body.content,
            encoding: body.encoding,
            bom: body.bom,
            refused: body.refused,
            checkout: ck,
          });
          // NOT `setActive(id)`: `restoreView` opens N tabs in one burst and
          // THEN activates the stored one, so the last read to answer must not
          // own the screen.
          this.syncViewer();
          if (fragment) WBViewer.jumpTo(id, fragment);
          if (find) WBViewer.find(id, find);
        });
      });
    },

    // Re-point every open tab under the moved path: a tab's id IS its path,
    // and saves would write to the old location.
    repathTabs(from: any, to: any) {
      // Snapshot: the collision branch CLOSES a tab, which mutates `this.tabs`.
      for (const t of [...this.tabs]) {
        if (t.kind === "diff" || t.project !== this.$store.projects.openSlug) continue;
        if (t.path !== from && !t.path?.startsWith(`${from}/`)) continue;
        const newPath = to + t.path.slice(from.length);
        const newId = fileTabId(t.project, newPath, t.checkout);
        if (this.tabs.some((x) => x.id === newId)) {
          // The destination is ALREADY open: a live editor on the old path
          // would recreate the file on its next save. Close it.
          this.closeTab(t.id);
          continue;
        }
        window.WBViewer?.repath(t.id, { id: newId, path: newPath });
        if (this.active === t.id) this.active = newId;
        if (this.slot?.kind === "pin" && this.slot.id === t.id) this.slot = { kind: "pin", id: newId };
        if (this.lastLeft === t.id) this.lastLeft = newId;
        t.path = newPath;
        t.id = newId;
      }
      // The viewer's own "what is shown" is keyed by id too.
      this.$nextTick(() => this.syncViewer());
      this.persistView();
    },

    // --- opening a Changes row into a diff tab ----------------------------
    // HEAD on one side, the working tree on the other (#311). Read-only;
    // Monaco computes the diff, nothing produces a patch.
    openDiff(project: any, entry: any) {
      // Both diff sides read text, and a binary side closes the tab again: an
      // image or other binary path never reaches the diff.
      const ftype = classify(entry.path.split("/").pop());
      if (ftype === "image" || ftype === "binary") {
        this.openChangedBinary(project, entry, ftype);
        return;
      }
      // Pinned to the selection at open (#407): both sides read `t.checkout`.
      const t = WBChanges.diffTarget(entry, project, this.checkoutOf(project));
      if (this.tabs.some((x) => x.id === t.id)) {
        this.activate(t.id);
        return;
      }
      this.tabs.push({
        id: t.id,
        kind: "diff",
        title: t.title,
        path: t.workingPath,
        project,
        checkout: t.checkout,
        icon: "bi bi-file-earmark-diff",
        closable: true,
      });
      this.active = t.id;
      window.WB.emit("open-diff", { project, path: t.workingPath, checkout: t.checkout });
      this.$nextTick(() => {
        // Latched: a path refused on BOTH sides would flash twice.
        let refused = false;
        const refuse = (reason: any) => {
          if (refused) return null;
          refused = true;
          this._flashAction?.(WBFail.failed({ message: reason }, "Could not open the diff: the daemon gave no reason."));
          this.closeTab(t.id);
          return null;
        };
        Promise.all([this.diffHeadSide(project, t, refuse), this.diffWorkSide(project, t, refuse)])
          .then(([head, work]) => {
            // A refusal on EITHER side aborts: half a diff reads as "no changes".
            if (head == null || work == null) return;
            // Closed during the round trips: mounting now would leak a pane
            // with no tab to close it.
            if (!this.tabs.some((x) => x.id === t.id)) return;
            WBViewer.open({
              id: t.id,
              project,
              checkout: t.checkout,
              label: this.$store.projects.projectLabel(project),
              path: t.workingPath,
              ftype: "diff",
              content: work,
              original: head,
            });
            // As `openTab`: the pane follows the CURRENT active tab.
            this.syncViewer();
          })
          .catch(() => refuse("transport"));
      });
    },

    // A Changes row that names an image or other binary. An image shows its
    // working copy in the image pane (ADR-0049); a deleted one has no working
    // copy, and `blob.read` serves text only, so it is refused like a binary.
    openChangedBinary(project: any, entry: any, ftype: any) {
      const path = entry.path;
      if (ftype === "binary") {
        window.WB.emit("open-refused", { project, path, reason: "binary" });
        this._flashAction?.("Cannot open binary files.");
        return;
      }
      if (entry.status === "deleted") {
        window.WB.emit("open-refused", { project, path, reason: "deleted" });
        this._flashAction?.("This image was deleted, so there is nothing to show.");
        return;
      }
      const title = path.split("/").pop();
      this.openTab({ project, path, title, ftype, checkout: this.checkoutOf(project) });
    },

    // The diff's HEAD side; an added/untracked path diffs against emptiness.
    diffHeadSide(project: any, t: any, refuse: any) {
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
    },

    // The diff's working side. `not found` is NOT a refusal: a stale row
    // (deleted between the list and the click) diffs against emptiness.
    diffWorkSide(project: any, t: any, refuse: any) {
      if (t.workingAbsent) return Promise.resolve("");
      // NOT `fetchContent`: it collapses every refusal to `null` and closes
      // the `file:` tab id rather than this diff's.
      return WBDaemon.observe(
        "file.read",
        WBDaemon.withCheckout({ repo: project, path: t.workingPath }, t.checkout),
      ).then((reply) => {
        if (!WBFail.isError(reply)) return reply.content;
        const reason = WBFail.message(reply, "refused");
        return reason === "not found" ? "" : refuse(reason);
      });
    },

    // Pop a file tab out into a standalone popup; the in-app tab closes.
    detachFile(desc: any) {
      const id = fileTabId(desc.project, desc.path, desc.checkout);
      // The descriptor is handed over by postMessage with `targetOrigin =
      // location.origin`, NOT in the URL hash: a hash let anyone render content
      // of their choosing on the daemon's origin. Passing the bytes keeps
      // unsaved edits alive across a detach.
      // `popup` is the daemon's route for the page (`Shell` in `assets.rs`).
      const win = window.open("popup", "_blank", "popup,width=920,height=760");
      if (!win) {
        window.WB.emit("detach-blocked", { project: desc.project, path: desc.path });
        return;
      }
      detached.watch(win, desc);
      window.WB.emit("detach", { project: desc.project, path: desc.path });
      this.closeTab(id);
      this.activate("consoles");
    },

    activate(id: any) {
      this.active = id;
      // The Spend tab's subject can change while it sits in the background.
      if (id === "spend" && this.spend.slug !== this.$store.projects.openSlug) this.refreshSpend();
      this.$nextTick(() => {
        this.syncViewer();
        // A console opened while another tab was active measured 0×0.
        if (id === "consoles") window.WBConsole?.refitAll?.();
      });
      this.persistView();
    },

    // The ONE place that tells the viewer which pane is on screen — and which
    // sits beside it: an async opener calling it late can only converge.
    // Paneless tabs map to `null`; the slot waits in state while one is up.
    PANELESS_TABS: ["consoles", "spend"],
    syncViewer() {
      const r = WBSplit.resolve({
        active: this.active,
        slot: this.slot,
        tabs: this.tabs,
        lastLeft: this.lastLeft,
        width: WBViewer.width(),
        paneless: this.PANELESS_TABS.includes(this.active),
      });
      // Remembered where the decision is made, not in `activate`: `openTab`
      // and `openDiff` put a tab on the left without going through it.
      if (r.left && r.left !== this.slot?.id) this.lastLeft = r.left;
      WBViewer.setActive(
        r.left,
        r.right && { id: r.right, mirror: r.mirror, focus: r.focus === "right", ratio: this.splitRatio },
      );
    },

    // --- the slot: pin a tab beside the active one, or mirror the active one --
    pinTab(id: any) {
      this.slot = { kind: "pin", id };
      this.syncLater();
    },
    toggleMirror() {
      this.slot = this.slot?.kind === "mirror" ? null : { kind: "mirror" };
      this.syncLater();
    },
    clearSlot() {
      this.slot = null;
      this.syncLater();
    },
    syncLater() {
      this.$nextTick(() => this.syncViewer());
      this.persistView();
    },
    // Whether the canvas is wide enough for two panes right now: the tab menu
    // greys its slot items below the floor rather than pinning into nothing.
    splitAvailable() {
      return WBSplit.available(WBViewer.width());
    },

    closeTab(id: any) {
      const idx = this.tabs.findIndex((t) => t.id === id);
      const tab = this.tabs[idx];
      if (!tab || !tab.closable) return; // Consoles never closes
      // A closed tab takes its pin with it (a mirror follows the active tab).
      this.slot = WBSplit.afterClose(this.slot, id);
      if (this.lastLeft === id) this.lastLeft = null;
      WBViewer.close(id);
      this.tabs.splice(idx, 1);
      if (this.active === id) {
        // fall back to the neighbour, else the Consoles tab
        const next = this.tabs[idx] || this.tabs[idx - 1] || this.tabs[0];
        this.activate(next.id);
      } else {
        // A background close can still be the slot's pane: repaint.
        this.$nextTick(() => this.syncViewer());
      }
      this.persistView();
    },

    // --- the per-client view: the open file tabs (issue #339) ----------------
    // The tabs half of `wb.view.v1` (`wb-console.ts` owns the offset half;
    // `patch` merges). Only `file:` tabs are stored: a `diff:` tab's sides are
    // LIVE git state.
    // Set while `restoreView` opens the stored tabs: `fetchContent` closes a
    // tab whose read fails, and those closes would otherwise REWRITE the store.
    _restoring: false,
    persistView() {
      if (this._restoring) return;
      const files = this.tabs
        .filter((t) => t.id.startsWith("file:"))
        .map((t) => ({
          project: t.project,
          path: t.path,
          title: t.title,
          kind: t.kind,
          // The pin (#406) survives a reload.
          checkout: t.checkout ?? null,
        }));
      // A stored `active` naming a tab this store does not carry would leave
      // the canvas blank: degrade to Consoles.
      const alive =
        this.active === "consoles" ||
        files.some((f) => fileTabId(f.project, f.path, f.checkout) === this.active);
      window.WBView?.patch({
        tabs: files,
        active: alive ? this.active : "consoles",
        // ALWAYS written, null included: `patch` is read-modify-write, and a
        // key left out would let a stale pin outlive the tab it named.
        split: WBSplit.toStored(this.slot, this.splitRatio, this.tabs),
      });
    },

    _viewRestored: false,
    // Latched, and AUTH-GATED by its callers: a pre-login `file.read` is
    // refused, `fetchContent` closes the tab, and that close persists the loss.
    restoreView() {
      if (this._viewRestored) return;
      this._viewRestored = true;
      const stored = window.WBView?.read();
      if (!stored) return;
      this._restoring = true;
      try {
        for (const t of stored.tabs || []) {
          if (!t || !t.project || !t.path) continue;
          this.openTab({
            project: t.project,
            path: t.path,
            title: t.title || t.path,
            ftype: t.kind,
            checkout: t.checkout ?? null,
          });
        }
        // After the tab entries exist (the panes land async; the viewer
        // paints single until they do) and before the activation that paints.
        this.slot = WBSplit.fromStored(stored.split, this.tabs);
        this.splitRatio = stored.split?.ratio ?? null;
        const want = stored.active;
        this.activate(want && this.tabs.some((t) => t.id === want) ? want : "consoles");
      } finally {
        // The reads are async: hold the suppressor so a refusal landing later
        // cannot rewrite the store. A LATER close persists normally.
        setTimeout(() => {
          this._restoring = false;
        }, 3000);
      }
    },
    // --- context menu -----------------------------------------------------
    // A tab's menu (ADR-0037 §3c): the slot's two entry points and Close.
    // Consoles and Spend have no pane to put beside another, so no menu.
    showTabMenu(x: any, y: any, t: any) {
      if (!t?.closable) return;
      const pinned = this.slot?.kind === "pin" && this.slot.id === t.id;
      const mirrored = this.slot?.kind === "mirror";
      const wide = this.splitAvailable();
      const narrowTitle = wide ? "" : "Needs a wider canvas";
      const items = [
        pinned
          ? { label: "Unpin", icon: "bi-pin-angle", run: () => this.clearSlot() }
          : {
              label: "Open to the side",
              icon: "bi-layout-split",
              disabled: !wide,
              title: narrowTitle,
              run: () => this.pinTab(t.id),
            },
        t.kind === "code" &&
          (mirrored
            ? { label: "Close mirror", icon: "bi-files", run: () => this.toggleMirror() }
            : {
                label: "Mirror editor",
                icon: "bi-files",
                disabled: !wide,
                title: narrowTitle,
                run: () => {
                  this.activate(t.id);
                  this.toggleMirror();
                },
              }),
        { sep: true },
        { label: "Close", icon: "bi-x-lg", run: () => this.closeTab(t.id) },
      ].filter(Boolean);
      this.renderMenu(x, y, items);
    },

    // Paint `items` into the one `#ctxmenu` and keep it on-screen. An item is
    // `{ label, icon, run }` with optional `sep`, `danger`, `disabled`, `title`.
    renderMenu(x: any, y: any, items: any) {
      const menu = document.getElementById("ctxmenu") as HTMLElement;
      menu.innerHTML = "";
      for (const it of items) {
        if (it.sep) {
          const hr = document.createElement("div");
          hr.className = "ctx-sep";
          menu.append(hr);
          continue;
        }
        const b = document.createElement("button");
        b.className = "ctx-item" + (it.danger ? " danger" : "");
        // Built as elements: a label can name a file, and a file name is text.
        const icon = document.createElement("i");
        icon.className = "bi " + it.icon;
        const label = document.createElement("span");
        label.textContent = it.label;
        b.append(icon, label);
        if (it.disabled) b.disabled = true;
        if (it.title) b.title = it.title;
        b.onclick = () => {
          this.hideMenu();
          it.run();
        };
        menu.append(b);
      }
      menu.style.display = "block";
      const w = menu.offsetWidth,
        h = menu.offsetHeight;
      menu.style.left = Math.min(x, innerWidth - w - 8) + "px";
      menu.style.top = Math.min(y, innerHeight - h - 8) + "px";
    },

    hideMenu() {
      const menu = document.getElementById("ctxmenu") as HTMLElement;
      if (menu) menu.style.display = "none";
    },

  });
}

// Everything the page wires at load: the window names other code reads,
// the document and window listeners, and the state of detached windows.
// `window` and `document` are parameters so a test passes its own stubs, and
// each call starts from fresh state (ADR-0075 D7).
export function wire(window: Window, document: Document) {
  // The one exit point: every gesture becomes a `workbench:action` event.
  // Each page sets its own `window.WB`, and its modules read it (ADR-0075 D9).
  window.WB = {
    emit(action: any, detail: any = {}) {
      const full = { action, ...detail, at: new Date().toISOString() };
      sendDocument(document, "workbench:action", full);
      // eslint-disable-next-line no-console
      console.log("[workbench:action]", full);
    },
  };
  window.shell = shell;

  // The live Alpine component instance. On `window` explicitly: two other
  // modules call it, and a bare declaration reaches them only by accident of
  // global scope.
  window.getShell = function getShell() {
    const root = document.querySelector<any>("[x-data]");
    return root && root._x_dataStack ? root._x_dataStack[0] : null;
  };

  // A viewer asked to detach → open the popup and close the tab.
  document.addEventListener("workbench:detach-request", (e) => window.getShell()?.detachFile(e.detail));

  // A rendered markdown link asked for a repo file → open (or focus) its tab.
  document.addEventListener("workbench:open-request", (e) => window.getShell()?.openLink(e.detail));

  // The divider between the two panes was dragged: the ratio is view state.
  document.addEventListener("workbench:split-ratio", (e) => {
    const sh = window.getShell();
    if (!sh) return;
    sh.splitRatio = e.detail.ratio;
    // Re-told, not only stored: the viewer repaints `--wb-split` from the ratio
    // it was last given, and a repaint can come before the next activation.
    sh.syncViewer();
    sh.persistView();
  });

  // The canvas resized. Only a crossing of the split's width floor changes what
  // is painted, so the fold reruns on the crossing alone — not per pixel.
  let wbCanvasWide: any = null;
  document.addEventListener("workbench:canvas-resize", (e) => {
    const sh = window.getShell();
    if (!sh) return;
    const wide = WBSplit.available(e.detail.width);
    if (wide === wbCanvasWide) return;
    wbCanvasWide = wide;
    if (sh.slot) sh.syncViewer();
  });

  // The popups this shell opened. Membership is the authorisation for every
  // message below.
  const detachedWindows = new Map();

  // A popup that closes sends its bytes home on unload (`wb-reattach`). One that
  // dies without an unload event (a crashed or killed renderer) is found by this
  // poll and comes home with the descriptor it was detached with. The poll acts
  // on the SECOND tick that sees it closed: the unload message carries the
  // edited bytes and must win over the detach-time copy.
  const detachedClosedSeen = new Set();
  let detachedPoll: any = null;

  function watchDetached(win: any, desc: any) {
    detachedWindows.set(win, desc);
    if (!detachedPoll) detachedPoll = window.setInterval(pollDetached, 500);
  }

  function pollDetached() {
    for (const [win, desc] of [...detachedWindows]) {
      if (!win.closed) continue;
      if (detachedClosedSeen.has(win)) reattachFile(win, desc);
      else detachedClosedSeen.add(win);
    }
  }

  // Whether a detached file window holds an edit not yet saved. The popups
  // are same-origin windows this shell opened, so it asks their viewer directly.
  function detachedDirty() {
    for (const win of detachedWindows.keys()) {
      if (!win.closed && win.WBViewer?.anyDirty?.()) return true;
    }
    return false;
  }

  // Close every detached file window; each one's unload sends its file home.
  function closeDetached() {
    for (const win of detachedWindows.keys()) if (!win.closed) win.close();
  }

  detached = { watch: watchDetached, dirty: detachedDirty, close: closeDetached };

  // The one way a detached file comes home: the button, the popup's unload and
  // the poll all end here. Closing the popup matters after an F5 inside it: the
  // unload sent the file home, and the reloaded page has nothing left to show.
  function reattachFile(win: any, desc: any) {
    // The pin comes home with the bytes (#406): explicit `null` is the primary.
    window.getShell()?.openTab({
      project: desc.project,
      path: desc.path,
      title: desc.path.split("/").pop(),
      ftype: desc.ftype,
      content: desc.content,
      checkout: desc.checkout ?? null,
      encoding: desc.encoding,
      bom: desc.bom,
    });
    detachedWindows.delete(win);
    detachedClosedSeen.delete(win);
    if (!detachedWindows.size) {
      window.clearInterval(detachedPoll);
      detachedPoll = null;
    }
    if (!win.closed) win.close();
  }

  // Messages from detached popups. Both guards matter: `e.origin` refuses a
  // page on another origin, `e.source` a same-origin window we did not open.
  // Without them this listener accepted `file.write` from anyone holding a
  // handle to this window.
  window.addEventListener("message", (e: any) => {
    if (e.origin !== window.location.origin) return;
    if (!detachedWindows.has(e.source)) return;
    const m = e.data;
    if (!m || typeof m !== "object") return;
    if (m.type === "wb-detach-ready") {
      // The popup booted and is asking for its file.
      e.source.postMessage({ type: "wb-detach-open", desc: detachedWindows.get(e.source) }, window.location.origin);
    } else if (m.type === "wb-emit") {
      // `fromWindow` lets a save's answer reach the pane that sent it.
      window.WB.emit(m.action, { ...m.detail, fromWindow: e.source });
    } else if (m.type === "wb-open-request" && m.detail) {
      // A link clicked inside a detached pane; `openLink` re-classifies, so the
      // popup decides nothing about what opens.
      window.getShell()?.openLink({
        project: m.detail.project,
        path: m.detail.path,
        fragment: m.detail.fragment,
        checkout: m.detail.checkout ?? null,
      });
    } else if (m.type === "wb-reattach" && m.desc) {
      // A second `wb-reattach` from the same popup (the button, then its own
      // unload) never gets here: the guard above drops a window no longer held.
      reattachFile(e.source, m.desc);
    }
  });

  // --- Write byte-ops (#197): the workspace-mutating seam actions go to the
  // daemon's confined `file.*` verbs; a refusal is flashed. The browser composes
  // the full rel path.
  (function wireWriteVerbs() {
    const daemonBacked = () => !!window.WBDaemon?.write;
    const flash = (msg: any) => window.getShell()?._flashAction?.(msg);
    const call = (verb: any, payload: any, okMsg?: any) => {
      WBDaemon.write(verb, payload)
        .then((reply: any) => {
          if (WBFail.isError(reply)) flash(WBFail.failed(reply, "Could not rename: the daemon gave no reason."));
          else if (okMsg) flash(okMsg);
        })
        .catch(() => flash("Could not rename: the daemon did not answer."));
    };

    document.addEventListener("workbench:action", async (e) => {
      if (!daemonBacked()) return;
      const d = e.detail || {};
      const repo = d.project;
      if (!repo) return;
      // Every Write carries the checkout it is aimed at (#406): a Save says its
      // tab's PIN (explicit `null` = the primary, never the selection), a tree
      // gesture says the current selection.
      const checkout =
        d.checkout !== undefined ? d.checkout : (window.getShell()?.checkoutOf?.(repo) ?? null);
      const aimed = (payload: any) => WBDaemon.withCheckout(payload, checkout);
      switch (d.action) {
        case "save": {
          // The pane's encoding rides the write (ADR-0036 amendment 2026-09-22)
          // and the pane hears the answer: its dirty mark waits for the ack, and
          // a refusal is read where the bytes are, not in a flash elsewhere.
          // A detached window's pane is `detached` in its own viewer; a tab's is
          // its tab id in this one.
          const viewer = () => (d.fromWindow ? d.fromWindow.WBViewer : window.WBViewer);
          const id = d.fromWindow ? "detached" : fileTabId(repo, d.path, checkout);
          const payload: any = { repo, path: d.path, content: d.content || "" };
          if (d.encoding) payload.encoding = d.encoding;
          if (d.bom) payload.bom = true;
          const send = (p: any): Promise<void> =>
            WBDaemon.write("file.write", aimed(p))
              .then((reply: any) => {
                if (!WBFail.isError(reply)) return viewer()?.saveDone?.(id);
                const reason = WBFail.message(reply, "the daemon gave no reason");
                viewer()?.saveFailed?.(id, reason, reply);
                // UTF-8 represents everything; a refusal under it is not a
                // conversion question, and asking again would loop.
                if (reason === "unencodable" && !/^utf-?8$/i.test(p.encoding || "utf-8")) {
                  return offerUtf8(p, reply);
                }
                flash(WBFail.failed(reply, "Could not save: the daemon gave no reason."));
              })
              .catch(() => {
                viewer()?.saveFailed?.(id, "the daemon did not answer");
                flash("Could not save: the daemon did not answer.");
              });
          // The daemon wrote nothing (a round-trip or a refusal, never a `?`):
          // the one repair the browser can offer is a DELIBERATE conversion,
          // named to the operator and made only on their yes.
          const offerUtf8 = (p: any, reply: any) => {
            const shell = window.getShell();
            // No shell, no dialog: the pane already says "Could not save" and why.
            if (!shell?.askConfirm) return;
            const at = Number(reply?.char_index ?? 0) + 1;
            const ask = shell.askConfirm({
              title: `Could not save as ${p.encoding}`,
              message: `Character ${at} is not representable in ${p.encoding}. Save the file as UTF-8 instead?`,
              confirmLabel: "Save as UTF-8",
            });
            return ask.then((ok: any) => {
              if (!ok) return;
              viewer()?.setEncoding?.(id, "UTF-8", false);
              const { bom: _bom, ...rest } = p;
              return send({ ...rest, encoding: "utf-8" });
            });
          };
          send(payload);
          break;
        }
        case "worktree-created": {
          // A console's switcher cut a worktree: re-read the listing, flash what
          // the add had to say.
          const c = window.getShell();
          c?.ensureWorktreeListing?.(repo, true);
          if (d.message) c?._flashAction?.(d.message.split("\n").map(WBFail.sentence).filter(Boolean).join(" "));
          break;
        }
        case "create": {
          // `create` carries the target DIRECTORY and no name: ask for it, then
          // open a created file so the operator lands in it.
          const folder = d.kind === "folder";
          const c = window.getShell();
          const name = c
            ? await c.askPrompt({
                // No placeholder: a plausible filename in an empty field reads as
                // a name already chosen, and operators pressed Enter on it.
                title: newEntryTitle(folder ? "folder" : "file", d.path),
                message: "",
                placeholder: "",
              })
            : window.prompt(folder ? "New folder name" : "New file name");
          if (!name) return;
          const path = d.path ? `${d.path}/${name}` : name;
          const reply = await WBDaemon.write("file.create", aimed({ repo, path, dir: folder })).catch(() => null);
          if (!reply) return flash(`Could not create ${name}: the daemon did not answer.`);
          if (WBFail.isError(reply)) return flash(WBFail.failed(reply, `Could not create ${name}: the daemon gave no reason.`));
          flash(`${name} created.`);
          if (!folder) c?.openTab({ project: repo, path, title: name, ftype: classify(name) });
          // The files re-list the level, then reveal the new entry (wb-files.ts).
          sendWindow(window, "workbench:tree-dirty", { rel: d.path || "", reveal: path });
          break;
        }
        case "rename": {
          // `from`/`to` are FULL rel paths (shared with the move gesture).
          call("file.rename", aimed({ repo, path: d.from, to: d.to }));
          break;
        }
        case "delete": {
          // Irreversible (a folder removes recursively): confirm first.
          const name = d.title || d.path!.split("/").pop() || d.path;
          const message = d.isFolder
            ? `Delete folder “${name}” and its contents? This cannot be undone.`
            : `Delete “${name}”? This cannot be undone.`;
          const c = window.getShell();
          const ok = c
            ? await c.askConfirm({ title: "Delete", message, confirmLabel: "Delete", danger: true })
            : window.confirm(message);
          if (!ok) return;
          const reply = await WBDaemon.write("file.delete", aimed({ repo, path: d.path })).catch(() => null);
          if (!reply) return flash("Could not delete: the daemon did not answer.");
          if (!WBFail.isError(reply)) return flash(`${name} deleted.`);
          const reason = WBFail.message(reply, "the daemon gave no reason");
          flash(WBFail.failed(reply, "Could not delete: the daemon gave no reason."));
          // "not found" on a delete says the ROW is the lie: re-list the parent
          // so the ghost ends up off the screen.
          if (/not found/i.test(reason)) {
            sendWindow(window, "workbench:tree-dirty", { rel: parentRel(d.path) });
          }
          break;
        }
        default:
          break;
      }
    });
  })();

  // Dismiss the context menu on any outside interaction.
  document.addEventListener("click", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu")!.style.display = "none"));
  document.addEventListener("scroll", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu")!.style.display = "none"), true);

  // `/` → the search on screen: FILES while a project is open, projects
  // otherwise.
  document.addEventListener("keydown", (e) => {
    if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
    const c = window.getShell();
    if (!c || c.consoleShortcutsBlocked()) return;
    e.preventDefault();
    if (window.Alpine.store("projects").openSlug) sendWindow(window, "workbench:file-search-open");
    else c.focusProjectSearch();
  });

  // Ctrl/Cmd+Shift+F → the FILES search. NOT `consoleShortcutsBlocked`: the
  // editor is exactly where "find in files" is reached for.
  document.addEventListener("keydown", (e) => {
    if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey) return;
    if (e.key !== "F" && e.key !== "f") return;
    const c = window.getShell();
    if (!c || !c.authed || !window.Alpine.store("projects").openSlug) return;
    if (c.modalOpen(WBSettingsDialog.openFlag) || c.modalOpen(WBSecurityDialog.openFlag) || c.runOpen || c.branchOpen || c.modalOpen(WBReleaseDialogs.whatsNewFlag)) return;
    e.preventDefault();
    sendWindow(window, "workbench:file-search-open");
  });

  window.WBRuns = {
    // Append a raw output chunk, capped so the DOM never grows unbounded.
    output(text) {
      const c = window.getShell();
      if (c) c.rawFeed = (c.rawFeed + text).slice(-8000);
    },
  };
}

const pascal = (name: any) =>
  name.replace(/(\w)(\w*)(_|-|\s*)/g, (_: any, first: any, rest: any) => first.toUpperCase() + rest.toLowerCase());

// `x-icon="'name'"` draws a lucide icon INTO its own `<svg>`. It never swaps the
// element (lucide's `createIcons` replaces it), so it is still the node Alpine
// bound: the icon renders wherever Alpine initializes an element — page load, a
// new `x-if` branch, a new `x-for` row — and a changed name redraws it. The
// output matches `createIcons` (lucide 0.460.0): the icon's default attributes
// where the markup set none, `data-lucide` (the stylesheets select on it), and
// the `lucide lucide-<name>` classes.
export function iconDirective(el: any, { expression }: any, { evaluateLater, effect }: any) {
  const read = evaluateLater(expression);
  const authored = new Set(el.getAttributeNames());
  let drawn: any = null;
  effect(() =>
    read((name: any) => {
      if (name === drawn) return;
      if (drawn) el.classList.remove(`lucide-${drawn}`);
      drawn = name;
      const node = window.lucide?.icons[pascal(String(name))];
      if (!node) {
        console.warn(`x-icon: no lucide icon named "${name}"`);
        el.replaceChildren();
        return;
      }
      const [, attrs, children] = node;
      for (const [key, value] of Object.entries(attrs)) {
        // `class` is merged below: `:class` on the same element owns the rest.
        if (key !== "class" && !authored.has(key)) el.setAttribute(key, String(value));
      }
      el.setAttribute("data-lucide", name);
      el.classList.add("lucide", `lucide-${name}`);
      el.replaceChildren(...children.map((child: any) => window.lucide.createElement(child)));
    }),
  );
}

/** The `shell()` component: a component's `uses` names come from it (ADR-0075 D6). */
export type Shell = ReturnType<typeof shell>;
