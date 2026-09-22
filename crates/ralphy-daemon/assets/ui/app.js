"use strict";
/* ---------------------------------------------------------------------------
   ralphy workbench shell — shell behaviour

   The sidebar is a project accordion (Alpine); the file tree is a Wunderbaum
   instance. The canvas is a tabbed workspace: "Consoles" is fixed and hosts
   the floating console windows (wb-console.js); every opened file is its own
   closable tab rendered by a viewer (wb-viewer.js).

   Every user gesture becomes one CustomEvent, `workbench:action`, on
   `document`. That event IS the seam: a backend subscribes and does the work.
--------------------------------------------------------------------------- */

// The one exit point: every gesture becomes a `workbench:action` event.
window.WB = {
  emit(action, detail = {}) {
    const full = { action, ...detail, at: new Date().toISOString() };
    document.dispatchEvent(new CustomEvent("workbench:action", { detail: full }));
    // eslint-disable-next-line no-console
    console.log("[workbench:action]", full);
  },
};

// Images the daemon serves as bytes (ADR-0049). The daemon holds the
// authoritative allowlist; this set only decides which VERB a click sends.
const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"]);

// Neither renderable source nor image: refused.
const BINARY_EXT = new Set([
  "pdf", "zip", "gz",
  "tar", "rar", "7z", "exe", "dll", "so", "dylib", "bin", "class", "jar", "wasm",
  "mp3", "wav", "flac", "ogg", "mp4", "mov", "avi", "mkv", "webm", "woff",
  "woff2", "ttf", "eot", "otf",
]);

function extOf(name) {
  const n = name.toLowerCase();
  return n.includes(".") ? n.split(".").pop() : "";
}

// The directories the daemon's Write path refuses (`fswrite::PROTECTED_DIRS`),
// mirrored so the UI never offers a gesture that can only be refused.
// Case-insensitive: NTFS resolves `.GIT` to `.git`.
const PROTECTED_DIRS = [".git", ".ralphy"];

function isProtectedDir(name) {
  return PROTECTED_DIRS.some((p) => name.toLowerCase() === p);
}

// Whether `rel` names or traverses a protected directory (the daemon's own
// component test).
function underProtectedDir(rel) {
  return rel.split("/").some(isProtectedDir);
}

