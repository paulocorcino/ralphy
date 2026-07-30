/* ---------------------------------------------------------------------------
   ralphy workbench shell — shell behaviour

   The sidebar is a project accordion (Alpine). The file tree inside the open
   project is a real Wunderbaum instance (mar10/wunderbaum) — a mature,
   dependency-free tree lib — loaded from a JSON tree the backend would send.

   The canvas is a tabbed workspace:
     • the first tab, "Consoles", is fixed (never closes) and hosts the floating
       console windows (see wb-console.js);
     • every opened file rides in as its own closable tab, rendered by a viewer
       (source code via Monaco, Markdown rendered with mermaid — see
       wb-viewer.js).

   Every user gesture (open, rename, delete, save, console-open…) is turned into
   a single CustomEvent, `workbench:action`, on `document`. That event *is* the
   seam: a backend engine subscribes and performs the real work. The UI itself
   performs nothing destructive — it only intents.
--------------------------------------------------------------------------- */

// The one exit point, shared by the sidebar, the consoles, and the viewers:
// every gesture becomes a `workbench:action` event a backend listens for.
window.WB = {
  emit(action, detail = {}) {
    const full = { action, ...detail, at: new Date().toISOString() };
    document.dispatchEvent(new CustomEvent("workbench:action", { detail: full }));
    // eslint-disable-next-line no-console
    console.log("[workbench:action]", full);
  },
};

// Images the daemon serves as bytes (ADR-0049): they open in the image pane.
// The daemon holds the authoritative allowlist and verifies the magic bytes —
// this set only decides which VERB a click sends, never what gets rendered.
const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"]);

// Files whose bytes aren't source we can render and aren't images — refuse to
// open them.
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