// The directory containing `rel`; "" for a top-level entry (the repo root).
function parentRel(rel) {
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

// A file tab's identity (#406): project, path and — ONLY under a selected
// worktree — the checkout, so the same rel in two trees is two tabs (the
// primary's id is the pre-#406 spelling, byte for byte). Mirrored by
// `WBViewer`'s `fileTabId`; the two must never disagree.
function fileTabId(project, path, checkout) {
  return checkout ? `file:${project}@${checkout}:${path}` : `file:${project}:${path}`;
}

// Which viewer a file gets: markdown → rendered pane, image → image pane,
// other binaries refused, everything else source code.
function classify(name) {
  const ext = extOf(name);
  if (ext === "md" || ext === "markdown") return "markdown";
  if (IMAGE_EXT.has(ext)) return "image";
  if (BINARY_EXT.has(ext)) return "binary";
  return "code";
}

function shell() {
  return {
    openSlug: null,
    // True only on the static `file://` demo bundle (#202).
    isDemo: window.WBMode.isDemo(),
    // Daemon-mode `/api/repos` failure (#202): a visible error, not the seed.
    reposError: "",
    // The local fleet's peers (ADR-0052 §5, #349), from `/api/fleet`. Empty: a
    // fleet of one, or a daemon too old to serve the route.
    fleetPeers: [],
    // Peers with a wake in flight, keyed by daemon_id: a cold WSL boot takes
    // seconds, and the key stops a second click sending a second nudge.
    waking: {},
    // Working-tree change count per slug (#307). `null` until a load succeeds
    // (rendered `—`, so a failed read never reads like a clean tree);
    // `changesReadError` carries the reason into the badge's title.
    changesCount: {},
    // Per slug, named apart from the shell-wide `changesError` below: a
    // duplicate key in this literal is a silent no-op.
    changesReadError: {},
    // The two rendered groups (#315). INVARIANT: every path that sets one must
    // set the OTHER in the SAME statement — a stale group left behind renders
    // rows under a headline while the badge already reads `—`.
    changesStaged: {},
    changesUnstaged: {},
    // The sync row per project (#316): the fold of `sync.status`. Same three
    // triggers as the change set, never a timer.
    syncByProject: {},
    // The commit message being composed (#318). One box, but it belongs to
    // `commitMsgSlug` ONLY: a message typed for repo A must never land as repo
    // B's commit. Cleared on success only.
    commitMsg: "",
    commitMsgSlug: null,
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
    // The FILES panel's states: "no files", "still looking" and "refused" must
    // not be one blank.
    treeLoading: false,
    treeError: "",
    // The tree shows a listing the daemon could not confirm (a refusal, a
    // dropped socket). Distinct from `treeError`: rows are on screen, but stale.
    treeStale: "",
    // A refused `branch.switch`/`branch.create`, held until the next branch act
    // or a project switch. Not `treeError` (the tree is fine) and not
    // `changesError` (the chip lives in THIS panel).
    branchError: "",
    // The FILES search (ADR-0036 amendment 2026-09-15). `seq` dates each
    // request so a slow reply never paints over a newer one; `expandedBefore`
    // is the expansion snapshot `clearFileSearch` folds the tree back to
    // (`null` = nothing expanded yet).
    fileSearch: {
      open: false,
      mode: "name",
      query: "",
      seq: 0,
      hits: [],
      truncated: false,
      note: "",
      expandedBefore: null,
    },
    _tree: null, // the live Wunderbaum instance, if any
    _treeSub: null, // the live `/ws/tree` subscription for the open project, if any
    // Tree memory, all three lazily created so they stay plain collections
    // outside Alpine's reactive data (a proxied Map is a trap):
    //   _treeCache     directory levels already shown, keyed `repo\nrel`.
    //                  Survives closing a project. Memory only.
    //   _treeValidated cached levels re-read against the disk during THIS open.
    //                  Cleared on every mount.
    //   _treeExpanded  folders expanded when a project was last closed, by repo.
    _runsSub: null, // the live run-snapshot subscription for the open project, if any
    _changesSub: null, // the run-completion nudge subscription for the open project (#310)
    _presenceSub: null, // the `/ws` heartbeat subscription, kept so a resume can re-open it
    // Monotonic hydration token: overlapping `runs.list` replies can land OUT
    // OF ORDER; only the newest hydration commits.
    _runsSeq: 0,
    // Same token for the Changes count (#310's nudge overlaps the open's read)
    // and for the sync row.
    _changesSeq: 0,
    _syncSeq: 0,

    // Alpine lifecycle.
    init() {
      this.initRuns();
      this.currentRunId = this.projectRuns()[0]?.runid || null;
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      this.probeSession();
      // In daemon mode the `projects` literal is demo seed: drop it BEFORE the
      // async loadRepos so it never flashes.
      if (!window.WBMode.seedAllowed()) this.projects = [];
      this.loadRepos();
      this.loadAgents();
      this.subscribePresence();
      this.loadIdentity();
      // One read at load: the daemon polls releases on its own six-hour clock.
      this.loadRelease();
      // The board's two time-driven refresh triggers (#301), registered ONCE;
      // the predicate (wb-kanban.js) decides.
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState !== "visible") return;
        this.maybeRefreshBoard("visible");
        // The Changes backstop did nothing while the tab was hidden.
        this.refreshChanges();
        this.resumeSockets();
      });
      // A tablet resumes on a different link; its sockets died without a close.
      window.addEventListener("online", () => this.resumeSockets(true));
      // The console module cannot know whether THIS document's connection is
      // alive; hand it the heartbeat verdict, or its hidden-time fallback
      // resets every console after a minute on another tab.
      window.WBConsole?.setStaleProbe?.(() => this.socketsAreStale());
      // The selected checkouts (#406): the ONE hook for `unknown checkout`, and
      // the copy of the desk mirror once the boot desk lands.
      window.WBDaemon?.onUnknownCheckout?.((repo, name) => this.checkoutGone(repo, name));
      window.WBConsole?.whenDeskLoaded?.().then(() => this.adoptDeskCheckouts());
      // Anchor the clock at page load: `_boardLoadedAt` at 0 would clear the
      // 120s floor on the first tick.
      this._boardLoadedAt = Date.now();
      this._boardBackstop = setInterval(() => this.boardBackstopTick(), 30000);
      // Registered once; the tick asks whether the panel is open.
      this._changesBackstop = setInterval(() => this.refreshChanges(), this.CHANGES_POLL_MS);
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
    // fetch (file:// demo) leaves the fields empty.
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
    resumeSockets(stale) {
      const verdict = stale === undefined ? this.socketsAreStale() : stale;
      this._runsSub?.resume?.(verdict);
      this._changesSub?.resume?.(verdict);
      this._presenceSub?.resume?.(verdict);
    },

    // The `/ws` presence heartbeat (daemon mode). Each tick stamps
    // `_lastHeartbeat`, refreshes uptime, carries name/avatar once baptized, and
    // re-derives `live` so the sidebar dots track sessions (~2s).
    subscribePresence() {
      if (!window.WBMode.isDaemon() || !window.WBDaemon?.subscribePresence) return;
      this._presenceSub = window.WBDaemon.subscribePresence((p) => {
        this._lastHeartbeat = Date.now();
        this.uptimeText = "up " + this.fmtUptime(p.uptime_secs);
        if (p.name) this.identityName = p.name;
        if (p.avatar) this.identityAvatar = p.avatar;
        this.refreshLive();
      });
    },

    // Seconds → a compact `1d 2h`, `2h 14m`, `5m`, `12s` uptime string.
    fmtUptime(secs) {
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
    // (file://) keeps `authed` at its seed default.
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
        // ONLY the `file://` demo: a daemon that merely threw still has
        // `authed` at its `true` seed, and a restore against a dead daemon
        // closes every tab whose read fails — and `closeTab` persists.
        if (window.WBMode.isDemo()) this.restoreView();
      }
    },

    // The daemon's adapter roster (#304). A file:// walkthrough falls back to
    // the seed; in DAEMON mode a failed fetch leaves the roster EMPTY rather
    // than showing adapters this daemon may not have.
    _agentsSeq: 0,
    async loadAgents(repo = this.openSlug) {
      const seq = ++this._agentsSeq;
      try {
        const r = await fetch(window.WBAgents.rosterUrl(repo));
        if (!r.ok) throw new Error(`/api/agents ${r.status}`);
        const state = window.WBAgents.rosterState(await r.json(), repo);
        if (seq !== this._agentsSeq) return;
        this.roster = state.roster;
        this.agents = state.agents;
      } catch {
        if (seq !== this._agentsSeq) return;
        const state = window.WBAgents.rosterState(
          window.WBMode.seedAllowed() ? window.WB_SEED_ROSTER || [] : [],
          repo,
        );
        this.roster = state.roster;
        this.agents = state.agents;
      }
    },
    // Hydrate the accordion from the daemon's repo registry. A thrown fetch
    // (file://) keeps the seed. `remote` is inferred from the slug shape
    // (`git::project_slug`'s `path-<hash>` fallback is a remoteless repo).
    async loadRepos() {
      this.reposLoading = true;
      try {
        const r = await fetch("/api/repos");
        if (r.ok) {
          const repos = await r.json();
          this.projects = repos.map((x) => ({
            slug: x.slug,
            // The on-disk path: `repoLabel` needs it for a remoteless repo,
            // whose slug is a hash. The SLUG stays the identity (ADR-0008 D7).
            path: x.path || "",
            // The ABSOLUTE native root (#362), distinct from `path` (git's
            // forward-slashed `--show-toplevel` that peers parse).
            root: x.root || "",
            branch: x.branch || "",
            branches: x.branch ? [x.branch] : [],
            // `remote` is the github|local classification the dot binds to; the
            // raw origin url rides in `remoteUrl` for `githubUrl()`.
            dirty: !!x.dirty,
            state: x.reachable ? "idle" : "offline",
            remote: x.remote && x.remote.includes("github.com") ? "github" : "local",
            remoteUrl: x.remote || "",
            tree: [],
          }));
          this.reposError = "";
          // Deliberately NOT awaited: a down peer costs `/api/fleet` its 2 s
          // per-peer timeout, and holding `reposLoading` open for that would make
          // a peer's absence stall the LOCAL sidebar's spinner and live dots.
          // Federation is additive in latency too.
          this.loadFleet();
          this.refreshLive();
        } else if (window.WBMode.isDaemon()) {
          // Daemon mode: a failed fetch must NOT keep the seed projects (M5) —
          // clear them and show the error.
          this.projects = [];
          this.reposError = "could not load projects from the daemon";
        }
      } catch {
        if (window.WBMode.isDaemon()) {
          this.projects = [];
          this.reposError = "could not load projects from the daemon";
        }
        // Demo (file://): keep the seed — the shell stays navigable offline.
      } finally {
        this.reposLoading = false;
        // The sidebar refresh button is the Changes count's manual reload (#307).
        if (this.openSlug) this.loadChanges(this.openSlug);
        if (this.openSlug) this.loadSync(this.openSlug);
        // The rows' lucide icons are converted by the `x-effect` on
        // `ul.projects` (#332), bound to the list's contents, not here.
      }
    },

    // The local fleet (ADR-0052 §5, #349): append every PEER's repos after the
    // local `/api/repos` pass, plus the peer list the group headers render.
    // INVARIANT: a `/api/fleet` failure leaves the LOCAL list exactly as it was.
    async loadFleet() {
      try {
        const r = await fetch("/api/fleet");
        if (!r.ok) throw new Error(`/api/fleet ${r.status}`);
        const fleet = await r.json();
        this.fleetPeers = Array.isArray(fleet.peers) ? fleet.peers : [];
        const rows = Array.isArray(fleet.repos) ? fleet.repos : [];
        // `/api/fleet` is the ONLY source of this daemon's own environment label
        // and name; the local rows are stamped with it here.
        const mine = rows.find((x) => x.local);
        if (mine) {
          for (const p of this.projects) {
            p.env = mine.environment || "";
            p.daemonName = mine.daemon_name || "";
          }
        }
        const peerRows = rows.filter((x) => !x.local);
        this.projects = this.projects.concat(
          peerRows.map((x) => ({
            // `<daemon_id>/<slug>`: the same `owner/repo` on two daemons is two rows.
            key: x.key,
            slug: x.slug,
            path: x.path || "",
            branch: x.branch || "",
            branches: [],
            // The peer's OWN working-tree facts, same classification as `loadRepos`.
            dirty: !!x.dirty,
            state: x.reachable ? "idle" : "offline",
            remote: x.remote && x.remote.includes("github.com") ? "github" : "local",
            remoteUrl: x.remote || "",
            tree: [],
            // What makes this a peer row.
            daemon: x.daemon_id,
            daemonName: x.daemon_name || "",
            env: x.environment || "",
            peerState: x.peer_state || "",
          })),
        );
      } catch {
        this.fleetPeers = [];
      }
    },

    // Wake a sleeping peer. The operator's action is the consent (as push,
    // ADR-0046), which is why this lives in the workbench: a daemon nudging on
    // every probe would be supervising by accident (ADR-0052 §4).
    // `/api/fleet/nudge` resolves when the environment is USABLE.
    async wakePeer(daemonId) {
      if (!daemonId || this.waking[daemonId]) return false;
      this.waking[daemonId] = true;
      try {
        const r = await fetch(`/api/fleet/nudge?daemon_id=${encodeURIComponent(daemonId)}`, {
          method: "POST",
        });
        const reply = await r.json().catch(() => ({}));
        if (!r.ok || !reply.ready) {
          // The daemon's own sentence names the environment and what is wrong.
          this._flashAction(reply.diagnosis || reply.error || "The peer did not respond.");
          return false;
        }
        // `loadRepos`, not `loadFleet`: the latter CONCATENATES peer rows.
        await this.loadRepos();
        // A row opened against the sleeping peer has an empty tree: remount.
        if (window.WBFleet.refDaemon(this.openSlug) === daemonId) {
          this.destroyTree();
          this.mountTree();
        }
        return true;
      } catch {
        this._flashAction("wake unavailable: no daemon");
        return false;
      } finally {
        delete this.waking[daemonId];
      }
    },

    // Opening a row on a sleeping peer wakes it. A no-op for every other row.
    wakePeerFor(ref) {
      const daemon = window.WBFleet.refDaemon(ref);
      if (!daemon) return;
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      if (window.WBFleet.wakeable(group)) this.wakePeer(daemon);
    },

    peerWakeable(g) {
      return window.WBFleet.wakeable(g);
    },

    // Local rows first, then one group per peer environment (wb-fleet.js).
    fleetGroups() {
      return window.WBFleet.fleetGroups(this.filteredProjects(), this.fleetPeers);
    },
    repoRef(p) {
      return window.WBFleet.repoRef(p);
    },
    // Each project's `live` dot from `/api/sessions` (#204). Never overrides
    // `offline`; a transport throw leaves the states untouched.
    async refreshLive() {
      if (!window.WBMode.isDaemon()) return;
      try {
        const r = await fetch("/api/sessions");
        if (!r.ok) return;
        const sessions = await r.json();
        // The console menu's fold reads this (#304).
        this.liveSessions = sessions;
        // The console windows read their own row off the same poll (ADR-0059).
        window.WBConsole?.ingestSessions?.(sessions);
        for (const p of this.projects) {
          if (p.state === "offline") continue;
          const mine = sessions.filter((s) =>
            window.WBSessionRoute.matchesRepo(s, this.repoRef(p)),
          );
          // A `waiting` agent outranks `live` on the dot (ADR-0059).
          p.state = !mine.length
            ? "idle"
            : window.WBProject.agentStateOf(mine) === "waiting"
              ? "waiting"
              : "live";
        }
      } catch {}
    },

    // --- chrome panels ----------------------------------------------------
    // Sidebar, Runs panel and Kanban board: each a layout flip on a body class.
    sideOpen: true,
    // `projects` or `changes` (#317). Changes is a VIEW scoped to `openSlug`.
    sideView: "projects",
    runsOpen: false,
    kanbanOpen: false,
    projectQuery: "",

    // Clicking the rail button of the view already showing collapses the sidebar.
    showSideView(view) {
      if (this.sideOpen && this.sideView === view) {
        this.sideOpen = false;
        return;
      }
      this.sideView = view;
      this.sideOpen = true;
      // Opening Changes IS a read trigger: the rows were last read when the
      // project was opened.
      this.refreshChanges();
      // the incoming view's lucide icons live behind x-show and mount here
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // Re-read the working tree, only while the Changes panel is on screen: both
    // reads are local but each is a subprocess.
    refreshChanges() {
      if (!window.WBMode.isDaemon()) return;
      if (!this.sideOpen || this.sideView !== "changes" || !this.openSlug) return;
      if (document.visibilityState !== "visible") return;
      this.loadChanges(this.openSlug);
      this.loadSync(this.openSlug);
    },
    // The slow backstop: the `/ws/tree` nudge (#310) only reports what a RUN
    // did; an operator's own editor produces no event. Two git subprocesses per
    // minute, only while the panel is open and the tab in front.
    CHANGES_POLL_MS: 50000,

    // The change indicator for one row. Only slugs whose count was READ render
    // one: a `changes.list` per repo would be N git subprocesses on open.
    projectBadge(slug) {
      return window.WBChanges.projectBadge(this.changesCount, this.changesReadError, slug);
    },

    // Case-insensitive slug/branch/label filter. The sidebar count keeps
    // showing `projects.length`.
    filteredProjects() {
      const q = this.projectQuery.trim().toLowerCase();
      if (!q) return this.projects;
      // The filter must never fail to match what the row DOES print (the
      // label, #332); the raw `path` is not matched.
      // INVARIANT: the OPEN row always passes. Its `<li>` hosts the file tree
      // and the `/ws/tree` subscription, and an `x-for` rebuild that drops it
      // is an unmount nobody asked for (`destroyTree` never runs).
      return this.projects.filter(
        (p) =>
          this.rowOpen(p) ||
          p.slug.toLowerCase().includes(q) ||
          p.branch.toLowerCase().includes(q) ||
          this.repoLabel(p).toLowerCase().includes(q)
      );
    },

    // Sidebar row label: the repo name, UPPERCASED (wb-project.js).
    repoLabel(p) {
      return window.WBProject.repoLabel(p);
    },

    // What every surface OUTSIDE the sidebar prints for a repo ref. A peer ref
    // is `<daemon_id>/<owner>/<repo>`: the ULID is how the fleet ROUTES
    // (ADR-0052 §5), so the environment is printed in its place. The ref itself
    // is untouched on the wire, the desk and the tab ids. Row lookup by
    // `repoRef`, not slug: the same `owner/repo` on two daemons is two rows.
    projectLabel(ref) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      return window.WBFleet.refLabel(ref, row?.env);
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
      // the panel's lucide icons mount on open (they live inside x-if)
      if (this.runsOpen) {
        // `nowMs` is as stale as the panel has been closed; re-anchor before
        // the first paint.
        this.nowMs = Date.now();
        this.hydrateRuns();
        this.$nextTick(() => window.lucide?.createIcons());
      }
    },
    toggleKanban() {
      // The tasks board (wb-kanban.js): an overlay flip over the canvas.
      this.kanbanOpen = !this.kanbanOpen;
      if (this.kanbanOpen) {
        this.kanbanSel = null;
        // Lazy-load the tracker for the open project when the board opens.
        this.loadBoard();
        this.$nextTick(() => window.lucide?.createIcons());
      }
      WB.emit("kanban-toggle", { open: this.kanbanOpen });
    },

    // --- branch switcher --------------------------------------------------
    // The branch chip opens a filtered picker; switching or creating goes
    // through the daemon's `branch.*` verbs. The header reflects the pick
    // optimistically.
    branchOpen: false,
    branchModal: {
      slug: null,
      filter: "",
      branches: [],
      current: "",
      primaryBranch: "",
      dirty: false,
      checkoutDirty: false,
    },
    // A `worktree.remove` in flight, per repo ref: the chip's menu greys the
    // row and a second click is ignored until the re-read lands.
    worktreeRemoving: {},
    // The selected checkout per repo ref (#406, ADR-0063 §4): the REACTIVE copy
    // of `WBConsole`'s desk mirror (a closure variable there is invisible to
    // Alpine). `worktreeListings` is the last `worktree.list` reply per ref;
    // `_treeCheckout` is the checkout the mounted tree was built for.
    checkouts: {},
    worktreeListings: {},
    _treeCheckout: null,

    // Only when the daemon can reach the repo on disk. NOT gated on `remote`:
    // a local-only repo still has branches.
    canSwitchBranch(p) {
      return window.WBProject.canSwitchBranch(p);
    },

    // The branch chip lives on the Files bar (#332), which only the OPEN
    // project renders. `.project-slug` keeps its own title: it is the ADR-0008
    // D7 identity and how the browser tests find a row.
    rowOpen(p) {
      return this.openSlug === this.repoRef(p);
    },

    // Drop a project from the daemon's registry (#363); the disk is NOT
    // touched. The confirm is awaited BEFORE any `WBDaemon` call: cancel must
    // open no socket.
    async removeProject(p) {
      const ref = this.repoRef(p);
      const ok = await this.askConfirm({
        title: "Remove project",
        message: `Remove “${p.slug}” from Ralphy? Files on disk are kept.`,
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
          !window.WBFail.isError(reply) || window.WBFail.message(reply, "") === "unknown repo";
        if (!gone) {
          this._flashAction(window.WBFail.message(reply, "remove refused"));
          return;
        }
        // Identity is `repoRef`, not the slug: a peer can list the same slug.
        this.projects = this.projects.filter((x) => this.repoRef(x) !== ref);
        if (this.openSlug === ref) this.openSlug = null;
        this.loadRepos();
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("remove unavailable: no daemon");
      }
    },

    rowTitle(p) {
      return window.WBProject.rowTitle(p);
    },

    branchChipTitle(p) {
      const ref = this.repoRef(p);
      return window.WBProject.branchChipTitle(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    chipDirty(p) {
      const ref = this.repoRef(p);
      return window.WBProject.chipDirty(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },

    openBranchModal(p) {
      if (!this.canSwitchBranch(p)) return;
      // Reaching for the picker IS the next branch act.
      this.branchError = "";
      const ref = this.repoRef(p);
      // Under a selection "current" is the WORKTREE's branch (#407).
      const ck = this.checkoutOf(ref);
      const wt = ck ? (this.worktreeListings[ref]?.worktrees || []).find((w) => w && w.name === ck) : null;
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
        window.lucide?.createIcons();
        this.$refs.branchFilter?.focus();
      });
    },

    // The repo's real local branches via `branch.list` (#199). On throw (no
    // daemon) the modal keeps its seed.
    async loadBranches(slug) {
      try {
        const reply = await window.WBDaemon.observe(
          "branch.list",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (this.branchModal.slug !== slug) return; // modal moved on — leave it
        if (!reply || reply.status !== "ok") {
          // Daemon mode: a failed `branch.list` must NOT keep the seed (M5).
          if (window.WBMode.isDaemon()) {
            this.branchModal.branches = [];
            this._flashAction?.("could not load branches");
          }
          return;
        }
        // The daemon nests the CLI's `{current, branches:[]}` JSON under the
        // `branches` field (lib.rs Query reply), same as `reply.board.*` /
        // `reply.issue.*` — read one level deeper, not the top level.
        const data = reply.branches || {};
        if (Array.isArray(data.branches)) this.branchModal.branches = data.branches;
        if (data.current) this.branchModal.current = data.current;
      } catch {
        // Daemon mode: transport error → honest empty list, not the seed (M5).
        if (this.branchModal.slug === slug && window.WBMode.isDaemon()) {
          this.branchModal.branches = [];
          this._flashAction?.("could not load branches");
        }
        // Demo (static shell): keep the seed.
      }
    },

    closeBranchModal() {
      this.branchOpen = false;
    },

    // The open project's change count (#307) via `changes.list`: reloads on
    // open, sidebar refresh and run-completion nudge (#310), never on a
    // repo-wide watch. The SELECTED checkout's (#407, ADR-0063 §2).
    async loadChanges(slug) {
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
          if (window.WBMode.isDaemon()) {
            // Honest absence beats another repo's number.
            this.changesCount[slug] = null;
            this.changesReadError[slug] = "could not read changes";
            this.changesStaged[slug] = [];
            this.changesUnstaged[slug] = [];
          }
          return;
        }
        const folded = window.WBChanges.fold(reply);
        this.changesCount[slug] = folded.count;
        this.changesStaged[slug] = folded.staged;
        this.changesUnstaged[slug] = folded.unstaged;
        this.changesReadError[slug] = "";
      } catch {
        if (seq === this._changesSeq && window.WBMode.isDaemon()) {
          this.changesCount[slug] = null;
          this.changesReadError[slug] = "could not read changes";
          this.changesStaged[slug] = [];
          this.changesUnstaged[slug] = [];
        }
        // Demo (static shell): leave whatever the seed/previous load holds.
      }
    },

    // The open project's sync state (#316) via `sync.status`, which makes NO
    // network call. No timer: a launcher holding N repos must never become a
    // scheduled network client.
    async loadSync(slug) {
      if (!slug) return;
      const seq = ++this._syncSeq;
      try {
        const reply = await window.WBDaemon.observe(
          "sync.status",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (seq !== this._syncSeq) return; // superseded → the newer read owns it
        this.syncByProject[slug] = window.WBChanges.foldSync(reply);
      } catch {
        if (seq === this._syncSeq && window.WBMode.isDaemon()) {
          // Honest absence beats a stale row.
          this.syncByProject[slug] = window.WBChanges.foldSync(null);
        }
        // Demo (static shell): leave whatever the previous load holds.
      }
    },

    // Fetch from the upstream — the operator's act, never a timer's. A refusal
    // is `{status:"error"}` whose message IS the core's prose.
    async syncFetch(slug) {
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "sync.fetch",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(window.WBFail.message(reply, "fetch refused"));
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered.
        if (window.WBMode.isDaemon()) this._changesRefused("fetch unavailable: no daemon");
      }
      this.loadSync(slug);
    },

    // Fast-forward from the upstream. A successful pull moves the working tree,
    // so the change set is reloaded beside the counts.
    async syncPull(slug) {
      this.changesError = "";
      let moved = false;
      try {
        const reply = await window.WBDaemon.observe(
          "sync.pull",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(window.WBFail.message(reply, "pull refused"));
        } else {
          moved = true;
        }
      } catch {
        if (window.WBMode.isDaemon()) this._changesRefused("pull unavailable: no daemon");
      }
      this.loadSync(slug);
      if (moved) this.loadChanges(slug);
    },

    // Publish the branch (#320). The OPERATOR's click is the whole consent (no
    // opt-in flag, ADR-0046 amendment); a refusal's message IS the core's
    // prose. No credential UI, by decision. Push moves no file, so only the
    // counts reload.
    async syncPush(slug) {
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "sync.push",
          window.WBDaemon.withCheckout({ repo: slug }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(window.WBFail.message(reply, "push refused"));
        }
      } catch {
        if (window.WBMode.isDaemon()) this._changesRefused("push unavailable: no daemon");
      }
      this.loadSync(slug);
    },

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
    async stopRun(runid) {
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
          repo: this.openSlug,
          runid,
        });
        if (window.WBFail.isError(reply)) {
          this.runVerbFailed(window.WBFail.message(reply, "stop refused"));
        } else {
          this._flashAction("Stop requested. The run is stopping.");
        }
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("stop unavailable: no daemon");
      } finally {
        this.runStopping = null;
      }
    },

    // ---- write controls (#318) ------------------------------------------
    // Disabled from the open repo's LIVE RUN list (`runs.list`, ADR-0047 §9).
    // A HINT, not the authority: the CLI's `guard_run_lock` refuses
    // unconditionally, and a `ralphy triage` holding the lock writes no run
    // snapshot, so a click can still be refused while these look enabled.
    writeLocked() {
      return !!this.writeLockReason();
    },
    writeLockReason() {
      return window.WBChanges.writeLockReason(this.runsByProject[this.openSlug]);
    },
    // The board's label editor, under the SAME lock: `label set` is a
    // run-lock-aware Mutate (mutate.rs).
    labelsLocked() {
      return !!this.labelLockReason();
    },
    labelLockReason() {
      return window.WBChanges.writeLockReason(
        this.runsByProject[this.openSlug],
        "Labels are read-only while a run is active.",
      );
    },
    // The run verbs reuse the Changes derivation LITERALLY (#331). CAVEAT:
    // `guard_run_lock` is called by changes/config/mutate/sync only; `ralphy
    // run`, `ralphy triage` and `push` do NOT refuse on a live lock (runlock.rs:
    // "a signal, never a mutex"), so for those this `disabled` is the only gate.
    verbLocked() {
      return this.writeLocked();
    },
    verbTitle(verb) {
      return window.WBRun.verbLockTitle(verb, this.writeLockReason());
    },
    rowActTitle(verb) {
      const locked = this.writeLockReason();
      if (locked) return locked;
      if (verb === "stage") return "stage this path";
      if (verb === "discard") return "discard this path's changes";
      return "unstage this path";
    },
    // Push's title (#320) states the run-lock reason, as `rowActTitle` does.
    pushTitle() {
      return this.writeLockReason() || "publish this branch to its remote";
    },
    groupNote(group) {
      return window.WBChanges.groupDiscardNote(group);
    },
    commitTarget() {
      return window.WBChanges.commitTarget(this.syncByProject[this.openSlug]);
    },
    // `withOriginal` only on the UNSTAGE direction — see `wb-changes.js`.
    groupPaths(list, withOriginal) {
      return window.WBChanges.groupPaths(list, withOriginal);
    },
    commitTitle() {
      const locked = this.writeLockReason();
      if (locked) return locked;
      if (!(this.changesStaged[this.openSlug] || []).length) {
        return "Stage a file first.";
      }
      if (!this.commitMsg.trim()) return "write a commit message first";
      return this.commitTarget().label;
    },
    canCommit() {
      return (
        !this.writeLocked() &&
        this.commitMsgSlug === this.openSlug &&
        !!this.commitMsg.trim() &&
        !!(this.changesStaged[this.openSlug] || []).length
      );
    },

    // Stage / unstage / commit, each in `syncFetch`'s shape, re-reading the
    // list on EVERY path. The list is never moved optimistically.
    async stagePaths(slug, paths) {
      if (!slug || !paths || !paths.length) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.stage",
          window.WBDaemon.withCheckout({ repo: slug, paths }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(window.WBFail.message(reply, "stage refused"));
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered.
        if (window.WBMode.isDaemon()) this._changesRefused("stage unavailable: no daemon");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    async unstagePaths(slug, paths) {
      if (!slug || !paths || !paths.length) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.unstage",
          window.WBDaemon.withCheckout({ repo: slug, paths }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(window.WBFail.message(reply, "unstage refused"));
        }
      } catch {
        if (window.WBMode.isDaemon()) this._changesRefused("unstage unavailable: no daemon");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    // Discard ONE row's changes (#319) — the only irreversible act here, so the
    // only one confirmed (`discardConfirm`). A cancel makes NO daemon call.
    async discardRow(slug, entry) {
      if (!slug || !entry || !entry.path) return;
      const c = window.WBChanges.discardConfirm(entry);
      const ok = await this.askConfirm({
        title: c.title,
        message: c.message,
        confirmLabel: c.confirmLabel,
        danger: true,
      });
      if (!ok) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.discard",
          window.WBDaemon.withCheckout({ repo: slug, paths: [entry.path] }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(window.WBFail.message(reply, "discard refused"));
        }
      } catch {
        if (window.WBMode.isDaemon()) this._changesRefused("discard unavailable: no daemon");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    async commitStaged(slug) {
      // Never commit a draft composed for another project.
      if (this.commitMsgSlug !== slug) return;
      const message = this.commitMsg.trim();
      if (!slug || !message) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.commit",
          window.WBDaemon.withCheckout({ repo: slug, message }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(window.WBFail.message(reply, "commit refused"));
        } else {
          // Cleared on success ONLY: a refused commit must not eat the message.
          this.commitMsg = "";
        }
      } catch {
        if (window.WBMode.isDaemon()) this._changesRefused("commit unavailable: no daemon");
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
    hasWorktrees(p) {
      return window.WBProject.hasWorktrees(this.worktreeListings[this.repoRef(p)] || null);
    },
    openCheckoutChip(p, anchor) {
      const ref = this.repoRef(p);
      const listing = this.worktreeListings[ref] || null;
      const mine = (this.liveSessions || []).filter((s) => window.WBSessionRoute.matchesRepo(s, ref));
      window.WBConsole.checkoutMenu({
        anchor,
        host: document.body,
        rows: window.WBConsole.checkoutMenuRows(listing, this.checkoutOf(ref), mine, p.branch, !!p.dirty),
        onPick: (row) => this.setCheckout(ref, row.primary ? null : row.name),
        onRemove: (row) => this.removeWorktree(ref, row),
      });
    },

    // --- the selected checkout (#406, ADR-0063 §4) ----------------------------
    checkoutOf(ref) {
      return this.checkouts[ref] || null;
    },
    chipLabel(p) {
      const ref = this.repoRef(p);
      return window.WBProject.chipLabel(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    // The reactive map is REPLACED so Alpine sees it; persistence goes to the
    // desk mirror; an open tree is remounted (cache key, watch and rows are
    // per checkout).
    setCheckout(ref, name) {
      const next = { ...this.checkouts };
      if (name) next[ref] = String(name);
      else delete next[ref];
      this.checkouts = next;
      window.WBConsole?.setCheckout?.(ref, name || null);
      if (this.openSlug === ref && this._treeCheckout !== (name || null)) {
        this.destroyTree();
        this.mountTree();
      }
      // The Changes panel and the sync row are the SELECTED checkout's (#407).
      if (this.openSlug === ref) {
        this.loadChanges(ref);
        this.loadSync(ref);
      }
    },
    // The daemon answered `unknown checkout` for `name`: drop the selection —
    // unless it already moved on, in which case a late reply says nothing.
    checkoutGone(ref, name) {
      if (this.checkoutOf(ref) !== name) return;
      this.setCheckout(ref, null);
      this._flashAction(`Worktree ${name} no longer exists. Showing the primary tree.`);
    },
    // The chip needs the worktree's BRANCH, which only `worktree.list` knows:
    // one read per ref, `force` re-reads (after a branch act the chip
    // converges from this reply, #407). A forced re-read that fails DROPS the
    // cached entry rather than showing the pre-act branch. Newest read wins.
    async ensureWorktreeListing(ref, force = false) {
      if (!ref || ref === "~" || (this.worktreeListings[ref] && !force)) return;
      const seq = (this._listingSeq = (this._listingSeq || 0) + 1);
      let listing = null;
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
      const before = this.openSlug ? this.checkoutOf(this.openSlug) : null;
      this.checkouts = window.WBConsole?.checkouts?.() || {};
      if (this.openSlug && this._treeCheckout !== this.checkoutOf(this.openSlug)) {
        this.destroyTree();
        this.mountTree();
      }
      if (this.openSlug) {
        this.ensureWorktreeListing(this.openSlug);
        // The desk can land AFTER the open's own reads: re-read under the
        // restored selection, only when it differs (two git spawns otherwise).
        if (this.checkoutOf(this.openSlug) !== before) {
          this.loadChanges(this.openSlug);
          this.loadSync(this.openSlug);
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
    switchBranch(name) {
      if (name !== this.branchModal.current) {
        const slug = this.branchModal.slug;
        const checkout = this.checkoutOf(slug);
        const p = checkout ? null : this.projects.find((x) => this.repoRef(x) === slug);
        const prev = p ? p.branch : null;
        if (p) p.branch = name; // optimistic — the chip updates immediately
        WB.emit("branch-switch", { project: slug, branch: name, checkout });
        // The run-lock-aware `branch.switch` Mutate (#199): refusal → revert.
        this._mutateBranch("branch.switch", slug, name, () => {
          if (p) p.branch = prev;
        });
      }
      this.closeBranchModal();
    },

    createBranch() {
      if (!this.canCreateBranch()) return;
      const name = this.branchModal.filter.trim();
      const from = this.branchModal.current;
      const slug = this.branchModal.slug;
      const checkout = this.checkoutOf(slug);
      const p = checkout ? null : this.projects.find((x) => this.repoRef(x) === slug);
      const prevBranch = p ? p.branch : null;
      const prevBranches = p ? [...(p.branches || [])] : null;
      if (p) {
        p.branches = [...(p.branches || []), name];
        p.branch = name; // a fresh branch is checked out onto
      }
      WB.emit("branch-create", { project: slug, name, from, checkout });
      this._mutateBranch("branch.create", slug, name, () => {
        if (p) {
          p.branch = prevBranch;
          p.branches = prevBranches;
        }
      });
      this.closeBranchModal();
    },


    // The chip menu's trash action: `worktree.remove` (#409). The listing is
    // the truth on every path (a `branch kept` reply is an error whose
    // directory is gone), so the selection resets from the re-read
    // (`checkoutAfterListing`), never from the reply's status. A refusal lands
    // verbatim in a notice with one OK: the menu it came from has closed.
    async removeWorktree(slug, w) {
      if (!slug || !w || w.primary || this.worktreeRemoving[slug]) return;
      this.worktreeRemoving = { ...this.worktreeRemoving, [slug]: w.name };
      const refused = (message) =>
        window.WBConsole.askNotice({ title: `Cannot remove worktree ${w.name}`, message });
      try {
        const reply = await window.WBDaemon.observe("worktree.remove", { repo: slug, name: w.name });
        if (window.WBFail.isError(reply)) {
          refused(window.WBFail.message(reply, "worktree remove refused"));
        } else {
          this._flashAction(`worktree ${w.name} removed`);
        }
      } catch {
        if (window.WBMode.isDaemon()) {
          refused("Could not reach the daemon. Check whether the worktree was removed.");
        }
      } finally {
        await this.ensureWorktreeListing(slug, true);
        const ck = this.checkoutOf(slug);
        if (ck && window.WBProject.checkoutAfterListing(ck, this.worktreeListings[slug]) === null) {
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
    async _mutateBranch(verb, slug, name, revert) {
      try {
        const reply = await window.WBDaemon.observe(
          verb,
          WBDaemon.withCheckout({ repo: slug, name }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          revert();
          this._branchRefused(window.WBFail.message(reply, "branch change refused"));
        }
      } catch {
        // A transport throw is NOT a refusal, so the optimistic update STAYS —
        // the verb may have landed.
        if (window.WBMode.isDaemon()) {
          this._branchRefused("Could not reach the daemon. Check whether the branch changed.");
        }
      } finally {
        // Re-read the listing on every path (an unconfirmed switch may have
        // landed); a moved HEAD also changes the working tree and sync row.
        this.ensureWorktreeListing(slug, true);
        this.loadChanges(slug);
        this.loadSync(slug);
      }
    },
    // The Projects panel's counterpart to `_changesRefused`.
    _branchRefused(msg) {
      this.branchError = msg || "";
      this._flashAction(msg);
    },

    // --- Runs panel -------------------------------------------------------
    // One entry per `runid`: issue queue + per-issue status, live phase, the
    // current issue's plan.md (helpers in wb-runs.js, `window.WBRun`).
    runsByProject: {},
    // An error must never render as "No active runs": an empty project and an
    // unreadable one are different facts (ADR-0047 §6).
    runsError: "",
    currentRunId: null,
    // The clock's "now", advanced by the tick in `init` while the panel is
    // open. STATE, not `Date.now()` in the getter: Alpine only re-renders what
    // it can observe changing.
    nowMs: Date.now(),
    // The trail node the operator arrived at from the board (#301): a marker,
    // not a selection.
    trailFocus: null,
    runMenu: false,
    planSection: "",

    // Hydrate runs from the seed, `file://`-ONLY (#300): daemon mode is fed by
    // `runs.list` + `runs.dirty` pushes (ADR-0047 §9).
    initRuns() {
      if (!window.WBMode.seedAllowed()) {
        this.runsByProject = {};
        return;
      }
      const src = window.WB_RUNS || {};
      const out = {};
      for (const [proj, runs] of Object.entries(src)) {
        out[proj] = runs.map((r) => {
          const planMd = (document.getElementById(r.planEl)?.textContent || "").trim();
          return {
            ...r,
            planMd,
            // The demo has no snapshot document: steps from the plan text (#330).
            steps: window.WBRun.parseSteps(planMd),
            planIssue: r.active ?? null,
            planReadFailed: false,
          };
        });
      }
      this.runsByProject = out;
    },

    // Hydrate from `runs.list` (ADR-0047 §9), by REPLACEMENT — a snapshot is
    // state, not a log. On panel open and project change.
    async hydrateRuns() {
      if (!window.WBMode.isDaemon()) return;
      const slug = this.openSlug;
      // Clear FIRST: a stale error must not outlive its project.
      this.runsError = "";
      if (!slug) return;
      const prevRuns = this.runsByProject[slug] || [];
      const seq = ++this._runsSeq;
      try {
        const reply = await window.WBDaemon.observe("runs.list", { repo: slug });
        // Superseded while in flight: the newer hydration owns the state.
        if (seq !== this._runsSeq || this.openSlug !== slug) return;
        if (reply?.status !== "ok") {
          this.runsByProject[slug] = [];
          this.runsError = reply?.reason || reply?.message || "could not read runs";
          return;
        }
        this.runsByProject[slug] = (reply.runs || []).map((d) => {
          const run = window.WBRun.fromSnapshot(d);
          // A push arrives on every snapshot write (~every few hundred ms);
          // re-fetching an unchanged plan would blank the viewer each time.
          const prev = prevRuns.find((p) => p.runid === run.runid);
          if (prev && prev.planPath === run.planPath) {
            run.planMd = prev.planMd;
            run.planReadFailed = prev.planReadFailed;
          }
          return run;
        });
        const bad = reply.unreadable || [];
        this.runsError = bad.length
          ? `Could not read ${bad.length} run${bad.length > 1 ? "s" : ""}: ` +
            bad.map((u) => `${u.runid} (${u.reason})`).join(", ")
          : "";
        // Keep the selected run while it is still listed.
        const listed = this.projectRuns();
        this.currentRunId = listed.some((r) => r.runid === this.currentRunId)
          ? this.currentRunId
          : listed[0]?.runid || null;
        // Only while showing: a whole-plan `file.read` nobody can see is cost.
        if (this.runsOpen) await this.loadRunPlan();
      } catch (err) {
        if (seq !== this._runsSeq || this.openSlug !== slug) return;
        // A transport failure is a read failure, not an idle project.
        this.runsByProject[slug] = [];
        this.runsError = String(err?.message || err || "could not reach the daemon");
      } finally {
        // The panel body is `x-if` on `projectRuns().length`, so its icons
        // exist only once THIS read lands (#332).
        this.$nextTick(() => window.lucide?.createIcons());
      }
    },

    // Read the selected run's plan via `file.read` (the document carries its
    // PATH, never its text). A refusal KEEPS the last good text and only flags
    // it (#330); `runsError` is untouched. Reads the PRIMARY tree — no
    // `checkout` (#406): a run takes the primary tree (ADR-0063 §7). Same for
    // `loadPlan`, `diffWorkSide` and the git-backed panels.
    async loadRunPlan() {
      if (!window.WBMode.isDaemon()) return;
      const run = this.currentRun();
      if (!run?.planPath) return;
      try {
        const reply = await window.WBDaemon.observe("file.read", {
          repo: this.openSlug,
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
      return this.runsByProject[this.openSlug] || [];
    },
    // The selected run, falling back to the first when the id is stale (e.g. the
    // project changed).
    currentRun() {
      const runs = this.projectRuns();
      return runs.find((r) => r.runid === this.currentRunId) || runs[0] || null;
    },
    selectRun(runid) {
      this.currentRunId = runid;
      this.trailFocus = null; // the arrival marker belonged to the run we left
      // reset the section dropdown to the new run's first non-Steps heading
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      // each run has its own plan; the viewer follows the selection.
      this.loadRunPlan();
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // Thin delegations to the faithful helpers in wb-runs.js.
    runPhaseLabel(run) {
      return run ? window.WBRun.runPhaseLabel(run) : "";
    },
    runTitle(run) {
      return window.WBRun.runTitle(run);
    },
    runIdentity(run) {
      return window.WBRun.runIdentity(run);
    },
    // Reading `nowMs` subscribes this binding to the 1 s tick.
    runClock(run) {
      return window.WBRun.phaseClock(run, this.nowMs);
    },
    // When this phase began, and the run's whole elapsed time.
    clockTitle(run) {
      if (!run) return "";
      const parts = [];
      const at = (iso) =>
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
    issueState(run, iss) {
      return window.WBRun.issueState(run, iss);
    },
    issueGlyph(run, iss) {
      return window.WBRun.glyph(run, iss);
    },
    sleepLabel(run) {
      return window.WBRun.sleepText(run?.sleep);
    },
    nodeTitle(run, iss) {
      if (!run || !iss) return "";
      const st = window.WBRun.issueState(run, iss);
      let t = `#${iss.number} — ${iss.title} · ${window.WBRun.LABEL[st] || st}`;
      // Per-issue: tier routing gives two issues of one run different models.
      const seg = window.WBRun.modelEffort(iss.model, iss.effort);
      if (seg) t += ` · ${seg}`;
      if (iss.blockedBy?.length) t += ` (blocked by ${iss.blockedBy.map((n) => "#" + n).join(", ")})`;
      return t;
    },
    // Run → board (#301): a trail node opens that issue's detail. The Runs
    // panel closes first (`z-index: 150`, sharing the drawer's right edge).
    // `toggleKanban()` resets `kanbanSel`, so it runs BEFORE `openIssue`.
    focusIssue(number) {
      WB.emit("run-issue-focus", { project: this.openSlug, runid: this.currentRun()?.runid, issue: number });
      this.runsOpen = false;
      this.trailFocus = null;
      if (!this.kanbanOpen) this.toggleKanban();
      this.openIssue(number);
    },

    // Board → run (#301): the card's run pill opens the Runs panel on THAT run,
    // marking the issue in the trail. The board stays open behind it.
    openRunFor(number) {
      const hit = window.WBKanban.runningFor(number, this.projectRuns());
      if (!hit) return;
      this.currentRunId = hit.runid;
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      this.loadRunPlan();
      // `toggleRuns()` would CLOSE an already-open panel — only open it.
      if (!this.runsOpen) this.toggleRuns();
      else this.$nextTick(() => window.lucide?.createIcons());
      this.trailFocus = number;
      this.$nextTick(() =>
        document
          .querySelector('.trail-node[data-issue="' + number + '"]')
          ?.scrollIntoView({ block: "nearest", inline: "nearest" }),
      );
    },

    // --- plan viewer ------------------------------------------------------
    // The issue whose plan this panel is showing: the snapshot's `plan.issue`
    // when it has one, else the run's active issue.
    planIssueWanted(run) {
      return run?.planIssue ?? run?.active ?? null;
    },
    // The issue the PROSE belongs to, from the plan's own trailer. The steps
    // are keyed by issue (ADR-0047 A1) but the prose is a `file.read` of
    // `.ralphy/plan.md`, and a failed read KEEPS the last text (#330): without
    // the key the block would render the PREVIOUS issue's plan. Unkeyed prose
    // (a half-written plan) stays empty.
    planProseIssue(run) {
      return window.WBRun.planTrailerIssue(run?.planMd);
    },
    planProseIsCurrent(run) {
      return window.WBRun.planBelongsTo(run?.planMd, this.planIssueWanted(run));
    },
    // Every `##` section except Steps (its own block); none while the prose
    // belongs to another issue.
    planHeadings(run) {
      if (!this.planProseIsCurrent(run)) return [];
      return window.WBRun.headings(run?.planMd).filter((h) => h.toLowerCase() !== "steps");
    },
    // Render one `##` section as sanitized HTML. Steps render from the
    // snapshot document, not from here (#330).
    renderPlanSection(run, name) {
      if (!run || !name || !this.planProseIsCurrent(run)) return "";
      const body = window.WBRun.section(run?.planMd, name);
      return DOMPurify.sanitize(marked.parse(body || "_(empty)_"));
    },

    // --- the step list (the plan block is state, #330) ---------------------
    planSteps() {
      return this.currentRun()?.steps || [];
    },
    stepGlyph(status) {
      return window.WBRun.stepGlyph(status);
    },
    stepLabel(status) {
      return window.WBRun.stepLabel(status);
    },
    stepClass(status) {
      return window.WBRun.stepClass(status);
    },
    // Why the step list is empty — an unexplained blank block reads as a bug.
    stepsNote() {
      const run = this.currentRun();
      if (this.planSteps().length) return "";
      if (run?.phase === "planning") return "writing the plan…";
      if (run?.planIssue != null) return "this plan has no steps";
      return "no plan for this issue yet";
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
      if (run.planReadFailed) return "could not read plan.md";
      if (run.phase === "planning") return "writing the plan…";
      if (run.planMd) return "The plan is still being written.";
      return wanted != null ? `No plan for #${wanted} yet.` : "no plan for this issue yet";
    },

    // --- run / triage / push (the daemon verbs) ---------------------------
    // The remote-trigger verbs (dispatch.rs), scoped to the open project.
    // `triage`/`push` are no-arg; `run` opens a modal for --agent,
    // --plan-agent and --branch-mode new|current.
    runOpen: false,
    runsActionMsg: "",
    // A CLI refusal, held until the next verb click (#331). Distinct from the
    // 2.6 s `runsActionMsg` flash.
    verbError: "",
    // Phase 1 raw merged output of the last daemon-spawned run (wb-daemon.js).
    rawFeed: "",
    // COLLAPSED by default and re-collapsed by every reset: a feed that opens
    // itself takes up to 30vh on every verb click. Opt-IN, not hidden.
    rawFeedOpen: false,
    runCfg: { agent: "claude", split: false, planAgent: "claude", branchMode: "new" },

    openRunModal() {
      // seed the planner to mirror the executor so an un-split run is coherent
      this.runCfg = { agent: "claude", split: false, planAgent: "claude", branchMode: "new" };
      this.runOpen = true;
      this.$nextTick(() => window.lucide?.createIcons());
    },
    closeRunModal() {
      this.runOpen = false;
    },
    // The current git branch of the open project (for the "current" mode blurb).
    openProjectBranch() {
      return this.projects.find((p) => this.repoRef(p) === this.openSlug)?.branch || "current";
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
      WB.emit("run-start", {
        project: this.openSlug,
        agent: c.agent,
        planAgent,
        branchMode: c.branchMode,
        command: this.runCommandPreview(),
      });
      this._flashAction("run started");
      this.closeRunModal();
    },
    // triage / push: the verb name is the whole intent; the client never
    // composes a command line.
    fireVerb(verb) {
      this._resetVerbSurface();
      WB.emit("command", { project: this.openSlug, verb });
      this._flashAction(`${verb} requested`);
    },
    // From wb-daemon.js on a TERMINAL frame only; an empty note is a no-op.
    runVerbFailed(msg) {
      if (msg) this.verbError = msg;
    },
    _flashAction(msg) {
      this.runsActionMsg = msg;
      clearTimeout(this._actionTimer);
      this._actionTimer = setTimeout(() => (this.runsActionMsg = ""), 2600);
    },
    // A refusal from the CHANGES panel lands in that panel and STAYS; the flash
    // is kept beside it. `runs-verb-error`'s counterpart (#331).
    _changesRefused(msg) {
      this.changesError = msg || "";
      this._flashAction(msg);
    },

    // --- inbound event fold (the backend seam) ----------------------------
    // Advance the panel per CloudEvent; unknown types are ignored.
    applyRunEvent(ev) {
      // Demo-only (#300): daemon mode is driven by snapshot REPLACEMENT, and a
      // client-side fold could only produce state the next push overwrites.
      if (!window.WBMode.seedAllowed()) return;
      if (!ev || !ev.runid) return;
      let run = null;
      for (const arr of Object.values(this.runsByProject)) {
        const f = arr.find((r) => r.runid === ev.runid);
        if (f) {
          run = f;
          break;
        }
      }
      if (!run) return;
      const d = ev.data || {};
      switch (ev.type) {
        case "dev.ralphy.plan.step": {
          // tick the next open checkbox (the panel just advances a step)
          run.planMd = run.planMd.replace(/-\s+\[ \]/, "- [x]");
          const open = (run.steps || []).find((s) => s.status === "open");
          if (open) open.status = "checked";
          break;
        }
        case "dev.ralphy.issue.closed": {
          const iss = run.issues.find((x) => x.number === d.number);
          if (iss) iss.status = "done";
          this._recount(run);
          break;
        }
        case "dev.ralphy.issue.skipped": {
          const iss = run.issues.find((x) => x.number === d.number);
          if (iss) {
            iss.status = "skipped";
            iss.blockedBy = d.blocked_by || [];
          }
          this._recount(run);
          break;
        }
        case "dev.ralphy.issue.started": {
          const iss = run.issues.find((x) => x.number === d.number);
          if (iss) iss.status = "executing";
          run.active = d.number;
          run.phase = "executing";
          break;
        }
        case "dev.ralphy.run.sleep_started":
          run.phase = "sleeping";
          run.sleep = { reset: d.reset || null, target_epoch: d.target_epoch || 0 };
          break;
        case "dev.ralphy.run.sleep_ended":
          run.phase = "executing";
          run.sleep = null;
          break;
        case "dev.ralphy.run.heartbeat":
          if (d.phase) run.phase = d.phase;
          if (typeof d.queue_done === "number") run.completed = d.queue_done;
          if (d.issue) run.active = d.issue.number;
          break;
      }
    },
    _recount(run) {
      run.completed = run.issues.filter((x) => window.WBRun.TERMINAL.has(x.status)).length;
    },

    // Demo: walk the selected run forward by synthesizing the next event.
    demoTick() {
      if (!window.WBMode.seedAllowed()) return; // the ⚡ control is demo-only (#300)
      const r = this.currentRun();
      if (!r) return;
      if ((r.planMd || "").match(/-\s+\[ \]/)) {
        this.applyRunEvent({ type: "dev.ralphy.plan.step", runid: r.runid, data: { status: "checked" } });
        return;
      }
      if (r.active != null) {
        this.applyRunEvent({ type: "dev.ralphy.issue.closed", runid: r.runid, data: { number: r.active } });
      }
      const next = r.issues.find((x) => x.status === "pending");
      if (next) {
        this.applyRunEvent({ type: "dev.ralphy.issue.started", runid: r.runid, data: { number: next.number } });
        r.planMd = "## Steps\n- [ ] plan for #" + next.number + " (planner writing…)\n";
        r.steps = [{ text: "plan for #" + next.number + " (planner writing…)", status: "open" }];
        r.planIssue = next.number;
      } else {
        r.active = null;
        r.phase = "consolidating";
      }
    },

    // --- Kanban board -----------------------------------------------------
    // The open project's issues in four columns (window.WBKanban). Read-only
    // except labels, the one mutation that moves a card.
    KANBAN: window.WBKanban,
    // Fed by `board.list` (#198): rows adapted to the issue shape, and the
    // repo's name→color label map. Empty until `loadBoard()` resolves.
    boardIssues: {},
    boardLabels: {},
    // A `board.list` failure (#207): a broken tracker connection must never
    // read as "no work to do".
    boardError: {},
    // The open drawer's detail-fetch failure (#302). One string: exactly one
    // drawer is open at a time.
    issueError: null,
    issueLoading: false,
    // Refresh bookkeeping (#301). `_boardLoadedAt` is stamped at fold START,
    // before any await, so the min-gap measures spacing between STARTS and an
    // erroring board throttles like a healthy one. `_boardPending` COALESCES a
    // trigger that arrived mid-fold into one follow-up load.
    _boardLoadedAt: 0,
    _boardPending: false,
    _boardBackstop: null,
    _changesBackstop: null,
    boardRefreshing: false,
    // The daemon awaits the board CLI with no timeout of its own; a wedged `gh`
    // must not disable the board for the page's life. Generous: a real fold
    // makes several calls.
    BOARD_FOLD_TIMEOUT_MS: 90000,
    kanbanSel: null, // the selected issue number → opens the detail drawer
    kanbanFilter: "", // search box (title / #num / body / label)
    kanbanLabel: "__all", // label filter: __all | __none | <label>
    kanbanSort: "num-desc", // Backlog sort (Ready columns keep graph order)

    // --- the repo's ready plan, on the board -------------------------------
    // A FINALIZED `.ralphy/plan.md` is executed by the next run (the trailer is
    // the resume signal, `WBRun.planTrailerIssue`), so the board shows it and
    // can throw it away. `planByProject[slug] = { md, summary }`, replaced on
    // every board load via the same confined `file.read`. Daemon-only (#300).
    planByProject: {},
    planModal: { open: false, issue: null },

    async loadPlan(slug) {
      if (!window.WBMode.isDaemon() || !slug) return;
      try {
        const reply = await window.WBDaemon.observe("file.read", {
          repo: slug,
          path: ".ralphy/plan.md",
        });
        // A refusal is the ORDINARY case (no plan sitting around): it clears
        // the entry rather than raising an error.
        const md = reply?.status === "ok" ? reply.content || "" : "";
        this.planByProject[slug] = md ? { md, summary: window.WBRun.planSummary(md) } : null;
      } catch {
        this.planByProject[slug] = null;
      }
    },
    // The open project's plan, or null. No trailer (`summary.issue` null) is a
    // plan still being written: not offered.
    openPlan() {
      const held = this.planByProject[this.openSlug];
      return held && held.summary.issue != null ? held : null;
    },
    // The plan for ONE card: only ever shown against the issue it names.
    planFor(number) {
      const held = this.openPlan();
      return held && held.summary.issue === number ? held : null;
    },
    // A plan left over from a closed issue is residue, and the pill says so.
    planIssueIsOpen() {
      const held = this.openPlan();
      if (!held) return true;
      const iss = this.projectIssues().find((i) => i.number === held.summary.issue);
      // Absent from the fold (filtered or cold): assume open.
      return !iss || iss.state !== "closed";
    },
    planPillLabel(number) {
      const held = this.planFor(number);
      return held ? window.WBRun.planPillLabel(held.summary, this.planIssueIsOpen()) : "";
    },
    planPillWarns(number) {
      const held = this.planFor(number);
      return !!held && window.WBRun.planPillWarns(held.summary, this.planIssueIsOpen());
    },
    // The head chip: for a plan whose issue is filtered out of the board or
    // absent from the fold, which would otherwise be invisible AND
    // undiscardable.
    planChipLabel() {
      const held = this.openPlan();
      if (!held) return "";
      return `#${held.summary.issue} · ${window.WBRun.planPillLabel(held.summary, this.planIssueIsOpen())}`;
    },
    openPlanModal() {
      const held = this.openPlan();
      if (!held) return;
      this.planModal = { open: true, issue: held.summary.issue };
      this.$nextTick(() => window.lucide?.createIcons());
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
      const slug = this.openSlug;
      try {
        const reply = await window.WBDaemon.write("plan.discard", { repo: slug });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "could not discard the plan"));
          return;
        }
        this._flashAction(`discarded the plan for #${held.summary.issue}`);
        this.closePlanModal();
      } catch {
        this._flashAction("discard unavailable: no daemon");
      } finally {
        // Re-read on EVERY path: the panel shows what is on disk now.
        await this.loadPlan(slug);
      }
    },

    // The open project's issues (#198). Empty until `loadBoard()` populates it.
    projectIssues() {
      return this.boardIssues[this.openSlug] || [];
    },

    // The whole-tracker board fold via `board.list`, cached under the slug. No
    // daemon or a transport error leaves the board empty.
    async loadBoard() {
      const slug = this.openSlug;
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
      this.loadPlan(this.openSlug);
      try {
        const reply = await Promise.race([
          window.WBDaemon.observe("board.list", { repo: slug }),
          new Promise((_, rej) =>
            setTimeout(() => rej(new Error("board fold timed out")), this.BOARD_FOLD_TIMEOUT_MS),
          ),
        ]);
        if (window.WBFail.isError(reply)) {
          // Drop any stale board: the error banner must not sit above data
          // that looks live.
          this.boardIssues[slug] = [];
          if (window.WBMode.isDaemon()) {
            const msg = window.WBFail.message(reply, "could not load board");
            this.boardError[slug] = msg;
            this._flashAction?.(msg);
          }
          return;
        }
        const board = reply.board || {};
        this.boardIssues[slug] = (board.issues || []).map((r) => this.boardRowToIssue(r));
        const colors = {};
        // Skip a blank color: a bare "#" is truthy and masks `labelColor`'s
        // fallback.
        for (const l of board.labels || []) {
          if (!l.color) continue;
          colors[l.name] = "#" + String(l.color).replace(/^#/, "");
        }
        this.boardLabels[slug] = colors;
        this.boardError[slug] = null;
        // Fold rows carry `body: ""`: re-merge the open drawer's detail or it
        // goes blank on every refresh.
        if (this.kanbanSel != null) this.loadIssueDetail(this.kanbanSel);
      } catch {
        // Transport error: distinct error state, stale board dropped.
        this.boardIssues[slug] = [];
        if (window.WBMode.isDaemon()) {
          this.boardError[slug] = "could not load board";
          this._flashAction?.("could not load board");
        }
        // Demo (static shell): leave it empty, no throw.
      } finally {
        this.boardRefreshing = false;
        // Exactly ONE follow-up for whatever was coalesced away, or for a
        // project that changed underneath this fold. `_boardPending` is cleared
        // by the recursive call before it awaits, so this settles.
        if (this._boardPending || this.openSlug !== slug) {
          this._boardPending = false;
          if (this.openSlug && this.kanbanOpen) this.loadBoard();
        }
      }
    },

    // The one door every refresh trigger goes through (#301): the predicate
    // (wb-kanban.js) decides.
    maybeRefreshBoard(trigger) {
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

    // A CLI fold row → the issue shape `wb-kanban.js` expects. Body + comments
    // are absent from the fold (`issue.show` fills them on open).
    boardRowToIssue(row) {
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
      const shown = all.filter((i) => K.matches(i, this.kanbanFilter) && K.hasLabelFilter(i, this.kanbanLabel));
      const bucket = { backlog: [], agent: [], human: [], closed: [] };
      for (const i of shown) bucket[K.columnOf(i)].push(i);
      return {
        backlog: K.sortBacklog(bucket.backlog, this.kanbanSort),
        // The Ready columns keep the SERVER's graph order (#198): the fold emits
        // them in `sort_queue_in_graph` order and bucketing preserves it, so
        // board order == core queue order. `K.orderGraph` (seed/demo only)
        // would diverge — it lacks the full open-set + `## Parent` context.
        agent: bucket.agent,
        human: bucket.human,
        closed: bucket.closed.sort((a, b) => (b.updated || "").localeCompare(a.updated || "")),
      };
    },
    // Per-column live count (post-filter), for the column header badge.
    kanbanCount(colId) {
      return this.kanbanColumns()[colId].length;
    },
    // The label set present in the project, for the filter dropdown.
    kanbanLabelOptions() {
      const seen = new Set();
      for (const i of this.projectIssues()) for (const l of i.labels || []) seen.add(l);
      return [...seen].sort();
    },

    // The run pill for a card (the actively-worked issue of a live run).
    issueRunning(number) {
      return window.WBKanban.runningFor(number, this.projectRuns());
    },

    // Thin delegations to the faithful helpers (used in the template).
    kanbanColumnOf(i) {
      return window.WBKanban.columnOf(i);
    },
    labelColor(l) {
      // The repo's real label hex, else the seed vocabulary.
      return this.boardLabels[this.openSlug]?.[l] || window.WBKanban.labelColor(l);
    },
    labelInk(l) {
      return window.WBKanban.labelInk(l);
    },
    labelShort(l) {
      return window.WBKanban.labelMeta(l).short;
    },
    closeLabel(i) {
      return window.WBKanban.closeLabel(i);
    },
    kanbanColumnTitle(i) {
      const id = window.WBKanban.columnOf(i);
      return (window.WBKanban.COLUMNS.find((c) => c.id === id) || {}).title || id;
    },
    kfmtDate(iso) {
      return window.WBKanban.fmtDate(iso);
    },

    // --- detail drawer ----------------------------------------------------
    // Selection is by number, so a label move (which can change the card's
    // column) keeps the drawer pointed at the same issue.
    selectedIssue() {
      if (this.kanbanSel == null) return null;
      return this.projectIssues().find((i) => i.number === this.kanbanSel) || null;
    },
    openIssue(number) {
      this.kanbanSel = number;
      this.$nextTick(() => window.lucide?.createIcons());
      // `issue.show` merges body + comments + blockers into the cached row.
      this.loadIssueDetail(number);
    },

    async loadIssueDetail(number) {
      const slug = this.openSlug;
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
        gen !== this._issueDetailGen || this.openSlug !== slug || this.kanbanSel !== number;
      const fail = (msg) => {
        if (stale() || !window.WBMode.isDaemon()) return;
        this.issueError = msg;
        this._flashAction?.(msg);
      };
      try {
        const reply = await window.WBDaemon.observe("issue.show", { repo: slug, number });
        if (window.WBFail.isError(reply)) {
          fail(window.WBFail.message(reply, "could not load issue detail"));
          return;
        }
        if (!reply || reply.status !== "ok" || !reply.issue || typeof reply.issue !== "object") {
          fail("could not load issue detail");
          return;
        }
        const detail = reply.issue;
        const iss = (this.boardIssues[slug] || []).find((i) => i.number === number);
        if (!iss || stale()) return;
        if (typeof detail.body === "string") iss.body = detail.body;
        if (Array.isArray(detail.comments)) iss.comments = detail.comments;
        if (Array.isArray(detail.blocked_by)) iss.blockedBy = detail.blocked_by;
        // Success owns the banner too: this one may land second.
        this.issueError = null;
      } catch {
        // Transport error: the drawer says so rather than reading as empty.
        fail("could not load issue detail");
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
    githubUrl(number) {
      const p = this.projects.find((x) => this.repoRef(x) === this.openSlug);
      return window.WBProject.issueUrl(p && p.remoteUrl, number);
    },

    // The selected issue's blockers, each with its live open/closed state.
    issueBlockers(iss) {
      if (!iss || !iss.blockedBy?.length) return [];
      const all = this.projectIssues();
      return iss.blockedBy.map((n) => {
        const b = all.find((x) => x.number === n);
        return { number: n, open: b ? b.state === "open" : false, known: !!b, title: b?.title || "" };
      });
    },

    // An issue body / comment as sanitized markdown.
    renderIssueMd(src) {
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
    hasLabel(iss, label) {
      return !!iss && (iss.labels || []).includes(label);
    },
    toggleLabel(iss, label) {
      if (!iss) return;
      // Defence in depth: a `:disabled` button is still reachable by keyboard
      // in some browsers.
      if (this.labelsLocked()) return;
      const has = this.hasLabel(iss, label);
      const op = has ? "remove" : "add";
      const prev = [...(iss.labels || [])];
      iss.labels = has ? iss.labels.filter((l) => l !== label) : [...(iss.labels || []), label];
      const slug = this.openSlug;
      WB.emit("issue-label-change", { project: slug, number: iss.number, label, op });
      // The run-lock-aware `label.set` Mutate (#199): refusal → revert + flash.
      (async () => {
        try {
          const reply = await window.WBDaemon.observe("label.set", {
            repo: slug,
            number: iss.number,
            label,
            op,
          });
          if (window.WBFail.isError(reply)) {
            iss.labels = prev;
            this._flashAction(window.WBFail.message(reply, "label change refused"));
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

    // --- settings modal ---------------------------------------------------
    // Data-driven (schema in wb-settings.js); the daemon persists via
    // `config.set`/`config.unset`.
    SETTINGS: window.WB_SETTINGS,
    TRISTATE: window.WB_TRISTATE,
    settingsOpen: false,
    // The daemon (machine-wide) group first; per-project sections follow.
    settingsSection: "daemon",
    settings: window.wbSettingsDefaults(),

    // Keys held in this browser profile's view store (`scope: "client"`).
    CLIENT_KEYS: window.wbClientKeys(),

    openSettings() {
      this.settingsOpen = true;
      this.avatarMenu = false;
      // Client-scoped keys come from the view store; `config.get` has none.
      const view = window.WBView.read() || {};
      this.settings["consoles.relaunch_on_load"] = view.relaunch === true;
      this.settings["consoles.key_bar"] = view.keys ?? "unset";
      this.settings["consoles.startup_command"] = view.command ?? "";
      // The open repo's resolved config (`config.get`), merged over the schema
      // defaults; with no repo open the project groups are disabled.
      if (this.openSlug) {
        WBDaemon.observe("config.get", { repo: this.openSlug })
          .then((reply) => {
            const cfg = reply && reply.status === "ok" ? reply.config : null;
            if (cfg && typeof cfg === "object") {
              for (const k in cfg) {
                // Never round-trip the MASKED secret: a save would persist the mask.
                if (k === "events.token") continue;
                if (cfg[k] !== null && k in this.settings) this.settings[k] = cfg[k];
              }
            }
          })
          .catch(() => {});
      }
      this.$nextTick(() => window.lucide?.createIcons());
    },
    closeSettings() {
      this.settingsOpen = false;
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
        project: this.openSlug,
        loading: this.spend.loading,
        error: this.spend.error,
        // A document for a project no longer open is stale by definition.
        doc: this.spend.slug === this.openSlug ? this.spend.doc : null,
        period: this.spendPeriod,
        // Titles ride whatever the board ALREADY holds; never a load
        // (`loadBoard` spawns a throttled tracker CLI).
        issues: this.boardIssues[this.openSlug] || [],
      });
    },
    // The window is a server-side filter: assign, then re-read.
    setSpendPeriod(key) {
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
      const slug = this.openSlug;
      // No project open is not a failure: the pane says so itself.
      if (!slug) {
        this.spend = { loading: false, error: "", doc: null, slug: null };
        return;
      }
      this.spend = { ...this.spend, loading: true, error: "" };
      let doc = null;
      let error = "";
      try {
        const r = await fetch(
          "/api/spend?project=" +
            encodeURIComponent(slug) +
            "&period=" +
            encodeURIComponent(this.spendPeriod || "all"),
        );
        if (r.ok) doc = await r.json();
        else error = "could not load spend from the daemon";
      } catch {
        error = "could not load spend from the daemon";
      }
      // The project changed while in flight: one cost under another's name.
      if (this.openSlug !== slug) return;
      this.spend = { loading: false, error, doc, slug };
      this.$nextTick(() => window.lucide?.createIcons());
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
    ledgerView() {
      // Rows for a project no longer open are stale (as `spendView()`); the
      // peer banner is gated with them.
      const fresh = this.ledger.slug === this.openSlug;
      return window.WBSpend.ledger({
        project: this.openSlug,
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
    setSpendPane(key) {
      this.spendPane = key;
      if (key === "ledger") this.loadLedger();
      this.$nextTick(() => window.lucide?.createIcons());
    },
    // The Overview's unpriced figure drills into the offending rows (#355).
    showUnpricedLedger() {
      this.spendLedgerUnpricedOnly = true;
      this.setSpendPane("ledger");
    },
    async loadLedger() {
      const slug = this.openSlug;
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
      let daemonId = null;
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
        } else error = "could not load the ledger from the daemon";
      } catch {
        error = "could not load the ledger from the daemon";
      }
      // The operator switched projects while this was in flight.
      if ((this.openSlug || "") !== want) {
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
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // --- about (read-only) ------------------------------------------------
    // The product card from `/api/about`; the seed stands in on `file://`.
    aboutOpen: false,
    about: {
      name: "ralphy",
      version: "",
      license: "GPL-3.0-or-later",
      repository: "https://github.com/paulocorcino/ralphy",
      creator: "Paulo Corcino",
      error: "",
    },
    // The release view the daemon computed (ADR-0056 §7), seeded empty.
    release: (window.WBRelease && window.WBRelease.EMPTY) || {
      current: "",
      channel: "rc",
      standing: "unknown",
      severity: "none",
      latest: null,
      gap: [],
      disabled: false,
    },
    // Dismissed by opening the panel — except urgent news.
    releaseSeen: false,
    whatsNewOpen: false,

    get releaseHasNews() {
      return !!window.WBRelease && window.WBRelease.hasNews(this.release);
    },
    // What the rail draws: an urgent release ignores the dismissal.
    get releaseUnread() {
      if (!this.releaseHasNews) return false;
      return !this.releaseSeen || window.WBRelease.isSticky(this.release);
    },
    get releaseSummary() {
      return window.WBRelease ? window.WBRelease.gapSummary(this.release) : "";
    },

    async loadRelease() {
      if (!window.WBRelease) return;
      this.release = await window.WBRelease.read();
    },
    openWhatsNew() {
      this.avatarMenu = false;
      this.whatsNewOpen = true;
      this.releaseSeen = true;
      this.$nextTick(() => window.lucide?.createIcons());
    },
    closeWhatsNew() {
      this.whatsNewOpen = false;
    },
    async setReleaseWatch(enable) {
      if (!window.WBRelease) return;
      try {
        await window.WBRelease.setWatch(enable);
        this.release = { ...this.release, disabled: !enable };
      } catch (e) {
        // A preference: the next read reports what actually took.
        console.warn("release watch:", e);
      }
    },

    async openAbout() {
      this.avatarMenu = false;
      this.aboutOpen = true;
      this.about.error = "";
      try {
        const r = await fetch("/api/about");
        if (r.ok) {
          const data = await r.json();
          // Merge onto the seed so any missing field keeps its fallback.
          this.about = { ...this.about, ...data, error: "" };
        } else if (window.WBMode.isDaemon()) {
          this.about.error = "could not load about info from the daemon";
        }
      } catch {
        // No daemon reachable (static demo): keep the seed, no error noise.
        if (window.WBMode.isDaemon()) {
          this.about.error = "could not load about info from the daemon";
        }
      }
      this.$nextTick(() => window.lucide?.createIcons());
    },
    closeAbout() {
      this.aboutOpen = false;
    },
    // The current year for the copyright line (client clock is fine here).
    aboutYear() {
      return new Date().getFullYear();
    },

    async saveSetting(key, value) {
      this.settings[key] = value;
      // A client-scoped key is this browser's preference: view store, never
      // `config.set`.
      if (this.CLIENT_KEYS.has(key)) {
        if (key === "consoles.relaunch_on_load") window.WBView.patch({ relaunch: value === true });
        // "unset" is the ABSENCE of a preference: written as null.
        if (key === "consoles.key_bar")
          window.WBView.patch({ keys: value === "on" || value === "off" ? value : null });
        // Blank is the ABSENCE of a startup command: the menu row goes away.
        if (key === "consoles.startup_command") {
          const command = typeof value === "string" ? value.trim() : "";
          window.WBView.patch({ command: command || null });
          this.consoleCommand = command || null;
        }
        WB.emit("setting-change", { project: null, key, value });
        return;
      }
      // The run-lock-aware config Mutates; an empty/"unset" value clears the
      // key. `observe` (not `spawn`) so a run-lock refusal surfaces (#207).
      if (this.openSlug) {
        const empty = value === "" || value === "unset" || value == null;
        try {
          const reply = await window.WBDaemon.observe(empty ? "config.unset" : "config.set", {
            repo: this.openSlug,
            key,
            value: String(value),
          });
          if (window.WBFail.isError(reply)) {
            this._flashAction(window.WBFail.message(reply, "config change refused"));
          }
        } catch {
          // No daemon reachable — leave the optimistic setting in place.
        }
      }
      WB.emit("setting-change", { project: this.openSlug, key, value });
    },

    // --- account menu + security -----------------------------------------
    // The daemon auth model (ADR-0032): an opt-in access token, an optional
    // password, and TOTP whose secret is shown exactly once.
    avatarMenu: false,
    securityOpen: false,
    security: {
      tokenSet: true, // a networked daemon always has one; localhost needs none
      passwordSet: false,
      passwordDraft: "",
      passwordConfirm: "",
      totpEnrolled: false,
      // Set only in the moment after enrolling; the daemon shows it once.
      secret: "",
      otpauthUri: "",
      qrHtml: "",
      pendingEnroll: false, // QR shown, awaiting the confirm code (ADR-0032 §C)
      confirmCode: "",
      totpError: "",
      requireLogin: false, // opt-in: mimics a non-loopback bind with TOTP
      policy: "session", // overwritten by probeSession(); demo default keeps login interactive
      // The enrolled password, typed once to change or remove it (step-up,
      // ADR-0032 amendment E). Never kept after the request.
      passwordCurrent: "",
      // The last step-up refusal, shown under the card that asked.
      stepUpError: "",
    },
    // The stored password, kept in-memory purely so the demo login can check it.
    _passwordValue: "",
    // The step-up prompt (ADR-0032 amendment E): lowering the auth posture —
    // rotating the token, lifting the login gate, revoking TOTP — costs a
    // fresh authenticator code once a seed is armed. One prompt serves all
    // three; `label` says which, `_stepUpResolve` hands the code back to the
    // action that asked.
    stepUp: { open: false, code: "", label: "" },
    _stepUpResolve: null,

    async openSecurity() {
      this.securityOpen = true;
      this.avatarMenu = false;
      // The REAL daemon auth state (GET /api/security/state).
      try {
        const r = await fetch("/api/security/state");
        if (r.ok) {
          const s = await r.json();
          this.security.tokenSet = s.token_set;
          this.security.passwordSet = s.password_set;
          this.security.totpEnrolled = s.totp_enrolled;
          this.security.requireLogin = s.require_login;
        }
      } catch {}
      this.$nextTick(() => window.lucide?.createIcons());
    },
    closeSecurity() {
      this.securityOpen = false;
      // Drop the one-time secret when leaving.
      this.security.secret = "";
      this.security.otpauthUri = "";
      this.security.qrHtml = "";
      // The pending seed survives server-side (mint-once).
      this.security.pendingEnroll = false;
      this.security.confirmCode = "";
      this.security.totpError = "";
      this.security.passwordCurrent = "";
      this.security.stepUpError = "";
      this.cancelStepUp();
    },

    // Whether the daemon will demand a fresh code for a posture downgrade: a
    // live TOTP seed is armed. A pending (unconfirmed) enrolment never counts.
    stepUpNeeded() {
      return this.security.totpEnrolled === true;
    },
    // Ask the operator for the current 6-digit code before `label`. Resolves
    // to the code, to `""` when no seed is armed (nothing to ask), or to
    // `null` when they cancel.
    askFreshCode(label) {
      if (!this.stepUpNeeded()) return Promise.resolve("");
      this.cancelStepUp();
      this.security.stepUpError = "";
      this.stepUp = { open: true, code: "", label };
      this.$nextTick?.(() => document.querySelector(".step-up input")?.focus());
      return new Promise((resolve) => {
        this._stepUpResolve = resolve;
      });
    },
    submitStepUp() {
      const code = this.stepUp.code.trim();
      if (code.length !== 6) return;
      const resolve = this._stepUpResolve;
      this._stepUpResolve = null;
      this.stepUp = { open: false, code: "", label: "" };
      resolve?.(code);
    },
    cancelStepUp() {
      const resolve = this._stepUpResolve;
      this._stepUpResolve = null;
      this.stepUp = { open: false, code: "", label: "" };
      resolve?.(null);
    },
    // The form body for a step-up-guarded mutation: the base fields plus the
    // code, only when there is one to send (the daemon treats an absent code
    // as "nothing armed", and a stray empty field would read as a wrong code).
    stepUpBody(fields, code) {
      const p = new URLSearchParams(fields);
      if (code) p.set("code", code);
      return p.toString();
    },
    // Turn a step-up refusal into the line under the card. `Retry-After` is
    // the throttle (amendment §D) — the same brake the login has.
    noteStepUpRefusal(r) {
      if (r.status === 429) {
        const wait = r.headers?.get?.("Retry-After") || "a few";
        this.security.stepUpError = `Too many attempts — wait ${wait} s and try again.`;
      } else if (r.status === 401) {
        this.security.stepUpError = "Code rejected. Enter the current code from your authenticator app.";
      } else {
        this.security.stepUpError = `The daemon refused (${r.status}).`;
      }
    },

    async enrollTotp() {
      // A PENDING seed (mint-once); NOT armed until `confirmTotp()` proves
      // possession.
      try {
        const r = await fetch("/api/security/totp/enroll", { method: "POST" });
        if (!r.ok) return;
        const { uri } = await r.json();
        this.security.pendingEnroll = true;
        this.security.totpError = "";
        this.security.confirmCode = "";
        this.security.otpauthUri = uri;
        this.security.secret = (uri.split("secret=")[1] || "").split("&")[0];
        this.security.qrHtml = window.wbQr(uri);
      } catch {}
    },

    async confirmTotp() {
      // Verifies against the pending seed and arms it (ADR-0032 §C).
      const code = this.security.confirmCode.trim();
      if (code.length !== 6) return;
      try {
        const r = await fetch("/api/security/totp/confirm", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "code=" + encodeURIComponent(code),
        });
        const ok = r.ok && (await r.json()).confirmed;
        if (ok) {
          this.security.totpEnrolled = true;
          this.security.pendingEnroll = false;
          this.security.secret = "";
          this.security.otpauthUri = "";
          this.security.qrHtml = "";
          this.security.confirmCode = "";
          this.security.totpError = "";
        } else {
          this.security.totpError = "Wrong code. Enter the current code from your authenticator app.";
        }
      } catch {
        this.security.totpError = "Cannot reach the daemon. Try again.";
      }
    },

    async cancelEnroll() {
      // Abandon an in-flight enrolment: drop the pending seed server-side too.
      // A pending seed never gated anything, so no code is asked (the route
      // only demands one for a LIVE seed).
      try {
        await fetch("/api/security/totp/revoke", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "",
        });
      } catch {}
      this.security.pendingEnroll = false;
      this.security.secret = "";
      this.security.otpauthUri = "";
      this.security.qrHtml = "";
      this.security.confirmCode = "";
      this.security.totpError = "";
    },

    async revokeTotp() {
      // POST /api/security/totp/revoke deletes the live AND pending seeds. With
      // a live seed armed it costs a fresh code (step-up): on a gated loopback
      // bind this is the whole gate going away.
      const code = await this.askFreshCode("revoke two-factor");
      if (code === null) return;
      try {
        const r = await fetch("/api/security/totp/revoke", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.stepUpBody({}, code),
        });
        if (!r.ok) {
          this.noteStepUpRefusal(r);
          return;
        }
      } catch {
        return;
      }
      this.security.totpEnrolled = false;
      this.security.pendingEnroll = false;
      this.security.secret = "";
      this.security.otpauthUri = "";
      this.security.qrHtml = "";
      this.security.confirmCode = "";
      this.security.totpError = "";
      // revoking the seed removes the session factor → login can't be required
      this.security.requireLogin = false;
    },

    // The `/api/security/password` body: the new password (empty = remove)
    // and, once one is enrolled, the current one — the step-up the daemon
    // demands before it changes or removes the factor. `password` is ALWAYS
    // present: an absent field is a 400, never a clear.
    passwordBody(pw) {
      const p = new URLSearchParams({ password: pw });
      if (this.security.passwordSet) p.set("current", this.security.passwordCurrent);
      return p.toString();
    },
    async savePassword() {
      const pw = this.security.passwordDraft.trim();
      // Require a matching confirmation before the value ever leaves the field.
      if (!pw || this.security.passwordDraft !== this.security.passwordConfirm) return;
      if (this.security.passwordSet && !this.security.passwordCurrent) return;
      this.security.stepUpError = "";
      try {
        const r = await fetch("/api/security/password", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.passwordBody(pw),
        });
        if (!r.ok) {
          this.notePasswordRefusal(r);
          return;
        }
        this.security.passwordSet = (await r.json()).password_set;
      } catch {
        return;
      } finally {
        this.security.passwordCurrent = "";
      }
      this._passwordValue = pw; // demo login still checks locally
      this.security.passwordDraft = "";
      this.security.passwordConfirm = "";
    },
    async clearPassword() {
      if (this.security.passwordSet && !this.security.passwordCurrent) return;
      this.security.stepUpError = "";
      try {
        const r = await fetch("/api/security/password", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.passwordBody(""),
        });
        if (!r.ok) {
          this.notePasswordRefusal(r);
          return;
        }
      } catch {
        return;
      } finally {
        this.security.passwordCurrent = "";
      }
      this._passwordValue = "";
      this.security.passwordSet = false;
      this.security.passwordDraft = "";
      this.security.passwordConfirm = "";
    },
    notePasswordRefusal(r) {
      if (r.status === 401) {
        this.security.stepUpError = "Current password rejected.";
      } else {
        this.noteStepUpRefusal(r);
      }
    },
    async remintToken() {
      // Rotates the token AND bumps the session epoch (ADR-0032 amendment §B):
      // every cookie, this browser's included, is invalidated IMMEDIATELY.
      // Costs a fresh code once TOTP is armed (step-up): the session must not
      // be able to rotate the key it rides on.
      const code = await this.askFreshCode("rotate the access token");
      if (code === null) return;
      try {
        const r = await fetch("/api/security/token/remint", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.stepUpBody({}, code),
        });
        if (!r.ok) {
          this.noteStepUpRefusal(r);
          return;
        }
      } catch {
        return;
      }
      if (this.security.policy === "session") this.logOff();
    },

    async toggleRequireLogin(ev) {
      // Only meaningful once TOTP is enrolled; the server refuses (400) an
      // enable with no seed, the client guard just avoids the round-trip.
      const want = !this.security.requireLogin;
      if (want && !this.security.totpEnrolled) {
        this.security.requireLogin = false;
        if (ev?.target) ev.target.checked = false;
        return;
      }
      // Turning the gate OFF is the gate itself being lowered: it costs a
      // fresh code (step-up). Turning it on stays free.
      const code = want ? "" : await this.askFreshCode("turn login off");
      if (code === null) {
        if (ev?.target) ev.target.checked = this.security.requireLogin;
        return;
      }
      let ok = false;
      try {
        const r = await fetch("/api/security/require-login", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.stepUpBody({ enable: String(want) }, code),
        });
        ok = r.ok;
        if (!ok && !want) this.noteStepUpRefusal(r);
      } catch {
        ok = false;
      }
      if (ok) this.security.requireLogin = want;
      // `:checked` won't re-sync when the bound value did not change.
      if (ev?.target) ev.target.checked = this.security.requireLogin;
      if (!ok) return;
      if (want) {
        // The gate applies to THIS bind, loopback included (ADR-0032 §A); the
        // daemon invalidated sessions, so drop to the login screen.
        this.security.policy = "session";
        this.closeSecurity();
        this.logOff();
      } else {
        // Gate lifted — re-sync authed/policy from the server.
        await this.probeSession();
      }
    },

    // --- login gate -------------------------------------------------------
    // An opaque overlay covers the shell while locked (`body.locked`).
    authed: true,
    // `remember` is "keep me signed in" (ADR-0032 amendment 2026-09-16):
    // opt-in, reset on every log-off.
    login: { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: false },

    async logOff() {
      this.avatarMenu = false;
      this.securityOpen = false;
      this.settingsOpen = false;
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
      WB.emit("logoff", {});
      this.$nextTick(() => window.lucide?.createIcons());
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
      window.WBConsole?.afterLogin()?.then(() => this.adoptDeskCheckouts());
      // Only now is `file.read` allowed (#339).
      this.restoreView();
    },

    // --- TOTP digit boxes -------------------------------------------------
    // One input per digit; a paste or OTP autofill landing all 6 in the first
    // box is spread. `login.code` is the joined string.

    _otpBoxes(el) {
      return el.closest(".login-otp").querySelectorAll("input");
    },

    _otpSync() {
      this.login.code = this.login.digits.join("");
    },

    // Once the 6th digit lands: the password field if required, else submit.
    _otpAdvancePastLast(el) {
      if (this.security.passwordSet || this.login.passwordRequired) {
        this.$refs.loginPassword?.focus();
      } else {
        el.closest("form")?.querySelector(".login-btn")?.focus();
      }
    },

    _otpFill(text, el) {
      const chars = text.replace(/\D/g, "").slice(0, 6).split("");
      for (let j = 0; j < 6; j++) this.login.digits[j] = chars[j] || "";
      this._otpSync();
      const boxes = this._otpBoxes(el);
      if (chars.length >= 6) this._otpAdvancePastLast(el);
      else boxes[chars.length].focus();
    },

    otpInput(i, e) {
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

    otpKeydown(i, e) {
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

    otpPaste(e) {
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
      const code = (this.login.code || "").trim();
      try {
        const res = await fetch("/api/login", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: this.loginBody(),
        });
        if (res.ok) {
          this.login.error = "";
          this.authed = true;
          this.rehydrateAfterAuth();
          WB.emit("login", {});
          this.$nextTick(() => window.lucide?.createIcons());
        } else {
          this.login.error = "Invalid code or password.";
        }
        return;
      } catch {
        // Daemon mode: a thrown fetch must NOT authenticate via the local
        // fallback, which exists only for the `file://` demo.
        if (!window.WBMode.isDemo()) {
          this.login.error = "Cannot reach the daemon. Try again.";
          return;
        }
        // Demo (file:// standalone) — fall back to the local seed check.
      }
      if (!/^[0-9]{6}$/.test(code)) {
        this.login.error = "Invalid code or password.";
        return;
      }
      if (this.security.passwordSet && this.login.password !== this._passwordValue) {
        this.login.error = "Invalid code or password.";
        return;
      }
      this.login.error = "";
      this.authed = true;
      this.rehydrateAfterAuth();
      WB.emit("login", {});
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // --- canvas tabs ------------------------------------------------------
    // The adapter roster comes from `/api/agents`, never a list here:
    // onboarding a vendor must not need a frontend change (#304).
    agents: [],
    roster: [],
    agentMenu: false,
    // The Go-to picker (#337) and the fence picker (#343): SNAPSHOTS taken
    // when the menu opens, because the windows and fences live in the DOM.
    windowMenu: false,
    windowList: [],
    fenceMenu: false,
    fenceItems: [],
    consoleCount: 0,
    // The stage extent, for the footer pill (#338).
    stageW: 0,
    stageH: 0,
    // The confirm dialog (replaces window.confirm); `askConfirm` opens it.
    confirmModal: {
      open: false,
      title: "",
      message: "",
      confirmLabel: "Confirm",
      cancelLabel: "Cancel",
      danger: false,
    },
    _confirmResolve: null,
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
    _promptResolve: null,
    // The move destination picker (#364): browses one level at a time through
    // `tree.list`. `from` is the FULL rel path; `dir` the browsed directory
    // ("" is the repo root).
    movePick: { open: false, from: "", isFolder: false, dir: "", entries: [], busy: false, error: "" },
    // The SAME terminal glyph as the New-console button and its menu rows.
    tabs: [{ id: "consoles", kind: "consoles", title: "Consoles", icon: "bi bi-terminal", closable: false }],
    active: "consoles",

    // The seed list is DEMO-ONLY (`assets/ui-demo/wb-seed-projects.js`,
    // outside the embedded tree); `loadRepos()` fills this at init.
    projects: window.WB_SEED_PROJECTS || [],

    // --- accordion --------------------------------------------------------
    toggle(ref, row) {
      this.openSlug = this.openSlug === ref ? null : ref;
      // Refusal notes name an act against the project that WAS open.
      this.changesError = "";
      this.branchError = "";
      // NOT awaited: the accordion must not sit behind a cold WSL boot.
      if (this.openSlug === ref) this.wakePeerFor(ref);
      this.loadAgents(this.openSlug);
      // The chip's `<branch> · <name>` needs the listing (#406).
      if (this.openSlug === ref) this.ensureWorktreeListing(ref);
      this.refreshSpend();
      // Everything scoped to the project that WAS open is dropped: the drawer
      // selection, the trail marker, an unsent commit message (#318) and a verb
      // refusal (#331, whose terminal frame can land long after the click).
      this.kanbanSel = null;
      this.trailFocus = null;
      if (this.commitMsgSlug !== this.openSlug) {
        this.commitMsg = "";
        this.commitMsgSlug = this.openSlug;
      }
      this.verbError = "";
      this.$nextTick(() => {
        this.destroyTree();
        if (this.openSlug) this.mountTree();
        // The runs (#300) and changes-nudge (#310) sockets follow the tree's
        // open/close path.
        this.destroyRunsSub();
        this.mountRunsSub();
        this.destroyChangesSub();
        this.mountChangesSub();
        // Only when the board is OPEN (#301): the fold spawns a tracker CLI.
        if (this.openSlug && this.kanbanOpen) this.loadBoard();
        this.currentRunId = this.projectRuns()[0]?.runid || null;
        this.planSection = this.planHeadings(this.currentRun())[0] || "";
        if (this.openSlug) this.hydrateRuns();
        if (this.openSlug) this.loadChanges(this.openSlug);
        if (this.openSlug) this.loadSync(this.openSlug);
        window.lucide?.createIcons();
      });
    },

    // The status dot: live → green, idle → grey, offline → red (unreachable
    // path), waiting → yellow (an agent is asking for you, ADR-0059).
    // Orthogonal to `remote`.
    dotClass(state) {
      return state === "live"
        ? "live"
        : state === "waiting"
          ? "waiting"
          : state === "offline"
            ? "offline"
            : "";
    },

    // Wunderbaum copies source keys it does not define into `node.data`, so
    // `folder:true` lands at `node.data.folder` and `node.folder` is forever
    // `undefined`; `node.children` is `null` on a lazy or empty folder. Either
    // alone made EVERY collapsed folder answer "file".
    isFolder(node) {
      if (!node) return false;
      return !!(node.data?.folder || node.lazy || Array.isArray(node.children));
    },

    // --- file-type icons (Devicon font; folders use Wunderbaum defaults) ---
    fileIcon(title) {
      const name = title.toLowerCase();
      if (name.endsWith("lock") || name === "package-lock.json") return "devicon-json-plain colored";
      const ext = name.includes(".") ? name.split(".").pop() : "";
      const map = {
        ts: "devicon-typescript-plain colored",
        tsx: "devicon-typescript-plain colored",
        js: "devicon-javascript-plain colored",
        mjs: "devicon-javascript-plain colored",
        cjs: "devicon-javascript-plain colored",
        json: "devicon-json-plain colored",
        md: "devicon-markdown-plain md-glyph",
        rs: "devicon-rust-plain rs-glyph",
        css: "devicon-css3-plain colored",
        html: "devicon-html5-plain colored",
        prisma: "devicon-prisma-plain colored",
        png: "bi bi-image",
        jpg: "bi bi-image",
        jpeg: "bi bi-image",
        gif: "bi bi-image",
        svg: "bi bi-image",
        toml: "bi bi-gear",
        yml: "bi bi-gear",
        yaml: "bi bi-gear",
      };
      return map[ext] || "bi bi-file-earmark";
    },

    // An `icon` on every *file* node, recursively, without mutating the source.
    withIcons(nodes) {
      return nodes.map((n) => {
        if (n.folder || n.children) {
          return { ...n, children: this.withIcons(n.children || []) };
        }
        return { ...n, icon: this.fileIcon(n.title) };
      });
    },

    // --- Wunderbaum mount / teardown --------------------------------------
    mountTree() {
      const host = document.querySelector(".project.open .wb-host");
      const project = this.projects.find((p) => this.repoRef(p) === this.openSlug);
      if (!host || !project) return;
      this.treeMem();
      // Freshness is per-open: the watch that kept a level honest died with
      // the last close.
      this._treeValidated.clear();
      // The spinner is armed only when the root has to come off the daemon; a
      // re-open paints from memory in the same frame.
      this.treeError = "";
      this.treeStale = "";
      // The checkout this tree is built for (#406): cache key, every level
      // read and the watch carry it.
      this._treeCheckout = this.checkoutOf(this.openSlug);
      // A mount generation: a root read failing AFTER this tree was replaced
      // must not paint its error onto the fresh mount.
      const gen = (this._treeGen = (this._treeGen || 0) + 1);
      this.treeLoading = this.useDaemonTree() && !this._treeCache.has(this.treeKey(""));

      this._tree = new mar10.Wunderbaum({
        element: host,
        header: false,
        // The FILES search narrows the tree through this (`applyFileSearch`).
        // `autoApply` lets `updateFilter()` re-run after a level (re)loads;
        // without it a reloaded level vanishes.
        filter: { autoApply: true, mode: "hide" },
        // Daemon: the root level from `tree.list`, folders `lazy`. `file://`:
        // the static tree.
        source: this.useDaemonTree()
          ? this.loadTreeLevel("").catch(() => {
              // Never the static seed on a failed root read: a plausible tree
              // that is not this repo's. Say so and render nothing.
              if (gen === this._treeGen) this.treeError = "could not read this project's files";
              return [];
            })
          : this.withIcons(project.tree),
        lazyLoad: (e) =>
          this.loadTreeLevel(this.relPath(e.node)).catch((err) => {
            // Rethrown: Wunderbaum must mark the node failed, not empty.
            this.treeWentStale(err);
            throw err;
          }),
        // A level (re)loaded while a search is on carries no match marks, and
        // in `hide` mode an unmarked row is not painted.
        load: (e) => {
          if (e.tree.isFilterActive?.()) e.tree.updateFilter();
        },
        // The content-search badge: hit count beside the title; nothing once
        // the filter is gone (the map is empty by then).
        render: (e) => {
          const count = this._fileHits?.get(this.relPath(e.node));
          const old = e.nodeElem.querySelector(".wb-hits");
          if (typeof count !== "number") {
            old?.remove();
            return;
          }
          const badge = old || document.createElement("span");
          badge.className = "wb-hits";
          badge.textContent = String(count);
          if (!old) e.nodeElem.querySelector(".wb-title")?.after(badge);
        },
        // The root level has settled: put the expanded folders back.
        init: (e) => {
          if (gen !== this._treeGen) return;
          this.treeLoading = false;
          if (e.error) this.treeError = "could not read this project's files";
          else this.restoreExpansion();
        },
        edit: {
          trigger: ["F2", "macEnter"],
          apply: (e) => {
            // The shared listener takes full rel paths.
            const parent = parentRel(this.relPath(e.node));
            this.emit("rename", e.node, {
              from: parent ? `${parent}/${e.oldValue}` : e.oldValue,
              to: parent ? `${parent}/${e.newValue}` : e.newValue,
            });
            return true; // let the tree reflect it optimistically
          },
        },
        // Live watch-set (#196): the daemon watches only the expanded set.
        expand: (e) => {
          if (!this.isFolder(e.node)) return;
          const rel = this.relPath(e.node);
          if (e.flag) this._treeSub?.watch(rel);
          else this._treeSub?.unwatch(rel);
          this.rememberExpansion();
        },
        // Double-click / Enter on a leaf = "open this file".
        dblclick: (e) => {
          if (!this.isFolder(e.node)) this.openFile(e.node);
          return false;
        },
      });

      // One `/ws/tree` subscription per open project; the root is always
      // watched. A `tree.dirty` push refetches only the affected subtree.
      if (this.useDaemonTree() && window.WBDaemon?.subscribeTree) {
        this._treeSub = WBDaemon.subscribeTree(
          this.openSlug,
          (rel) => this.onTreeDirty(rel),
          this._treeCheckout,
        );
        this._treeSub.watch("");
      }

      // Right-click → our own context menu. Empty space below the rows is the
      // repo root: the only gesture that can create a top-level entry.
      host.addEventListener("contextmenu", (ev) => {
        const node = mar10.Wunderbaum.getNode(ev);
        ev.preventDefault();
        node?.setActive();
        this.showMenu(ev.clientX, ev.clientY, node || null);
      });
    },

    // Snapshot which folders are open, on every expand/collapse rather than
    // at close: a sidebar refresh or a peer going away is not a close we see.
    rememberExpansion() {
      // A restore expands nodes itself; recording a half-restored tree would
      // truncate the list being replayed.
      if (this._restoringExpansion || !this._tree || !this.openSlug) return;
      this.treeMem();
      const rels = [];
      this.rawTree().root.visit((n) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      this._treeExpanded.set(this.openSlug, rels);
    },

    // Re-expand the folders this project was left with, shallow-first (a
    // child cannot be found before its parent loads). Each level comes from
    // `_treeCache`; every expand still re-registers the daemon watch.
    async restoreExpansion() {
      const slug = this.openSlug;
      this.treeMem();
      const rels = this._treeExpanded.get(slug) || [];
      if (!rels.length || !this._tree) return;
      this._restoringExpansion = true;
      try {
        for (const rel of [...rels].sort((a, b) => a.split("/").length - b.split("/").length)) {
          // A project switch mid-replay: this list no longer describes the tree.
          if (slug !== this.openSlug || !this._tree) return;
          const node = this.rawTree().findFirst((n) => this.relPath(n) === rel);
          if (node && !node.expanded) await node.setExpanded(true);
        }
      } finally {
        this._restoringExpansion = false;
      }
    },

    // A daemon backs the tree only off `file://`.
    useDaemonTree() {
      return window.WBMode.isDaemon() && !!window.WBDaemon?.observe;
    },

    // One directory level from `tree.list`, folders lazy. Cache-FIRST
    // (stale-while-revalidate): a level read once is painted from memory and
    // re-read in the BACKGROUND, touching the DOM only when the directory
    // changed. Nothing is cached on the daemon (ADR-0036); the revalidation
    // is not optional, since the watch is dropped when the project closes.
    loadTreeLevel(rel) {
      this.treeMem();
      const key = this.treeKey(rel);
      const hit = this._treeCache.get(key);
      if (!hit) return this.fetchTreeLevel(rel);
      // Revalidate once per level per open, deferred so the paint happens first.
      if (!this._treeValidated.has(key)) {
        this._treeValidated.add(key);
        setTimeout(() => this.revalidateLevel(rel), 0);
      }
      return Promise.resolve(this.treeNodes(hit));
    },

    // Lazily create the three tree-memory collections (see the note there).
    treeMem() {
      this._treeCache ||= new Map();
      this._treeValidated ||= new Set();
      this._treeExpanded ||= new Map();
      // path → count (content) or `true` (name): what the filter predicate reads.
      this._fileHits ||= new Map();
    },

    // The un-cached read, always fresh: a reconcile served from the cache it
    // corrects would validate itself. INVARIANT: a read that FAILED throws; it
    // does NOT resolve `[]`, which is the real statement "no entries" every
    // caller acts on by replacing what is on screen.
    fetchTreeLevel(rel) {
      this.treeMem();
      const key = this.treeKey(rel);
      const payload = WBDaemon.withCheckout(
        { repo: this.openSlug, path: rel },
        this.checkoutOf(this.openSlug),
      );
      return WBDaemon.observe("tree.list", payload).then((reply) => {
        if (!reply || reply.status !== "ok" || !Array.isArray(reply.entries)) {
          throw new Error(window.WBFail.message(reply, "read failed"));
        }
        this.treeFresh();
        this.pruneTreeCache(rel, reply.entries);
        this._treeCache.set(key, reply.entries);
        this._treeValidated.add(key);
        return this.treeNodes(reply.entries);
      });
    },

    // Evict every remembered level the fresh listing of `rel` CONTRADICTS: a
    // cache key is a path, not an identity, so a renamed-then-recreated
    // directory would inherit the old one's children. A name no longer among
    // the subdirectories cannot have children, so its remembered subtree goes.
    pruneTreeCache(rel, entries) {
      this.treeMem();
      const dirs = new Set(entries.filter((en) => en.dir).map((en) => en.name));
      // The keys DESCENDING from `rel`; its own key (empty `child`) is what
      // this listing replaces.
      const prefix = this.treeKey(rel === "" ? "" : `${rel}/`);
      for (const key of [...this._treeCache.keys()]) {
        if (!key.startsWith(prefix)) continue;
        const child = key.slice(prefix.length).split("/")[0];
        if (!child || dirs.has(child)) continue;
        this._treeCache.delete(key);
        this._treeValidated.delete(key);
      }
    },

    // Cache key, scoped by REPO and by CHECKOUT (#406).
    treeKey(rel) {
      return `${this.openSlug}\n${this.checkoutOf(this.openSlug) || ""}\n${rel}`;
    },

    // Daemon entries → fresh Wunderbaum node specs, rebuilt on every call: the
    // tree OWNS and mutates the objects it is given.
    treeNodes(entries) {
      return entries.map((en) =>
        en.dir
          ? { title: en.name, folder: true, lazy: true }
          : { title: en.name, icon: this.fileIcon(en.name) },
      );
    },

    // Re-read a level painted from cache and reconcile ONLY if the directory
    // changed: the common case costs one read and zero DOM work.
    revalidateLevel(rel) {
      if (!this._tree || !this.useDaemonTree()) return Promise.resolve();
      this.treeMem();
      const key = this.treeKey(rel);
      const before = JSON.stringify(this._treeCache.get(key) ?? null);
      const slug = this.openSlug;
      return this.fetchTreeLevel(rel)
        .then(() => {
          // A project switch in flight: another project's tree.
          if (slug !== this.openSlug || !this._tree) return;
          if (JSON.stringify(this._treeCache.get(key) ?? null) === before) return;
          const node = rel === "" ? this.rawTree().root : this.findFolderByRel(rel);
          if (node) return this.reconcileLevel(node, rel);
        })
        // A dropped read leaves the cached level on screen, but says so.
        .catch((err) => this.treeWentStale(err));
    },

    // The tree shows a listing it could not confirm: record the reason, leave
    // every row alone. Cleared by `treeFresh`.
    treeWentStale(err) {
      if (!this.useDaemonTree()) return;
      const reason = (err && err.message) || "read failed";
      this.treeStale = `Could not refresh the file list (${reason}). Showing the last known list.`;
    },

    // A read landed: whatever the tree is showing is confirmed again.
    treeFresh() {
      this.treeStale = "";
    },

    // --- the FILES search (ADR-0036 amendment 2026-09-15) -----------------
    // The daemon answers with the hits' rel paths, every hit's ancestors are
    // loaded, and Wunderbaum's filter hides every other row. The decisions
    // are `WBFileSearch`'s.
    toggleFileSearch() {
      if (this.fileSearch.open) this.closeFileSearch();
      else this.openFileSearch();
    },

    openFileSearch() {
      if (!this.openSlug) return;
      this.fileSearch.open = true;
      this.$nextTick(() => this.$refs.fileSearch?.focus?.());
    },

    // Escape, or the lupe again: field, query and filter all go.
    closeFileSearch() {
      this.fileSearch.open = false;
      this.fileSearch.query = "";
      this.fileSearch.seq++;
      return this.clearFileSearch();
    },

    // The clear button: the field stays open and focused.
    clearFileSearchQuery() {
      this.fileSearch.query = "";
      this.fileSearch.seq++;
      this.$refs.fileSearch?.focus?.();
      return this.clearFileSearch();
    },

    setFileSearchMode(mode) {
      if (this.fileSearch.mode === mode) return Promise.resolve();
      this.fileSearch.mode = mode;
      this.$refs.fileSearch?.focus?.();
      return this.fileSearchNow();
    },

    // A keystroke arms the debounce; a query under the floor clears instead.
    fileSearchTyped() {
      clearTimeout(this._fileSearchTimer);
      if (!window.WBFileSearch.worthSearching(this.fileSearch.query)) {
        this.fileSearch.seq++;
        this.clearFileSearch();
        return;
      }
      this._fileSearchTimer = setTimeout(() => this.fileSearchNow(), window.WBFileSearch.DEBOUNCE_MS);
    },

    // One Observe read, dated by `seq`: a reply that is not the newest, or
    // arrives after a project switch, is dropped.
    fileSearchNow() {
      clearTimeout(this._fileSearchTimer);
      const query = String(this.fileSearch.query ?? "").trim();
      if (!window.WBFileSearch.worthSearching(query)) {
        return this.clearFileSearch();
      }
      const seq = ++this.fileSearch.seq;
      if (!this.useDaemonTree()) {
        this.fileSearch.note = "search needs a daemon";
        return Promise.resolve();
      }
      const slug = this.openSlug;
      const verb = window.WBFileSearch.verbFor(this.fileSearch.mode);
      this.fileSearch.note = "searching…";
      // Find and grep walk the SELECTED tree (#406).
      const payload = window.WBDaemon.withCheckout({ repo: slug, query }, this.checkoutOf(slug));
      return window.WBDaemon.observe(verb, payload)
        .then((reply) => {
          if (seq !== this.fileSearch.seq || slug !== this.openSlug) return;
          if (window.WBFail.isError(reply) || !Array.isArray(reply?.hits)) {
            this.fileSearch.note = window.WBFail.message(reply, "search failed");
            return;
          }
          return this.applyFileSearch(reply.hits, !!reply.truncated, seq);
        })
        .catch((err) => {
          if (seq !== this.fileSearch.seq) return;
          this.fileSearch.note = `search failed (${(err && err.message) || "read failed"})`;
        });
    },

    // The Wunderbaum instance WITHOUT Alpine's Proxy. Every node reached
    // through `this._tree` is a Proxy while the tree's own handlers hold raw
    // objects, and the row painter compares by identity: a mixed paint leaves
    // rows at stale offsets (MEASURED 2026-09-15/16). INVARIANT: everything
    // that reads or mutates the tree goes through this; `this._tree` is only
    // assigned, null-checked and destroyed.
    rawTree() {
      const t = this._tree;
      return t && window.Alpine?.raw ? window.Alpine.raw(t) : t;
    },

    // The rels of every expanded folder — the tree's current shape.
    expandedRels() {
      const rels = [];
      this.rawTree()?.root?.visit((n) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      return rels;
    },

    // Narrow the tree to `hits`: snapshot the expansion once per search
    // session, load every ancestor level (shallow-first), then filter. The
    // expands run under `_restoringExpansion`: the remembered expansion must
    // not learn them.
    async applyFileSearch(hits, truncated, seq) {
      this.treeMem();
      this.fileSearch.hits = hits;
      this.fileSearch.truncated = truncated;
      this.fileSearch.note = window.WBFileSearch.note({ hits, truncated });
      const tree = this.rawTree();
      if (!tree) return;
      if (this.fileSearch.expandedBefore === null) this.fileSearch.expandedBefore = this.expandedRels();
      // ONE paint, at the end: every lazy level repaints on its own, and one
      // landing between the old marks cleared and the new ones set left a
      // blank tree that nothing repainted (MEASURED 2026-09-15).
      this._restoringExpansion = true;
      tree.enableUpdate(false);
      try {
        for (const dir of window.WBFileSearch.dirsToLoad(hits)) {
          // A newer search, or a torn-down tree, owns the screen now.
          if (seq !== this.fileSearch.seq || tree !== this.rawTree()) return;
          const f = tree.findFirst((n) => this.relPath(n) === dir);
          if (f && this.isFolder(f) && !f.expanded) await f.setExpanded(true);
        }
        if (seq !== this.fileSearch.seq || tree !== this.rawTree()) return;
        this._fileHits = window.WBFileSearch.hitMap(hits);
        // `autoExpand: false`: the extension would also open every MATCHED
        // folder, a burst of lazy loads.
        tree.filterNodes((n) => this._fileHits.has(this.relPath(n)), {
          mode: "hide",
          autoExpand: false,
          matchBranch: false,
          noData: false,
        });
      } finally {
        this._restoringExpansion = false;
        // Set BEFORE the paint: the row window is computed from `scrollTop`,
        // and a leftover offset painted two rows at the bottom of a 52-row tree.
        if (tree === this.rawTree()) {
          tree.element.scrollTop = 0;
          // Re-enabling paints immediately and in full.
          tree.enableUpdate(true);
        }
      }
    },

    // Take the filter off and fold back what the search opened, deepest first.
    async clearFileSearch() {
      this.treeMem();
      this._fileHits = new Map();
      this.fileSearch.hits = [];
      this.fileSearch.truncated = false;
      this.fileSearch.note = "";
      const before = this.fileSearch.expandedBefore;
      this.fileSearch.expandedBefore = null;
      const tree = this.rawTree();
      if (!tree) return;
      // Same one-paint discipline as `applyFileSearch`.
      this._restoringExpansion = true;
      tree.enableUpdate(false);
      try {
        if (tree.isFilterActive?.()) tree.clearFilter();
        const fold = window.WBFileSearch.toCollapse(before, this.expandedRels());
        for (const rel of fold) {
          if (tree !== this.rawTree()) return;
          const f = tree.findFirst((n) => this.relPath(n) === rel);
          if (f && f.expanded) await f.setExpanded(false);
        }
      } finally {
        this._restoringExpansion = false;
        if (tree === this.rawTree()) {
          tree.element.scrollTop = 0;
          tree.enableUpdate(true);
        }
      }
    },

    // The search's memory of a tree that no longer exists (see `destroyTree`).
    resetFileSearch() {
      clearTimeout(this._fileSearchTimer);
      this.fileSearch.seq++;
      this.fileSearch.query = "";
      this.fileSearch.hits = [];
      this.fileSearch.truncated = false;
      this.fileSearch.note = "";
      this.fileSearch.expandedBefore = null;
      this._fileHits = new Map();
    },

    // `file.read`; on refusal surface the daemon's reason and close the tab,
    // returning `null`. `checkout` is the tab's PINNED checkout (#406).
    fetchContent(project, path, ftype, checkout) {
      if (!this.useDaemonTree()) return Promise.resolve(fakeContent(path, ftype));
      // An image is `file.image` (ADR-0049): a `data:` URL. Same refusal shape.
      if (ftype === "image") {
        return WBDaemon.readImage(project, path, (reason) => {
          WB.emit("open-refused", { project, path, reason });
          this._flashAction?.(reason);
          this.closeTab(fileTabId(project, path, checkout));
        }, checkout).catch(() => {
          WB.emit("open-refused", { project, path, reason: "transport" });
          this._flashAction?.("read failed");
          this.closeTab(fileTabId(project, path, checkout));
          return null;
        });
      }
      return WBDaemon.observe("file.read", WBDaemon.withCheckout({ repo: project, path }, checkout))
        .then((reply) => {
          if (!window.WBFail.isError(reply)) return reply.content;
          const reason = window.WBFail.message(reply, "refused");
          WB.emit("open-refused", { project, path, reason });
          this._flashAction?.(reason);
          this.closeTab(fileTabId(project, path, checkout));
          return null;
        })
        .catch(() => {
          // A transport drop must NOT fall back to `fakeContent`.
          WB.emit("open-refused", { project, path, reason: "transport" });
          this._flashAction?.("read failed");
          this.closeTab(fileTabId(project, path, checkout));
          return null;
        });
    },

    // A `tree.dirty` nudge for `rel`: refetch that level IF it is on screen. A
    // nudge for a collapsed/absent dir is DROPPED (ADR-0036 §4).
    onTreeDirty(rel) {
      const tree = this.rawTree();
      if (!tree) return;
      const node = rel === "" ? tree.root : this.findFolderByRel(rel);
      if (!node) return; // not in the tree → invisible, drop
      if (rel !== "" && !node.expanded) return; // collapsed → invisible, drop
      // Reconcile in place, then freshen the open tabs in this directory. A
      // reconcile failure leaves every row in place (`_reconcileOnce` resolves
      // the listing BEFORE touching the tree) and must still refresh viewers.
      return this.reconcileLevel(node, rel)
        .catch((err) => this.treeWentStale(err))
        .then(() => this.refreshOpenViewers(rel));
    },

    // Re-list one level WITHOUT duplicating nodes, preserving descendant
    // expansion and the active selection. `node.load` appends, so
    // `removeChildren()` first, then re-expand and re-activate by captured rel.
    async reconcileLevel(node, rel) {
      // Reentrancy guard: overlapping removeChildren()+load() passes double
      // the children. A pass in flight for `rel` re-runs once when it finishes.
      this._reconciling ||= new Set();
      this._reconcilePending ||= new Set();
      this._reconcileWaiters ||= new Map();
      // A COALESCED caller still gets a promise that settles when the level
      // does; otherwise a reveal awaiting it runs against children the pending
      // pass is about to tear down.
      if (this._reconciling.has(rel)) {
        this._reconcilePending.add(rel);
        return new Promise((resolve) => {
          if (!this._reconcileWaiters.has(rel)) this._reconcileWaiters.set(rel, []);
          this._reconcileWaiters.get(rel).push(resolve);
        });
      }
      this._reconciling.add(rel);
      // The drain is in an OUTER `finally`: `WBDaemon.observe` REJECTS on a
      // socket drop, and every coalesced awaiter would otherwise hang forever.
      try {
        try {
          await this._reconcileOnce(node, rel);
        } finally {
          this._reconciling.delete(rel);
        }
        if (this._reconcilePending.delete(rel)) {
          const again = rel === "" ? this.rawTree()?.root : this.findFolderByRel(rel);
          if (again) await this.reconcileLevel(again, rel);
        }
      } finally {
        // A pass that threw never consumed its pending flag.
        this._reconcilePending.delete(rel);
        // Drain by SPLICE: waiters arriving during the nested re-run land on
        // THIS list and are drained here once.
        const waiters = this._reconcileWaiters.get(rel);
        if (waiters?.length) for (const r of waiters.splice(0)) r();
      }
    },

    async _reconcileOnce(nodeAtCall, rel) {
      let node = nodeAtCall;
      // The selection restored is a SNAPSHOT; a reveal landing mid-pass must
      // not be undone by it. `_revealSeq` dates the snapshot.
      const seq = this._revealSeq || 0;

      // Resolve the fresh level BEFORE touching the tree: `node.load` given a
      // PROMISE appends (Wunderbaum quirk); a resolved ARRAY after
      // `removeChildren()` replaces cleanly. FRESH, never the cache.
      const source = await this.fetchTreeLevel(rel);
      // A reload of an ANCESTOR can land inside the fetch: its
      // `removeChildren()` unregisters this node (`node.tree` null) and a
      // teardown on the dead node throws inside Wunderbaum (MEASURED
      // 2026-09-16, Safari). Re-resolve by rel: none is a level that no longer
      // exists; one still loading is the ancestor's own re-expansion, and a
      // teardown under an in-flight load doubles the children.
      if (!node.tree) {
        const raw = this.rawTree();
        node = rel === "" ? raw?.root : raw?.findFirst((n) => this.relPath(n) === rel);
        if (!node || node.isLoading?.()) return;
      }
      // Read AFTER the fetch, right before the teardown: read before it, the
      // snapshot missed every folder a FILES search opened meanwhile.
      const expandedRels = [];
      node.visit((n) => {
        if (this.isFolder(n) && n.expanded) expandedRels.push(this.relPath(n));
      });
      const activeRel = this.relPath(this.rawTree()?.getActiveNode?.() || null) || null;
      node.removeChildren();
      await node.load(source);
      // `load` leaves the reloaded node collapsed, and the NEXT nudge would
      // hit the `!expanded` drop guard.
      if (rel !== "" && !node.expanded) await node.setExpanded(true);

      // Shallow-first. Match by rel path (NOT findFolderByRel): a freshly
      // reloaded folder has neither `folder` nor loaded `children` yet.
      expandedRels.sort((a, b) => a.split("/").length - b.split("/").length);
      for (const r of expandedRels) {
        const f = this.rawTree()?.findFirst((n) => this.relPath(n) === r);
        if (f && !f.expanded) await f.setExpanded(true);
      }
      const target = (this._revealSeq || 0) > seq ? this._revealedRel : activeRel;
      if (target) await this.revealRel(target, { restore: true });
      // A search is on: the reloaded level has no match marks yet.
      const raw = this.rawTree();
      if (raw?.isFilterActive?.()) raw.updateFilter();
    },

    // After a directory nudge, re-read any open tab whose file lives in `rel`
    // and push the bytes to its viewer. A failure keeps the tab's bytes.
    refreshOpenViewers(rel) {
      if (!this.useDaemonTree()) return Promise.resolve();
      const dirOf = (p) => {
        if (typeof p !== "string") return null;
        const i = p.lastIndexOf("/");
        return i < 0 ? "" : p.slice(0, i);
      };
      const reads = [];
      for (const t of this.tabs) {
        if (t.project !== this.openSlug || dirOf(t.path) !== rel) continue;
        // A tab pinned to another checkout (#406) is not this nudge's.
        if ((t.checkout ?? null) !== (this._treeCheckout ?? null)) continue;
        // An image tab re-reads through its OWN verb (ADR-0049 §1).
        const fresh =
          t.kind === "image"
            ? WBDaemon.readImage(t.project, t.path, undefined, t.checkout)
            : WBDaemon.observe(
                "file.read",
                WBDaemon.withCheckout({ repo: t.project, path: t.path }, t.checkout),
              ).then((reply) => (reply?.status === "ok" ? reply.content : null));
        reads.push(
          fresh
            .then((content) => {
              if (content != null) WBViewer.externalChange(t.id, content);
            })
            .catch(() => {}),
        );
      }
      // The settled batch, so a caller can await a fully-refreshed set.
      return Promise.all(reads);
    },

    // Bring `rel` on screen and select it: expand every ancestor
    // shallow-first, then activate. The one reveal primitive. Matches by rel
    // path, NOT findFolderByRel (a freshly loaded folder carries neither
    // `folder` nor children yet). `opts.restore` is the reconcile path's own
    // re-activation: it must NOT bump `_revealSeq`, or a stale pass would date
    // its restore as newer than the reveal it undoes.
    async revealRel(rel, opts = {}) {
      const tree = this.rawTree();
      if (!tree || typeof rel !== "string" || rel === "") return null;
      if (!opts.restore) {
        this._revealSeq = (this._revealSeq || 0) + 1;
        this._revealedRel = rel;
      }
      const parts = rel.split("/");
      // Ancestors only: a revealed FILE has nothing to expand.
      for (let i = 1; i < parts.length; i++) {
        const prefix = parts.slice(0, i).join("/");
        const f = tree.findFirst((n) => this.relPath(n) === prefix);
        if (!f) return null; // an unmounted ancestor: nothing to reveal
        if (!f.expanded) await f.setExpanded(true);
      }
      const node = tree.findFirst((n) => this.relPath(n) === rel);
      if (!node) return null;
      node.setActive();
      return node;
    },

    // The folder node whose rel path is `rel`, or `null` if none is mounted.
    findFolderByRel(rel) {
      return this.rawTree()?.findFirst((n) => this.isFolder(n) && this.relPath(n) === rel) || null;
    },

    // The open project's run-snapshot subscription (#300, ADR-0047 §9).
    mountRunsSub() {
      if (!window.WBMode.isDaemon() || !window.WBDaemon?.subscribeRuns || !this.openSlug) return;
      // A snapshot change means the tracker may have moved, so the same push
      // nudges the board (#301); the predicate coalesces it.
      this._runsSub = window.WBDaemon.subscribeRuns(this.openSlug, () => {
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

    // The run-completion subscription (#310, ADR-0036 amendment). The socket
    // carries EVERY repo's nudge, so the filter is here.
    mountChangesSub() {
      if (!window.WBMode.isDaemon() || !window.WBDaemon?.subscribeChanges || !this.openSlug) return;
      this._changesSub = window.WBDaemon.subscribeChanges(this.openSlug, (frame) => {
        // Optional-chained: a frame without wb-changes.js must not throw
        // inside `onmessage`.
        if (window.WBChanges?.shouldReload?.(frame, this.openSlug)) {
          this.loadChanges(this.openSlug);
          this.loadSync(this.openSlug);
        }
      });
    },
    destroyChangesSub() {
      try {
        this._changesSub?.close();
      } catch {}
      this._changesSub = null;
    },

    destroyTree() {
      try {
        this._treeSub?.close();
      } catch {}
      this._treeSub = null;
      this._treeCheckout = null;
      try {
        this._tree?.destroy?.();
      } catch {}
      this._tree = null;
      document.querySelectorAll(".wb-host").forEach((h) => (h.innerHTML = ""));
      // Both states describe a tree that no longer exists.
      this.treeLoading = false;
      this.treeError = "";
      // A search describes THIS tree; the field stays open.
      this.resetFileSearch();
      this.hideMenu();
    },

    // --- opening a file into a tab ----------------------------------------
    openFile(node) {
      const path = this.relPath(node);
      const ftype = classify(node.title);
      this.emit("open", node, { ftype });
      if (ftype === "binary") {
        // Flash it too: a click that silently does nothing reads as a broken
        // tree.
        WB.emit("open-refused", { project: this.openSlug, path, reason: "binary" });
        this._flashAction?.("Cannot open binary files.");
        return;
      }
      // Out of a CONTENT search: the tab lands on the first occurrence
      // (ADR-0036 amendment 2026-09-15).
      const find = this.fileSearchFindTerm();
      this.openTab({ project: this.openSlug, path, title: node.title, ftype, find });
    },

    // The term to land on: the live query, only while the CONTENT filter is on.
    fileSearchFindTerm() {
      const fs = this.fileSearch;
      if (!fs.open || fs.mode !== "content" || !fs.hits.length) return null;
      const q = String(fs.query ?? "").trim();
      return q || null;
    },

    // A markdown link to another repo file: same viewer choice and binary
    // refusal as a tree click. `checkout` is the SOURCE pane's pin (#406): a
    // link in a worktree's document names that worktree's file.
    openLink({ project, path, fragment, checkout }) {
      const title = path.split("/").pop();
      const ftype = classify(title);
      if (ftype === "binary") {
        WB.emit("open-refused", { project, path, reason: "binary" });
        this._flashAction?.("Cannot open binary files.");
        return;
      }
      this.openTab({ project, path, title, ftype, fragment, checkout });
    },

    // `content`: a re-attached popup passes its (possibly edited) bytes back.
    // `fragment`/`find`: a `#heading` or search term to land on once the bytes
    // are shown. `checkout` PINS the tab to the tree it was opened in (#406):
    // a Save from a tab showing worktree bytes must never land on the primary.
    openTab({ project, path, title, ftype, content, fragment, find, checkout }) {
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
          content != null ? Promise.resolve(content) : this.fetchContent(project, path, ftype, ck);
        bytes.then((body) => {
          if (body == null) return; // refused: fetchContent surfaced the reason
          WBViewer.open({
            id,
            project,
            label: this.projectLabel(project),
            path,
            ftype,
            content: body,
            checkout: ck,
          });
          // NOT `setActive(id)`: `restoreView` opens N tabs in one burst and
          // THEN activates the stored one, so the last read to answer must not
          // own the screen.
          this.syncViewer();
          window.lucide?.createIcons();
          if (fragment) WBViewer.jumpTo(id, fragment);
          if (find) WBViewer.find(id, find);
        });
      });
    },

    // --- opening a Changes row into a diff tab ----------------------------
    // HEAD on one side, the working tree on the other (#311). Read-only;
    // Monaco computes the diff, nothing produces a patch.
    openDiff(project, entry) {
      // Pinned to the selection at open (#407): both sides read `t.checkout`.
      const t = window.WBChanges.diffTarget(entry, project, this.checkoutOf(project));
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
      WB.emit("open-diff", { project, path: t.workingPath, checkout: t.checkout });
      this.$nextTick(() => {
        // Latched: a path refused on BOTH sides would flash twice.
        let refused = false;
        const refuse = (reason) => {
          if (refused) return null;
          refused = true;
          this._flashAction?.(reason);
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
              label: this.projectLabel(project),
              path: t.workingPath,
              ftype: "diff",
              content: work,
              original: head,
            });
            // As `openTab`: the pane follows the CURRENT active tab.
            this.syncViewer();
            window.lucide?.createIcons();
          })
          .catch(() => refuse("diff read failed"));
      });
    },

    // The diff's HEAD side; an added/untracked path diffs against emptiness.
    diffHeadSide(project, t, refuse) {
      if (t.headAbsent) return Promise.resolve("");
      if (!this.useDaemonTree()) {
        return Promise.resolve(fakeContent(t.headPath, "code"));
      }
      return WBDaemon.observe(
        "blob.read",
        WBDaemon.withCheckout({ repo: project, revision: "head", path: t.headPath }, t.checkout),
      ).then((reply) => {
        if (window.WBFail.isError(reply)) return refuse(window.WBFail.message(reply, "refused"));
        const blob = reply.blob || {};
        if (blob.status === "present") return blob.content;
        if (blob.status === "absent") return "";
        return refuse(blob.reason || "refused");
      });
    },

    // The diff's working side. `not found` is NOT a refusal: a stale row
    // (deleted between the list and the click) diffs against emptiness.
    diffWorkSide(project, t, refuse) {
      if (t.workingAbsent) return Promise.resolve("");
      if (!this.useDaemonTree()) {
        // The static demo: a synthesised one-line delta.
        return Promise.resolve("// (demo) edited line\n" + fakeContent(t.workingPath, "code"));
      }
      // NOT `fetchContent`: it collapses every refusal to `null` and closes
      // the `file:` tab id rather than this diff's.
      return WBDaemon.observe(
        "file.read",
        WBDaemon.withCheckout({ repo: project, path: t.workingPath }, t.checkout),
      ).then((reply) => {
        if (!window.WBFail.isError(reply)) return reply.content;
        const reason = window.WBFail.message(reply, "refused");
        return reason === "not found" ? "" : refuse(reason);
      });
    },

    // Pop a file tab out into a standalone popup; the in-app tab closes.
    detachFile(desc) {
      const id = fileTabId(desc.project, desc.path, desc.checkout);
      // The descriptor is handed over by postMessage with `targetOrigin =
      // location.origin`, NOT in the URL hash: a hash let anyone render content
      // of their choosing on the daemon's origin. Passing the bytes keeps
      // unsaved edits alive across a detach.
      const win = window.open("detached.html", "_blank", "popup,width=920,height=760");
      if (!win) {
        WB.emit("detach-blocked", { project: desc.project, path: desc.path });
        return;
      }
      detachedWindows.set(win, desc);
      WB.emit("detach", { project: desc.project, path: desc.path });
      this.closeTab(id);
      this.activate("consoles");
    },

    activate(id) {
      this.active = id;
      // The Spend tab's subject can change while it sits in the background.
      if (id === "spend" && this.spend.slug !== this.openSlug) this.refreshSpend();
      this.$nextTick(() => {
        this.syncViewer();
        window.lucide?.createIcons();
        // A console opened while another tab was active measured 0×0.
        if (id === "consoles") window.WBConsole?.refitAll?.();
      });
      this.persistView();
    },

    // The ONE place that tells the viewer which pane is on screen: an async
    // opener calling it late can only converge. Paneless tabs map to `null`.
    PANELESS_TABS: ["consoles", "spend"],
    syncViewer() {
      WBViewer.setActive(this.PANELESS_TABS.includes(this.active) ? null : this.active);
    },

    closeTab(id) {
      const idx = this.tabs.findIndex((t) => t.id === id);
      const tab = this.tabs[idx];
      if (!tab || !tab.closable) return; // Consoles never closes
      WBViewer.close(id);
      this.tabs.splice(idx, 1);
      if (this.active === id) {
        // fall back to the neighbour, else the Consoles tab
        const next = this.tabs[idx] || this.tabs[idx - 1] || this.tabs[0];
        this.activate(next.id);
      }
      this.persistView();
    },

    // --- the per-client view: the open file tabs (issue #339) ----------------
    // The tabs half of `wb.view.v1` (`wb-console.js` owns the offset half;
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
      window.WBView?.patch({ tabs: files, active: alive ? this.active : "consoles" });
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

    // --- consoles (the Consoles tab) ----------------------------------------
    // The "New console" menu (wb-agents.js): the roster folded against the
    // live sessions, plus a plain console pinned LAST. Each row carries an
    // Alt+Shift+<digit> accelerator, matched by physical key (e.code) so it
    // fires regardless of layout. Console is Alt+Shift+0; the startup-command
    // console (Settings → Consoles), when one is set, is Alt+Shift+9.
    liveSessions: [],
    // Read ONCE from the view store: Alpine cannot observe the store, so the
    // settings save writes this field beside it.
    consoleCommand: window.WBView?.read()?.command ?? null,
    consoleItems() {
      return window.WBAgents.menuRows({
        roster: this.roster,
        sessions: this.liveSessions,
        openSlug: this.openSlug,
        command: this.consoleCommand,
      });
    },
    isMac: /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || ""),
    shortcutLabel(digit) {
      return this.isMac ? `⌥⇧${digit}` : `Alt+Shift+${digit}`;
    },
    // `opts.fresh` is the row's "+" button: another console for this agent
    // even though one is live.
    openConsoleItem(item, opts = {}) {
      const intent = window.WBAgents.consoleIntent(item, opts);
      if (!intent) return;
      if (item.plain) this.newPlainConsole(item.command);
      else if (intent === "attach") {
        if (this.active !== "consoles") this.activate("consoles");
        WBConsole.reach({ id: item.sessionId, agent: item.kind, repo: this.openSlug });
        this.consoleCount = WBConsole.count();
      } else this.newConsole(item.kind);
      this.agentMenu = false;
    },

    newConsole(agent) {
      // The accelerator path calls this directly: refuse with no repo here too.
      if (!this.openSlug) return;
      if (this.active !== "consoles") this.activate("consoles");
      // Always the primary: only the console's own title switcher moves it
      // (ADR-0063, amendment 2026-09-16 b).
      WBConsole.open({ repo: this.openSlug, agent, checkout: null });
      this.consoleCount = WBConsole.count();
    },
    // a bare shell in the repo dir (no agent) — the daemon's per-repo console;
    // with `command`, the shell runs it instead of a prompt (Settings → Consoles)
    newPlainConsole(command) {
      if (this.active !== "consoles") this.activate("consoles");
      WBConsole.open({ repo: this.openSlug, plain: true, command: command || undefined });
      this.consoleCount = WBConsole.count();
    },

    // Accelerators are ignored while typing or while a modal is up.
    consoleShortcutsBlocked() {
      if (!this.authed) return true;
      if (this.settingsOpen || this.securityOpen || this.runOpen || this.branchOpen) return true;
      if (this.whatsNewOpen) return true;
      const el = document.activeElement;
      return !!(
        el &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable || el.closest(".monaco-editor"))
      );
    },

    // The fence cap, stated before the click. Read off `fenceItems` (the
    // snapshot `toggleFenceMenu` takes), NOT `WBConsole.atFenceCap()`:
    // MEASURED, the module's fence array is not Alpine state, so a binding on
    // it never re-evaluated. The module stays the AUTHORITY for the gesture
    // (`newFence`), so a stale snapshot can never create a thirteenth fence.
    fenceAtCap() {
      return this.fenceItems.length >= window.WBConsole.FENCE_MAX;
    },
    fenceCapMessage() {
      return `Maximum of ${window.WBConsole.FENCE_MAX} fences. Remove one to add another.`;
    },
    fenceCapReason() {
      return this.fenceAtCap() ? this.fenceCapMessage() : "draw a named fence on the plane";
    },
    newFence() {
      if (this.active !== "consoles") this.activate("consoles");
      // The menu closes BEFORE the fence is drawn, or its list goes stale.
      this.fenceMenu = false;
      // The module decides; `false` is its refusal at the cap, and saying so is
      // this layer's job (`wb-console.js` reaches no shell).
      if (WBConsole.createFence() === false) this._flashAction(this.fenceCapMessage());
    },

    // ONE dropdown at a time: each trigger closes every other menu before
    // toggling its own. Enumerated here, once.
    closeMenus() {
      this.agentMenu = false;
      this.windowMenu = false;
      this.fenceMenu = false;
      this.avatarMenu = false;
    },
    toggleAgentMenu() {
      const was = this.agentMenu;
      this.closeMenus();
      this.agentMenu = !was;
    },
    toggleAvatarMenu() {
      const was = this.avatarMenu;
      this.closeMenus();
      this.avatarMenu = !was;
    },
    toggleWindowMenu() {
      this.windowList = WBConsole.list();
      const was = this.windowMenu;
      this.closeMenus();
      this.windowMenu = !was;
    },
    revealWindow(id) {
      if (this.active !== "consoles") this.activate("consoles");
      this.windowMenu = false;
      // AFTER the tab is laid out: a `display:none` tab measures 0.
      this.$nextTick(() => WBConsole.reveal(id));
    },

    // The fence list is the map (#343). Snapshot on open, like the Go-to picker.
    toggleFenceMenu() {
      this.fenceItems = WBConsole.fenceList();
      const was = this.fenceMenu;
      this.closeMenus();
      this.fenceMenu = !was;
    },
    // Alt+Shift+←/→. Returns the fence landed on, or null when the plane has
    // none (the shortcut decides whether to swallow the key). Runs against the
    // LIVE stage.
    stepFence(step) {
      if (this.active !== "consoles") return null;
      return WBConsole.stepFence(step);
    },
    jumpFence(id) {
      if (this.active !== "consoles") this.activate("consoles");
      this.fenceMenu = false;
      // As `revealWindow`: a `display:none` tab measures 0.
      this.$nextTick(() => WBConsole.jumpToFence(id));
    },
    // Alt+Shift+F<n> → the n-th fence, up to F12 (the fence cap). MEASURED
    // (Chromium 148): Alt+Shift+F10/F11/F12 all reach the document —
    // `Shift+F10`, F11 and F12 want their exact combo, and Alt takes this out
    // of their way. The ROW carries only its own key; the modifier pair is a
    // legend in the head.
    fenceShortcutLabel(n) {
      return `F${n}`;
    },
    fenceShortcutHint() {
      return this.isMac ? "⌥⇧F<n>" : "Alt+Shift+F<n>";
    },
    // Ordinal, not id: the row's position in `fenceList()`, read LIVE (the
    // menu's snapshot may be stale). Returns whether it landed.
    jumpFenceAt(n) {
      if (this.active !== "consoles") return false;
      const f = WBConsole.fenceList()[n - 1];
      if (!f) return false;
      this.fenceMenu = false;
      return !!WBConsole.jumpToFence(f.id);
    },

    // --- context menu -----------------------------------------------------
    // `node` is null for empty tree space, which addresses the repo root: the
    // create items apply, the per-node items drop out.
    showMenu(x, y, node) {
      const menu = document.getElementById("ctxmenu");
      const isFolder = this.isFolder(node);
      const items = [
        node && !isFolder && { label: "Open", icon: "bi-box-arrow-up-right", run: () => this.openFile(node) },
        node && { label: "Rename…", icon: "bi-pencil", run: () => node.startEditTitle() },
        // FLAT rows, not a submenu, for a two-item choice made constantly.
        node && { label: "Copy full path", icon: "bi-clipboard", run: () => this.copyPath(node, true) },
        node && { label: "Copy relative path", icon: "bi-clipboard", run: () => this.copyPath(node, false) },
        node && !isFolder && { label: "Duplicate", icon: "bi-files", run: () => this.duplicateNode(node) },
        // Folders too. Dropped for a node inside `.git`/`.ralphy`, whose every
        // move the daemon refuses on the SOURCE.
        node && !underProtectedDir(this.relPath(node)) && {
          label: "Move to…",
          icon: "bi-arrow-right-square",
          run: () => this.moveNode(node),
        },
        node && { sep: true },
        // Creating targets the folder itself, or the folder CONTAINING the
        // clicked file.
        { label: "New file…", icon: "bi-file-earmark-plus", run: () => this.emitCreate(node, "file") },
        { label: "New folder…", icon: "bi-folder-plus", run: () => this.emitCreate(node, "folder") },
        node && { sep: true },
        node && { label: "Delete", icon: "bi-trash", danger: true, run: () => this.emit("delete", node) },
      ].filter(Boolean);

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
        b.innerHTML = `<i class="bi ${it.icon}"></i><span>${it.label}</span>`;
        b.onclick = () => {
          this.hideMenu();
          it.run();
        };
        menu.append(b);
      }
      // Keep the menu on-screen.
      menu.style.display = "block";
      const w = menu.offsetWidth,
        h = menu.offsetHeight;
      menu.style.left = Math.min(x, innerWidth - w - 8) + "px";
      menu.style.top = Math.min(y, innerHeight - h - 8) + "px";
    },

    hideMenu() {
      const menu = document.getElementById("ctxmenu");
      if (menu) menu.style.display = "none";
    },

    // --- the backend seam -------------------------------------------------
    // Build the repo-relative path by walking parent titles.
    relPath(node) {
      const parts = [];
      let n = node;
      while (n && n.title && n.parent) {
        parts.unshift(n.title);
        n = n.parent;
      }
      return parts.join("/");
    },

    // `full` joins the project's absolute `root` onto the rel path in the
    // ROOT's own separator, so it pastes into a native shell; the rel path
    // when no root is known. `navigator.clipboard` is undefined on an
    // insecure non-loopback origin, so the call is optional-chained.
    copyPath(node, full = false) {
      const rel = this.relPath(node);
      const root = full ? this.projects.find((p) => p.slug === this.openSlug)?.root : "";
      let path = rel;
      if (root) {
        // Windows by SHAPE (drive letter or UNC lead), not by "contains a
        // backslash".
        const sep = /^[A-Za-z]:/.test(root) || root.startsWith("\\\\") ? "\\" : "/";
        // Trim a trailing separator so a drive root (`C:\`) does not double it.
        const base = root.replace(/[\\/]+$/, "");
        path = base + sep + rel.split("/").join(sep);
      }
      navigator.clipboard?.writeText(path).catch(() => {});
      this.emit("copy-path", node, { path });
    },

    // Duplicate a file beside itself, NO prompt. `file.copy` refuses an
    // existing dst, so the free-name search happens HERE: the first of `<stem>
    // copy<ext>`, `<stem> copy 2<ext>`, … not taken.
    async duplicateNode(node) {
      const rel = this.relPath(node);
      if (!rel) return;
      const parent = parentRel(rel);
      const name = rel.slice(parent ? parent.length + 1 : 0);
      const dot = name.lastIndexOf(".");
      // A leading dot is the whole name of a dotfile, not an extension.
      const stem = dot > 0 ? name.slice(0, dot) : name;
      const ext = dot > 0 ? name.slice(dot) : "";

      // The tree's gestures speak the tree's checkout (#406); a refusal beats
      // a primary-aimed copy that silently misfires.
      const checkout = this.checkoutOf(this.openSlug);
      const listing = await WBDaemon.observe(
        "tree.list",
        WBDaemon.withCheckout({ repo: this.openSlug, path: parent }, checkout),
      ).catch(() => null);
      // A refused listing must NOT degrade to an empty `taken` set.
      if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
        this._flashAction?.("couldn't list the folder");
        return;
      }
      const taken = new Set(listing.entries.map((e) => e.name));
      let candidate = `${stem} copy${ext}`;
      for (let i = 2; taken.has(candidate); i++) candidate = `${stem} copy ${i}${ext}`;
      const to = parent ? `${parent}/${candidate}` : candidate;

      const reply = await WBDaemon.write(
        "file.copy",
        WBDaemon.withCheckout({ repo: this.openSlug, path: rel, to }, checkout),
      ).catch(() => null);
      if (!reply || WBFail.isError(reply)) {
        this._flashAction?.(reply?.reason || "duplicate failed");
        return;
      }
      await this.onTreeDirty(parent);
      await this.revealRel(to);
    },

    // --- move (issue #364) ------------------------------------------------
    // The destination is PICKED, never typed: the picker browses real
    // directories through `tree.list`.
    moveNode(node) {
      const rel = this.relPath(node);
      if (!rel) return;
      this.movePick = {
        open: true,
        from: rel,
        dir: "",
        entries: [],
        busy: false,
        error: "",
      };
      this._movePickSeq = (this._movePickSeq || 0) + 1;
      this.movePickLoad(parentRel(rel));
    },

    async movePickLoad(dir) {
      // Stamp the request: two quick clicks would settle out of order.
      const seq = (this._movePickSeq = (this._movePickSeq || 0) + 1);
      this.movePick.busy = true;
      this.movePick.error = "";
      // Same checkout as the tree the row came from (#406).
      const listing = await WBDaemon.observe(
        "tree.list",
        WBDaemon.withCheckout({ repo: this.openSlug, path: dir }, this.checkoutOf(this.openSlug)),
      ).catch(() => null);
      if (seq !== this._movePickSeq) return;
      this.movePick.busy = false;
      // A refused listing is a REASON, not an empty folder.
      if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
        this.movePick.entries = [];
        this.movePick.error = WBFail.message(listing, "couldn't list the folder");
        return;
      }
      const from = this.movePick.from;
      this.movePick.dir = dir;
      this.movePick.entries = listing.entries
        .filter((e) => e.dir)
        // A folder cannot move into itself or its own subtree.
        .filter((e) => {
          const rel = dir ? `${dir}/${e.name}` : e.name;
          return rel !== from && !rel.startsWith(`${from}/`);
        })
        // Never a destination the Write path refuses (`fswrite::PROTECTED_DIRS`);
        // `tree` still LISTS `.ralphy` so the run artifacts stay watchable.
        .filter((e) => !isProtectedDir(e.name));
    },

    movePickInto(name) {
      this.movePickLoad(this.movePick.dir ? `${this.movePick.dir}/${name}` : name);
    },

    movePickUp() {
      if (!this.movePick.dir) return;
      this.movePickLoad(parentRel(this.movePick.dir));
    },

    movePickCancel() {
      this.movePick.open = false;
    },

    // "Move here" is dead while the browsed directory IS the source's parent
    // (a no-op the daemon reports as `exists`) and while the listing FAILED
    // (`dir` still names the last directory that loaded).
    movePickBlocked() {
      return (
        this.movePick.busy ||
        !!this.movePick.error ||
        this.movePick.dir === parentRel(this.movePick.from)
      );
    },

    movePickConfirm() {
      const from = this.movePick.from;
      const dir = this.movePick.dir;
      const leaf = from.slice(parentRel(from) ? parentRel(from).length + 1 : 0);
      // A no-op the daemon would report as a bare `exists`: name the reason.
      if (dir === parentRel(from)) {
        this.movePick.error = "it is already in that folder";
        return;
      }
      this.movePick.open = false;
      return this.performMove(from, dir ? `${dir}/${leaf}` : leaf);
    },

    // Through `WBDaemon.write`, not the fire-and-forget `WB.emit("rename")`:
    // the reveal, the flash and the tab re-path need the reply. INVARIANT: no
    // tab is re-pathed and no reveal happens on a refusal.
    async performMove(from, to) {
      const reply = await WBDaemon.write(
        "file.rename",
        WBDaemon.withCheckout(
          { repo: this.openSlug, path: from, to },
          this.checkoutOf(this.openSlug),
        ),
      ).catch(() => null);
      if (!reply) {
        this._flashAction?.("move failed");
        return;
      }
      if (WBFail.isError(reply)) {
        this._flashAction?.(WBFail.message(reply, "move refused"));
        return;
      }
      await this.onTreeDirty(parentRel(from));
      await this.onTreeDirty(parentRel(to));
      this.repathTabs(from, to);
      // LAST, so its `setActive()` is the final write.
      await this.revealRel(to);
    },

    // Re-point every open tab under the moved path: a tab's id IS its path,
    // and saves would write to the old location.
    repathTabs(from, to) {
      // Snapshot: the collision branch CLOSES a tab, which mutates `this.tabs`.
      for (const t of [...this.tabs]) {
        if (t.kind === "diff" || t.project !== this.openSlug) continue;
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
        t.path = newPath;
        t.id = newId;
      }
      this.persistView();
    },

    // A `create` intent carries the DIRECTORY, already resolved (`createDir`).
    emitCreate(node, kind) {
      WB.emit("create", { project: this.openSlug, path: this.createDir(node), kind, isFolder: true });
    },

    // The directory a create addressed at `node` lands in: the folder itself,
    // the folder CONTAINING a file, or the repo root ("") for no node at all.
    createDir(node) {
      const rel = node ? this.relPath(node) : "";
      return !node || this.isFolder(node) ? rel : parentRel(rel);
    },

    // The Files-header buttons create relative to the tree's active node.
    createHere(kind) {
      this.emitCreate(this.rawTree()?.getActiveNode() || null, kind);
    },

    // What the header buttons' tooltip names as the destination.
    createTargetLabel() {
      return this.createDir(this.rawTree()?.getActiveNode() || null) || "the repo root";
    },

    // Node-shaped gestures funnel through the shared WB.emit.
    emit(action, node, extra = {}) {
      WB.emit(action, {
        project: this.openSlug,
        path: this.relPath(node),
        title: node.title,
        isFolder: this.isFolder(node),
        ...extra,
      });
    },

    // Resolve `true`/`false` on the operator's choice. A pending dialog is
    // settled `false` first so a second call never strands its promise.
    askConfirm(opts = {}) {
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
    confirmRespond(ok) {
      this.confirmModal.open = false;
      const resolve = this._confirmResolve;
      this._confirmResolve = null;
      if (resolve) resolve(ok);
    },

    // Resolve the typed string, or `null`. Mirrors askConfirm.
    askPrompt(opts = {}) {
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
        const el = document.getElementById("prompt-input");
        if (!el) return;
        el.focus();
        el.setSelectionRange(el.value.length, el.value.length);
      });
      return new Promise((resolve) => {
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
    promptRespond(name) {
      this.promptModal.open = false;
      const resolve = this._promptResolve;
      this._promptResolve = null;
      if (resolve) resolve(name);
    },
  };
}

window.shell = shell;

// The live Alpine component instance. On `window` explicitly: two other
// modules call it, and a bare declaration reaches them only by accident of
// global scope.
window.getShell = function getShell() {
  const root = document.querySelector("[x-data]");
  return root && root._x_dataStack ? root._x_dataStack[0] : null;
};

// The Alpine mirror of the live console count.
document.addEventListener("workbench:consoles-changed", (e) => {
  const c = window.getShell();
  if (c) c.consoleCount = e.detail.count;
});

// …and of the stage extent, for the footer pill (#338).
document.addEventListener("workbench:stage-extent", (e) => {
  const c = window.getShell();
  if (!c) return;
  c.stageW = e.detail.width;
  c.stageH = e.detail.height;
});

// A viewer asked to detach → open the popup and close the tab.
document.addEventListener("workbench:detach-request", (e) => {
  window.getShell()?.detachFile(e.detail);
});

// A rendered markdown link asked for a repo file → open (or focus) its tab.
document.addEventListener("workbench:open-request", (e) => {
  window.getShell()?.openLink(e.detail);
});

// The popups this shell opened. Membership is the authorisation for every
// message below.
const detachedWindows = new Map();

// The origin we accept messages from and send to. `file://` documents have
// an opaque origin, where the only usable target is `"*"`.
const wbPeerOrigin = () => (window.WBMode?.isDemo() ? "*" : window.location.origin);

// Messages from detached popups. Both guards matter: `e.origin` refuses a
// page on another origin, `e.source` a same-origin window we did not open.
// Without them this listener accepted `file.write` from anyone holding a
// handle to this window.
window.addEventListener("message", (e) => {
  if (!window.WBMode?.isDemo() && e.origin !== window.location.origin) return;
  if (!detachedWindows.has(e.source)) return;
  const m = e.data;
  if (!m || typeof m !== "object") return;
  if (m.type === "wb-detach-ready") {
    // The popup booted and is asking for its file.
    e.source.postMessage({ type: "wb-detach-open", desc: detachedWindows.get(e.source) }, wbPeerOrigin());
  } else if (m.type === "wb-emit") {
    WB.emit(m.action, m.detail || {});
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
    // The pin comes home with the bytes (#406): explicit `null` is the primary.
    window.getShell()?.openTab({
      project: m.desc.project,
      path: m.desc.path,
      title: m.desc.path.split("/").pop(),
      ftype: m.desc.ftype,
      content: m.desc.content,
      checkout: m.desc.checkout ?? null,
    });
    detachedWindows.delete(e.source);
  }
});

// --- Write byte-ops (#197): the workspace-mutating seam actions go to the
// daemon's confined `file.*` verbs; a refusal is flashed. The browser composes
// the full rel path.
(function wireWriteVerbs() {
  const daemonBacked = () => window.WBMode.isDaemon() && !!window.WBDaemon?.write;
  const flash = (msg) => window.getShell()?._flashAction?.(msg);
  const call = (verb, payload, okMsg) => {
    WBDaemon.write(verb, payload)
      .then((reply) => {
        if (window.WBFail.isError(reply)) flash(window.WBFail.message(reply, "refused"));
        else if (okMsg) flash(okMsg);
      })
      .catch(() => flash("write failed"));
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
    const aimed = (payload) => WBDaemon.withCheckout(payload, checkout);
    switch (d.action) {
      case "save":
        call("file.write", aimed({ repo, path: d.path, content: d.content || "" }));
        break;
      case "worktree-created": {
        // A console's switcher cut a worktree: re-read the listing, flash what
        // the add had to say.
        const c = window.getShell();
        c?.ensureWorktreeListing?.(repo, true);
        if (d.message) c?._flashAction?.(d.message);
        break;
      }
      case "create": {
        // `create` carries the target DIRECTORY and no name: ask for it, then
        // open a created file so the operator lands in it.
        const folder = d.kind === "folder";
        const where = d.path || "repo root";
        const c = window.getShell();
        const name = c
          ? await c.askPrompt({
              // No placeholder: a plausible filename in an empty field reads as
              // a name already chosen, and operators pressed Enter on it.
              title: `${folder ? "New folder" : "New file"} in ${where}`,
              message: "",
              placeholder: "",
            })
          : window.prompt(folder ? "New folder name" : "New file name");
        if (!name) return;
        const path = d.path ? `${d.path}/${name}` : name;
        const reply = await WBDaemon.write("file.create", aimed({ repo, path, dir: folder })).catch(() => null);
        if (!reply) return flash("write failed");
        if (window.WBFail.isError(reply)) return flash(window.WBFail.message(reply, "refused"));
        flash(`created ${name}`);
        if (!folder) c?.openTab({ project: repo, path, title: name, ftype: classify(name) });
        // Reveal AFTER the level has settled, so `setActive()` is the last
        // write. `revealRel` expands the ancestors: a nudge for a COLLAPSED dir
        // is dropped, so a nudge alone would leave the new entry invisible.
        await c?.onTreeDirty(d.path || "");
        await c?.revealRel(path);
        break;
      }
      case "rename": {
        // `from`/`to` are FULL rel paths (shared with the move gesture).
        call("file.rename", aimed({ repo, path: d.from, to: d.to }));
        break;
      }
      case "delete": {
        // Irreversible (a folder removes recursively): confirm first.
        const name = d.title || d.path.split("/").pop() || d.path;
        const message = d.isFolder
          ? `Delete folder “${name}” and its contents? This cannot be undone.`
          : `Delete “${name}”? This cannot be undone.`;
        const c = window.getShell();
        const ok = c
          ? await c.askConfirm({ title: "Delete", message, confirmLabel: "Delete", danger: true })
          : window.confirm(message);
        if (!ok) return;
        const reply = await WBDaemon.write("file.delete", aimed({ repo, path: d.path })).catch(() => null);
        if (!reply) return flash("write failed");
        if (!window.WBFail.isError(reply)) return flash("deleted");
        const reason = window.WBFail.message(reply, "refused");
        flash(reason);
        // "not found" on a delete says the ROW is the lie: re-list the parent
        // so the ghost ends up off the screen.
        if (/not found/i.test(reason)) await c?.onTreeDirty(parentRel(d.path));
        break;
      }
      default:
        break;
    }
  });
})();

// Dismiss the context menu on any outside interaction.
document.addEventListener("click", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu").style.display = "none"));
document.addEventListener("scroll", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu").style.display = "none"), true);

document.addEventListener("alpine:initialized", () => window.lucide?.createIcons());

// Alt+Shift+<digit> → the menu row carrying that digit, through the SAME row
// action as a click. Matched on `e.code` so layout does not matter.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  if (!/^Digit\d$/.test(e.code)) return;
  const c = window.getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  const row = c.consoleItems().find((it) => e.code === "Digit" + it.digit);
  // No row, or a disabled one: inert, and the key is not swallowed.
  if (!row || row.disabled) return;
  e.preventDefault();
  c.openConsoleItem(row);
});

// Alt+Shift+←/→ → walk the fences in reading order (`fenceCycle`). With no
// fence the key is left UNSWALLOWED.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  if (e.code !== "ArrowRight" && e.code !== "ArrowLeft") return;
  const c = window.getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  if (!c.stepFence(e.code === "ArrowRight" ? 1 : -1)) return;
  e.preventDefault();
});

// Alt+Shift+F<n> → the n-th fence, F1..F12 (the fence cap). None of the
// reserved neighbours is hit — Alt+F4, Shift+F10, F11, F12 each want their
// exact combo (MEASURED: with Alt+Shift held, F10–F12 reach the document).
// With no fence at that ordinal the key is left UNSWALLOWED.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  if (!/^F(?:[1-9]|1[0-2])$/.test(e.code)) return;
  const c = window.getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  if (!c.jumpFenceAt(Number(e.code.slice(1)))) return;
  e.preventDefault();
});

// `/` → the search on screen: FILES while a project is open, projects
// otherwise.
document.addEventListener("keydown", (e) => {
  if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
  const c = window.getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  e.preventDefault();
  if (c.openSlug) c.openFileSearch();
  else c.focusProjectSearch();
});

// Ctrl/Cmd+Shift+F → the FILES search. NOT `consoleShortcutsBlocked`: the
// editor is exactly where "find in files" is reached for.
document.addEventListener("keydown", (e) => {
  if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey) return;
  if (e.key !== "F" && e.key !== "f") return;
  const c = window.getShell();
  if (!c || !c.authed || !c.openSlug) return;
  if (c.settingsOpen || c.securityOpen || c.runOpen || c.branchOpen || c.whatsNewOpen) return;
  e.preventDefault();
  c.openFileSearch();
});

// Inbound run events, `file://` demo ONLY (#300): gated here AND in
// `applyRunEvent` (also called by `demoTick`).
document.addEventListener("ralphy:run-event", (e) => {
  if (!window.WBMode.seedAllowed()) return;
  window.getShell()?.applyRunEvent(e.detail);
});
window.WBRuns = {
  emit(evt) {
    document.dispatchEvent(new CustomEvent("ralphy:run-event", { detail: evt }));
  },
  // Append a raw output chunk, capped so the DOM never grows unbounded.
  output(text) {
    const c = window.getShell();
    if (c) c.rawFeed = (c.rawFeed + text).slice(-8000);
  },
};