// The directory containing `rel`, as a repo-relative path; "" for a top-level
// entry (which is the repo root, the same value the tree verbs take for it).
function parentRel(rel) {
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

// What kind of viewer a file gets: markdown gets the rendered pane, an image
// gets the image pane, other binaries are refused, everything else opens as
// source code.
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
    // True only on the static `file://` demo bundle; drives the topbar "demo"
    // badge and keeps seeds confined to demo (#202).
    isDemo: window.WBMode.isDemo(),
    // Daemon-mode `/api/repos` failure surface (M5, #202): a visible error
    // instead of the seed projects. Empty when repos loaded (or in demo).
    reposError: "",
    // The local fleet's peers (ADR-0052 §5, #349), read from `/api/fleet` after
    // the local `/api/repos` pass. Empty is the honest default: a fleet of one,
    // or a daemon too old to serve the route.
    fleetPeers: [],
    // Working-tree change count per slug (#307), loaded when a project opens and
    // on the sidebar refresh. A slug holds `null` until a load succeeds — the
    // badge renders that as `—`, so a failed read never reads like a clean tree —
    // and `changesError` carries the reason into the badge's title.
    changesCount: {},
    changesError: {},
    // The two rendered groups (#315). INVARIANT: every path that sets one must
    // set the OTHER in the SAME statement — a stale group left behind renders
    // rows under a headline while the badge already reads `—`.
    changesStaged: {},
    changesUnstaged: {},
    // The sync row per project (#316): the fold of `sync.status`. Read on the
    // same three triggers as the change set — never on a timer, because a fetch
    // is the operator's own act and a status read must not become a habit the
    // UI schedules.
    syncByProject: {},
    // The commit message being composed (#318). One box for the whole shell,
    // but it belongs to `commitMsgSlug` and NOTHING else: a message typed for
    // repo A, abandoned, must never land as repo B's commit. `commitStaged`
    // clears it on success only — a refused commit must not eat what the
    // operator typed.
    commitMsg: "",
    commitMsgSlug: null,
    // True while a manual/initial repo refresh is in flight — spins the sidebar
    // refresh button and disables it. The list does NOT auto-refresh (only the
    // live dots do, via the presence heartbeat), so the button is the way to pick
    // up a newly-registered repo or a branch/dirty change without a page reload.
    reposLoading: false,
    // Live presence + identity (#204): the topbar uptime is the `/ws` heartbeat's
    // age, and the brand/avatar are `/api/identity`. Empty until the first tick /
    // a baptized daemon; `_lastHeartbeat` (epoch ms) drives the stale indicator.
    uptimeText: "",
    identityName: "",
    identityAvatar: "",
    _lastHeartbeat: 0,
    _tree: null, // the live Wunderbaum instance, if any
    _treeSub: null, // the live `/ws/tree` subscription for the open project, if any
    _runsSub: null, // the live run-snapshot subscription for the open project, if any
    _changesSub: null, // the run-completion nudge subscription for the open project (#310)
    // Monotonic hydration token: pushes arrive faster than a `runs.list` round
    // trip, so two hydrations overlap and their replies can land OUT OF ORDER —
    // an older reply would then overwrite a newer snapshot. Only the newest
    // hydration is allowed to commit.
    _runsSeq: 0,
    // Same token for the Changes count: #310's nudge makes overlapping
    // `changes.list` reads routine (a nudge during the open's own read).
    _changesSeq: 0,
    // Same token for the sync row: it rides the same three triggers, plus the
    // reload a Fetch/Pull click performs.
    _syncSeq: 0,

    // Alpine lifecycle: hydrate the Runs seed once the DOM (incl. the hidden
    // plan <script> blocks) is present.
    init() {
      this.initRuns();
      this.currentRunId = this.projectRuns()[0]?.runid || null;
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      this.probeSession();
      // In daemon mode the `projects` literal is demo seed (lingopilot &c.) — drop
      // it BEFORE the async loadRepos so it never flashes on screen; the real
      // registry replaces it. Demo (file://) keeps the seed. Mirrors initRuns.
      if (!window.WBMode.seedAllowed()) this.projects = [];
      this.loadRepos();
      this.loadAgents();
      this.subscribePresence();
      this.loadIdentity();
      // The board's two time-driven refresh triggers (#301). Both are registered
      // ONCE for the page's life and both defer the decision to the predicate —
      // the listener/timer only names the trigger, so "board open? tab focused?
      // long enough ago?" lives in one testable place (wb-kanban.js).
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState !== "visible") return;
        this.maybeRefreshBoard("visible");
        // Coming back to the tab is the other moment the Changes panel is worth
        // a read: its backstop below did nothing while the tab was hidden.
        this.refreshChanges();
      });
      // Anchor the clock at page load: leaving `_boardLoadedAt` at 0 makes the
      // first tick see `sinceMs === Date.now()`, which clears the 120s floor
      // trivially and folds the board 30s after open for no reason.
      this._boardLoadedAt = Date.now();
      this._boardBackstop = setInterval(() => this.boardBackstopTick(), 30000);
      // Registered once for the page's life, like the board's: the tick itself
      // asks whether the panel is open and in front, so there is no arm/disarm
      // state to keep in step with the rail.
      this._changesBackstop = setInterval(() => this.refreshChanges(), this.CHANGES_POLL_MS);
      // The phase clock's tick. One assignment a second, and only while the panel
      // that shows a clock is open: the run document is NOT re-read (its anchor is
      // a timestamp, not a countdown), so this is the whole cost of a live clock.
      this._clockTick = setInterval(() => {
        if (this.runsOpen) this.nowMs = Date.now();
      }, 1000);
    },

    // The daemon's real identity (name + avatar), shown in the topbar brand. A
    // 404 (un-baptized daemon) or a thrown fetch (file:// demo) leaves the
    // fields empty and the markup falls back to `ralphy` / no avatar.
    // The daemon's mark, in ONE place: the topbar button, the account menu's
    // head and the About card all render this, so they can never disagree about
    // what this daemon looks like. The fallback is a picture too — an
    // unbaptized daemon still needs something in a 26px circle, and a blank one
    // reads as a failed load rather than as "no name yet".
    identityMark() {
      return this.identityAvatar || "🤖";
    },
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

    // Subscribe to the `/ws` presence heartbeat (daemon mode only). Each tick
    // stamps `_lastHeartbeat` (the connection-liveness signal) and refreshes the
    // topbar uptime; a baptized daemon also carries name/avatar. Every tick
    // re-derives `live` so the sidebar dots track session open/close (~2s).
    subscribePresence() {
      if (!window.WBMode.isDaemon() || !window.WBDaemon?.subscribePresence) return;
      window.WBDaemon.subscribePresence((p) => {
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

    // Ask the daemon whether this browser is authorized. A thrown fetch (file://
    // standalone, no daemon) is swallowed so `authed` keeps its seed default —
    // the shell stays navigable offline; only a real /api/session response gates.
    async probeSession() {
      try {
        const r = await fetch("/api/session");
        if (r.ok) {
          const s = await r.json();
          this.authed = s.authed;
          this.login.passwordRequired = s.password;
          this.security.policy = s.policy;
          // The login card's mark. `/api/identity` is gated, so this pre-login
          // leg is the only way the gate can wear this daemon's face rather than
          // a generic robot; `loadIdentity` still overwrites it (with the name
          // too) once a cookie exists.
          if (s.avatar) this.identityAvatar = s.avatar;
          // Gated on `authed`: a pre-login restore would have every tab refused
          // and closed, persisting the loss (issue #339). `rehydrateAfterAuth`
          // is the other end of this guard.
          if (s.authed) this.restoreView();
        }
      } catch {
        // ONLY the `file://` demo, never a daemon that merely threw. In daemon
        // mode a thrown `/api/session` means unreachable or restarting — but
        // `authed` still holds its `true` seed, so restoring here would open N
        // `file.read` sockets against that same dead daemon, and `fetchContent`
        // closes a tab whose read fails. Since `closeTab` persists, one
        // transient failure would permanently erase the operator's tab set.
        // `submitLogin` draws the same demo-only line for the same reason.
        // Leaving `_viewRestored` false lets `rehydrateAfterAuth` still restore.
        if (window.WBMode.isDemo()) this.restoreView();
      }
    },

    // Hydrate the accordion from the daemon's real repo registry. A thrown
    // fetch (file:// standalone, no daemon) is swallowed so `projects` keeps
    // its seed — same offline-navigable contract as `probeSession()`. `state`
    // maps only to idle/offline this slice ("live" means an active session,
    // not yet tracked here); `remote` is inferred from the slug shape
    // (`git::project_slug`'s only `path-<hash>` fallback is a remoteless repo).
    // The daemon's adapter roster (#304). Same demo/daemon split as loadRepos:
    // a file:// walkthrough has no daemon to ask, so it falls back to the seed;
    // in DAEMON mode a failed fetch leaves the roster EMPTY rather than showing
    // adapters this daemon may not have — the menu keeps its plain console row.
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
          window.WBMode.seedAllowed() ? window.WBAgents.DEMO_ROSTER : [],
          repo,
        );
        this.roster = state.roster;
        this.agents = state.agents;
      }
    },
    async loadRepos() {
      this.reposLoading = true;
      try {
        const r = await fetch("/api/repos");
        if (r.ok) {
          const repos = await r.json();
          this.projects = repos.map((x) => ({
            slug: x.slug,
            // The absolute on-disk path, served since #204 and dropped here
            // until now. `repoLabel` needs it for a remoteless repo, whose slug
            // is a hash; the SLUG stays the identity (ADR-0008 D7) and the path
            // is only ever read for display (#332).
            path: x.path || "",
            // The canonical ABSOLUTE native root (#362), distinct from `path`:
            // "Copy full path" pastes this into a shell, and `path` carries git's
            // forward-slashed `--show-toplevel` output that peers parse.
            root: x.root || "",
            branch: x.branch || "",
            branches: x.branch ? [x.branch] : [],
            // Real working-tree + remote from `/api/repos` (#204). `remote` keeps
            // the existing github|local classification the dot binds to; the raw
            // origin url rides in `remoteUrl` so `githubUrl()` can rebuild links.
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
        // NOTE: this loader does NOT convert the rows' lucide icons. That is the
        // `x-effect` on `ul.projects` (#332), bound to the list's contents
        // rather than to one of the routes that change them — a fix here would
        // have covered the arrival and still left the list blank after one
        // keystroke in the search box.
      }
    },

    // The local fleet (ADR-0052 §5, #349): append every PEER's repos to the
    // sidebar after the local `/api/repos` pass, plus the peer list the group
    // headers render from.
    //
    // INVARIANT: a `/api/fleet` failure leaves the LOCAL list exactly as it was.
    // Federation is additive — a peer this daemon cannot reach, or a daemon too
    // old to serve the route, must never blank the sidebar the operator is
    // actually working in.
    async loadFleet() {
      try {
        const r = await fetch("/api/fleet");
        if (!r.ok) throw new Error(`/api/fleet ${r.status}`);
        const fleet = await r.json();
        this.fleetPeers = Array.isArray(fleet.peers) ? fleet.peers : [];
        const rows = Array.isArray(fleet.repos) ? fleet.repos : [];
        // `/api/fleet` is the ONLY source of this daemon's own environment label
        // and name — `/api/repos` has neither — so the local rows are stamped
        // with it here. Without this the local group header renders blank.
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
            // `key` is `<daemon_id>/<slug>`: the same `owner/repo` on two
            // daemons is two rows, so the slug alone cannot key this list.
            key: x.key,
            slug: x.slug,
            path: x.path || "",
            branch: x.branch || "",
            branches: [],
            dirty: false,
            state: x.reachable ? "idle" : "offline",
            remote: "local",
            remoteUrl: "",
            tree: [],
            // What makes this a peer row: the owning daemon, its environment,
            // and what this daemon last observed about it.
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

    // The sidebar's grouped view: local rows first, then one group per peer
    // environment. Pure fold in wb-fleet.js, unit-tested in node.
    fleetGroups() {
      return window.WBFleet.fleetGroups(this.filteredProjects(), this.fleetPeers);
    },
    repoRef(p) {
      return window.WBFleet.repoRef(p);
    },
    // Derive each project's `live` dot from the daemon's live sessions (#204): a
    // project is `live` when some `/api/sessions` entry's `repo` equals its slug.
    // Never overrides `offline` (an unreachable repo can't host a session).
    // Daemon-mode only; a transport throw leaves the current states untouched.
    async refreshLive() {
      if (!window.WBMode.isDaemon()) return;
      try {
        const r = await fetch("/api/sessions");
        if (!r.ok) return;
        const sessions = await r.json();
        // The console menu's fold reads this, so every presence tick refreshes
        // the per-row live counts too (#304).
        this.liveSessions = sessions;
        for (const p of this.projects) {
          if (p.state === "offline") continue;
          p.state = sessions.some((s) =>
            window.WBSessionRoute.matchesRepo(s, this.repoRef(p)),
          )
            ? "live"
            : "idle";
        }
      } catch {}
    },

    // --- chrome panels ----------------------------------------------------
    // Projects sidebar visibility (rail Projects button), the right-hand Runs
    // panel (rail Runs button), and the Kanban/tasks board (rail Kanban button,
    // a stub for now). Each is a pure layout flip driven by a body class.
    sideOpen: true,
    // Which view the sidebar is showing (#317): the rail switches it between
    // `projects` and `changes`. Changes is a VIEW, not a section inside the
    // project row — it is scoped to `openSlug` alone.
    sideView: "projects",
    runsOpen: false,
    kanbanOpen: false,
    projectQuery: "",

    // Clicking the rail button of the view already showing collapses the
    // sidebar — that preserves the pre-#317 `toggleSide()` gesture for the
    // Projects button, so the promotion adds a view without removing a feel.
    showSideView(view) {
      if (this.sideOpen && this.sideView === view) {
        this.sideOpen = false;
        return;
      }
      this.sideView = view;
      this.sideOpen = true;
      // Opening Changes IS a read trigger (#307's list was open/refresh/nudge,
      // and none of the three fires on the click that reveals the panel). The
      // rows were last read when the project was opened, which can be hours of
      // editing ago — the panel would show yesterday's tree until something else
      // happened to poke it.
      this.refreshChanges();
      // the incoming view's lucide icons live behind x-show and mount here
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // Re-read the working tree for the open project, but only when the Changes
    // panel is actually on screen. Both reads are cheap and LOCAL — `changes
    // list` is a `git status` and `sync status` makes no network call — but each
    // is still a subprocess, so nothing here runs for a panel nobody is looking
    // at. The `visible` gate is the same one the board's backstop uses.
    refreshChanges() {
      if (!window.WBMode.isDaemon()) return;
      if (!this.sideOpen || this.sideView !== "changes" || !this.openSlug) return;
      if (document.visibilityState !== "visible") return;
      this.loadChanges(this.openSlug);
      this.loadSync(this.openSlug);
    },
    // The slow backstop for the Changes panel. The `/ws/tree` nudge (#310) only
    // reports what a RUN did; an operator editing in their own editor produces
    // no event, and the panel would sit stale under their eyes. 50s is the
    // measured cost/staleness trade: two git subprocesses per minute, charged
    // only while the panel is open and the tab is in front.
    CHANGES_POLL_MS: 50000,

    // The Projects-view change indicator for one row, delegated to the pure
    // fold. Only slugs whose count was actually READ render one — fanning out a
    // `changes.list` per registered repo would cost N git subprocesses on open.
    projectBadge(slug) {
      return window.WBChanges.projectBadge(this.changesCount, this.changesError, slug);
    },

    // Case-insensitive slug/branch filter over the sidebar project list. The
    // sidebar count keeps showing `projects.length` (total located) — the
    // filter is a view concern, not a change to what's located.
    filteredProjects() {
      const q = this.projectQuery.trim().toLowerCase();
      if (!q) return this.projects;
      // The label is matched too (#332). This filter may match what the row does
      // NOT print — it already matches the owner half of a slug — but it must
      // never fail to match what the row DOES print: typing the visible
      // `MY-LOCAL-REPO` and getting an empty list is the defect a directory
      // label would otherwise introduce. The raw `path` is deliberately not
      // matched: an invisible absolute path is the opposite lie.
      return this.projects.filter(
        (p) =>
          p.slug.toLowerCase().includes(q) ||
          p.branch.toLowerCase().includes(q) ||
          this.repoLabel(p).toLowerCase().includes(q)
      );
    },

    // Sidebar row label: just the repo name (last slug segment), UPPERCASED.
    // The full `owner/repo` already shows in the top crumb, so trimming the
    // owner here declutters the accordion. Falls back to the whole slug if it
    // has no `/` (e.g. the remoteless `path-<hash>` fallback).
    repoLabel(p) {
      // A remoteless repo has no name in its slug: ADR-0008 D7 keys it
      // `path-<hash>`, which reads as twenty useless characters in a fixed 300px
      // column. The directory basename is what the operator calls it. The `/`
      // test is not optional — `slug_from_url` always yields `owner/repo`, so a
      // real GitHub repo named `owner/path-utils` is NOT this case and must
      // never be re-labelled off disk (#332).
      if (!p.slug.includes("/") && p.slug.startsWith("path-")) {
        // Windows and POSIX in one pass. Trailing separators go FIRST, or
        // `C:\src\widget\` basenames to the empty string.
        const base = String(p.path || "")
          .replace(/[\\/]+$/, "")
          .split(/[\\/]/)
          .pop();
        if (base) return base.toUpperCase();
      }
      return (p.slug.split("/").pop() || p.slug).toUpperCase();
    },

    // Opens the sidebar (if collapsed) and focuses the project search input —
    // the target of the global `/` shortcut.
    focusProjectSearch() {
      // `/` must never focus an input the Changes view is hiding (#317).
      this.sideView = "projects";
      this.sideOpen = true;
      this.$nextTick(() => this.$refs.projectSearch?.focus());
    },
    toggleRuns() {
      this.runsOpen = !this.runsOpen;
      // Closing drops the board-arrival marker: it names a navigation that is
      // over, and a same-numbered issue in another run would inherit it.
      if (!this.runsOpen) this.trailFocus = null;
      // the panel's lucide icons mount on open (they live inside x-if)
      if (this.runsOpen) {
        // The tick only runs while the panel is open, so `nowMs` is as stale as
        // the panel has been closed — re-anchor it before the first paint, or the
        // clock opens minutes behind and then jumps.
        this.nowMs = Date.now();
        this.hydrateRuns();
        this.$nextTick(() => window.lucide?.createIcons());
      }
    },
    toggleKanban() {
      // The tasks board: the open project's issues placed in four columns by
      // ralphy's own judgment (see wb-kanban.js). A pure overlay flip over the
      // canvas; the intent still fires so a backend can lazy-load the tracker.
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
    // Clicking a project's branch chip opens a filtered picker. The seed holds
    // the branch list per project (a backend would deliver it, e.g. `git
    // branch`); switching or creating emits an intent on the seam and the
    // daemon runs the real `git checkout` / `checkout -b`. The header reflects
    // the pick optimistically (like the tree's optimistic rename).
    branchOpen: false,
    branchModal: { slug: null, filter: "", branches: [], current: "", dirty: false },

    // Switching is possible only when the daemon can reach the repo on disk.
    // NOT gated on `remote`: a local-only repo (no GitHub) is still a git
    // checkout with branches — it's an *unreachable* path (state offline) the
    // daemon can't run `git branch`/`checkout` against.
    canSwitchBranch(p) {
      return p.state !== "offline";
    },

    // What the COLLAPSED row can no longer show. The branch chip moved to the
    // Files bar (#332), which only the OPEN project renders — and
    // `filteredProjects()` matches on branch, so a row that answers a branch
    // query while showing no branch is a lie. The slug keeps `.project-slug`'s
    // own title to itself: it is the ADR-0008 D7 identity, and it is how the
    // browser tests find a row.
    rowOpen(p) {
      return this.openSlug === this.repoRef(p);
    },

    rowTitle(p) {
      if (p.daemon) {
        return `${p.slug} · ${p.env}`;
      }
      if (!p.branch) return p.slug;
      return `${p.slug} · ${p.branch}${p.dirty ? " (uncommitted changes)" : ""}`;
    },

    branchChipTitle(p) {
      if (!this.canSwitchBranch(p)) return "repo unreachable — branch switching unavailable";
      return (p.dirty ? "switch branch (uncommitted changes) — " : "switch branch — ") + p.branch;
    },

    openBranchModal(p) {
      if (!this.canSwitchBranch(p)) return;
      const ref = this.repoRef(p);
      this.branchModal = {
        slug: ref,
        filter: "",
        branches: [...(p.branches || [p.branch])],
        current: p.branch,
        dirty: !!p.dirty,
      };
      this.branchOpen = true;
      this.loadBranches(ref);
      this.$nextTick(() => {
        window.lucide?.createIcons();
        this.$refs.branchFilter?.focus();
      });
    },

    // Replace the seed branch list with the repo's real local branches (#199),
    // served read-only via the `branch.list` Query verb. Graceful on throw (no
    // daemon reachable in a static shell) — the modal keeps its seed, mirroring
    // `loadBoard`.
    async loadBranches(slug) {
      try {
        const reply = await window.WBDaemon.observe("branch.list", { repo: slug });
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

    // The open project's working-tree change count (#307), served read-only via
    // the `changes.list` Query verb. The count is a snapshot between events: it
    // reloads when a project is opened, on the sidebar refresh, and on a
    // run-completion nudge (#310) — never on a poll or a repo-wide watch.
    async loadChanges(slug) {
      if (!slug) return;
      // Nudges (#310) can land while a read is in flight, so two reads of the
      // same slug overlap and their replies can return OUT OF ORDER — an older
      // reply would then overwrite a newer count (same hazard as `_runsSeq`).
      const seq = ++this._changesSeq;
      try {
        const reply = await window.WBDaemon.observe("changes.list", { repo: slug });
        if (seq !== this._changesSeq) return; // superseded → the newer read owns it
        if (!reply || reply.status !== "ok") {
          if (window.WBMode.isDaemon()) {
            // Honest absence beats another repo's number.
            this.changesCount[slug] = null;
            this.changesError[slug] = "could not read changes";
            this.changesStaged[slug] = [];
            this.changesUnstaged[slug] = [];
          }
          return;
        }
        const folded = window.WBChanges.fold(reply);
        this.changesCount[slug] = folded.count;
        this.changesStaged[slug] = folded.staged;
        this.changesUnstaged[slug] = folded.unstaged;
        this.changesError[slug] = "";
      } catch {
        if (seq === this._changesSeq && window.WBMode.isDaemon()) {
          this.changesCount[slug] = null;
          this.changesError[slug] = "could not read changes";
          this.changesStaged[slug] = [];
          this.changesUnstaged[slug] = [];
        }
        // Demo (static shell): leave whatever the seed/previous load holds.
      }
    },

    // The open project's sync state (#316), served read-only via the
    // `sync.status` Query verb — which makes NO network call, so this read is
    // safe on every trigger `loadChanges` rides. There is deliberately no timer:
    // a launcher holding N repos must never become a scheduled network client.
    async loadSync(slug) {
      if (!slug) return;
      const seq = ++this._syncSeq;
      try {
        const reply = await window.WBDaemon.observe("sync.status", { repo: slug });
        if (seq !== this._syncSeq) return; // superseded → the newer read owns it
        this.syncByProject[slug] = window.WBChanges.foldSync(reply);
      } catch {
        if (seq === this._syncSeq && window.WBMode.isDaemon()) {
          // Honest absence beats a stale row: an unreachable daemon must not
          // leave yesterday's counts on screen looking current.
          this.syncByProject[slug] = window.WBChanges.foldSync(null);
        }
        // Demo (static shell): leave whatever the previous load holds.
      }
    },

    // Fetch from the upstream — the operator's own act, never a timer's. A
    // refusal arrives as the Mutate branch's `{status:"error"}` and its message
    // IS the core's prose (`sync fetch` exits non-zero carrying it).
    async syncFetch(slug) {
      try {
        const reply = await window.WBDaemon.observe("sync.fetch", { repo: slug });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "fetch refused"));
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered. Saying
        // "refused" there would report a decision nobody made.
        if (window.WBMode.isDaemon()) this._flashAction("fetch unavailable: no daemon");
      }
      this.loadSync(slug);
    },

    // Fast-forward from the upstream. A successful pull moves the working tree,
    // so the change set is reloaded beside the counts.
    async syncPull(slug) {
      let moved = false;
      try {
        const reply = await window.WBDaemon.observe("sync.pull", { repo: slug });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "pull refused"));
        } else {
          moved = true;
        }
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("pull unavailable: no daemon");
      }
      this.loadSync(slug);
      if (moved) this.loadChanges(slug);
    },

    // Publish the branch (#320). The OPERATOR's own click is the whole consent
    // — there is no opt-in flag on this path (ADR-0046 amendment) — and every
    // refusal the core models (protected ref, a remote that moved on, a
    // credential the remote rejected) arrives as `{status:"error"}` whose
    // message IS the core's prose. Nothing here remediates a credential: there
    // is no prompt and no credential UI, by decision.
    //
    // Push moves no file, so unlike `syncPull` it reloads the counts only.
    async syncPush(slug) {
      try {
        const reply = await window.WBDaemon.observe("sync.push", { repo: slug });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "push refused"));
        }
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("push unavailable: no daemon");
      }
      this.loadSync(slug);
    },

    // The runid whose stop is in flight — disables the button, so a double-click
    // cannot dispatch two `ralphy stop` children.
    runStopping: null,

    // Does the open project have a live run? This is what flips the toolbar's
    // first control between `run` and `stop` (docs/adr/0054).
    //
    // Derived from the SAME snapshot-backed list `writeLockReason` reads, not
    // from a second signal: the two must agree, or the panel would offer `run`
    // while the lock note beside it says a run holds the repo. Snapshot-derived
    // means it is also self-clearing — the run's document is removed at exit,
    // `runs.dirty` fires, and this returns to `run` with no client bookkeeping.
    runIsLive() {
      return this.projectRuns().length > 0;
    },

    // Ask a live run to stop (docs/adr/0054). This does NOT kill anything: it
    // dispatches a short `ralphy stop`, which writes a request the run itself
    // acts on. The daemon never signals a dispatched child (ADR-0032 §5/§6), and
    // this button is the reason that invariant could survive gaining a stop.
    //
    // There is deliberately no wait: the reply says the request was written, not
    // that the run died. Confirmation arrives on the channel that already
    // exists — the run's snapshot document is removed at exit, `runs.dirty`
    // fires, and the run leaves this panel. Blocking the socket on a tree-kill
    // plus a 5 s output grace would be a UI hang with no ceiling.
    async stopRun(runid) {
      // No runid means the run left the panel between the render and the click —
      // there is nothing to address, and the button is about to become `run`.
      if (!runid || this.runStopping) return;
      // ADR-0032 §6 asks for a strong confirmation, and it is right to: this is
      // the one control here that throws away work in progress. Through the
      // shell's OWN dialog (`askConfirm`), not `window.confirm`: the native box
      // is the browser's chrome — it names the origin, ignores the theme, and
      // blocks the whole page — for the most consequential click in this panel.
      // Same words, same Enter-confirms/Escape-cancels, one design system.
      const ok = await this.askConfirm({
        title: "Stop this run?",
        message:
          "The agent's current issue is abandoned; commits already made stay on the branch.",
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
          this._flashAction("stop requested — the run is unwinding");
        }
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("stop unavailable: no daemon");
      } finally {
        this.runStopping = null;
      }
    },

    // ---- write controls (#318) ------------------------------------------
    // The disabled state is derived from the open repo's LIVE RUN list
    // (`runs.list`, ADR-0047 §9) — already wired and already refreshed by the
    // `runs.dirty` push. It is a HINT, not the authority: the CLI's
    // `guard_run_lock` refuses unconditionally, and a `ralphy triage` holding
    // the lock writes no run snapshot, so a click can still be refused while
    // these controls look enabled. That refusal is flashed verbatim.
    writeLocked() {
      return !!this.writeLockReason();
    },
    writeLockReason() {
      return window.WBChanges.writeLockReason(this.runsByProject[this.openSlug]);
    },
    // The board's label editor, under the SAME lock: `label set` is a run-lock-
    // aware Mutate (mutate.rs), so with a live run every toggle is refused, the
    // optimistic chip snaps back and the operator is left thinking the click was
    // lost. It was the one write control in the shell with no gate.
    labelsLocked() {
      return !!this.labelLockReason();
    },
    labelLockReason() {
      return window.WBChanges.writeLockReason(
        this.runsByProject[this.openSlug],
        "labels are read-only until it finishes",
      );
    },
    // The run verbs reuse the Changes derivation LITERALLY (#331) — a second
    // predicate is the drift #318 avoided, and the gate's whole contract is
    // that it agrees with the controls beside it.
    //
    // CAVEAT, unlike the write controls: `guard_run_lock` is called by
    // changes/config/mutate/sync only. `ralphy run` and `ralphy triage` warn
    // "proceeding anyway" on a live lock (run.rs, triage.rs) and `push` never
    // reads it — runlock.rs calls the lock "a signal, never a mutex". So for
    // these three verbs the CLI does NOT refuse, and this `disabled` is the
    // only thing stopping the click. #331 asked for the gate explicitly; that
    // it hardens a documented signal into a block is a maintainer's call.
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
    // Push's own title (#320). It states the run-lock reason when there is one,
    // exactly as `rowActTitle` does — a disabled control that explains itself
    // is this shell's idiom. Fetch and pull keep their plain titles: they are
    // run-lock-aware in the CLI too, but push is the one that publishes, so it
    // is the one whose inertness has to be legible before the click.
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
        return "nothing is staged — stage a file first";
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

    // Stage / unstage / commit. Each follows `syncFetch`'s exact shape, and each
    // re-reads the list from the daemon on EVERY path — success, refusal and
    // transport throw alike. The list is never moved optimistically: a row that
    // jumped groups on a click the daemon refused would be a lie the operator
    // acts on next.
    async stagePaths(slug, paths) {
      if (!slug || !paths || !paths.length) return;
      try {
        const reply = await window.WBDaemon.observe("changes.stage", { repo: slug, paths });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "stage refused"));
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered.
        if (window.WBMode.isDaemon()) this._flashAction("stage unavailable: no daemon");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    async unstagePaths(slug, paths) {
      if (!slug || !paths || !paths.length) return;
      try {
        const reply = await window.WBDaemon.observe("changes.unstage", { repo: slug, paths });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "unstage refused"));
        }
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("unstage unavailable: no daemon");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    // Discard ONE row's changes (#319) — the only irreversible act on this
    // panel, so it is the only one gated on a confirmation, and the dialog's
    // wording comes from `discardConfirm` (the untracked case is emphatically
    // its own). A cancel makes NO daemon call at all.
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
      try {
        const reply = await window.WBDaemon.observe("changes.discard", {
          repo: slug,
          paths: [entry.path],
        });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "discard refused"));
        }
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("discard unavailable: no daemon");
      }
      this.loadChanges(slug);
      this.loadSync(slug);
    },

    async commitStaged(slug) {
      // Belt to the `toggle` braces: never commit a draft composed for another
      // project, whatever path left the two out of step.
      if (this.commitMsgSlug !== slug) return;
      const message = this.commitMsg.trim();
      if (!slug || !message) return;
      try {
        const reply = await window.WBDaemon.observe("changes.commit", { repo: slug, message });
        if (window.WBFail.isError(reply)) {
          this._flashAction(window.WBFail.message(reply, "commit refused"));
        } else {
          // Cleared on success ONLY: a refused commit must not eat the message.
          this.commitMsg = "";
        }
      } catch {
        if (window.WBMode.isDaemon()) this._flashAction("commit unavailable: no daemon");
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

    switchBranch(name) {
      if (name !== this.branchModal.current) {
        const slug = this.branchModal.slug;
        const p = this.projects.find((x) => this.repoRef(x) === slug);
        const prev = p ? p.branch : null;
        if (p) p.branch = name; // optimistic — the chip updates immediately
        WB.emit("branch-switch", { project: slug, branch: name });
        // Route through the run-lock-aware `branch.switch` Mutate verb (#199); a
        // held-lock refusal comes back `{status:"error",message}` → revert + flash.
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
      const p = this.projects.find((x) => this.repoRef(x) === slug);
      const prevBranch = p ? p.branch : null;
      const prevBranches = p ? [...(p.branches || [])] : null;
      if (p) {
        p.branches = [...(p.branches || []), name];
        p.branch = name; // a fresh branch is checked out onto
      }
      WB.emit("branch-create", { project: slug, name, from });
      this._mutateBranch("branch.create", slug, name, () => {
        if (p) {
          p.branch = prevBranch;
          p.branches = prevBranches;
        }
      });
      this.closeBranchModal();
    },

    // Await a `branch.switch`/`branch.create` Mutate; on a `{status:"error"}`
    // refusal (a held run.lock, per ADR-0036 §6) run `revert` and flash the
    // verb's verbatim message. Silent on a transport throw (static shell).
    async _mutateBranch(verb, slug, name, revert) {
      try {
        const reply = await window.WBDaemon.observe(verb, { repo: slug, name });
        if (window.WBFail.isError(reply)) {
          revert();
          this._flashAction(window.WBFail.message(reply, "branch change refused"));
        }
      } catch {
        // No daemon reachable — leave the optimistic update in place.
      }
    },

    // --- Runs panel -------------------------------------------------------
    // What's running in ralphy for the open project. Data mirrors the fold of
    // the CloudEvents bus (ADR-0019): one entry per `runid`, with the ordered
    // issue queue + per-issue status, the live phase, and the current issue's
    // plan.md. A project can host several concurrent runs → a run picker. See
    // wb-runs.js for the seed + the status/glyph/plan helpers (window.WBRun).
    runsByProject: {},
    // Why the read failed, when it did. Load-bearing: an error must never render
    // as "No active runs" — an empty project and an unreadable one are different
    // facts (ADR-0047 §6).
    runsError: "",
    currentRunId: null,
    // The clock's "now", advanced once a second by the tick in `init` while the
    // panel is open. It is a piece of STATE rather than a `Date.now()` inside the
    // getter because Alpine only re-renders what it can observe changing.
    nowMs: Date.now(),
    // The trail node the operator arrived at from the board (#301) — a marker,
    // not a selection: the run's own state is unchanged by navigating to it.
    trailFocus: null,
    runMenu: false,
    planSection: "",

    // Hydrate runs from the seed: copy each run's plan.md out of its hidden
    // <script> block into a live, mutable `planMd` the fold can update.
    // `file://`-ONLY since #300: the seed runs, the `seed-plan-*` blocks and the
    // fold that mutates them are unreachable in daemon mode, where the panel is
    // fed by `runs.list` + the `runs.dirty` pushes (ADR-0047 §9).
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
            // The demo has no snapshot document, so its steps are seeded from
            // the plan text — the live panel gets them off `plan` (#330).
            steps: window.WBRun.parseSteps(planMd),
            planIssue: r.active ?? null,
            planReadFailed: false,
          };
        });
      }
      this.runsByProject = out;
    },

    // Hydrate the panel from the daemon's `runs.list` (ADR-0047 §9): the live
    // snapshot documents the run processes publish under each repo's `.ralphy/`.
    // Applied by REPLACEMENT — a snapshot is state, not a log. Fires when the
    // panel opens and when the open project changes; the demo keeps its seed.
    async hydrateRuns() {
      if (!window.WBMode.isDaemon()) return;
      const slug = this.openSlug;
      // Clear FIRST: a stale error from the previous project must not outlive
      // the project it described (nor an early return below).
      this.runsError = "";
      if (!slug) return;
      const prevRuns = this.runsByProject[slug] || [];
      const seq = ++this._runsSeq;
      try {
        const reply = await window.WBDaemon.observe("runs.list", { repo: slug });
        // Superseded while this read was in flight → drop it; the newer
        // hydration owns the state (and re-read the same disk anyway).
        if (seq !== this._runsSeq || this.openSlug !== slug) return;
        if (reply?.status !== "ok") {
          this.runsByProject[slug] = [];
          this.runsError = reply?.reason || reply?.message || "could not read runs";
          return;
        }
        this.runsByProject[slug] = (reply.runs || []).map((d) => {
          const run = window.WBRun.fromSnapshot(d);
          // A push arrives on every snapshot write (~every few hundred ms during
          // a run); re-fetching an unchanged plan on each one would blank the
          // viewer between the replacement and the `file.read` reply.
          const prev = prevRuns.find((p) => p.runid === run.runid);
          if (prev && prev.planPath === run.planPath) {
            run.planMd = prev.planMd;
            run.planReadFailed = prev.planReadFailed;
          }
          return run;
        });
        const bad = reply.unreadable || [];
        this.runsError = bad.length
          ? `${bad.length} unreadable run document${bad.length > 1 ? "s" : ""}: ` +
            bad.map((u) => `${u.runid} (${u.reason})`).join(", ")
          : "";
        // Replacement must not yank the operator's selection: keep the selected
        // run while it is still listed, else fall back to the first.
        const listed = this.projectRuns();
        this.currentRunId = listed.some((r) => r.runid === this.currentRunId)
          ? this.currentRunId
          : listed[0]?.runid || null;
        // Only when the panel is showing: a push lands every few hundred ms
        // during a run, and a whole-plan `file.read` nobody can see is pure cost.
        // `toggleRuns()` hydrates on open, so the plan arrives with the panel.
        if (this.runsOpen) await this.loadRunPlan();
      } catch (err) {
        if (seq !== this._runsSeq || this.openSlug !== slug) return;
        // A transport failure is a read failure, not an idle project.
        this.runsByProject[slug] = [];
        this.runsError = String(err?.message || err || "could not reach the daemon");
      } finally {
        // Same defect as `loadRepos` (#332): the panel body is `x-if` on
        // `projectRuns().length`, so its icons exist only once THIS read lands —
        // and `toggleRuns()`'s `$nextTick` already fired, before the fetch.
        this.$nextTick(() => window.lucide?.createIcons());
      }
    },

    // Read the selected run's plan through the confined `file.read` verb — the
    // document carries the plan's repo-relative PATH, never its text. A refusal
    // (no plan yet, too large, deleted between issues) KEEPS the last good text
    // and only flags it (#330): the steps live in the snapshot document, so a
    // failed prose read must not blank the viewer. It is NOT a read failure of
    // the run list either, so `runsError` is untouched.
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
      // A replacement mints new run objects, so a `run` that is no longer the
      // current one belongs to a superseded hydration — its section choice must
      // not overwrite the live one (same out-of-order reason as `_runsSeq`).
      if (this.currentRun() !== run) return;
      // Same reason as the run selection: reassign the section dropdown only when
      // the operator's chosen heading is gone from the reloaded plan.
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
    // The live phase clock. Reading `nowMs` is what subscribes this binding to
    // the 1 s tick — Alpine re-evaluates only the bindings that touch it, so the
    // clock advances without re-rendering the panel around it.
    runClock(run) {
      return window.WBRun.phaseClock(run, this.nowMs);
    },
    // What does not fit on one line: when this phase began, and the run's whole
    // elapsed time (free from `started_at` — no second anchor needed).
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
      // Per-issue, because it IS per-issue: tier routing gives two issues of the
      // same run two different models, and the trail node is the only place that
      // can say which one this issue got.
      const seg = window.WBRun.modelEffort(iss.model, iss.effort);
      if (seg) t += ` · ${seg}`;
      if (iss.blockedBy?.length) t += ` (blocked by ${iss.blockedBy.map((n) => "#" + n).join(", ")})`;
      return t;
    },
    // Clicking an issue node is a read intent — a backend could scroll its log or
    // surface that issue's plan; today it only announces it.
    // Run → board (#301): a trail node opens that issue's detail. The Runs panel
    // closes first — it is `z-index: 150` over the board, and the detail drawer
    // shares the same right edge, so leaving it open would bury the destination.
    // Order matters: `toggleKanban()` resets `kanbanSel`, so it must run BEFORE
    // `openIssue` sets the selection.
    focusIssue(number) {
      WB.emit("run-issue-focus", { project: this.openSlug, runid: this.currentRun()?.runid, issue: number });
      this.runsOpen = false;
      this.trailFocus = null;
      if (!this.kanbanOpen) this.toggleKanban();
      this.openIssue(number);
    },

    // Board → run (#301): the card's run pill opens the Runs panel on THAT run,
    // marking the issue in the trail. The board stays open behind it — `.runs`
    // floats over it — but note the reverse leg (`focusIssue`) must close the
    // panel, because the board's detail drawer shares this right edge.
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
    // The issue the PROSE on screen actually belongs to, read from the plan's own
    // trailer, and whether that is the issue above.
    //
    // Why this gate exists: the steps come from the snapshot document and are
    // keyed by issue (ADR-0047 A1), but the prose is a `file.read` of
    // `.ralphy/plan.md`. Planning deletes that file before the planner rewrites
    // it, and a failed read KEEPS the last text (#330) — so without a key the
    // block renders the PREVIOUS issue's plan under the current issue's chrome.
    // Refusing unkeyed prose also means the block stays empty while a plan is
    // half-written, which is the honest reading: a plan exists once its author
    // says it is finished.
    planProseIssue(run) {
      return window.WBRun.planTrailerIssue(run?.planMd);
    },
    planProseIsCurrent(run) {
      return window.WBRun.planBelongsTo(run?.planMd, this.planIssueWanted(run));
    },
    // Every `##` section except Steps (which is pinned in its own block above) —
    // and none at all while the prose belongs to another issue, so the picker
    // cannot offer a heading out of a stale plan.
    planHeadings(run) {
      if (!this.planProseIsCurrent(run)) return [];
      return window.WBRun.headings(run?.planMd).filter((h) => h.toLowerCase() !== "steps");
    },
    // Render one `##` section as sanitized HTML. Steps no longer pass through
    // here — they render from the snapshot document, not from the prose (#330).
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
    // Why the prose block is empty — the block's single explanation, in the order
    // an operator needs it. A blank block with no sentence reads as a bug, and
    // each of these is a DIFFERENT fact: unreadable, not written yet, or written
    // for another issue.
    proseNote() {
      const run = this.currentRun();
      if (!run) return "";
      const wanted = this.planIssueWanted(run);
      if (this.planProseIsCurrent(run)) {
        // The prose IS this issue's. A stale-read flag still matters: the text on
        // screen is the last good copy of the right plan, not a live read.
        return run.planReadFailed ? "could not read plan.md — showing the last version read" : "";
      }
      const theirs = this.planProseIssue(run);
      if (theirs != null) {
        // The one this whole gate exists for: the plan on disk is the PREVIOUS
        // issue's, and naming both numbers is what makes that legible.
        return wanted != null
          ? `the plan on disk belongs to #${theirs} — waiting for #${wanted}'s plan`
          : `the plan on disk belongs to #${theirs}`;
      }
      if (run.planReadFailed) return "could not read plan.md";
      if (run.phase === "planning") return "writing the plan…";
      if (run.planMd) return "the plan on disk is unfinished — waiting for the planner to finish it";
      return wanted != null ? `no plan read for #${wanted} yet` : "no plan for this issue yet";
    },

    // --- run / triage / push (the daemon verbs) ---------------------------
    // The three remote-trigger verbs (ralphy-daemon dispatch.rs), scoped to the
    // open project. `triage`/`push` are blessed no-arg invocations fired straight
    // onto the seam; `run` opens a modal to enrich it with the agent(s) + branch
    // mode. Faithful flags: --agent (executor, default claude), --plan-agent
    // (optional planner), --branch-mode new|current.
    runOpen: false,
    runsActionMsg: "",
    // A CLI refusal, held until the next verb click clears it (#331). Distinct
    // from `runsActionMsg`, which is a 2.6 s flash shared with 20+ call sites.
    verbError: "",
    // Phase 1 raw merged output of the last daemon-spawned run (wb-daemon.js).
    rawFeed: "",
    // COLLAPSED by default, and re-collapsed by every reset below. The panel's
    // job is the structured view — the trail and the plan — and a feed that
    // opens itself takes up to 30vh of it on every verb click. The head still
    // renders (with a chevron) the moment output exists, so the buffer is one
    // click away and never silent; it is opt-IN, not hidden.
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
    // The feed is dismissible, not just collapsible: dismiss drops the buffer AND
    // returns the box to its collapsed default, so the next run starts from the
    // same quiet state a fresh page does.
    dismissFeed() {
      this.rawFeed = "";
      this.rawFeedOpen = false;
    },
    // What every verb click resets. The feed is cleared, not appended to: its
    // buffer would otherwise concatenate two runs with no separator between them.
    // It is also re-collapsed — one expansion is a decision about THAT output,
    // not a preference the next run inherits.
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
    // triage / push: no params — the verb name is the whole intent (the client
    // never composes a command line, mirroring the daemon).
    fireVerb(verb) {
      this._resetVerbSurface();
      WB.emit("command", { project: this.openSlug, verb });
      this._flashAction(`${verb} requested`);
    },
    // Set from wb-daemon.js on a TERMINAL frame only (non-zero exit, or an error
    // frame); an empty note is a no-op so a clean exit never raises a banner.
    runVerbFailed(msg) {
      if (msg) this.verbError = msg;
    },
    _flashAction(msg) {
      this.runsActionMsg = msg;
      clearTimeout(this._actionTimer);
      this._actionTimer = setTimeout(() => (this.runsActionMsg = ""), 2600);
    },

    // --- inbound event fold (the backend seam) ----------------------------
    // A backend WebSocket would call this per CloudEvent to advance the panel
    // live. Handles the load-bearing types; unknown types are ignored (lossy bus
    // tolerance). Dispatched via `ralphy:run-event` (see the listener below).
    applyRunEvent(ev) {
      // Demo-only since #300: in daemon mode the panel is driven by snapshot
      // REPLACEMENT (`runs.dirty` → `hydrateRuns`), so a client-side fold could
      // only produce state the next push overwrites — or contradicts.
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

    // Demo: walk the selected run forward by synthesizing the next plausible
    // event — tick a step while the active issue has open ones, else close it and
    // start the next pending issue. Proves the live-update seam end to end.
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
    // The backlog as a board: the open project's issues (WB_KANBAN, a backend
    // replaces it from the tracker) placed in four columns by ralphy's own
    // judgment (window.WBKanban). Read-only except labels — the one mutation
    // that moves a card between columns; everything else opens on GitHub. Data
    // is project-scoped like the Runs panel.
    KANBAN: window.WBKanban,
    // Live board data, project-scoped, fed by the daemon's `board.list` Query verb
    // (issue #198). `boardIssues[slug]` = the whole-tracker fold rows adapted to the
    // adapted issue shape; `boardLabels[slug]` = the repo's name→color label map. Both
    // stay empty until `loadBoard()` resolves (or when no daemon answers — no throw).
    boardIssues: {},
    boardLabels: {},
    // Distinct error state, project-scoped, for a `board.list` failure in daemon
    // mode (issue #207 / audit C2) — kept apart from the empty-state "No issues"
    // so a broken tracker connection never reads as "no work to do".
    boardError: {},
    // The open drawer's detail-fetch failure, daemon mode only (#302). One
    // string, not a per-number map: exactly one drawer is open at a time, and an
    // empty drawer must never lie about an issue that has content.
    issueError: null,
    // True while the open drawer's `issue.show` is on the wire. One flag for the
    // same reason `issueError` is one string: exactly one drawer is open.
    issueLoading: false,
    // Refresh bookkeeping (#301). `_boardLoadedAt` is stamped at fold START, so
    // the min-gap measures spacing between fold STARTS: stamping on completion
    // would give a fold slower than the gap zero idle time, and a push arriving
    // the instant it finished would re-fold immediately. It is stamped before
    // any await, so an erroring board throttles exactly like a healthy one.
    // `boardRefreshing` is both the in-flight guard and the control's disabled
    // state; `_boardPending` is what a trigger that arrived mid-fold leaves
    // behind, so a concurrent trigger COALESCES into one follow-up load instead
    // of being silently dropped.
    _boardLoadedAt: 0,
    _boardPending: false,
    _boardBackstop: null,
    _changesBackstop: null,
    boardRefreshing: false,
    // A fold that never answers must not disable the board forever: the daemon
    // awaits the board CLI with no timeout of its own, so a wedged `gh` would
    // leave `boardRefreshing` true (and `.kanban-refresh` disabled) for the
    // page's life. Generous — a real whole-tracker fold makes several calls.
    BOARD_FOLD_TIMEOUT_MS: 90000,
    kanbanSel: null, // the selected issue number → opens the detail drawer
    kanbanFilter: "", // search box (title / #num / body / label)
    kanbanLabel: "__all", // label filter: __all | __none | <label>
    kanbanSort: "num-desc", // Backlog sort (Ready columns keep graph order)

    // --- the repo's ready plan, on the board -------------------------------
    // `.ralphy/plan.md` is not a document about the past: a FINALIZED plan is
    // executed by the next run (the trailer is the resume signal — see
    // `WBRun.planTrailerIssue`). So the board says one exists, shows it, and can
    // throw it away; without that, changing your mind about a planned issue meant
    // deleting a file by hand.
    //
    // `planByProject[slug] = { md, summary }`, replaced on every board load —
    // state, not a log, exactly like `runsByProject`. Read through the SAME
    // confined `file.read` the Runs panel uses, so this adds no read surface.
    // Daemon-only, like the Runs panel's own hydration (#300): the `file://` demo
    // has no repo to read a plan out of.
    planByProject: {},
    planModal: { open: false, issue: null },

    async loadPlan(slug) {
      if (!window.WBMode.isDaemon() || !slug) return;
      try {
        const reply = await window.WBDaemon.observe("file.read", {
          repo: slug,
          path: ".ralphy/plan.md",
        });
        // A refusal is the ORDINARY case here — most repos have no plan sitting
        // around — so it clears the entry rather than raising an error state. A
        // board that shouted about a missing plan would shout on nearly every load.
        const md = reply?.status === "ok" ? reply.content || "" : "";
        this.planByProject[slug] = md ? { md, summary: window.WBRun.planSummary(md) } : null;
      } catch {
        this.planByProject[slug] = null;
      }
    },
    // The open project's plan, or null. `summary.issue` null means the file exists
    // but carries no trailer — a plan still being written, which belongs to nobody
    // yet and is therefore not offered as one.
    openPlan() {
      const held = this.planByProject[this.openSlug];
      return held && held.summary.issue != null ? held : null;
    },
    // The plan for ONE card, or null. The whole affordance keys on this, so a plan
    // is only ever shown against the issue it names.
    planFor(number) {
      const held = this.openPlan();
      return held && held.summary.issue === number ? held : null;
    },
    // Is the issue the plan names still open? A plan left over from a closed issue
    // is residue, not an invitation, and the pill says so.
    planIssueIsOpen() {
      const held = this.openPlan();
      if (!held) return true;
      const iss = this.projectIssues().find((i) => i.number === held.summary.issue);
      // Absent from the board fold: assume open rather than declaring residue —
      // the fold may be filtered or cold, and "leftover" is the stronger claim.
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
    // The head chip's line. It exists for the case the card cannot cover: a plan
    // whose issue is filtered out of the board, or absent from the fold entirely.
    // Without it that plan is invisible AND undiscardable, which is the state this
    // whole slice exists to end.
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
    // The plan's own words, rendered through the same sanitize→markdown pipeline as
    // the Runs panel's prose. The WHOLE document: the operator is deciding whether
    // to keep it, and a summary is not enough to decide on.
    renderPlanDoc() {
      const held = this.openPlan();
      if (!held) return "";
      return DOMPurify.sanitize(marked.parse(held.md));
    },
    // The banner above it. The verdict is the runner's own test — zero open steps —
    // not the heading's claim, so a plan that says "Feasible: yes" with nothing to
    // do still reads as a refusal here, exactly as the loop will treat it.
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
        "delete this plan — the next run will plan this issue from scratch"
      );
    },
    // Throw the plan away. `plan.discard` carries no path (the daemon fixes the
    // target), so there is nothing here to compose. Gated while a run holds the
    // repo: a run owns the plan it is executing, and deleting it mid-flight would
    // take the plan out from under a live executor.
    async discardPlan() {
      const held = this.openPlan();
      if (!held || this.writeLocked()) return;
      const ok = await this.askConfirm({
        title: "Discard this plan?",
        message:
          `The plan for #${held.summary.issue} is deleted. The next run plans that issue ` +
          "again from scratch; the issue itself is untouched.",
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
        // Re-read on EVERY path, refusal included: the panel must show what is on
        // disk now, not what it hoped for.
        await this.loadPlan(slug);
      }
    },

    // The open project's issues — the live board fold (issue #198), project-scoped.
    // Empty until `loadBoard()` populates it (or when no daemon answers).
    projectIssues() {
      return this.boardIssues[this.openSlug] || [];
    },

    // Fetch the whole-tracker board fold for the open project via the daemon's
    // `board.list` Query verb, adapt each row to the issue shape, and cache the
    // rows + the repo label colors under the slug. A no-daemon (static demo) or
    // transport error leaves the board empty (no throw), degrading gracefully.
    async loadBoard() {
      const slug = this.openSlug;
      if (!slug) return;
      // A fold is already in flight: remember that a trigger fired rather than
      // dropping it. Dropping loses a project switch (the in-flight fold writes
      // the OLD slug's rows) and loses a label write (an older fold replaces the
      // rows wholesale, reverting the optimistic edit).
      if (this.boardRefreshing) {
        this._boardPending = true;
        return;
      }
      this.boardRefreshing = true;
      this._boardPending = false;
      this._boardLoadedAt = Date.now();
      // The repo's ready plan rides every board trigger — the manual refresh, the
      // `runs.dirty` push, the backstop — so no new schedule is invented for it.
      // NOT awaited: a plan read must never delay the rows, and it feeds a pill
      // that appears when it appears.
      this.loadPlan(this.openSlug);
      try {
        const reply = await Promise.race([
          window.WBDaemon.observe("board.list", { repo: slug }),
          new Promise((_, rej) =>
            setTimeout(() => rej(new Error("board fold timed out")), this.BOARD_FOLD_TIMEOUT_MS),
          ),
        ]);
        if (window.WBFail.isError(reply)) {
          // Drop any stale board from a prior successful load — else the error
          // banner would sit above data that looks live but isn't (self-review).
          this.boardIssues[slug] = [];
          // Daemon mode: distinct error state (audit C2) + flash the failure.
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
        // Skip a blank color so `labelColor`'s seed-vocabulary fallback engages —
        // a bare "#" would be truthy and mask it.
        for (const l of board.labels || []) {
          if (!l.color) continue;
          colors[l.name] = "#" + String(l.color).replace(/^#/, "");
        }
        this.boardLabels[slug] = colors;
        this.boardError[slug] = null;
        // The fold REPLACED the rows, and fold rows carry `body: ""` — an open
        // drawer would go blank on every refresh (and on arriving from the run
        // trail with the board cold). Re-merge the detail for the open issue.
        if (this.kanbanSel != null) this.loadIssueDetail(this.kanbanSel);
      } catch {
        // Daemon mode: transport error → distinct error state + flash; drop
        // any stale board (see the isError branch above).
        this.boardIssues[slug] = [];
        if (window.WBMode.isDaemon()) {
          this.boardError[slug] = "could not load board";
          this._flashAction?.("could not load board");
        }
        // Demo (static shell): leave it empty, no throw.
      } finally {
        // Every return path above lands here — including the `isError` early
        // return and the timeout rejection — so the guard always clears.
        this.boardRefreshing = false;
        // Exactly ONE follow-up for whatever was coalesced away, or for a
        // project that changed underneath this fold (whose rows landed under the
        // old slug). `_boardPending` is cleared by the recursive call before it
        // awaits, so this settles instead of looping.
        if (this._boardPending || this.openSlug !== slug) {
          this._boardPending = false;
          if (this.openSlug && this.kanbanOpen) this.loadBoard();
        }
      }
    },

    // The one door every refresh trigger goes through (#301): ask the pure
    // predicate (wb-kanban.js), load only on a yes. Nothing here decides policy.
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
    // The slow backstop: one interval for the page's life, the PREDICATE (not
    // the timer) deciding whether an individual tick is allowed to load.
    boardBackstopTick() {
      this.maybeRefreshBoard("backstop");
    },

    // Bridge a CLI fold row (snake_case `blocked_by`, lowercased `reason`) to the
    // issue shape (`blockedBy`, `reason`) `wb-kanban.js` expects. Body +
    // comments are absent from the board fold — the drawer's `issue.show` fills
    // them on open — so seed them empty here.
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

    // The four columns after search + label filter, each ordered for its kind:
    // Backlog by the chosen sort; the two Ready columns by the dependency graph
    // (Kahn); Closed newest-first, grouped later by close reason in the view.
    kanbanColumns() {
      const all = this.projectIssues();
      const K = window.WBKanban;
      const shown = all.filter((i) => K.matches(i, this.kanbanFilter) && K.hasLabelFilter(i, this.kanbanLabel));
      const bucket = { backlog: [], agent: [], human: [], closed: [] };
      for (const i of shown) bucket[K.columnOf(i)].push(i);
      return {
        backlog: K.sortBacklog(bucket.backlog, this.kanbanSort),
        // The Ready columns keep the SERVER's graph order (issue #198): the fold
        // already emitted the Ready subset in `sort_queue_in_graph` order, and
        // bucketing preserves encounter order — so board order == core queue order
        // by construction. A client re-sort (`K.orderGraph`) would diverge (it
        // lacks the full open-set + `## Parent` context) — `orderGraph` stays in
        // wb-kanban.js only for the seed/demo and the parity cross-check.
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

    // The run pill descriptor for a card (the actively-worked issue of a live
    // run), or null. Cross-refs the Runs seed via window.WBRun.
    issueRunning(number) {
      return window.WBKanban.runningFor(number, this.projectRuns());
    },

    // Thin delegations to the faithful helpers (used in the template).
    kanbanColumnOf(i) {
      return window.WBKanban.columnOf(i);
    },
    labelColor(l) {
      // Prefer the repo's real label hex (from the board fold's `labels[]`), then
      // fall back to the seed vocabulary for an unknown label.
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
    // Clicking a card opens a right-hand drawer with the GitHub-style detail:
    // meta, labels (editable), assignees, blocked-by, body + comments, and an
    // Open-on-GitHub link. Selection is by number so a label move (which can
    // change the card's column) keeps the drawer pointed at the same issue.
    selectedIssue() {
      if (this.kanbanSel == null) return null;
      return this.projectIssues().find((i) => i.number === this.kanbanSel) || null;
    },
    openIssue(number) {
      this.kanbanSel = number;
      this.$nextTick(() => window.lucide?.createIcons());
      // Fetch the drawer detail (body + comments + blockers) via the `issue.show`
      // Query verb and merge it into the cached board row — the fold omits body +
      // comments, so this is what makes `renderIssueMd`/`issueBlockers` show real
      // content. No-daemon / error ⇒ the drawer keeps the row's empty body.
      this.loadIssueDetail(number);
    },

    async loadIssueDetail(number) {
      const slug = this.openSlug;
      this.issueError = null;
      // The drawer's own in-flight flag. Set BEFORE the first await so the
      // markup never paints one frame of `_(empty)_` for an issue whose body is
      // still on the wire; cleared only by the NEWEST fetch (`stale()` below),
      // so a superseded load cannot switch the spinner off under a live one.
      this.issueLoading = true;
      // Cross-path invariant (#302): a reply — content OR error — is applied only
      // by the NEWEST fetch, and only while its own project+issue is still the
      // open drawer. The number alone is not enough on either axis: the board
      // fold re-fires this for the open drawer on every refresh (two loads for
      // the same number can be in flight, and a slow failure must not paint over
      // a fast success), and two projects routinely carry the same issue number.
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
        // Success owns the banner too: an older failed load cleared at entry is
        // not enough when this one lands second.
        this.issueError = null;
      } catch {
        // Transport error: the board row keeps its empty body, but the drawer
        // says so rather than reading as an issue with nothing in it.
        fail("could not load issue detail");
      } finally {
        // Every return path above lands here, including the early ones.
        if (!stale()) this.issueLoading = false;
      }
    },
    closeIssue() {
      this.kanbanSel = null;
      this.issueError = null;
      this.issueLoading = false;
    },
    // The real GitHub URL of an issue on the OPEN project — the drawer's editing
    // door (read-only here; edits happen on GitHub). Rebuilt from the project's
    // real `remoteUrl` (#204): parse `owner/repo` from an `https://github.com/o/r`
    // or `git@github.com:o/r` origin (`.git` stripped). `null` when the open
    // project has no GitHub remote, so the markup can hide the link.
    githubUrl(number) {
      const p = this.projects.find((x) => this.repoRef(x) === this.openSlug);
      const url = p && p.remoteUrl;
      if (!url || !url.includes("github.com")) return null;
      const m = url.match(/github\.com[/:]([^/]+)\/(.+?)(?:\.git)?\/?$/);
      if (!m) return null;
      return `https://github.com/${m[1]}/${m[2]}/issues/${number}`;
    },

    // The open blockers of the selected issue (for the drawer's Blocked-by row),
    // each with its live open/closed state in this project.
    issueBlockers(iss) {
      if (!iss || !iss.blockedBy?.length) return [];
      const all = this.projectIssues();
      return iss.blockedBy.map((n) => {
        const b = all.find((x) => x.number === n);
        return { number: n, open: b ? b.state === "open" : false, known: !!b, title: b?.title || "" };
      });
    },

    // Render an issue body / comment as sanitized markdown (marked + DOMPurify,
    // already loaded for the file viewers and the Runs plan).
    renderIssueMd(src) {
      return DOMPurify.sanitize(marked.parse(src || "_(empty)_"));
    },

    // --- the one allowed mutation: labels ---------------------------------
    // Toggling a label is the sole write the board permits — it can move the
    // card to another column. Faithful to the shell's ethos: emit an intent
    // (`issue-label-change`), the daemon does the real `gh` label call; we
    // reflect it optimistically. Everything else is read-only + Open on GitHub.
    KANBAN_LABELS: Object.keys(window.WBKanban.LABELS),
    labelMenuOpen: false,
    // Opening the menu scrolls it into the drawer's viewport: in flow it can no
    // longer be CLIPPED, but on a card near the bottom it can still open below
    // the fold, and a panel you have to hunt for is barely better than a clipped
    // one. Same `scrollIntoView` idiom the trail's arrival marker uses.
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
      // Defence in depth behind the disabled rows: a `:disabled` button is still
      // reachable by keyboard in some browsers, and an optimistic edit that the
      // CLI is certain to refuse is worse than no edit at all.
      if (this.labelsLocked()) return;
      const has = this.hasLabel(iss, label);
      const op = has ? "remove" : "add";
      const prev = [...(iss.labels || [])];
      iss.labels = has ? iss.labels.filter((l) => l !== label) : [...(iss.labels || []), label];
      const slug = this.openSlug;
      WB.emit("issue-label-change", { project: slug, number: iss.number, label, op });
      // Persist via the run-lock-aware `label.set` Mutate verb (#199). Selection
      // stays `kanbanSel` (by number), so the drawer follows the card across a
      // re-column. On a `{status:"error"}` refusal, revert + flash.
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
          // The write landed: re-fold so the card's column (and every other
          // row the tracker may have touched) reflects the server, not just
          // our optimistic edit (#301).
          this.maybeRefreshBoard("label");
        } catch {
          // No daemon reachable — leave the optimistic edit in place.
        }
      })();
    },

    // --- settings modal ---------------------------------------------------
    // A data-driven config panel (schema in wb-settings.js). Values are held in
    // `settings` and every change is an intent on the seam — the daemon persists
    // it via `config.set`/`config.unset`.
    SETTINGS: window.WB_SETTINGS,
    TRISTATE: window.WB_TRISTATE,
    settingsOpen: false,
    // land on the daemon (machine-wide) group first; the per-project sections
    // follow, scoped to whichever repo is open.
    settingsSection: "daemon",
    settings: window.wbSettingsDefaults(),

    // The keys held in this browser profile's view store, not in any ralphy
    // config (wb-settings.js `scope: "client"`).
    CLIENT_KEYS: window.wbClientKeys(),

    openSettings() {
      this.settingsOpen = true;
      this.avatarMenu = false;
      // The client-scoped keys come from the view store — they never travelled
      // to the daemon, so `config.get` below would answer nothing for them.
      const view = window.WBView.read() || {};
      this.settings["consoles.relaunch_on_load"] = view.relaunch === true;
      // Load the open repo's REAL resolved config via the daemon Query verb
      // (config.get). Merge each non-null key over the schema defaults so the
      // panel shows reality; with no repo open the project groups are disabled
      // (index.html `x-show="sec.scope === 'daemon' || openSlug"`).
      if (this.openSlug) {
        WBDaemon.observe("config.get", { repo: this.openSlug })
          .then((reply) => {
            const cfg = reply && reply.status === "ok" ? reply.config : null;
            if (cfg && typeof cfg === "object") {
              for (const k in cfg) {
                // Never round-trip the MASKED secret back into the editable model —
                // a later save would persist the mask over the real token.
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

    // --- usage (read-only, #204) -----------------------------------------
    // A read-only view of `/api/usage` (ADR-0033): the run-record token ledger
    // plus the interactive-session scan. Opened from the account dropdown; a
    // single fetch, no writes. A daemon-mode failure surfaces `usage.error`.
    usageOpen: false,
    usage: { records: [], interactive: [], missing: [], error: "" },
    async openUsage() {
      this.avatarMenu = false;
      this.usageOpen = true;
      this.usage.error = "";
      try {
        const r = await fetch("/api/usage");
        if (r.ok) {
          const data = await r.json();
          this.usage.records = Array.isArray(data.records) ? data.records : [];
          this.usage.interactive = Array.isArray(data.interactive) ? data.interactive : [];
          this.usage.missing = Array.isArray(data.missing) ? data.missing : [];
        } else if (window.WBMode.isDaemon()) {
          this.usage.records = [];
          this.usage.interactive = [];
          this.usage.missing = [];
          this.usage.error = "could not load usage from the daemon";
        }
      } catch {
        if (window.WBMode.isDaemon()) {
          this.usage.records = [];
          this.usage.interactive = [];
          this.usage.missing = [];
          this.usage.error = "could not load usage from the daemon";
        }
      }
      this.$nextTick(() => window.lucide?.createIcons());
    },
    closeUsage() {
      this.usageOpen = false;
    },
    // Sum a record's token buckets into one total for the compact list. A null
    // `tokens` means the vendor keeps no count anywhere (Cursor, ADR-0042 D11) —
    // render that as "unavailable", never as 0, which would read as "spent
    // nothing". A `lower_bound` record is a FLOOR, not the bill (Gemini hides its
    // router's tokens, ADR-0043 D10) — carry the caveat on the number itself, so
    // it cannot be read without it.
    usageTokens(rec) {
      const t = rec && rec.tokens;
      if (!t) return "unavailable";
      const total =
        (t.input || 0) + (t.output || 0) + (t.cache_read || 0) + (t.cache_creation || 0);
      return rec.lower_bound ? "≥ " + total + " (lower bound)" : total;
    },

    // --- about (read-only) ------------------------------------------------
    // The daemon's product card from `/api/about`: the git-published version
    // (embedded at build time, so it tracks the release tag) and the license /
    // source / creator facts — no description. Opened from the account
    // dropdown; a single fetch, no writes. On the static `file://` bundle (no
    // daemon to answer) the seed below stands in so the card is never empty.
    aboutOpen: false,
    about: {
      name: "ralphy",
      version: "",
      license: "GPL-3.0-or-later",
      repository: "https://github.com/paulocorcino/ralphy",
      creator: "Paulo Corcino",
      error: "",
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
      // A client-scoped key stops here: it is this browser's preference, so it
      // goes to the view store and never to `config.set` — which would put a
      // per-browser choice in a repo's settings.json for every client to obey.
      if (this.CLIENT_KEYS.has(key)) {
        if (key === "consoles.relaunch_on_load") window.WBView.patch({ relaunch: value === true });
        WB.emit("setting-change", { project: null, key, value });
        return;
      }
      // Persist through the run-lock-aware config Mutate verbs (config.set /
      // config.unset). An empty/"unset" value clears the key. Only fired for the
      // open repo — a config verb runs in that repo's cwd. `observe` (not
      // `spawn`) closes the socket after the one reply, so a run-lock refusal
      // surfaces instead of being silently discarded (#207 / audit A3).
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
    // The avatar dropdown (Security / Log off) and the Security modal, which
    // mirrors ralphy's real daemon auth model (ADR-0032): an opt-in access
    // token, an optional password (PBKDF2), and TOTP 2FA whose secret is shown
    // exactly once. "Revoke" here = the real "delete the daemon-totp file".
    avatarMenu: false,
    securityOpen: false,
    security: {
      tokenSet: true, // a networked daemon always has one; localhost needs none
      passwordSet: false,
      passwordDraft: "",
      passwordConfirm: "",
      totpEnrolled: false,
      // set only in the one moment after enrolling — the real daemon prints the
      // secret/QR a single time and never again.
      secret: "",
      otpauthUri: "",
      qrHtml: "",
      pendingEnroll: false, // QR shown, awaiting the confirm code (ADR-0032 §C)
      confirmCode: "",
      totpError: "",
      requireLogin: false, // opt-in: mimics a non-loopback bind with TOTP
      policy: "session", // overwritten by probeSession(); demo default keeps login interactive
    },
    // The stored password, kept in-memory purely so the demo login can check it.
    _passwordValue: "",

    async openSecurity() {
      this.securityOpen = true;
      this.avatarMenu = false;
      // Reflect the REAL daemon auth state (GET /api/security/state): access
      // token presence, optional password, TOTP enrolment (require_login is
      // derived from the seed server-side).
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
      // drop the one-time secret when leaving, like the daemon never re-showing it
      this.security.secret = "";
      this.security.otpauthUri = "";
      this.security.qrHtml = "";
      // reset any in-flight enrolment UI; the pending seed survives server-side
      // (mint-once) and re-appears on the next Enroll click.
      this.security.pendingEnroll = false;
      this.security.confirmCode = "";
      this.security.totpError = "";
    },

    async enrollTotp() {
      // POST /api/security/totp/enroll returns the REAL one-time provisioning URI
      // for a PENDING seed (mint-once); the QR is rendered from THAT uri. The
      // factor is NOT armed yet — confirmTotp() proves possession first.
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
      // POST /api/security/totp/confirm verifies the code against the pending
      // seed and arms it on success (ADR-0032 §C). A wrong code prompts a retry.
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
          this.security.totpError = "That code didn't match — try the current one.";
        }
      } catch {
        this.security.totpError = "Daemon unreachable — cannot verify.";
      }
    },

    async cancelEnroll() {
      // Abandon an in-flight enrolment: drop the pending seed server-side too.
      try {
        await fetch("/api/security/totp/revoke", { method: "POST" });
      } catch {}
      this.security.pendingEnroll = false;
      this.security.secret = "";
      this.security.otpauthUri = "";
      this.security.qrHtml = "";
      this.security.confirmCode = "";
      this.security.totpError = "";
    },

    async revokeTotp() {
      // POST /api/security/totp/revoke deletes the live AND pending seeds.
      try {
        await fetch("/api/security/totp/revoke", { method: "POST" });
      } catch {}
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

    async savePassword() {
      const pw = this.security.passwordDraft.trim();
      // Require a matching confirmation before the value ever leaves the field.
      if (!pw || this.security.passwordDraft !== this.security.passwordConfirm) return;
      try {
        const r = await fetch("/api/security/password", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "password=" + encodeURIComponent(pw),
        });
        if (r.ok) this.security.passwordSet = (await r.json()).password_set;
      } catch {}
      this._passwordValue = pw; // demo login still checks locally
      this.security.passwordDraft = "";
      this.security.passwordConfirm = "";
    },
    async clearPassword() {
      try {
        await fetch("/api/security/password", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "password=",
        });
      } catch {}
      this._passwordValue = "";
      this.security.passwordSet = false;
      this.security.passwordDraft = "";
      this.security.passwordConfirm = "";
    },
    async remintToken() {
      // POST /api/security/token/remint rotates the token AND rebuilds the live
      // policy + bumps the session epoch (ADR-0032 amendment §B), so every cookie
      // — including this browser's — is invalidated IMMEDIATELY. Under a gated
      // bind, drop to the login screen so the operator re-authenticates now.
      try {
        await fetch("/api/security/token/remint", { method: "POST" });
      } catch {}
      if (this.security.policy === "session") this.logOff();
    },

    async toggleRequireLogin(ev) {
      // Requiring login is only meaningful once TOTP is enrolled (the session
      // factor). Hit the server-side gate (POST /api/security/require-login), which
      // refuses (400) an enable with no seed — the authoritative AC4 check; the
      // client guard just avoids a doomed round-trip.
      const want = !this.security.requireLogin;
      if (want && !this.security.totpEnrolled) {
        this.security.requireLogin = false;
        if (ev?.target) ev.target.checked = false;
        return;
      }
      let ok = false;
      try {
        const r = await fetch("/api/security/require-login", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "enable=" + want,
        });
        ok = r.ok;
      } catch {
        ok = false;
      }
      if (ok) this.security.requireLogin = want;
      // The checkbox's :checked binding won't re-sync when the bound value
      // didn't actually change (blocked case), so force the DOM to match state.
      if (ev?.target) ev.target.checked = this.security.requireLogin;
      if (!ok) return;
      if (want) {
        // The gate now applies to THIS bind — even loopback (ADR-0032 §A). The
        // daemon swapped to the Session policy and invalidated sessions, so the
        // browser is effectively logged out: drop to the login screen.
        this.security.policy = "session";
        this.closeSecurity();
        this.logOff();
      } else {
        // Gate lifted — re-sync authed/policy from the server.
        await this.probeSession();
      }
    },

    // --- login gate -------------------------------------------------------
    // When locked, a fully-opaque overlay covers the shell so nothing behind is
    // readable — the real daemon simply never renders the app until /api/login
    // succeeds. Here we blank the chrome too (body.locked) to make the point.
    authed: true,
    login: { code: "", password: "", error: "", passwordRequired: false },

    async logOff() {
      this.avatarMenu = false;
      this.securityOpen = false;
      this.settingsOpen = false;
      // The session cookie is HttpOnly — only the server can clear it.
      try {
        await fetch("/api/logout", { method: "POST" });
      } catch {}
      // Localhost/Bearer have no login gate to drop to — dropping `authed`
      // there strands the operator behind a form that posts to a dead end
      // (issue #205, audit finding C3).
      if (this.security.policy === "session") {
        this.authed = false;
        this.login = { code: "", password: "", error: "", passwordRequired: this.login.passwordRequired };
      }
      WB.emit("logoff", {});
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // After a successful login the data endpoints that returned 401 while the UI
    // was gated must be re-fetched — nothing else re-runs them (issue: content
    // didn't refresh after login under require-login). The presence socket
    // self-reconnects on its own 3s backoff, so it's not re-run here.
    rehydrateAfterAuth() {
      this.reposError = "";
      this.loadRepos();
      this.loadIdentity();
      // `/api/agents` is gated too, so the pre-login load left the roster empty:
      // without this the console menu offers only the plain console after login.
      this.loadAgents();
      // `/api/desk` is gated too: the pre-login fetch was refused, so the desk
      // is unread AND unwritable until it is re-read here (issue #327).
      window.WBConsole?.afterLogin();
      // Only now is `file.read` allowed: restoring the tabs before login would
      // have each one refused and immediately closed (issue #339).
      this.restoreView();
    },

    async submitLogin() {
      const code = (this.login.code || "").trim();
      try {
        const body = new URLSearchParams({ code });
        if (this.login.passwordRequired || this.security.passwordSet) {
          body.set("password", this.login.password || "");
        }
        const res = await fetch("/api/login", {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: body.toString(),
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
        // 6-digit fallback (M4) — that fallback exists only for the `file://`
        // demo. In daemon mode, surface the failure and stop.
        if (!window.WBMode.isDemo()) {
          this.login.error = "Daemon unreachable — cannot verify.";
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
    // The Consoles tab is permanent; file tabs are appended and closable.
    // The adapter roster comes from the daemon (`/api/agents`), never from a
    // list here: onboarding a vendor must not need a frontend change (#304).
    // `agents` carries id + presence signal for the run dialog's pickers.
    agents: [],
    roster: [],
    agentMenu: false,
    // The Go-to picker (issue #337): its rows are a SNAPSHOT taken when the menu
    // opens, because the window set lives in the DOM (the stage), not here.
    windowMenu: false,
    windowList: [],
    // The fence picker (issue #343), a snapshot on the same terms — the fences
    // live in the DOM too, and re-opening the menu is what "without a reload"
    // means here.
    fenceMenu: false,
    fenceItems: [],
    consoleCount: 0,
    // The stage extent, mirrored for the frame's footer pill (issue #338). The
    // plane is invisible until it is measured, so the pill is what makes "the
    // stage grew" legible without a devtools inspection.
    stageW: 0,
    stageH: 0,
    // The design-system confirm dialog (replaces window.confirm). `askConfirm`
    // opens it and returns a promise resolved by the operator's choice.
    confirmModal: {
      open: false,
      title: "",
      message: "",
      confirmLabel: "Confirm",
      cancelLabel: "Cancel",
      danger: false,
    },
    _confirmResolve: null,
    // The design-system prompt dialog (replaces window.prompt), same shape as
    // the confirm above. `askPrompt` opens it and resolves the typed string, or
    // null when the operator backs out. Naming a new file is the one gesture the
    // workbench cannot complete without a word from the operator, so it gets a
    // real dialog rather than the browser's — which is unstyled, is suppressible
    // per-origin by a single "prevent this page from creating more dialogues"
    // tick, and never appears at all in a detached popup that has lost focus.
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
    // The Consoles tab wears the SAME terminal glyph as the New-console button
    // and the rows in its menu — one picture for one thing. It used to be a
    // robot, which named the agents rather than the plane they run on.
    tabs: [{ id: "consoles", kind: "consoles", title: "Consoles", icon: "bi bi-terminal", closable: false }],
    active: "consoles",

    // Projects carry a *nested* file tree (folder → children), the shape a
    // backend would deliver as JSON. `state` is daemon reachability (the dot);
    // `remote` is provenance — a GitHub-backed repo vs one that lives only on
    // this disk. Icons are resolved at mount time. `loadRepos()` overwrites
    // this seed with the real registry at init; it survives only as the
    // file:// standalone fallback (no daemon to fetch from).
    projects: [
      {
        slug: "lingopilot",
        branch: "main",
        // local branches the picker offers (impl: `git branch`, current marked)
        branches: [
          "main",
          "feat/xterm-v6-webgl",
          "feat/chat-streaming",
          "feat/onboarding-flow",
          "fix/auth-redirect",
          "fix/db-pool-leak",
          "chore/deps-bump",
          "chore/ci-cache",
          "experiment/rag-eval",
        ],
        dirty: true, // uncommitted changes → the modal warns before checkout
        state: "live",
        remote: "github",
        tree: [
          {
            title: "src",
            folder: true,
            expanded: true,
            children: [
              { title: "app", folder: true, children: [{ title: "page.tsx" }, { title: "layout.tsx" }] },
              { title: "components", folder: true, children: [{ title: "Chat.tsx" }, { title: "Sidebar.tsx" }] },
              { title: "lib", folder: true, children: [{ title: "db.ts" }, { title: "auth.ts" }] },
            ],
          },
          { title: "prisma", folder: true, children: [{ title: "schema.prisma" }] },
          { title: "package.json" },
          { title: "next.config.ts" },
          { title: "tsconfig.json" },
          { title: "logo.png" },
          { title: "README.md" },
        ],
      },
      {
        slug: "fincal",
        branch: "feat/triage",
        branches: ["main", "feat/triage", "feat/reconcile", "fix/csv-import"],
        dirty: false,
        state: "idle",
        remote: "github",
        tree: [
          { title: ".ralphy", folder: true, children: [{ title: "plan.md" }, { title: "triage-draft.json" }] },
          {
            title: "docs",
            folder: true,
            children: [
              { title: "adr", folder: true, children: [{ title: "0001-vocabulary.md" }] },
              { title: "issues", folder: true, children: [] },
            ],
          },
          { title: "src", folder: true, children: [{ title: "index.ts" }, { title: "styles.css" }] },
          { title: "CONTEXT.md" },
          { title: "package.json" },
        ],
      },
      {
        slug: "ralphy",
        branch: "feat/xterm-v6-webgl",
        branches: ["main", "feat/xterm-v6-webgl", "feat/daemon-mode", "feat/assignee-filter"],
        dirty: false,
        state: "idle",
        remote: "github",
        tree: [
          {
            title: "crates",
            folder: true,
            children: [
              { title: "ralphy-cli", folder: true, children: [{ title: "main.rs" }] },
              { title: "ralphy-core", folder: true, children: [{ title: "lib.rs" }] },
              { title: "ralphy-daemon", folder: true, children: [{ title: "protocol.rs" }, { title: "dispatch.rs" }] },
            ],
          },
          { title: "docs", folder: true, children: [{ title: "adr", folder: true, children: [{ title: "0035-daemon-ui-visual-language.md" }] }] },
          { title: "Cargo.toml" },
        ],
      },
      {
        slug: "bioledger",
        branch: "main",
        branches: ["main", "wip/ocr-tuning"],
        dirty: false,
        state: "offline",
        remote: "local", // never pushed anywhere — lives only on this disk
        tree: [
          { title: "src", folder: true, children: [{ title: "ocr.ts" }] },
          { title: "tests", folder: true, children: [{ title: "ocr.test.mjs" }] },
          { title: "notes.md" },
          { title: "package.json" },
        ],
      },
    ],

    // --- accordion --------------------------------------------------------
    toggle(ref, row) {
      this.openSlug = this.openSlug === ref ? null : ref;
      this.loadAgents(this.openSlug);
      // a selected issue belongs to the project that was open — closing or
      // switching projects must drop the Kanban detail drawer (its selection is
      // now stale/absent), else the empty drawer lingers on the right.
      this.kanbanSel = null;
      this.trailFocus = null; // ditto: the marker named an issue of the old project
      // …and so does an unsent commit message (#318): it was composed FOR the
      // project that was open, and one click in the next project would land it
      // on the wrong repo. Dropped whenever the open project changes.
      if (this.commitMsgSlug !== this.openSlug) {
        this.commitMsg = "";
        this.commitMsgSlug = this.openSlug;
      }
      // …and so does a verb refusal (#331): it named the OLD project's CLI, and
      // a terminal frame can land long after the click, so the banner would
      // otherwise describe a repo that is no longer on screen. It is sticky
      // WITHIN a project, not across one — and while locked both verbs that
      // clear it are disabled, so this is the only path that retires it.
      this.verbError = "";
      this.$nextTick(() => {
        this.destroyTree();
        if (this.openSlug) this.mountTree();
        // The runs subscription follows the same open/close path as the tree, so
        // closing a project (openSlug → null) drops BOTH sockets (#300).
        this.destroyRunsSub();
        this.mountRunsSub();
        // …and so does the run-completion nudge socket (#310).
        this.destroyChangesSub();
        this.mountChangesSub();
        // Refresh the board fold for the newly-open project (issue #198) so the
        // Kanban + drawer read this project's live tracker, not a stale slug —
        // but only when the board is actually OPEN (#301): the fold spawns a CLI
        // that makes several tracker calls, and nobody is looking at it.
        // `toggleKanban()` loads on open, so the closed case loses nothing.
        if (this.openSlug && this.kanbanOpen) this.loadBoard();
        // point the Runs panel at this project's first run + its first section
        this.currentRunId = this.projectRuns()[0]?.runid || null;
        this.planSection = this.planHeadings(this.currentRun())[0] || "";
        // …then re-read the newly-open project's live runs (ADR-0047 §9).
        if (this.openSlug) this.hydrateRuns();
        // The Changes count is scoped to the open project (#307).
        if (this.openSlug) this.loadChanges(this.openSlug);
        if (this.openSlug) this.loadSync(this.openSlug);
        window.lucide?.createIcons();
      });
    },

    // The status dot's colour = the project's daemon-reachability right now:
    //   live    → green  (a session/daemon is active there)
    //   idle    → grey   (registered & reachable, but stopped) — the default
    //   offline → red    (unreachable path: moved/deleted)
    // This is orthogonal to `remote` (GitHub vs local-only): a local-only repo
    // can be live, and a GitHub repo can be offline. A real daemon would derive
    // this the way the live UI does (repo.reachable in the daemon's /api/repos).
    dotClass(state) {
      return state === "live" ? "live" : state === "offline" ? "offline" : "";
    },

    // Is this node a directory? Wunderbaum has no isFolder() on the node, and
    // reading `node.folder` does NOT work: the tree copies source keys it does
    // not itself define into `node.data`, so our `folder:true` lands at
    // `node.data.folder` and `node.folder` is forever `undefined`. Nor is
    // `node.children` a fallback on its own — a lazy folder holds `null` there
    // until it is expanded, and an empty one still holds `null` afterwards.
    // Reading either alone made EVERY collapsed folder answer "file", which is
    // why the tree offered no New file/New folder, watched no subdirectory, and
    // tried to open a folder as bytes on double-click.
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

    // Attach an `icon` to every *file* node (folders keep the theme default),
    // recursively, without mutating the source shape the backend sent.
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

      this._tree = new mar10.Wunderbaum({
        element: host,
        header: false,
        // Served over a daemon: seed the root level from `tree.list` (folders
        // marked `lazy` so expanding fetches their children on demand) and fall
        // back to the static seed if the read fails. Under `file://` (no
        // backend) keep the static tree.
        source: this.useDaemonTree()
          ? this.loadTreeLevel("").catch(() => this.withIcons(project.tree))
          : this.withIcons(project.tree),
        lazyLoad: (e) => this.loadTreeLevel(this.relPath(e.node)),
        edit: {
          trigger: ["F2", "macEnter"],
          // A committed rename is an intent, not a mutation done here.
          apply: (e) => {
            this.emit("rename", e.node, { from: e.oldValue, to: e.newValue });
            return true; // let the tree reflect it optimistically
          },
        },
        // Live watch-set (#196): watch a folder's dir when it expands, unwatch on
        // collapse, so the daemon watches only what is on screen (the expanded set).
        expand: (e) => {
          if (!this.isFolder(e.node)) return;
          const rel = this.relPath(e.node);
          if (e.flag) this._treeSub?.watch(rel);
          else this._treeSub?.unwatch(rel);
        },
        // Double-click / Enter on a leaf = "open this file".
        dblclick: (e) => {
          if (!this.isFolder(e.node)) this.openFile(e.node);
          return false;
        },
      });

      // One `/ws/tree` subscription per open project; the root is always watched
      // (the top level is visible whenever a project is open). A `tree.dirty` push
      // refetches only the affected, still-expanded subtree (see `onTreeDirty`).
      if (this.useDaemonTree() && window.WBDaemon?.subscribeTree) {
        this._treeSub = WBDaemon.subscribeTree(this.openSlug, (rel) => this.onTreeDirty(rel));
        this._treeSub.watch("");
      }

      // Right-click anywhere in the tree → our own context menu. Empty space
      // below the rows resolves to NO node, which is the repo root, not a
      // no-op: it is the only gesture that can create a top-level entry.
      host.addEventListener("contextmenu", (ev) => {
        const node = mar10.Wunderbaum.getNode(ev);
        ev.preventDefault();
        node?.setActive();
        this.showMenu(ev.clientX, ev.clientY, node || null);
      });
    },

    // A real daemon backs the tree only when NOT loaded from `file://` (the
    // static-demo case, which has no `/ws/command` to talk to).
    useDaemonTree() {
      return window.WBMode.isDaemon() && !!window.WBDaemon?.observe;
    },

    // One directory level from the daemon (`tree.list`), mapped to Wunderbaum
    // node shape: folders lazy so they fetch their own children on expand.
    loadTreeLevel(rel) {
      return WBDaemon.observe("tree.list", { repo: this.openSlug, path: rel }).then((reply) => {
        if (!reply || reply.status !== "ok" || !Array.isArray(reply.entries)) return [];
        return reply.entries.map((en) =>
          en.dir
            ? { title: en.name, folder: true, lazy: true }
            : { title: en.name, icon: this.fileIcon(en.name) },
        );
      });
    },

    // Fetch a file's real bytes via `file.read`; on refusal surface the daemon's
    // reason (binary / too large / not found) and close the just-opened tab.
    // Returns `null` when refused so the caller skips the viewer.
    fetchContent(project, path, ftype) {
      if (!this.useDaemonTree()) return Promise.resolve(fakeContent(path, ftype));
      // An image is a different read (`file.image`, ADR-0049) whose "content" is
      // a `data:` URL, not text. Same refusal shape: surface the reason, close
      // the tab, hand back `null`.
      if (ftype === "image") {
        return WBDaemon.readImage(project, path, (reason) => {
          WB.emit("open-refused", { project, path, reason });
          this._flashAction?.(reason);
          this.closeTab(`file:${project}:${path}`);
        }).catch(() => {
          WB.emit("open-refused", { project, path, reason: "transport" });
          this._flashAction?.("read failed");
          this.closeTab(`file:${project}:${path}`);
          return null;
        });
      }
      return WBDaemon.observe("file.read", { repo: project, path })
        .then((reply) => {
          if (!window.WBFail.isError(reply)) return reply.content;
          const reason = window.WBFail.message(reply, "refused");
          WB.emit("open-refused", { project, path, reason });
          this._flashAction?.(reason);
          this.closeTab(`file:${project}:${path}`);
          return null;
        })
        .catch(() => {
          // Daemon mode: a transport drop must NOT fall back to `fakeContent`
          // (C1) — surface the failure and close the tab, mirroring refusal.
          WB.emit("open-refused", { project, path, reason: "transport" });
          this._flashAction?.("read failed");
          this.closeTab(`file:${project}:${path}`);
          return null;
        });
    },

    // A `tree.dirty` nudge for `rel`: refetch that one directory level IF it is
    // currently on screen (the root, or an expanded folder). A nudge for a
    // collapsed/absent dir is DROPPED — the change is invisible, so re-listing it
    // would be wasted traffic (ADR-0036 §4).
    onTreeDirty(rel) {
      const tree = this._tree;
      if (!tree) return;
      const node = rel === "" ? tree.root : this.findFolderByRel(rel);
      if (!node) return; // not in the tree → invisible, drop
      if (rel !== "" && !node.expanded) return; // collapsed → invisible, drop
      // Reconcile this level in place (no duplication), then freshen any open
      // tabs that live in this directory (A6). Returns the promise so callers
      // that need to sequence after a settled tree (tests) can await it. A
      // reconcile failure (e.g. a transport-dropped `tree.list`) must NOT strand
      // open viewers stale nor surface an unhandled rejection — swallow it and
      // still refresh.
      return this.reconcileLevel(node, rel)
        .catch(() => {})
        .then(() => this.refreshOpenViewers(rel));
    },

    // Re-list one directory level and reconcile its children WITHOUT duplicating
    // nodes (A5) while preserving descendant expansion + the active selection
    // (criterion 2). `node.load` appends, so we `removeChildren()` first — which
    // also destroys descendant + active nodes — then explicitly re-expand and
    // re-activate by captured rel-path after the reload (the re-expansion cascade
    // re-triggers lazy loads).
    async reconcileLevel(node, rel) {
      // Reentrancy guard: two nudges for the same dir (the watcher plus a rapid
      // second write) must NOT run overlapping removeChildren()+load() passes —
      // `load` appends, so concurrent passes double the children. Coalesce: if a
      // pass is in flight for `rel`, mark it pending and let the running pass
      // re-run once when it finishes.
      this._reconciling ||= new Set();
      this._reconcilePending ||= new Set();
      if (this._reconciling.has(rel)) {
        this._reconcilePending.add(rel);
        return;
      }
      this._reconciling.add(rel);
      try {
        await this._reconcileOnce(node, rel);
      } finally {
        this._reconciling.delete(rel);
      }
      if (this._reconcilePending.delete(rel)) {
        const again = rel === "" ? this._tree?.root : this.findFolderByRel(rel);
        if (again) await this.reconcileLevel(again, rel);
      }
    },

    async _reconcileOnce(node, rel) {
      const expandedRels = [];
      node.visit((n) => {
        if (this.isFolder(n) && n.expanded) expandedRels.push(this.relPath(n));
      });
      const activeRel = this.relPath(this._tree.getActiveNode?.() || null) || null;

      // Resolve the fresh level BEFORE touching the tree: `node.load` given a
      // PROMISE leaves stale children in place and appends (children double on a
      // second nudge — Wunderbaum quirk); loading a resolved ARRAY after
      // `removeChildren()` replaces cleanly and avoids an empty-tree flicker
      // during the fetch.
      const source = await this.loadTreeLevel(rel);
      node.removeChildren();
      await node.load(source);
      // A non-root reconcile targets an EXPANDED folder (onTreeDirty only calls
      // us for one), but `load` leaves the reloaded node collapsed — leaving it
      // so would make the NEXT nudge for this dir hit the `!expanded` drop guard
      // and silently stop refreshing. Re-expand the node itself.
      if (rel !== "" && !node.expanded) await node.setExpanded(true);

      // Shallow-first so a parent exists before its child re-expands. Match by
      // rel path (NOT findFolderByRel): a freshly reloaded folder is collapsed
      // and lazy, so it has neither `folder` nor loaded `children` yet — the
      // isFolder() filter would miss it.
      expandedRels.sort((a, b) => a.split("/").length - b.split("/").length);
      for (const r of expandedRels) {
        const f = this._tree.findFirst((n) => this.relPath(n) === r);
        if (f && !f.expanded) await f.setExpanded(true);
      }
      if (activeRel) await this.revealRel(activeRel);
    },

    // After a directory nudge, re-read any open tab whose file lives in `rel` and
    // push the fresh bytes to its viewer (A6). Daemon mode only; a non-ok or
    // transport failure is dropped silently — the tab keeps its bytes (C1: no
    // fabricated content).
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
        // An image tab re-reads through its OWN verb: `file.read` would refuse
        // its bytes, and the drop-on-failure rule below would then make an image
        // the one viewer that never refreshes (ADR-0049 §1).
        const fresh =
          t.kind === "image"
            ? WBDaemon.readImage(t.project, t.path)
            : WBDaemon.observe("file.read", { repo: t.project, path: t.path }).then((reply) =>
                reply?.status === "ok" ? reply.content : null,
              );
        reads.push(
          fresh
            .then((content) => {
              if (content != null) WBViewer.externalChange(t.id, content);
            })
            .catch(() => {}),
        );
      }
      // Return the settled batch so a caller (a test, a chained nudge) can await
      // a fully-refreshed set of viewers rather than racing the reads.
      return Promise.all(reads);
    },

    // Bring `rel` on screen and select it: expand every ancestor shallow-first,
    // then activate the node itself. Returns the node, or `null` when the path is
    // not in the tree. The one reveal primitive — the reconcile path's
    // re-activation and the create path both go through it, so a collapsed parent
    // is never the reason a just-created entry stays invisible.
    //
    // Matches by rel path, NOT findFolderByRel: a freshly loaded folder is lazy
    // and collapsed, so it carries neither `folder` nor loaded children yet and
    // the isFolder() filter would miss it.
    async revealRel(rel) {
      const tree = this._tree;
      if (!tree || typeof rel !== "string" || rel === "") return null;
      const parts = rel.split("/");
      // Ancestors only — expanding the target itself is the caller's business
      // (a revealed FILE has nothing to expand).
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

    // The expanded folder node whose rel path is `rel`, or `null` if none is
    // mounted (so a nudge for an off-screen dir drops).
    findFolderByRel(rel) {
      return this._tree?.findFirst((n) => this.isFolder(n) && this.relPath(n) === rel) || null;
    },

    // The open project's run-snapshot subscription (#300, ADR-0047 §9). Daemon
    // mode only — the `file://` demo has no socket to push over.
    mountRunsSub() {
      if (!window.WBMode.isDaemon() || !window.WBDaemon?.subscribeRuns || !this.openSlug) return;
      // A snapshot change also means the tracker may have moved (an issue closed,
      // a label set by the run) — so the same push nudges the board (#301). The
      // predicate coalesces it: pushes arrive every few hundred ms, board folds
      // spawn a CLI.
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

    // The open project's run-completion subscription (#310, ADR-0036 amendment).
    // The socket carries EVERY repo's nudge, so the filter is here: a nudge for
    // another project must not re-read this one's count.
    mountChangesSub() {
      if (!window.WBMode.isDaemon() || !window.WBDaemon?.subscribeChanges || !this.openSlug) return;
      this._changesSub = window.WBDaemon.subscribeChanges(this.openSlug, (frame) => {
        // Optional-chained like the mount guard above: a frame arriving before
        // (or without) wb-changes.js must not throw inside `onmessage` and kill
        // the nudge path for this connection.
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
      try {
        this._tree?.destroy?.();
      } catch {}
      this._tree = null;
      document.querySelectorAll(".wb-host").forEach((h) => (h.innerHTML = ""));
      this.hideMenu();
    },

    // --- opening a file into a tab ----------------------------------------
    // Decide the viewer, refuse binaries, and (for text) open — or focus — a
    // tab. The `open` intent still fires for the backend regardless.
    openFile(node) {
      const path = this.relPath(node);
      const ftype = classify(node.title);
      this.emit("open", node, { ftype });
      if (ftype === "binary") {
        // Flash it too, not just the seam event: the daemon-side refusals all
        // reach the operator, and a click that silently does nothing reads as a
        // broken tree rather than a refused file.
        WB.emit("open-refused", { project: this.openSlug, path, reason: "binary" });
        this._flashAction?.("binary");
        return;
      }
      this.openTab({ project: this.openSlug, path, title: node.title, ftype });
    },

    // `content` is optional: opening from the tree synthesises it, re-attaching
    // a detached popup passes the current (possibly edited) bytes back in.
    openTab({ project, path, title, ftype, content }) {
      const id = `file:${project}:${path}`;
      if (this.tabs.some((t) => t.id === id)) {
        this.activate(id);
        return;
      }
      const icon =
        ftype === "markdown"
          ? "bi bi-file-earmark-text"
          : ftype === "image"
            ? "bi bi-file-earmark-image"
            : "bi bi-file-earmark-code";
      this.tabs.push({ id, kind: ftype, title, path, project, icon, closable: true });
      this.active = id;
      this.persistView();
      this.$nextTick(() => {
        // A re-attach passes its (possibly edited) bytes in; a fresh open fetches
        // the real file via the daemon (`file.read`), falling back to the seed.
        const bytes = content != null ? Promise.resolve(content) : this.fetchContent(project, path, ftype);
        bytes.then((body) => {
          if (body == null) return; // refused: fetchContent surfaced the reason
          WBViewer.open({ id, project, path, ftype, content: body });
          WBViewer.setActive(id);
          window.lucide?.createIcons();
        });
      });
    },

    // --- opening a Changes row into a diff tab ----------------------------
    // HEAD on one side, the working tree on the other, so reviewing what the
    // agent wrote is one gesture away from noticing it (#311). Read-only: no
    // commit, no discard, no staging. Monaco computes the diff from the two
    // texts, so nothing here — and nothing in the daemon — produces a patch.
    openDiff(project, entry) {
      const t = window.WBChanges.diffTarget(entry, project);
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
        icon: "bi bi-file-earmark-diff",
        closable: true,
      });
      this.active = t.id;
      WB.emit("open-diff", { project, path: t.workingPath });
      this.$nextTick(() => {
        // Latched: both sides share this, and a path refused on BOTH (a binary
        // one) would otherwise flash twice for one gesture.
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
            // A refusal on EITHER side aborts: half a diff would read as
            // "no changes" on the side that resolved.
            if (head == null || work == null) return;
            // Closed during the two round trips? The tab is already gone, and
            // WBViewer holds no record to close — mounting now would build a
            // visible pane with no tab to close it, leaking an editor and two
            // models nothing can reach.
            if (!this.tabs.some((x) => x.id === t.id)) return;
            WBViewer.open({
              id: t.id,
              project,
              path: t.workingPath,
              ftype: "diff",
              content: work,
              original: head,
            });
            WBViewer.setActive(t.id);
            window.lucide?.createIcons();
          })
          .catch(() => refuse("diff read failed"));
      });
    },

    // The diff's HEAD side. An added/untracked path has none — it diffs against
    // emptiness, which is the whole point of reviewing a new file.
    diffHeadSide(project, t, refuse) {
      if (t.headAbsent) return Promise.resolve("");
      if (!this.useDaemonTree()) {
        return Promise.resolve(fakeContent(t.headPath, "code"));
      }
      return WBDaemon.observe("blob.read", {
        repo: project,
        revision: "head",
        path: t.headPath,
      }).then((reply) => {
        if (window.WBFail.isError(reply)) return refuse(window.WBFail.message(reply, "refused"));
        const blob = reply.blob || {};
        if (blob.status === "present") return blob.content;
        if (blob.status === "absent") return "";
        return refuse(blob.reason || "refused");
      });
    },

    // The diff's working side. A `not found` is NOT a refusal here: the row may
    // be stale (the file deleted between the list and the click), and a stale row
    // must still diff against emptiness rather than close the tab.
    diffWorkSide(project, t, refuse) {
      if (t.workingAbsent) return Promise.resolve("");
      if (!this.useDaemonTree()) {
        // The static demo has no daemon; a synthesised one-line delta keeps the
        // pane demonstrable without fabricating anything in daemon mode.
        return Promise.resolve("// (demo) edited line\n" + fakeContent(t.workingPath, "code"));
      }
      // Deliberately NOT `fetchContent`: it collapses every refusal to `null`, so
      // a stale row would be indistinguishable from a binary one, and it closes
      // the `file:` tab id rather than this diff's.
      return WBDaemon.observe("file.read", { repo: project, path: t.workingPath }).then((reply) => {
        if (!window.WBFail.isError(reply)) return reply.content;
        const reason = window.WBFail.message(reply, "refused");
        return reason === "not found" ? "" : refuse(reason);
      });
    },

    // Pop a file tab out into a standalone browser popup, so it can be read
    // side-by-side with an agent console in the main window. The descriptor is
    // handed over via a shared same-origin global (no serialisation limits); the
    // in-app tab then closes and we drop back to the Consoles workspace.
    detachFile(desc) {
      const id = `file:${desc.project}:${desc.path}`;
      // The descriptor is handed over by postMessage, NOT in the URL hash. A
      // hash is readable by whoever composed the link, so a bare
      // `detached.html#<json>` let anyone render content of their choosing on
      // the daemon's own origin. The popup instead asks its opener for the
      // descriptor with `targetOrigin = location.origin`, which is the whole
      // discriminator: a page on any other origin never receives that request,
      // so it can never answer it. Passing the bytes (rather than re-reading the
      // file) is what keeps unsaved edits alive across a detach.
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
      this.$nextTick(() => {
        WBViewer.setActive(this.active === "consoles" ? null : this.active);
        window.lucide?.createIcons();
        // A console opened/reattached while another tab was active measured 0×0
        // (its tab was display:none); refit now that the Consoles tab is visible.
        if (id === "consoles") window.WBConsole?.refitAll?.();
      });
      this.persistView();
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
    // The tabs half of `wb.view.v1`; `wb-console.js` owns the offset half and
    // `patch` merges, so neither clobbers the other. Only `file:` tabs are
    // stored: a `diff:` tab's two sides are derived from LIVE git state
    // (`WBChanges.diffTarget`), so restoring one would resurrect a review of a
    // diff that may no longer exist.
    // Set while `restoreView` is opening the stored tabs. `fetchContent` closes
    // a tab whose read fails, and a daemon restart during the restore burst
    // would otherwise have those closes REWRITE the store — deleting the very
    // tabs being restored, with no operator action and no way back.
    _restoring: false,
    persistView() {
      if (this._restoring) return;
      const files = this.tabs
        .filter((t) => t.id.startsWith("file:"))
        .map((t) => ({ project: t.project, path: t.path, title: t.title, kind: t.kind }));
      // A stored `active` naming a tab this store does not carry (a diff tab, or
      // one that just closed) would restore to a tab that never opens, leaving
      // the canvas blank — degrade to Consoles instead.
      const alive =
        this.active === "consoles" ||
        files.some((f) => `file:${f.project}:${f.path}` === this.active);
      window.WBView?.patch({ tabs: files, active: alive ? this.active : "consoles" });
    },

    _viewRestored: false,
    // Latched, and AUTH-GATED by its callers: under `require-login` a pre-login
    // `file.read` is refused and `fetchContent` closes the tab — and that close
    // persists the loss, so a restore attempted too early destroys the very
    // state it is restoring. Same trap `deskLoaded` guards for the desk.
    restoreView() {
      if (this._viewRestored) return;
      this._viewRestored = true;
      const stored = window.WBView?.read();
      if (!stored) return;
      this._restoring = true;
      try {
        for (const t of stored.tabs || []) {
          if (!t || !t.project || !t.path) continue;
          this.openTab({ project: t.project, path: t.path, title: t.title || t.path, ftype: t.kind });
        }
        const want = stored.active;
        this.activate(want && this.tabs.some((t) => t.id === want) ? want : "consoles");
      } finally {
        // The reads themselves are async: hold the suppressor past the microtask
        // queue so a refusal that lands in the same turn cannot rewrite the
        // store either. A LATER close (an operator gesture) persists normally.
        setTimeout(() => {
          this._restoring = false;
        }, 3000);
      }
    },

    // --- consoles (the Consoles tab) ----------------------------------------
    // The "New console" menu: the daemon's roster folded against the live
    // sessions and the open repo (wb-agents.js), plus a plain console (no agent
    // — a shell in the repo dir) the fold pins LAST. Each row carries an
    // Alt+Shift+<digit> accelerator: Alt+Shift lives outside the browser's
    // reserved combos on Windows/Linux/macOS, and the digits are matched by
    // physical key (e.code), so they fire regardless of layout or the glyph
    // macOS' Option produces. Console is Alt+Shift+0 (last, the "zero").
    liveSessions: [],
    consoleItems() {
      return window.WBAgents.menuRows({
        roster: this.roster,
        sessions: this.liveSessions,
        openSlug: this.openSlug,
      });
    },
    isMac: /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || ""),
    shortcutLabel(digit) {
      return this.isMac ? `⌥⇧${digit}` : `Alt+Shift+${digit}`;
    },
    // `opts.fresh` is the row's secondary "+" button: launch another console for
    // this agent even though one is live, so a deliberate second console stays
    // reachable. Without it, `action === "attach"` would remove that capability.
    openConsoleItem(item, opts = {}) {
      const intent = window.WBAgents.consoleIntent(item, opts);
      if (!intent) return;
      if (item.plain) this.newPlainConsole();
      else if (intent === "attach") {
        if (this.active !== "consoles") this.activate("consoles");
        WBConsole.reach({ id: item.sessionId, agent: item.kind, repo: this.openSlug });
        this.consoleCount = WBConsole.count();
      } else this.newConsole(item.kind);
      this.agentMenu = false;
    },

    newConsole(agent) {
      // Defense-in-depth: the accelerator path calls this directly, so refuse an
      // agent launch with no repo here too (the dropdown already disables it).
      if (!this.openSlug) return;
      if (this.active !== "consoles") this.activate("consoles");
      WBConsole.open({ repo: this.openSlug, agent });
      this.consoleCount = WBConsole.count();
    },
    // a bare shell in the repo dir (no agent) — the daemon's per-repo console
    newPlainConsole() {
      if (this.active !== "consoles") this.activate("consoles");
      WBConsole.open({ repo: this.openSlug, plain: true });
      this.consoleCount = WBConsole.count();
    },

    // The Alt+Shift+digit accelerators are ignored while typing, or when a modal
    // or the login gate is up, so they never fight a text field or a dialog.
    consoleShortcutsBlocked() {
      if (!this.authed) return true;
      if (this.settingsOpen || this.securityOpen || this.runOpen || this.branchOpen) return true;
      const el = document.activeElement;
      return !!(
        el &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable || el.closest(".monaco-editor"))
      );
    },

    // A fence is placed at the viewport's CURRENT offset, so the tab must be on
    // screen to be measured. The button itself lives inside `.canvas-tools`
    // (`x-show="active === 'consoles'"`), so the switch below is a guard for a
    // programmatic caller, not a path the operator can take (issue #340).
    // The cap, stated before the click and again if one gets through. Refusing is
    // the whole point: the store keeps FENCE_MAX by pruning the oldest `ts`, so a
    // 13th fence used to cost the operator a DIFFERENT one — named, positioned,
    // and merely the least recently touched. Nothing else in the shell throws the
    // operator's state away to make room.
    //
    // Read off `fenceItems` — the snapshot `toggleFenceMenu` takes — and NOT off
    // `WBConsole.atFenceCap()`. MEASURED: the module's fence array is not Alpine
    // state, so a binding that read it never re-evaluated and the row stayed
    // enabled at a full plane. The snapshot is refreshed on the click that opens
    // this menu, which is exactly when the row has to be right; the module stays
    // the AUTHORITY for the gesture itself (see `newFence`), so a stale snapshot
    // can dim a row late but can never create a thirteenth fence.
    fenceAtCap() {
      return this.fenceItems.length >= window.WBConsole.FENCE_MAX;
    },
    fenceCapMessage() {
      return `${window.WBConsole.FENCE_MAX} fences is the cap — remove one before drawing another`;
    },
    fenceCapReason() {
      return this.fenceAtCap() ? this.fenceCapMessage() : "draw a named fence on the plane";
    },
    newFence() {
      if (this.active !== "consoles") this.activate("consoles");
      // The menu this row lives in must close BEFORE the fence is drawn: the
      // spawn rect is anchored on the viewport's current offset, and leaving an
      // open dropdown over the plane changes nothing about the geometry but
      // does leave a stale list — the new fence would be missing from it.
      this.fenceMenu = false;
      // The module decides — it holds the live fence list, and the disabled row
      // above is only a hint drawn from a snapshot. `false` is its refusal at the
      // cap, and saying so is this layer's job: `wb-console.js` reaches no shell.
      if (WBConsole.createFence() === false) this._flashAction(this.fenceCapMessage());
    },

    // Nothing is clamped into the viewport any more (#336), so a restored window
    // can sit entirely off-view: this is the one action that reaches it (#337).
    // ONE dropdown at a time, wherever it hangs from. Each trigger closes every
    // other menu before toggling its own — including the account menu on the
    // far side of the bar, which used to open ON TOP of a live console picker
    // because the two enumerations never knew about each other. Enumerated
    // here, once, so a fifth menu is one line rather than four edits.
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
      // AFTER the tab is laid out: a `display:none` Consoles tab measures a 0
      // viewport, and centring against zero is centring against nothing.
      this.$nextTick(() => WBConsole.reveal(id));
    },

    // The fence list is the map (issue #343): no zoom, no minimap — the names
    // are the anchors. Snapshot on open, exactly like the Go-to picker above.
    toggleFenceMenu() {
      this.fenceItems = WBConsole.fenceList();
      const was = this.fenceMenu;
      this.closeMenus();
      this.fenceMenu = !was;
    },
    // Alt+Shift+←/→. Returns the fence landed on, or null when the plane has
    // none — the shortcut needs that to decide whether to swallow the key. The
    // walk runs against the LIVE stage, so it needs no snapshot: unlike the
    // menu, there is no list on screen that could go stale.
    stepFence(step) {
      if (this.active !== "consoles") return null;
      return WBConsole.stepFence(step);
    },
    jumpFence(id) {
      if (this.active !== "consoles") this.activate("consoles");
      this.fenceMenu = false;
      // Same reason as `revealWindow`: a `display:none` tab measures a 0
      // viewport, and the jump would slide the plane to 0,0.
      this.$nextTick(() => WBConsole.jumpToFence(id));
    },
    // Alt+Shift+F<n> → the n-th fence, so the menu's rows are a keyboard map and
    // not just a click target. F for fence, and Alt+Shift is the modifier pair
    // the digits and the arrows already proved free of the browser's reserved
    // combos — the digits themselves are spoken for by the New-console rows.
    // Runs to F12, which is the fence cap — so every fence the plane can hold is
    // reachable by key, and the menu never advertises a row without one. The F
    // keys past F9 are the reason this needed measuring rather than reasoning:
    // MEASURED (Chromium 148, this host) Alt+Shift+F10/F11/F12 all reach the
    // document with nothing intercepting them. `Shift+F10` is the Windows context
    // menu and F11/F12 are fullscreen/DevTools, but all three want their exact
    // combo — adding Alt takes this out of their way.
    //
    // The ROW carries only its own key. The modifier pair is identical on all
    // twelve rows, so repeating it there is what crowded the menu: `Alt+Shift+F10`
    // is wide enough that the name beside it wrapped mid-word ("Fence" / "13"),
    // and the panel paid that width twelve times for a prefix read once. It moves
    // to the head as a legend — one statement of the pattern, then a column of
    // numbers under it.
    fenceShortcutLabel(n) {
      return `F${n}`;
    },
    fenceShortcutHint() {
      return this.isMac ? "⌥⇧F<n>" : "Alt+Shift+F<n>";
    },
    // Ordinal, not id: the row's own position in `fenceList()` is what the label
    // promises, and that list is the same fold the menu renders — so the key and
    // the row can no more disagree than the row and the fence can. Read LIVE
    // (the menu's snapshot may be closed or stale); returns whether it landed,
    // which the listener needs to decide whether to swallow the key.
    jumpFenceAt(n) {
      if (this.active !== "consoles") return false;
      const f = WBConsole.fenceList()[n - 1];
      if (!f) return false;
      this.fenceMenu = false;
      return !!WBConsole.jumpToFence(f.id);
    },

    // --- context menu -----------------------------------------------------
    // `node` is null for a right-click on empty tree space, which addresses the
    // repo root: the create items still apply (and are the only way to make a
    // top-level entry), while the per-node items drop out.
    showMenu(x, y, node) {
      const menu = document.getElementById("ctxmenu");
      const isFolder = this.isFolder(node);
      const items = [
        node && !isFolder && { label: "Open", icon: "bi-box-arrow-up-right", run: () => this.openFile(node) },
        node && { label: "Rename…", icon: "bi-pencil", run: () => node.startEditTitle() },
        // Both path forms are FLAT rows, for files and folders alike: a submenu
        // costs a hover-and-wait for a two-item choice made constantly.
        node && { label: "Copy full path", icon: "bi-clipboard", run: () => this.copyPath(node, true) },
        node && { label: "Copy relative path", icon: "bi-clipboard", run: () => this.copyPath(node, false) },
        node && !isFolder && { label: "Duplicate", icon: "bi-files", run: () => this.duplicateNode(node) },
        node && { sep: true },
        // Creating targets the node's own directory: the folder itself, or the
        // folder CONTAINING the clicked file. Right-clicking a file to make its
        // sibling is the gesture every file explorer has, and refusing it was
        // half of why nothing could be created.
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

    // `full` joins the project's absolute root (served by `/api/repos` as `root`)
    // onto the rel path, in the ROOT's own separator so the result pastes into a
    // native shell. Falls back to the rel path when no root is known — an
    // unreachable repo has none, and half a path is worse than a relative one.
    //
    // `navigator.clipboard` is undefined on an insecure non-loopback origin, so
    // the call stays optional-chained: a remote operator loses the clipboard, not
    // the menu.
    copyPath(node, full = false) {
      const rel = this.relPath(node);
      const root = full ? this.projects.find((p) => p.slug === this.openSlug)?.root : "";
      let path = rel;
      if (root) {
        const sep = root.includes("\\") ? "\\" : "/";
        path = root + sep + rel.split("/").join(sep);
      }
      navigator.clipboard?.writeText(path).catch(() => {});
      this.emit("copy-path", node, { path });
    },

    // Duplicate a file beside itself, with NO prompt: the name is derived, and
    // being asked to invent one is the friction the gesture exists to skip. The
    // daemon stays a pure byte-op (`file.copy` refuses an existing dst), so the
    // free-name search happens HERE — list the parent, then take the first of
    // `<stem> copy<ext>`, `<stem> copy 2<ext>`, … that is not taken.
    async duplicateNode(node) {
      const rel = this.relPath(node);
      if (!rel) return;
      const parent = parentRel(rel);
      const name = rel.slice(parent ? parent.length + 1 : 0);
      const dot = name.lastIndexOf(".");
      // A leading dot is the whole name of a dotfile, not an extension.
      const stem = dot > 0 ? name.slice(0, dot) : name;
      const ext = dot > 0 ? name.slice(dot) : "";

      const listing = await WBDaemon.observe("tree.list", {
        repo: this.openSlug,
        path: parent,
      }).catch(() => null);
      const taken = new Set((listing?.entries || []).map((e) => e.name));
      let candidate = `${stem} copy${ext}`;
      for (let i = 2; taken.has(candidate); i++) candidate = `${stem} copy ${i}${ext}`;
      const to = parent ? `${parent}/${candidate}` : candidate;

      const reply = await WBDaemon.write("file.copy", {
        repo: this.openSlug,
        path: rel,
        to,
      }).catch(() => null);
      if (!reply || WBFail.isError(reply)) {
        this._flashAction?.(reply?.reason || "duplicate failed");
        return;
      }
      await this.onTreeDirty(parent);
      await this.revealRel(to);
    },

    // A `create` intent carries the DIRECTORY the new entry goes into, already
    // resolved — a folder node addresses itself, a file node addresses its
    // parent, and no node at all addresses the repo root (""). The listener only
    // has to append the name it prompts for.
    emitCreate(node, kind) {
      WB.emit("create", { project: this.openSlug, path: this.createDir(node), kind, isFolder: true });
    },

    // The directory a create addressed at `node` lands in: the folder itself,
    // the folder CONTAINING a file, or the repo root ("") for no node at all.
    createDir(node) {
      const rel = node ? this.relPath(node) : "";
      return !node || this.isFolder(node) ? rel : parentRel(rel);
    },

    // The Files-header buttons create relative to the tree's active node, so
    // clicking a folder and hitting "New file" does the obvious thing. Nothing
    // selected is the repo root.
    createHere(kind) {
      this.emitCreate(this._tree?.getActiveNode() || null, kind);
    },

    // What the header buttons' tooltip names as the destination.
    createTargetLabel() {
      return this.createDir(this._tree?.getActiveNode() || null) || "the repo root";
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

    // Open the confirm dialog and resolve `true`/`false` on the operator's
    // choice. A pending dialog is settled `false` first so a second call never
    // strands the prior promise. Options override the label/danger defaults.
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

    // Open the prompt dialog and resolve the typed string, or `null` when the
    // operator backs out. Mirrors askConfirm, including settling a pending
    // dialog first so a second call never strands the prior promise.
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
      // Focus after Alpine has painted the dialog, and put the caret at the end
      // rather than selecting: a prefilled name is a starting point to extend.
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

    // Submit the typed name. A name that cannot become a single directory entry
    // is refused HERE, with the dialog left open so the operator can correct it
    // in place. The daemon confines every path regardless (`confine_write`
    // rejects the same shapes) — this check exists to say *which* character was
    // wrong instead of surfacing a flat "refused" after the dialog is gone.
    promptSubmit() {
      const name = this.promptModal.value.trim();
      const bad = !name
        ? "a name is required"
        : /[\\/]/.test(name)
          ? "a name cannot contain a path separator"
          : name === "." || name === ".."
            ? "that name addresses a directory, not an entry"
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

// The live Alpine component instance (Alpine stores it on the x-data element).
function getShell() {
  const root = document.querySelector("[x-data]");
  return root && root._x_dataStack ? root._x_dataStack[0] : null;
}

// Keep the Alpine mirror of the live console count fresh (windows can close
// themselves via their own chrome, outside the New-console button).
document.addEventListener("workbench:consoles-changed", (e) => {
  const c = getShell();
  if (c) c.consoleCount = e.detail.count;
});

// …and of the stage extent, for the frame's second footer pill (issue #338).
// `wb-console.js` only emits this when a number actually changed — a drag folds
// the extent per mousemove.
document.addEventListener("workbench:stage-extent", (e) => {
  const c = getShell();
  if (!c) return;
  c.stageW = e.detail.width;
  c.stageH = e.detail.height;
});

// A viewer asked to detach → open the popup and close the tab.
document.addEventListener("workbench:detach-request", (e) => {
  getShell()?.detachFile(e.detail);
});

// The popups this shell opened, each mapped to the descriptor it is waiting for.
// Membership is the authorisation for every message below: a window we did not
// open is not a detached pane of ours, whatever it claims in `type`.
const detachedWindows = new Map();

// The origin we accept messages from and send them to. `file://` documents get
// an opaque origin, where the only usable target is `"*"` — acceptable there
// because the static demo has no backend to drive and no session to ride.
const wbPeerOrigin = () => (window.WBMode?.isDemo() ? "*" : window.location.origin);

// Messages from detached popups: hand over the descriptor the popup asks for,
// re-emit its save/reload intents on our seam so the backend sees them in one
// place, and fold a re-attached file back into the shell.
//
// Both guards matter and neither replaces the other. `e.origin` refuses a page
// on another origin (which is how a cross-site opener is kept from driving the
// seam); `e.source` refuses a same-origin window we did not open ourselves.
// Without them this listener accepted `file.write` from anyone holding a handle
// to this window.
window.addEventListener("message", (e) => {
  if (!window.WBMode?.isDemo() && e.origin !== window.location.origin) return;
  if (!detachedWindows.has(e.source)) return;
  const m = e.data;
  if (!m || typeof m !== "object") return;
  if (m.type === "wb-detach-ready") {
    // The popup booted and is asking for its file. Answering same-origin-only is
    // what stops a foreign opener from ever supplying one of its own.
    e.source.postMessage({ type: "wb-detach-open", desc: detachedWindows.get(e.source) }, wbPeerOrigin());
  } else if (m.type === "wb-emit") {
    WB.emit(m.action, m.detail || {});
  } else if (m.type === "wb-reattach" && m.desc) {
    getShell()?.openTab({
      project: m.desc.project,
      path: m.desc.path,
      title: m.desc.path.split("/").pop(),
      ftype: m.desc.ftype,
      content: m.desc.content,
    });
    detachedWindows.delete(e.source);
  }
});

// --- Write byte-ops (#197): route the workspace-mutating seam actions to the
// daemon's confined `file.*` verbs. Daemon-backed only (a `file://` standalone
// demo keeps its synthesised behaviour); a confinement/conflict refusal comes
// back as `{status:"error",reason}` and is flashed. The browser composes the
// full rel path from the tree node — the daemon verbs take a complete rel path.
(function wireWriteVerbs() {
  const daemonBacked = () => window.WBMode.isDaemon() && !!window.WBDaemon?.write;
  const flash = (msg) => getShell()?._flashAction?.(msg);
  const parentOf = (rel) => {
    const i = rel.lastIndexOf("/");
    return i < 0 ? "" : rel.slice(0, i);
  };
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
    switch (d.action) {
      case "save":
        call("file.write", { repo, path: d.path, content: d.content || "" });
        break;
      case "create": {
        // The tree emits `create` carrying the target DIRECTORY and no name
        // (`emitCreate` already resolved a file node to its parent, and no node
        // at all to the repo root ""). Ask for the name, compose the full rel
        // path the daemon verb expects, and — for a file — open it once the
        // write lands, so creating a file leaves the operator in it.
        const folder = d.kind === "folder";
        const where = d.path || "the repo root";
        const c = getShell();
        const name = c
          ? await c.askPrompt({
              title: folder ? "New folder" : "New file",
              // No placeholder: a plausible filename sitting in an empty field
              // reads as a name already chosen, and operators pressed Enter on
              // it. The field asks; it does not suggest.
              message: `In ${where} — what should it be called?`,
              placeholder: "",
            })
          : window.prompt(folder ? "New folder name" : "New file name");
        if (!name) return;
        const path = d.path ? `${d.path}/${name}` : name;
        const reply = await WBDaemon.write("file.create", { repo, path, dir: folder }).catch(() => null);
        if (!reply) return flash("write failed");
        if (window.WBFail.isError(reply)) return flash(window.WBFail.message(reply, "refused"));
        flash(`created ${name}`);
        if (!folder) c?.openTab({ project: repo, path, title: name, ftype: classify(name) });
        // Reveal AFTER the level has settled, so this `setActive()` is the last
        // write and a concurrent watcher nudge cannot deselect what was just
        // made. `revealRel` expands the ancestors itself — a `tree.dirty` for a
        // COLLAPSED dir is deliberately dropped, so a nudge alone would leave a
        // new entry under a closed folder invisible.
        await c?.onTreeDirty(d.path || "");
        await c?.revealRel(path);
        break;
      }
      case "rename": {
        // The tree emits leaf `from`/`to`; compose both against the node's parent.
        const parent = parentOf(d.path);
        const from = parent ? `${parent}/${d.from}` : d.from;
        const to = parent ? `${parent}/${d.to}` : d.to;
        call("file.rename", { repo, path: from, to });
        break;
      }
      case "delete": {
        // Deleting is irreversible (a folder removes recursively, server-side);
        // confirm through the design-system dialog before the verb leaves the
        // browser. Fall back to the native confirm if the shell is unreachable.
        const name = d.title || d.path.split("/").pop() || d.path;
        const message = d.isFolder
          ? `Delete folder “${name}” and everything inside it? This cannot be undone.`
          : `Delete “${name}”? This cannot be undone.`;
        const c = getShell();
        const ok = c
          ? await c.askConfirm({ title: "Delete", message, confirmLabel: "Delete", danger: true })
          : window.confirm(message);
        if (!ok) return;
        call("file.delete", { repo, path: d.path }, "deleted");
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

// Alt+Shift+<digit> → the menu row carrying that digit, invoking the SAME row
// action as clicking it (reach a live session, else launch): one code path, so a
// digit can never launch the duplicate its row refuses to. The digits come from
// the daemon's roster; digit 0 is the plain console. Matched on the physical key
// (e.code) so layout / macOS Option glyphs don't matter; guarded so it never
// hijacks a text field, modal, or the login.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  if (!/^Digit\d$/.test(e.code)) return;
  const c = getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  const row = c.consoleItems().find((it) => e.code === "Digit" + it.digit);
  // No row, or a row an agent console can't take yet (no repo selected): inert,
  // and don't swallow the key so nothing else is starved of it.
  if (!row || row.disabled) return;
  e.preventDefault();
  c.openConsoleItem(row);
});

// Alt+Shift+←/→ → walk the fences, in the plane's own reading order (top band
// first, left to right inside it — `fenceCycle`). The same modifier pair as the
// digits above and the same guard, so it never fights a text field, a modal or
// the login; matched on `e.code` for the same layout-independence. With no fence
// on the plane the key is left UNSWALLOWED, so nothing downstream is starved.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  if (e.code !== "ArrowRight" && e.code !== "ArrowLeft") return;
  const c = getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  if (!c.stepFence(e.code === "ArrowRight" ? 1 : -1)) return;
  e.preventDefault();
});

// Alt+Shift+F<n> → the n-th fence in the Fence menu, the accelerator that menu's
// rows advertise. Same modifier pair and same guard as the two listeners above;
// `e.code` again, so an F-key is an F-key on any layout. With no fence at that
// ordinal the key is left UNSWALLOWED.
//
// F1..F12, matching the fence cap (`WBConsole.FENCE_MAX`), so every fence the
// plane can hold has a key. None of the reserved neighbours is hit: Alt+Shift+F4
// is not the Windows close combo (that one is Alt+F4 exactly, no Shift), and the
// same holds for Shift+F10 (context menu), F11 (fullscreen) and F12 (DevTools) —
// each wants its own exact combo. MEASURED rather than reasoned: with Alt+Shift
// held, F10, F11 and F12 all reach the document uninterrupted.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  if (!/^F(?:[1-9]|1[0-2])$/.test(e.code)) return;
  const c = getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  if (!c.jumpFenceAt(Number(e.code.slice(1)))) return;
  e.preventDefault();
});

// `/` → focus the project search (reuses consoleShortcutsBlocked so it never
// hijacks a text field, modal, or the login).
document.addEventListener("keydown", (e) => {
  if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
  const c = getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  e.preventDefault();
  c.focusProjectSearch();
});

// Inbound run events, `file://` demo ONLY (#300): the fold that advances the
// panel from a `{ type, runid, data }` detail is the demo's stand-in for the live
// feed. In daemon mode the panel advances by snapshot replacement instead, so the
// listener is gated here AND in `applyRunEvent` (the method is also called
// directly by `demoTick`). `window.WBRuns.emit(evt)` is the console door.
document.addEventListener("ralphy:run-event", (e) => {
  if (!window.WBMode.seedAllowed()) return;
  getShell()?.applyRunEvent(e.detail);
});
window.WBRuns = {
  emit(evt) {
    document.dispatchEvent(new CustomEvent("ralphy:run-event", { detail: evt }));
  },
  // Phase 1: append a raw output chunk from a daemon-spawned run into the panel,
  // capping the buffer so a long run never grows the DOM unbounded.
  output(text) {
    const c = getShell();
    if (c) c.rawFeed = (c.rawFeed + text).slice(-8000);
  },
};
