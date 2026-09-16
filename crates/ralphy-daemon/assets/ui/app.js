"use strict";
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

// The two directories the daemon's Write path refuses outright
// (`fswrite::PROTECTED_DIRS`), mirrored here so the UI never offers a gesture
// that can only be refused. Case-insensitive: NTFS resolves `.GIT` to `.git`.
const PROTECTED_DIRS = [".git", ".ralphy"];

function isProtectedDir(name) {
  return PROTECTED_DIRS.some((p) => name.toLowerCase() === p);
}

// Whether `rel` names or traverses a protected directory — the same component
// test the daemon applies, so a node inside `.ralphy` is not offered a move it
// cannot have.
function underProtectedDir(rel) {
  return rel.split("/").some(isProtectedDir);
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
// A file tab's identity (#406): the project, the path, and — ONLY under a
// selected worktree — the checkout, so the same rel in two trees is two tabs
// (the primary's id is the pre-#406 spelling, byte for byte). Mirrored by
// `WBViewer`'s `fileTabId`; the two must never disagree, or a tab and its
// pane come apart.
function fileTabId(project, path, checkout) {
  return checkout ? `file:${project}@${checkout}:${path}` : `file:${project}:${path}`;
}

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
    // True only on the static `file://` demo bundle; drives the account menu's "demo"
    // badge and keeps seeds confined to demo (#202).
    isDemo: window.WBMode.isDemo(),
    // Daemon-mode `/api/repos` failure surface (M5, #202): a visible error
    // instead of the seed projects. Empty when repos loaded (or in demo).
    reposError: "",
    // The local fleet's peers (ADR-0052 §5, #349), read from `/api/fleet` after
    // the local `/api/repos` pass. Empty is the honest default: a fleet of one,
    // or a daemon too old to serve the route.
    fleetPeers: [],
    // Peers with a wake in flight, keyed by daemon_id. A cold WSL boot takes
    // seconds, so the operator needs to see that the click landed — and the same
    // key is what stops a second click sending a second nudge.
    waking: {},
    // Working-tree change count per slug (#307), loaded when a project opens and
    // on the sidebar refresh. A slug holds `null` until a load succeeds — the
    // badge renders that as `—`, so a failed read never reads like a clean tree —
    // and `changesReadError` carries the reason into the badge's title.
    changesCount: {},
    // Per slug, and named apart from the shell-wide `changesError` below on
    // purpose: the two were both called `changesError` in this same literal,
    // the later declaration won, and every `changesError[slug] = …` write here
    // was a silent no-op against a string. The badge's title never once showed
    // a read failure.
    changesReadError: {},
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
    // The last refusal an act in the Changes panel came back with, held until
    // the next act replaces it (or the operator dismisses it). NOT derived from
    // `runsActionMsg`: that flash renders only inside `aside.runs`, so with the
    // Runs panel closed — its default — a refused push wrote its reason to a
    // node that was not in the DOM and the button read as dead. One string, not
    // per-project: it describes the act just dispatched, and switching projects
    // is itself the next act.
    changesError: "",
    // True while a manual/initial repo refresh is in flight — spins the sidebar
    // refresh button and disables it. The list does NOT auto-refresh (only the
    // live dots do, via the presence heartbeat), so the button is the way to pick
    // up a newly-registered repo or a branch/dirty change without a page reload.
    reposLoading: false,
    // Live presence + identity (#204): the account menu's uptime is the `/ws`
    // heartbeat's age, and the name/avatar are `/api/identity`. Empty until the
    // first tick / a baptized daemon.
    uptimeText: "",
    identityName: "",
    identityAvatar: "",
    _lastHeartbeat: 0,
    // Staleness is DERIVED, and it has to be derived on a clock rather than in
    // the binding. The markup used to read `Date.now() - _lastHeartbeat > 6000`
    // directly, which cannot fire: the only reactive value in it is written by
    // the heartbeat, so the very event that makes the daemon stale — ticks
    // STOPPING — is the one thing that never re-renders it. A dead daemon read
    // exactly like a live one. `_clockTick` recomputes this instead; writing the
    // same boolean is inert under Alpine's reactivity, so the steady state costs
    // one comparison a second.
    presenceStale: false,
    // The FILES panel's own two states. It had neither: a slow first read showed
    // an empty box, and a FAILED read showed the same empty box, so "this project
    // has no files", "the daemon is still looking" and "the read was refused"
    // were one indistinguishable blank.
    treeLoading: false,
    treeError: "",
    // The tree is showing a listing the daemon could not confirm (a refusal, a
    // dropped socket). Distinct from `treeError`, which is the root read that
    // never landed at all: this one has rows on screen and is telling the
    // operator not to trust their age.
    treeStale: "",
    // A refused `branch.switch`/`branch.create`, held until the next branch act
    // (opening the picker) or a project switch. Beside `treeError` rather than
    // inside it because the two are different subjects: the tree is fine, the
    // branch did not move. Same reason it is not `changesError` — the chip that
    // was clicked lives in THIS panel, and an answer belongs where the question
    // was asked.
    branchError: "",
    // The FILES search (ADR-0036 amendment 2026-09-15). `query`/`mode` are what
    // the operator typed and chose; `seq` dates each request so a slow reply
    // never paints over a newer one; `hits`/`truncated` are the last reply;
    // `note` is the gutter line; `expandedBefore` is the expansion snapshot the
    // first apply takes so `clearFileSearch` can fold the tree back to it
    // (`null` = no search has expanded anything yet).
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
    // Tree memory, all three lazily created (the `_reconciling` idiom below) so
    // they are plain collections rather than members of Alpine's reactive data —
    // nothing here drives a binding, and a proxied Map is a trap:
    //   _treeCache     directory levels this browser has already been shown, keyed
    //                  `repo\nrel`. Survives closing a project (the point of it).
    //                  Memory only: never written to the daemon or to storage, so
    //                  it cannot outlive the tab nor be mistaken for daemon state.
    //   _treeValidated which cached levels were re-read against the disk during
    //                  THIS open. Cleared on every mount, because a level's
    //                  freshness expires the moment its watch is dropped.
    //   _treeExpanded  the folders expanded when a project was last closed, keyed
    //                  by repo, so re-opening returns the tree the operator left.
    _runsSub: null, // the live run-snapshot subscription for the open project, if any
    _changesSub: null, // the run-completion nudge subscription for the open project (#310)
    _presenceSub: null, // the `/ws` heartbeat subscription, kept so a resume can re-open it
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
      // One read at load: the daemon polls on its own six-hour clock, so the
      // page never fetches releases itself and never waits on the network.
      this.loadRelease();
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
        this.resumeSockets();
      });
      // A tablet resumes on a different link than the one it slept on, and the
      // sockets that link carried are dead without ever having heard a close.
      window.addEventListener("online", () => this.resumeSockets(true));
      // The console module owns its own windows' sockets and its own resume
      // trigger; what it cannot know on its own is whether THIS document's
      // connection is alive. Hand it the heartbeat verdict the shell already
      // computes — without it the popup's hidden-time fallback would reset every
      // desktop console after a minute on another tab.
      window.WBConsole?.setStaleProbe?.(() => this.socketsAreStale());
      // The selected checkouts (#406): the ONE hook for `unknown checkout` from
      // any verb, and the copy of the desk mirror once the boot desk lands.
      window.WBDaemon?.onUnknownCheckout?.((repo, name) => this.checkoutGone(repo, name));
      window.WBConsole?.whenDeskLoaded?.().then(() => this.adoptDeskCheckouts());
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
        // Three missed ~2s heartbeats. Unconditional (unlike the clock above):
        // the account menu is usually closed, and the point of the flag is to be
        // already true when the operator opens it to ask.
        this.presenceStale = this.socketsAreStale();
      }, 1000);
    },

    // The daemon's real identity (name + avatar), shown in the account menu. A
    // 404 (un-baptized daemon) or a thrown fetch (file:// demo) leaves the
    // fields empty and the markup falls back to `ralphy` / no avatar.
    // The daemon's mark, in ONE place: the rail's puck, the account menu's
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
    // menu's uptime; a baptized daemon also carries name/avatar. Every tick
    // re-derives `live` so the sidebar dots track session open/close (~2s).
    // Three missed ~2s heartbeats means this document's connection is gone, not
    // that the tab was merely in the background — the same predicate the account
    // menu shows as `presenceStale`. It is the shell's whole staleness signal:
    // a suspended tablet runs no JS, so on return the stamp is old, while an
    // ordinary desktop tab switch leaves it fresh and nothing is torn down.
    socketsAreStale() {
      return !this._lastHeartbeat || Date.now() - this._lastHeartbeat > 6000;
    },

    // Bring the three long-lived subscriptions back after a suspend. Each one
    // decides for itself (`resumeDecision`) and debounces, so calling this from
    // both triggers on one resume costs nothing.
    resumeSockets(stale) {
      const verdict = stale === undefined ? this.socketsAreStale() : stale;
      this._runsSub?.resume?.(verdict);
      this._changesSub?.resume?.(verdict);
      this._presenceSub?.resume?.(verdict);
    },

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
          window.WBMode.seedAllowed() ? window.WB_SEED_ROSTER || [] : [],
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
            // The peer's OWN working-tree facts, carried through `/api/fleet`
            // unread — the same github|local classification `loadRepos` makes,
            // so a GitHub-backed peer repo gets the GitHub dot and links. These
            // were hard-coded `false`/"local" before, and every peer repo
            // rendered as local-only.
            dirty: !!x.dirty,
            state: x.reachable ? "idle" : "offline",
            remote: x.remote && x.remote.includes("github.com") ? "github" : "local",
            remoteUrl: x.remote || "",
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

    // Wake a sleeping peer. The operator's own action is the whole consent —
    // the same model as push (ADR-0046) — and that is precisely why this lives
    // in the workbench and not in the daemon: a daemon that nudged on every
    // probe would be supervising by accident (ADR-0052 §4), and a peer the
    // operator deliberately stopped would never stay stopped.
    //
    // `/api/fleet/nudge` waits for the peer to answer, so this resolves when the
    // environment is USABLE, not when `wsl.exe` was spawned.
    async wakePeer(daemonId) {
      if (!daemonId || this.waking[daemonId]) return false;
      this.waking[daemonId] = true;
      try {
        const r = await fetch(`/api/fleet/nudge?daemon_id=${encodeURIComponent(daemonId)}`, {
          method: "POST",
        });
        const reply = await r.json().catch(() => ({}));
        if (!r.ok || !reply.ready) {
          // The daemon's own sentence names the environment and what is wrong
          // with it; inventing a shorter one here would lose that.
          this._flashAction(reply.diagnosis || reply.error || "The peer did not respond.");
          return false;
        }
        // `loadRepos`, not `loadFleet`: the latter CONCATENATES peer rows onto
        // the local ones, so calling it alone would list the woken peer twice.
        await this.loadRepos();
        // A row that was opened against a sleeping peer has an empty tree — its
        // mount failed while the environment was down. Now that it answers, the
        // mount is worth repeating, but only for the peer that just woke.
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

    // Waking triggered by the ordinary act of opening a row that lives on a
    // sleeping peer — the toll this removes is paying a 502 for something the
    // operator plainly wants. A no-op for every other row.
    wakePeerFor(ref) {
      const daemon = window.WBFleet.refDaemon(ref);
      if (!daemon) return;
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      if (window.WBFleet.wakeable(group)) this.wakePeer(daemon);
    },

    peerWakeable(g) {
      return window.WBFleet.wakeable(g);
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
        // The console windows read their own row off the same poll (ADR-0059).
        window.WBConsole?.ingestSessions?.(sessions);
        for (const p of this.projects) {
          if (p.state === "offline") continue;
          const mine = sessions.filter((s) =>
            window.WBSessionRoute.matchesRepo(s, this.repoRef(p)),
          );
          // A `waiting` agent outranks `live` on the project dot: the
          // operator is being asked for something there (ADR-0059).
          p.state = !mine.length
            ? "idle"
            : window.WBProject.agentStateOf(mine) === "waiting"
              ? "waiting"
              : "live";
        }
      } catch {}
    },
    // The project's own sessions, for the picker's worktree-row dots.
    worktreeStateOf(w) {
      const mine = (this.liveSessions || []).filter((s) =>
        window.WBSessionRoute.matchesRepo(s, this.branchModal.slug),
      );
      return window.WBProject.worktreeStates(this.worktreeRows(), mine)[w.name] || null;
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
      return window.WBChanges.projectBadge(this.changesCount, this.changesReadError, slug);
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
      //
      // The OPEN row always passes, whatever the query. Its `<li>` hosts the
      // file tree (`.wb-host`) and the `/ws/tree` subscription, and an `x-for`
      // rebuild that drops it is an unmount nobody asked for — `destroyTree`
      // never ran, and clearing the query rebuilt an empty host that nobody
      // re-mounted (2026-09-10: FILES blank, then a nudge against the orphaned
      // tree read as "could not refresh"). Nothing visible changes: while a
      // project is open, `has-open` already hides every other row.
      return this.projects.filter(
        (p) =>
          this.rowOpen(p) ||
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
      return window.WBProject.repoLabel(p);
    },

    // What every surface OUTSIDE the sidebar prints for a repo ref.
    //
    // A peer ref is `<daemon_id>/<owner>/<repo>` — the ULID is how the fleet
    // ROUTES (ADR-0052 §5), never what the repo is called, and rendering it raw
    // is how a WSL project came to be titled `01KY…/paulocorcino/vibeforge`. The
    // environment takes its place, because it is what the ULID was really
    // saying: which machine this copy lives on. The ref itself is untouched
    // everywhere it matters — the wire, the desk records, the viewer tab ids.
    //
    // The row lookup is by `repoRef`, not by slug: the same `owner/repo` on two
    // daemons is two rows, and matching the slug would label one with the
    // other's environment.
    projectLabel(ref) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      return window.WBFleet.refLabel(ref, row?.env);
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
    branchModal: {
      slug: null,
      filter: "",
      branches: [],
      current: "",
      primaryBranch: "",
      dirty: false,
      checkoutDirty: false,
      checkouts: null,
      // The branch a new worktree is cut from when the operator picked one
      // off a branch row (its `+` action); `null` = the current branch.
      worktreeBase: null,
      creating: false,
      removing: null,
      // What the last `worktree.add` said on success: the carry-over's
      // warnings (`worktree.copy` / `worktree.share` entries it skipped).
      createNote: "",
    },
    // The selected checkout per repo ref (#406, ADR-0063 §4): the REACTIVE
    // copy of `WBConsole`'s desk mirror — a closure variable there is
    // invisible to Alpine, and this is what the chip, the picker rows and the
    // tree key render. `worktreeListings` is the last `worktree.list` reply per
    // ref, for the chip's branch. `_treeCheckout` is the checkout the mounted
    // tree was built for, so a change of selection remounts it.
    checkouts: {},
    worktreeListings: {},
    _treeCheckout: null,

    // Switching is possible only when the daemon can reach the repo on disk.
    // NOT gated on `remote`: a local-only repo (no GitHub) is still a git
    // checkout with branches — it's an *unreachable* path (state offline) the
    // daemon can't run `git branch`/`checkout` against.
    canSwitchBranch(p) {
      return window.WBProject.canSwitchBranch(p);
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

    // Drop a project from the daemon's registry (#363). The directory on disk is
    // NOT touched — the confirm says so literally, because "remove" in a file
    // tree means delete and this one does not.
    //
    // The confirm is awaited BEFORE any `WBDaemon` call, on every path: cancel
    // must open no socket at all, which is only true if nothing is sent until
    // the answer is in hand.
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
          // The envelope routes on a REGISTERED repo before dispatch runs, so
          // `repo` names the cwd and `slug` names what is unregistered. Normally
          // the same project; the split is what keeps the peer proxy working.
          repo: ref,
          slug: p.slug,
        });
        // `unknown repo` is the routing lookup saying the project is already
        // gone from the registry — which is exactly the state this click asks
        // for, so it is a success, not a failure to report.
        const gone =
          !window.WBFail.isError(reply) || window.WBFail.message(reply, "") === "unknown repo";
        if (!gone) {
          this._flashAction(window.WBFail.message(reply, "remove refused"));
          return;
        }
        // Identity is `repoRef`, not the bare slug: a peer environment can list
        // the same slug, and dropping one row must not take its twin with it.
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
      // Reaching for the picker again IS the next branch act — the previous
      // answer describes a choice the operator is about to replace.
      this.branchError = "";
      const ref = this.repoRef(p);
      // Under a selection "current" is the WORKTREE's branch (#407): seeded
      // from the cached listing, then confirmed by `branch.list` run there.
      const ck = this.checkoutOf(ref);
      const wt = ck ? (this.worktreeListings[ref]?.worktrees || []).find((w) => w && w.name === ck) : null;
      this.branchModal = {
        slug: ref,
        filter: "",
        branches: [...(p.branches || [p.branch])],
        current: wt ? wt.branch || "HEAD" : p.branch,
        primaryBranch: p.branch,
        dirty: !!p.dirty,
        // The dirty warning is about the tree the switch will hit (#407):
        // the selected worktree's, from the listing; `dirty` stays the
        // primary's for its picker row.
        checkoutDirty: wt ? wt.dirty === true : !!p.dirty,
        checkouts: null,
        worktreeBase: null,
        creating: false,
        removing: null,
        createNote: "",
      };
      this.branchOpen = true;
      this.loadBranches(ref);
      this.loadWorktrees(ref);
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

    // The picker's Worktrees section (#403, ADR-0063 §4) reads the workbench
    // worktrees through the `worktree.list` Query verb, the same honesty rule as
    // `loadBranches`: a failed read is `null` (the section does not render),
    // never a stale listing. No toast, unlike `loadBranches`: the section is
    // additive and a peer on an older ralphy answers `unknown verb` on EVERY
    // picker open, which would be a flash per click for a list that is simply
    // absent there. The reason goes to the console.
    async loadWorktrees(slug) {
      try {
        const reply = await window.WBDaemon.observe("worktree.list", { repo: slug });
        if (this.branchModal.slug !== slug) return; // modal moved on — leave it
        if (!reply || reply.status !== "ok") {
          this.branchModal.checkouts = null;
          if (window.WBMode.isDaemon()) {
            console.warn("worktree.list failed", reply && reply.message);
          }
          return;
        }
        // The CLI's `{primary, worktrees:[]}` JSON is nested under the Query
        // field `checkouts` — one level deeper, like `reply.branches`.
        this.branchModal.checkouts = reply.checkouts || null;
        this.worktreeListings = { ...this.worktreeListings, [slug]: reply.checkouts || null };
        // The consoles' title switcher (#412) reads the same listing.
        window.WBConsole?.ingestWorktrees?.(slug, reply.checkouts || null);
      } catch (e) {
        if (this.branchModal.slug === slug) {
          this.branchModal.checkouts = null;
          if (window.WBMode.isDaemon()) console.warn("worktree.list failed", e);
        }
      }
    },
    closeBranchModal() {
      this.branchOpen = false;
    },

    // The open project's working-tree change count (#307), served read-only via
    // the `changes.list` Query verb. The count is a snapshot between events: it
    // reloads when a project is opened, on the sidebar refresh, and on a
    // run-completion nudge (#310) — never on a poll or a repo-wide watch.
    // The list is the SELECTED checkout's (#407, ADR-0063 §2): the daemon runs
    // the command in the worktree.
    async loadChanges(slug) {
      if (!slug) return;
      // Nudges (#310) can land while a read is in flight, so two reads of the
      // same slug overlap and their replies can return OUT OF ORDER — an older
      // reply would then overwrite a newer count (same hazard as `_runsSeq`).
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

    // The open project's sync state (#316), served read-only via the
    // `sync.status` Query verb — which makes NO network call, so this read is
    // safe on every trigger `loadChanges` rides. There is deliberately no timer:
    // a launcher holding N repos must never become a scheduled network client.
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
        // A transport throw is NOT a refusal: the repo never answered. Saying
        // "refused" there would report a decision nobody made.
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

    // Publish the branch (#320). The OPERATOR's own click is the whole consent
    // — there is no opt-in flag on this path (ADR-0046 amendment) — and every
    // refusal the core models (a remote that moved on, a credential the
    // remote rejected) arrives as `{status:"error"}` whose
    // message IS the core's prose. Nothing here remediates a credential: there
    // is no prompt and no credential UI, by decision.
    //
    // Push moves no file, so unlike `syncPull` it reloads the counts only.
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
        "Labels are read-only while a run is active.",
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

    // Stage / unstage / commit. Each follows `syncFetch`'s exact shape, and each
    // re-reads the list from the daemon on EVERY path — success, refusal and
    // transport throw alike. The list is never moved optimistically: a row that
    // jumped groups on a click the daemon refused would be a lie the operator
    // acts on next.
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
      // Belt to the `toggle` braces: never commit a draft composed for another
      // project, whatever path left the two out of step.
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

    // The Worktrees rows under the branch list: empty (no section) until a
    // non-empty `worktree.list` reply has landed. Not filtered by the box.
    // The primary row names the PRIMARY's branch, which `current` is not under
    // a selection (#407).
    worktreeRows() {
      return window.WBProject.worktreeRows(
        this.branchModal.checkouts,
        this.branchModal.primaryBranch ?? this.branchModal.current,
        this.branchModal.dirty,
      );
    },

    // The branch rows with where each one lives (2026-09-16): the filtered
    // list joined with the listing, so a worktree's branch offers "go there"
    // instead of a switch git would refuse.
    branchRows() {
      return window.WBProject.branchRows(
        this.branchList(),
        this.branchModal.current,
        this.branchModal.primaryBranch ?? this.branchModal.current,
        this.branchModal.checkouts,
      );
    },
    // The base a new worktree is cut from: the branch picked off a row, else
    // the current one.
    worktreeBase() {
      return this.branchModal.worktreeBase || this.branchModal.current;
    },
    setWorktreeBase(name) {
      this.branchModal.worktreeBase = name || null;
      this.$nextTick(() => this.$refs.branchFilter?.focus());
    },
    // The "Create worktree “<name>” from <base>" row: the typed name, once
    // the listing has answered (#405). Null before that — no daemon, no row.
    worktreeCreateRow() {
      return window.WBProject.worktreeCreateRow(
        this.branchModal.checkouts,
        this.worktreeBase(),
        this.branchModal.filter,
      );
    },
    carryOverNote() {
      return window.WBProject.CARRY_OVER_NOTE;
    },
    // A branch row's click: switch — or, for a branch that lives in a
    // checkout, go there (git refuses to check a branch out twice).
    branchAct(row) {
      if (row.checkout) {
        this.selectCheckout({ primary: row.checkout === "primary", name: row.checkout });
      } else {
        this.switchBranch(row.name);
      }
    },

    // --- the selected checkout (#406, ADR-0063 §4) ----------------------------
    checkoutOf(ref) {
      return this.checkouts[ref] || null;
    },
    isSelectedCheckout(w) {
      const c = this.checkoutOf(this.branchModal.slug);
      return w.primary ? !c : c === w.name;
    },
    chipLabel(p) {
      const ref = this.repoRef(p);
      return window.WBProject.chipLabel(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    // The reactive map is REPLACED (never mutated in place) so Alpine sees it;
    // persistence goes to the desk mirror; an open tree of the same project is
    // remounted, because its cache key, its watch and its rows are all per
    // checkout.
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
    // A picker row click: `primary` clears the selection, a worktree row sets it.
    selectCheckout(w) {
      const slug = this.branchModal.slug;
      if (!slug) return;
      this.setCheckout(slug, w.primary ? null : w.name);
      this.closeBranchModal();
    },
    // The daemon answered `unknown checkout` for `name` (registered in `init`
    // through `WBDaemon.onUnknownCheckout`): the worktree is gone, so the
    // selection is dropped and the primary tree shown — unless the selection
    // already moved on, in which case a late reply says nothing.
    checkoutGone(ref, name) {
      if (this.checkoutOf(ref) !== name) return;
      this.setCheckout(ref, null);
      this._flashAction(`Worktree ${name} no longer exists. Showing the primary tree.`);
    },
    // The chip needs the worktree's BRANCH, which only a `worktree.list` reply
    // knows: one read per project open with a selection and no cached listing
    // (the picker's own `loadWorktrees` fills the same cache). `force` re-reads
    // a cached listing — after a branch act under a selection the chip
    // converges from this reply, not from `p.branch` (#407).
    // A forced re-read that fails DROPS the cached entry: the chip then shows
    // the bare name (its "listing not landed" state) rather than the branch
    // the tree was on before the act. Replies can land out of order like
    // `loadChanges`'s, so the newest read owns the entry.
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
        // selection it just restored — only when it differs from what the
        // open read under, or every desk landing pays two git spawns.
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

    // Enter = act on the top match; else the typed name creates a worktree
    // when a base was picked off a row, a branch otherwise (quick-pick).
    branchEnter() {
      const rows = this.branchRows();
      if (rows.length) this.branchAct(rows[0]);
      else if (this.branchModal.worktreeBase && this.worktreeCreateRow()) this.createWorktree();
      else if (this.canCreateBranch()) this.createBranch();
    },

    // Under a selected worktree (#407) the act lands on THAT tree's HEAD:
    // `p.branch` is the primary's and must not move, so there is no optimistic
    // update and nothing to revert — the chip converges from the forced
    // `worktree.list` re-read `_mutateBranch` issues after the reply.
    switchBranch(name) {
      if (name !== this.branchModal.current) {
        const slug = this.branchModal.slug;
        const checkout = this.checkoutOf(slug);
        const p = checkout ? null : this.projects.find((x) => this.repoRef(x) === slug);
        const prev = p ? p.branch : null;
        if (p) p.branch = name; // optimistic — the chip updates immediately
        WB.emit("branch-switch", { project: slug, branch: name, checkout });
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

    // The create row: `worktree.add` with the row's name and base (the
    // label's, so the label is the truth). The modal stays open and the list
    // re-reads on every path — a refusal keeps the typed name and surfaces
    // the verb's verbatim message via `_branchRefused`, rendered inside the
    // modal too, since the chip's copy sits behind the scrim.
    async createWorktree() {
      const row = this.worktreeCreateRow();
      const slug = this.branchModal.slug;
      // One create in flight: a second Enter before the reply would send a
      // duplicate whose `already exists` refusal masks the first's success.
      if (!row || !slug || this.branchModal.creating) return;
      const name = row.name;
      this.branchModal.creating = true;
      this.branchError = "";
      this.branchModal.createNote = "";
      const payload = { repo: slug, name };
      if (row.base) payload.base = row.base;
      try {
        const reply = await window.WBDaemon.observe("worktree.add", payload);
        if (this.branchModal.slug !== slug) return; // modal moved on — leave it
        if (window.WBFail.isError(reply)) {
          this._branchRefused(window.WBFail.message(reply, "worktree create refused"));
        } else {
          this.branchModal.filter = "";
          this.branchModal.worktreeBase = null;
          // A clean add may still have something to say: the carry-over
          // entries it skipped, one `warning:` line each, verbatim.
          this.branchModal.createNote = typeof reply?.message === "string" ? reply.message : "";
        }
      } catch {
        if (this.branchModal.slug !== slug) return;
        if (window.WBMode.isDaemon()) {
          this._branchRefused("Could not reach the daemon. Check whether the worktree was created.");
        }
      } finally {
        if (this.branchModal.slug === slug) {
          this.branchModal.creating = false;
          this.loadWorktrees(slug);
          // The add cut a branch too (name = branch): the list below the
          // checkouts must show it, tagged with where it lives.
          this.loadBranches(slug);
        }
      }
    },

    // The row's trash action: `worktree.remove` (#409). The listing is the
    // truth on every path — a `branch kept` reply is an error whose directory
    // is gone; a refusal keeps the row — so the selection resets from the
    // re-read (`checkoutAfterListing`), never from the reply's status. Each
    // gate's message lands verbatim through `_branchRefused`.
    async removeWorktree(w) {
      const slug = this.branchModal.slug;
      if (!slug || !w || w.primary || this.branchModal.removing) return;
      this.branchModal.removing = w.name;
      this.branchError = "";
      try {
        const reply = await window.WBDaemon.observe("worktree.remove", { repo: slug, name: w.name });
        if (this.branchModal.slug !== slug) return; // modal moved on — leave it
        if (window.WBFail.isError(reply)) {
          this._branchRefused(window.WBFail.message(reply, "worktree remove refused"));
        } else {
          this._flashAction(`worktree ${w.name} removed`);
        }
      } catch {
        if (this.branchModal.slug !== slug) return;
        if (window.WBMode.isDaemon()) {
          this._branchRefused("Could not reach the daemon. Check whether the worktree was removed.");
        }
      } finally {
        if (this.branchModal.slug === slug) {
          await this.loadWorktrees(slug);
          // Re-checked AFTER the await: a picker opened on another project
          // meanwhile owns `branchModal.checkouts` now, and reading THAT
          // listing would drop this project's selection for nothing.
          if (this.branchModal.slug === slug) {
            const ck = this.checkoutOf(slug);
            if (ck && window.WBProject.checkoutAfterListing(ck, this.branchModal.checkouts) === null) {
              this.checkoutGone(slug, ck);
            }
            // Cleared last: `removing === null` means the re-read landed too.
            this.branchModal.removing = null;
          }
        }
      }
    },

    // Await a `branch.switch`/`branch.create` Mutate; on a `{status:"error"}`
    // refusal (a held run.lock, per ADR-0036 §6) run `revert` and report the
    // verb's verbatim message. It lands in the Projects panel, under the chip,
    // AND in the runs flash: the revert alone is a chip that snaps back with no
    // reason given, and the flash's only renderer is the runs aside, which is
    // closed by default.
    // The act carries the selected checkout (#407): the daemon runs it in the
    // worktree, so the worktree's HEAD moves and never the primary's.
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
        // the verb may well have landed, and reverting a switch that happened
        // would put a lie in the chip. In the static shell there is nothing to
        // report; with a daemon behind it, an unanswered branch change is
        // exactly the thing the operator must not read as "done".
        if (window.WBMode.isDaemon()) {
          this._branchRefused("Could not reach the daemon. Check whether the branch changed.");
        }
      } finally {
        // Under a selection the chip reads the listing's branch: re-read it on
        // every path (a refused switch left it where it was; an unconfirmed
        // one may have landed). A moved HEAD also changes the working tree and
        // the sync row's branch, so both reload like every other write does.
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
          ? `Could not read ${bad.length} run${bad.length > 1 ? "s" : ""}: ` +
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
    // Reads the PRIMARY tree on purpose — no `checkout` (#406): a run takes
    // the primary tree (ADR-0063 §7) and its `.ralphy/` documents live there.
    // Same for `loadPlan`, `diffWorkSide` and the git-backed panels.
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
        return run.planReadFailed ? "Could not read the plan. Showing the last version loaded." : "";
      }
      const theirs = this.planProseIssue(run);
      if (theirs != null) {
        // The one this whole gate exists for: the plan on disk is the PREVIOUS
        // issue's, and naming both numbers is what makes that legible.
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
    // A refusal an act in the CHANGES panel came back with. It lands in that
    // panel, where the click happened, and STAYS there — the `_flashAction`
    // flash is kept beside it so nothing that used to be visible with the Runs
    // panel open stops being visible, but the flash is no longer the only
    // renderer of an answer the operator has to read. `runs-verb-error` made
    // this same trade for the run verbs (#331); this is its counterpart.
    _changesRefused(msg) {
      this.changesError = msg || "";
      this._flashAction(msg);
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
    // The backlog as a board: the open project's issues (from the tracker in
    // daemon mode; from the demo seed under `file://`) placed in four columns by
    // ralphy's own judgment (window.WBKanban). Read-only except labels — the one
    // mutation that moves a card between columns; everything else opens on
    // GitHub. Data is project-scoped like the Runs panel.
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
        "Delete this plan. The next run plans this issue again."
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
    // Which project is open is component state; what its remote means is not.
    // Only the lookup stays here.
    githubUrl(number) {
      const p = this.projects.find((x) => this.repoRef(x) === this.openSlug);
      return window.WBProject.issueUrl(p && p.remoteUrl, number);
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
      this.settings["consoles.key_bar"] = view.keys ?? "unset";
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

    // --- spend (the Spend tab, #358) ---------------------------------------
    // A canvas tab, not a modal and not an overlay: cost is something the
    // operator reads BESIDE the work, so it rides in the strip like a file does
    // (ADR-0037, amended by this issue — a closable tab may also be a daemon
    // view). Scoped to the open project, the same identity the board scopes on.
    //
    // Everything numeric on screen is rendered by the daemon (`/api/spend`);
    // `WBSpend` folds only which state the pane is in. See wb-spend.js.
    spend: { loading: false, error: "", doc: null, slug: null },
    // The window the operator picked. Sent on the fetch and echoed back by the
    // daemon; the pane renders the document's own key, never this one, so a
    // label can never lead the figures under it.
    spendPeriod: "all",
    // The tab's whole view model, recomputed by Alpine whenever the fetch or the
    // open project moves. `openSlug` is read HERE and not stashed, so closing a
    // project drops the pane to its empty state with no extra bookkeeping.
    spendView() {
      return window.WBSpend.state({
        project: this.openSlug,
        loading: this.spend.loading,
        error: this.spend.error,
        // A document for a project that is no longer open is stale by
        // definition — never render it under the new project's name.
        doc: this.spend.slug === this.openSlug ? this.spend.doc : null,
        period: this.spendPeriod,
        // Titles ride whatever the board ALREADY holds. This never triggers a
        // load: `loadBoard` spawns a CLI that makes tracker calls and is
        // throttled for that reason, and a cost page must not pay it.
        issues: this.boardIssues[this.openSlug] || [],
      });
    },
    // The period control. Assign, then re-read — the window is a server-side
    // filter, so every tile and grid on the page moves with it.
    setSpendPeriod(key) {
      if (this.spendPeriod === key) return;
      this.spendPeriod = key;
      this.loadSpend();
      // The Ledger is scoped to the SAME window (the daemon derives `since` from
      // this key for both routes), so a period change invalidates its rows too —
      // otherwise the grid keeps a week the tiles above it no longer describe.
      this.ledger.slug = null;
      if (this.spendPane === "ledger") this.loadLedger();
    },
    openSpend() {
      if (!this.tabs.some((t) => t.id === "spend")) {
        // Bootstrap's cash-stack, not a lucide glyph: the strip renders `t.icon`
        // as a class (see the Consoles tab above).
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
      // No project open is not a failure to report — the pane says so itself,
      // and a request with no project is one the daemon refuses by design.
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
      // The operator switched projects (or closed one) while this was in flight:
      // landing it now would put one project's cost under another's name.
      if (this.openSlug !== slug) return;
      this.spend = { loading: false, error, doc, slug };
      this.$nextTick(() => window.lucide?.createIcons());
    },
    // Re-read whenever the tab is open and its subject may have moved: on
    // activation, and when the accordion opens or closes a project.
    refreshSpend() {
      if (!this.tabs.some((t) => t.id === "spend")) return;
      // The Ledger's rows belong to the project they were fetched for; dropping
      // the slug is what makes the next switch to that pane re-read rather than
      // paint one project's lines under another's name. The unpriced filter goes
      // with them — it was an answer to a click on the PREVIOUS project's gap.
      this.ledger.slug = null;
      this.spendLedgerUnpricedOnly = false;
      this.loadSpend();
      if (this.spendPane === "ledger") this.loadLedger();
    },

    // --- the Ledger pane (#360) --------------------------------------------
    // `Overview | Ledger` inside the ONE Spend tab: the summary and the raw
    // per-phase grid are two readings of the same project, so they are two panes
    // and not two tabs. The Ledger side is what the removed Usage modal did, with
    // columns, scoped to the open project, and with the daemon's unpriced verdict
    // on every row (`/api/usage?project=`).
    spendPane: "overview",
    // Set by the click-through from the Overview's unpriced figure: the operator
    // asked to see the offending rows, so the pane opens already filtered.
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
      // Rows fetched for a project that is no longer open are stale by
      // definition — the same rule `spendView()` applies to the document. The
      // peer banner is gated with them: a peer that failed to answer for the
      // PREVIOUS project is not a fact about this one.
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
    // Loading is deferred to the first switch: the Overview is the tab's landing
    // pane, and the ledger is the one response on this page that grows with the
    // project's history.
    setSpendPane(key) {
      this.spendPane = key;
      if (key === "ledger") this.loadLedger();
      this.$nextTick(() => window.lucide?.createIcons());
    },
    // The Overview's unpriced figure is the drill-down PRD #355 story 25 asks
    // for: the gap gets a concrete owner instead of staying a number.
    showUnpricedLedger() {
      this.spendLedgerUnpricedOnly = true;
      this.setSpendPane("ledger");
    },
    async loadLedger() {
      const slug = this.openSlug;
      // With no project open there are still PEERS to report — a contribution
      // that never arrived is fleet health, not project data, and this pane is
      // its only surface since the Usage modal was removed. The empty `project=`
      // scopes the rows to none while the daemon still answers `missing`.
      const want = slug || "";
      // The `loading` half is what stops a second click duplicating the heaviest
      // request on the page; `slug` alone would not, since it is written only
      // when the fetch lands.
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
    // The release view the daemon computed (ADR-0056 §7). Seeded empty so a
    // static bundle and a daemon that has not polled yet both render nothing
    // rather than a half-drawn badge.
    release: (window.WBRelease && window.WBRelease.EMPTY) || {
      current: "",
      channel: "rc",
      standing: "unknown",
      severity: "none",
      latest: null,
      gap: [],
      disabled: false,
    },
    // Dismissed by opening the panel — except when the news is urgent, which is
    // the one case a single click must not silence.
    releaseSeen: false,
    whatsNewOpen: false,

    get releaseHasNews() {
      return !!window.WBRelease && window.WBRelease.hasNews(this.release);
    },
    // What the rail draws. A dismissal hides it; an urgent release ignores the
    // dismissal, because a breaking change is not something to forget by
    // clicking once.
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
        // Nothing here is worth interrupting the operator over: the flag is a
        // preference, and the next read reports what actually took.
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
      // A client-scoped key stops here: it is this browser's preference, so it
      // goes to the view store and never to `config.set` — which would put a
      // per-browser choice in a repo's settings.json for every client to obey.
      if (this.CLIENT_KEYS.has(key)) {
        if (key === "consoles.relaunch_on_load") window.WBView.patch({ relaunch: value === true });
        // Only the two explicit choices are stored. "unset" is the ABSENCE of a
        // preference, so it is written as null rather than as a third string the
        // reader would then have to know about.
        if (key === "consoles.key_bar")
          window.WBView.patch({ keys: value === "on" || value === "off" ? value : null });
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
          this.security.totpError = "Wrong code. Enter the current code from your authenticator app.";
        }
      } catch {
        this.security.totpError = "Cannot reach the daemon. Try again.";
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
    // `remember` is the "keep me signed in" box (ADR-0032 amendment 2026-09-16):
    // opt-in, so it resets to off on every log-off like the other fields.
    login: { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: false },

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
        this.login = { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: this.login.passwordRequired };
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
      // is unread AND unwritable until it is re-read here (issue #327) — and
      // the selected checkouts ride that same desk (#406).
      window.WBConsole?.afterLogin()?.then(() => this.adoptDeskCheckouts());
      // Only now is `file.read` allowed: restoring the tabs before login would
      // have each one refused and immediately closed (issue #339).
      this.restoreView();
    },

    // --- TOTP digit boxes -------------------------------------------------
    // One input per digit: typing advances, Backspace on an empty box retreats
    // (clearing the previous digit), and a paste — or the browser's OTP
    // autofill landing all 6 chars in the first box — is spread across the
    // boxes. `login.code` stays the joined string submitLogin() already reads.

    _otpBoxes(el) {
      return el.closest(".login-otp").querySelectorAll("input");
    },

    _otpSync() {
      this.login.code = this.login.digits.join("");
    },

    // Focus target once the 6th digit lands: the password field when one is
    // required, else the submit button — either way the operator's next
    // keystroke goes where the flow continues.
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

    // The url-encoded `POST /api/login` body: the code, the password when one is
    // enrolled, and `remember=true` only when the box is checked — the daemon
    // defaults an absent field to a standard session. Pure so the harness can
    // assert it without a network.
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
        // 6-digit fallback (M4) — that fallback exists only for the `file://`
        // demo. In daemon mode, surface the failure and stop.
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
    // The move destination picker (issue #364): browses directories one level at
    // a time through `tree.list`, so a move needs no new verb and no typed path.
    // `from` is the FULL rel path of the node being moved; `dir` is the
    // currently-browsed destination directory ("" is the repo root).
    movePick: { open: false, from: "", isFolder: false, dir: "", entries: [], busy: false, error: "" },
    // The Consoles tab wears the SAME terminal glyph as the New-console button
    // and the rows in its menu — one picture for one thing. It used to be a
    // robot, which named the agents rather than the plane they run on.
    tabs: [{ id: "consoles", kind: "consoles", title: "Consoles", icon: "bi bi-terminal", closable: false }],
    active: "consoles",

    // The seed project list is DEMO-ONLY and lives in
    // `assets/ui-demo/wb-seed-projects.js`, outside the tree the daemon embeds.
    // `loadRepos()` fills this from the real registry at init; off `file://` the
    // global is undefined and the list starts honestly empty (see also the
    // `seedAllowed()` wipe below).
    projects: window.WB_SEED_PROJECTS || [],

    // --- accordion --------------------------------------------------------
    toggle(ref, row) {
      this.openSlug = this.openSlug === ref ? null : ref;
      // The refusal notes name an act against the project that WAS open — for
      // the same reason `kanbanSel` is dropped below, they must not survive into
      // a project they do not describe.
      this.changesError = "";
      this.branchError = "";
      // Opening a row that lives on a sleeping peer wakes it. NOT awaited: the
      // rest of the accordion must not sit behind a cold WSL boot, and the wake
      // reloads the sidebar itself when it lands.
      if (this.openSlug === ref) this.wakePeerFor(ref);
      this.loadAgents(this.openSlug);
      // The chip's `<branch> · <name>` needs the listing when a checkout is
      // selected and the picker has not been opened this page (#406).
      if (this.openSlug === ref) this.ensureWorktreeListing(ref);
      // …and so does the Spend tab, whose whole subject is the open project.
      this.refreshSpend();
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
    //   waiting → yellow (an agent there is asking for you — ADR-0059)
    dotClass(state) {
      return state === "live"
        ? "live"
        : state === "waiting"
          ? "waiting"
          : state === "offline"
            ? "offline"
            : "";
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
      this.treeMem();
      // Freshness is per-open: every cached level must prove itself again against
      // the disk, because the watch that kept it honest died with the last close.
      this._treeValidated.clear();
      // The spinner is for a real WAIT, so it is armed only when the root level
      // has to come off the daemon. A re-open paints from memory in the same
      // frame, and a spinner that flashes for one frame on the fast path teaches
      // the operator to ignore it on the slow one.
      this.treeError = "";
      this.treeStale = "";
      // The checkout this tree is built for (#406): the cache key, every level
      // read and the watch below carry it; `setCheckout` remounts on a change.
      this._treeCheckout = this.checkoutOf(this.openSlug);
      // A mount generation: a root read that fails AFTER this tree was
      // replaced (an `unknown checkout` reply remounts the primary underneath
      // it) must not paint its error onto the fresh mount.
      const gen = (this._treeGen = (this._treeGen || 0) + 1);
      this.treeLoading = this.useDaemonTree() && !this._treeCache.has(this.treeKey(""));

      this._tree = new mar10.Wunderbaum({
        element: host,
        header: false,
        // The filter extension is what a FILES search narrows the tree with
        // (`applyFileSearch`). `autoApply` is what lets `updateFilter()` re-run
        // the last filter after a level (re)loads under it — without it the
        // extension warns and does nothing, and a reloaded level vanishes.
        filter: { autoApply: true, mode: "hide" },
        // Served over a daemon: seed the root level from `tree.list` (folders
        // marked `lazy` so expanding fetches their children on demand) and fall
        // back to the static seed if the read fails. Under `file://` (no
        // backend) keep the static tree.
        source: this.useDaemonTree()
          ? this.loadTreeLevel("").catch(() => {
              // A failed root read is the one case the operator cannot see: the
              // static seed would render a plausible tree that is not this repo's
              // (C1: never fabricate content). Say so instead, and render nothing.
              if (gen === this._treeGen) this.treeError = "could not read this project's files";
              return [];
            })
          : this.withIcons(project.tree),
        lazyLoad: (e) =>
          this.loadTreeLevel(this.relPath(e.node)).catch((err) => {
            // Rethrown: Wunderbaum must still mark the node as failed rather
            // than render it as an empty folder.
            this.treeWentStale(err);
            throw err;
          }),
        // A level (re)loaded while a search is on carries no match marks, and
        // in `hide` mode an unmarked row is not painted: re-run the filter so
        // the level shows what it should. Root load included.
        load: (e) => {
          if (e.tree.isFilterActive?.()) e.tree.updateFilter();
        },
        // The content-search badge: a row whose path is a hit shows its count
        // beside the title, and shows nothing once the filter is gone (the
        // row is re-rendered on `clearFilter`, and the map is empty by then).
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
        // Fired once the root level has settled — the end of the only wait the
        // operator sits through, and the moment the folders they left expanded
        // can be put back.
        init: (e) => {
          if (gen !== this._treeGen) return;
          this.treeLoading = false;
          if (e.error) this.treeError = "could not read this project's files";
          else this.restoreExpansion();
        },
        edit: {
          trigger: ["F2", "macEnter"],
          // A committed rename is an intent, not a mutation done here.
          apply: (e) => {
            // Rename-in-place: this is the caller that owns the composition,
            // since the shared listener now takes full rel paths.
            const parent = parentRel(this.relPath(e.node));
            this.emit("rename", e.node, {
              from: parent ? `${parent}/${e.oldValue}` : e.oldValue,
              to: parent ? `${parent}/${e.newValue}` : e.newValue,
            });
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
          this.rememberExpansion();
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
        this._treeSub = WBDaemon.subscribeTree(
          this.openSlug,
          (rel) => this.onTreeDirty(rel),
          this._treeCheckout,
        );
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

    // Snapshot which folders are open, so the next mount can put them back. Taken
    // on every expand/collapse rather than at close, because a project can also
    // leave the screen by a sidebar refresh or a peer going away, and neither of
    // those is a close we get to observe.
    rememberExpansion() {
      // A restore expands nodes itself, and each one fires this — recording a
      // half-restored tree would truncate the very list being replayed.
      if (this._restoringExpansion || !this._tree || !this.openSlug) return;
      this.treeMem();
      const rels = [];
      this.rawTree().root.visit((n) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      this._treeExpanded.set(this.openSlug, rels);
    },

    // Re-expand the folders this project was left with. Shallow-first, because a
    // child cannot be found before its parent has been loaded — and each level
    // comes from `_treeCache`, so this is a replay from memory, not a burst of
    // reads. Every expand still re-registers the daemon watch through the normal
    // `expand` handler, so the restored tree is as live as one expanded by hand.
    async restoreExpansion() {
      const slug = this.openSlug;
      this.treeMem();
      const rels = this._treeExpanded.get(slug) || [];
      if (!rels.length || !this._tree) return;
      this._restoringExpansion = true;
      try {
        for (const rel of [...rels].sort((a, b) => a.split("/").length - b.split("/").length)) {
          // The operator can close or switch projects mid-replay; expanding then
          // would edit a tree this list does not describe.
          if (slug !== this.openSlug || !this._tree) return;
          const node = this.rawTree().findFirst((n) => this.relPath(n) === rel);
          if (node && !node.expanded) await node.setExpanded(true);
        }
      } finally {
        this._restoringExpansion = false;
      }
    },

    // A real daemon backs the tree only when NOT loaded from `file://` (the
    // static-demo case, which has no `/ws/command` to talk to).
    useDaemonTree() {
      return window.WBMode.isDaemon() && !!window.WBDaemon?.observe;
    },

    // One directory level from the daemon (`tree.list`), mapped to Wunderbaum
    // node shape: folders lazy so they fetch their own children on expand.
    //
    // Cache-FIRST (stale-while-revalidate). Closing a project tears the tree down,
    // so re-opening one used to re-read every level from the daemon before a
    // single row could be painted — the operator paid the full first-open cost
    // for a project they had already opened. A level that was read once is now
    // painted from memory immediately and re-read in the BACKGROUND, and the
    // re-read only touches the DOM when the directory actually changed.
    //
    // This caches nothing on the daemon: `tree.list` still reads the disk fresh
    // on every request (ADR-0036). What is remembered is what this browser was
    // already shown, which is why the revalidation is not optional — the watch
    // that keeps a level honest is dropped when the project closes, so anything
    // remembered across a close is by definition unverified.
    loadTreeLevel(rel) {
      this.treeMem();
      const key = this.treeKey(rel);
      const hit = this._treeCache.get(key);
      if (!hit) return this.fetchTreeLevel(rel);
      // Stale until proven current: revalidate once per level per open. Deferred
      // so the paint happens first — that is the entire point of the cache.
      if (!this._treeValidated.has(key)) {
        this._treeValidated.add(key);
        setTimeout(() => this.revalidateLevel(rel), 0);
      }
      return Promise.resolve(this.treeNodes(hit));
    },

    // Lazily create the three tree-memory collections. Not initialised in the
    // data literal on purpose (see the note there).
    treeMem() {
      this._treeCache ||= new Map();
      this._treeValidated ||= new Set();
      this._treeExpanded ||= new Map();
      // path → count (content) or `true` (name): what the filter predicate reads.
      this._fileHits ||= new Map();
    },

    // The un-cached read: always the daemon, always fresh. Every caller that
    // needs the truth on disk (a reconcile after a `tree.dirty`, a revalidation)
    // goes through here, never through `loadTreeLevel` — a reconcile served from
    // the cache it is supposed to correct would validate itself and never notice
    // a change.
    // A read that FAILED throws; it does NOT resolve `[]`. An empty array is a
    // real statement — "this directory has no entries" — and every caller acts on
    // it by replacing what is on screen. Handing that answer back for a refusal or
    // a dropped socket is what turned an unreachable peer into a tree that
    // silently emptied itself, and a file the operator had just created into a
    // file that "did not exist" (2026-09-01). Only `entries` is empty; failure is
    // a rejection the caller has to decide about.
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

    // Evict every remembered level the fresh listing of `rel` CONTRADICTS.
    //
    // A cache key is a path, and a path is not an identity. Rename `ideias/` to
    // `ideias_vbforge/` and then create a new `ideias/`, and the new directory
    // inherits the old one's remembered children: the tree painted a `dossie/`
    // that this folder never held, and expanding it asked the daemon for a
    // directory that does not exist ("not found"). Nothing corrected it either —
    // the level was already in `_treeValidated`, so the stale-while-revalidate
    // path skipped the re-read that would have noticed (2026-09-09).
    //
    // The listing of a directory is the statement that decides this: a name that
    // is no longer among its subdirectories cannot have children, so its whole
    // remembered subtree goes. That covers the rename above, an outright delete,
    // and a directory replaced by a file of the same name.
    pruneTreeCache(rel, entries) {
      this.treeMem();
      const dirs = new Set(entries.filter((en) => en.dir).map((en) => en.name));
      // The keys DESCENDING from `rel` — its own key ends here (empty `child`)
      // and is left alone: this listing is what replaces it.
      const prefix = this.treeKey(rel === "" ? "" : `${rel}/`);
      for (const key of [...this._treeCache.keys()]) {
        if (!key.startsWith(prefix)) continue;
        const child = key.slice(prefix.length).split("/")[0];
        if (!child || dirs.has(child)) continue;
        this._treeCache.delete(key);
        this._treeValidated.delete(key);
      }
    },

    // Cache key. Scoped by REPO and by CHECKOUT: two projects have their own
    // `src/`, and so do two trees of one project (#406) — a key of `rel` alone
    // would show one tree's directory inside the other.
    treeKey(rel) {
      return `${this.openSlug}\n${this.checkoutOf(this.openSlug) || ""}\n${rel}`;
    },

    // Daemon entries → fresh Wunderbaum node specs. Rebuilt on every call rather
    // than cached as nodes: the tree OWNS the objects it is given (it decorates
    // and mutates them), so handing the same objects to a second mount would let
    // a destroyed tree's leftovers into the new one.
    treeNodes(entries) {
      return entries.map((en) =>
        en.dir
          ? { title: en.name, folder: true, lazy: true }
          : { title: en.name, icon: this.fileIcon(en.name) },
      );
    },

    // Re-read a level that was painted from cache and reconcile it ONLY if the
    // directory really changed. The comparison is what keeps this cheap: the
    // common case (nothing changed while the project was closed) costs one read
    // and zero DOM work, instead of the full `removeChildren()` + reload +
    // re-expand cascade — which would flicker the tree on every re-open and undo
    // the win this cache exists for.
    revalidateLevel(rel) {
      if (!this._tree || !this.useDaemonTree()) return Promise.resolve();
      this.treeMem();
      const key = this.treeKey(rel);
      const before = JSON.stringify(this._treeCache.get(key) ?? null);
      const slug = this.openSlug;
      return this.fetchTreeLevel(rel)
        .then(() => {
          // The project may have been closed or switched while this was in
          // flight; reconciling then would edit another project's tree.
          if (slug !== this.openSlug || !this._tree) return;
          if (JSON.stringify(this._treeCache.get(key) ?? null) === before) return;
          const node = rel === "" ? this.rawTree().root : this.findFolderByRel(rel);
          if (node) return this.reconcileLevel(node, rel);
        })
        // A dropped read leaves the cached level on screen — but says so, because
        // a level that could not be revalidated is exactly the one the operator
        // must not trust.
        .catch((err) => this.treeWentStale(err));
    },

    // The tree is showing a listing it could not confirm. Records the reason for
    // the FILES gutter and leaves every row alone: the last known listing beats
    // both a blank panel (which reads as "this project is empty") and a fabricated
    // one (C1). Cleared by `treeFresh` on the next read that succeeds.
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
    // One field under the FILES bar, a Name | Content toggle, and the tree
    // itself as the result: the daemon answers with the hits' rel paths, the
    // ancestors of every hit are loaded, and Wunderbaum's filter hides every
    // other row. The decisions are `WBFileSearch`'s; the tree and the socket
    // are handled here.
    toggleFileSearch() {
      if (this.fileSearch.open) this.closeFileSearch();
      else this.openFileSearch();
    },

    openFileSearch() {
      if (!this.openSlug) return;
      this.fileSearch.open = true;
      this.$nextTick(() => this.$refs.fileSearch?.focus?.());
    },

    // Escape, or the lupe again: the field goes, the query goes, the tree is
    // put back the way the operator had it.
    closeFileSearch() {
      this.fileSearch.open = false;
      this.fileSearch.query = "";
      this.fileSearch.seq++;
      return this.clearFileSearch();
    },

    // The clear button: the query goes and the tree comes back, the field
    // stays open and focused for the next one.
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

    // A keystroke arms the debounce; a query under the floor clears instead of
    // searching, so deleting back to one character restores the tree at once.
    fileSearchTyped() {
      clearTimeout(this._fileSearchTimer);
      if (!window.WBFileSearch.worthSearching(this.fileSearch.query)) {
        this.fileSearch.seq++;
        this.clearFileSearch();
        return;
      }
      this._fileSearchTimer = setTimeout(() => this.fileSearchNow(), window.WBFileSearch.DEBOUNCE_MS);
    },

    // The search itself: one Observe read, dated by `seq`. A reply that is not
    // the newest — or that arrives after the project switched — is dropped,
    // never painted. Returns the promise so a test can await the settle.
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
      // Find and grep walk the SELECTED tree (#406): hits come back relative
      // to it, which is exactly what the tree's `relPath` speaks.
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

    // The Wunderbaum instance WITHOUT Alpine's reactive proxy around it.
    // `_tree` lives in the component's data, so `this._tree` hands back a
    // Proxy, and every node reached through it is a Proxy too — while the
    // tree's own timers and event handlers hold the raw objects. Wunderbaum's
    // row painter compares nodes by identity, and a paint that mixes the two
    // views leaves rows behind at stale offsets (2026-09-15: a filtered tree
    // painted two rows at the bottom and nothing else; 2026-09-16: an iPad's
    // tree painted its two root rows a third of the way down an otherwise
    // blank panel until a scroll repainted it). Everything that reads or
    // mutates the tree goes through the raw instance; `this._tree` itself is
    // only ever assigned, null-checked and destroyed.
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
    // session, load every ancestor level (shallow-first, cache-first — a
    // `setExpanded` on a lazy folder is the load), then filter. The expands
    // run under `_restoringExpansion` because they are the search's, not the
    // operator's: the remembered expansion must not learn them.
    async applyFileSearch(hits, truncated, seq) {
      this.treeMem();
      this.fileSearch.hits = hits;
      this.fileSearch.truncated = truncated;
      this.fileSearch.note = window.WBFileSearch.note({ hits, truncated });
      const tree = this.rawTree();
      if (!tree) return;
      if (this.fileSearch.expandedBefore === null) this.fileSearch.expandedBefore = this.expandedRels();
      // ONE paint, at the end. Every lazy level that lands repaints the tree
      // on its own (status node, addChildren, the `load` re-filter), and on a
      // deep repo that was hundreds of paints per search — one of which
      // could land between the old marks being cleared and the new ones set,
      // and leave a blank or one-row tree that nothing repainted afterwards
      // (2026-09-15, VIBEFORGE over the WSL peer). Holding updates until the
      // levels are in and the filter is on turns the sequence into a single
      // consistent paint; the operator keeps the previous rows meanwhile.
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
        // `autoExpand: false`: the ancestors are already open (above), and
        // the extension's own auto-expand also opens every MATCHED folder —
        // a burst of lazy loads nobody asked for, each a repaint.
        tree.filterNodes((n) => this._fileHits.has(this.relPath(n)), {
          mode: "hide",
          autoExpand: false,
          matchBranch: false,
          noData: false,
        });
      } finally {
        this._restoringExpansion = false;
        // A narrowed tree starts at the top. Set BEFORE the paint: the row
        // window is computed from `scrollTop`, and a scroll offset left over
        // from the taller, unfiltered list painted two rows at the bottom of
        // a 52-row tree — with nothing else on screen and no repaint coming.
        if (tree === this.rawTree()) {
          tree.element.scrollTop = 0;
          // Re-enabling paints immediately and in full (`update(any)`),
          // whether this pass finished or yielded to a newer one.
          tree.enableUpdate(true);
        }
      }
    },

    // Take the filter off and fold back what the search opened, deepest first.
    // The remembered expansion never learned the search's expands, so the
    // tree returns to the operator's own shape.
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
      // Same one-paint discipline as `applyFileSearch`: the unfilter and every
      // fold are one change to the operator, and painted once.
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

    // Fetch a file's real bytes via `file.read`; on refusal surface the daemon's
    // reason (binary / too large / not found) and close the just-opened tab.
    // Returns `null` when refused so the caller skips the viewer. `checkout`
    // is the tab's PINNED checkout (#406), not the current selection.
    fetchContent(project, path, ftype, checkout) {
      if (!this.useDaemonTree()) return Promise.resolve(fakeContent(path, ftype));
      // An image is a different read (`file.image`, ADR-0049) whose "content" is
      // a `data:` URL, not text. Same refusal shape: surface the reason, close
      // the tab, hand back `null`.
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
          // Daemon mode: a transport drop must NOT fall back to `fakeContent`
          // (C1) — surface the failure and close the tab, mirroring refusal.
          WB.emit("open-refused", { project, path, reason: "transport" });
          this._flashAction?.("read failed");
          this.closeTab(fileTabId(project, path, checkout));
          return null;
        });
    },

    // A `tree.dirty` nudge for `rel`: refetch that one directory level IF it is
    // currently on screen (the root, or an expanded folder). A nudge for a
    // collapsed/absent dir is DROPPED — the change is invisible, so re-listing it
    // would be wasted traffic (ADR-0036 §4).
    onTreeDirty(rel) {
      const tree = this.rawTree();
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
        // A reconcile that could not read the level leaves every row in place
        // (`_reconcileOnce` resolves the listing BEFORE it touches the tree), so
        // all that is left to do is say the rows are unconfirmed.
        .catch((err) => this.treeWentStale(err))
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
      this._reconcileWaiters ||= new Map();
      // A COALESCED caller still gets a promise that settles when the level does.
      // Returning early instead made `await onTreeDirty(dir)` mean "a pass is
      // running", not "the level has settled" — so a reveal awaiting it ran
      // against children the pending pass was about to tear down, and that pass's
      // own captured `activeRel` (the node active BEFORE the reveal) won the last
      // write. Symptom: the just-created entry appears, unselected.
      if (this._reconciling.has(rel)) {
        this._reconcilePending.add(rel);
        return new Promise((resolve) => {
          if (!this._reconcileWaiters.has(rel)) this._reconcileWaiters.set(rel, []);
          this._reconcileWaiters.get(rel).push(resolve);
        });
      }
      this._reconciling.add(rel);
      // The drain is in an OUTER `finally` because `_reconcileOnce` awaits
      // `loadTreeLevel`, and `WBDaemon.observe` REJECTS by contract on a socket
      // drop — a drain in the happy path alone would be skipped by that throw and
      // every coalesced awaiter would hang forever (`duplicateNode` never reaching
      // its reveal). A settle-with-a-failed-level is still a settle; the level's
      // freshness is the caller's business, its liveness is not.
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
        // A pass that threw never consumed its pending flag; clearing it here
        // keeps a dropped nudge from pinning the level as permanently dirty.
        this._reconcilePending.delete(rel);
        // Drain by SPLICE, not by clearing: the nested re-run above coalesces
        // (this `rel` is still in `_reconciling` while it runs), so waiters that
        // arrive during it land on THIS list and are drained here — once, never
        // twice, never lost.
        const waiters = this._reconcileWaiters.get(rel);
        if (waiters?.length) for (const r of waiters.splice(0)) r();
      }
    },

    async _reconcileOnce(nodeAtCall, rel) {
      let node = nodeAtCall;
      // The selection this pass restores is a SNAPSHOT, and the reload below
      // awaits the network — so a reveal (a create, a duplicate) can land
      // mid-pass and be undone by this pass's own stale restore. `_revealSeq`
      // dates the snapshot: a reveal after it wins.
      const seq = this._revealSeq || 0;

      // Resolve the fresh level BEFORE touching the tree: `node.load` given a
      // PROMISE leaves stale children in place and appends (children double on a
      // second nudge — Wunderbaum quirk); loading a resolved ARRAY after
      // `removeChildren()` replaces cleanly and avoids an empty-tree flicker
      // during the fetch.
      // FRESH, never the cache: this pass exists to correct the level, so it must
      // read the disk (see `fetchTreeLevel`).
      const source = await this.fetchTreeLevel(rel);
      // The fetch is a wait, and a reload of an ANCESTOR level can land inside
      // it: its `removeChildren()` unregisters this node (`node.tree` goes
      // null), and `removeChildren()` on the dead node then throws inside
      // Wunderbaum — Safari's "null is not an object (evaluating
      // 'i.activeNode')" (2026-09-16, iPad) — which painted the stale gutter
      // over a tree that was in fact fresh. The per-rel guard above cannot see
      // this: the two passes are for different rels. Re-resolve by rel: the
      // node that now stands for this level is the one to reconcile. None at
      // all is a level that no longer exists; one still loading is the
      // ancestor's own re-expansion listing it from disk right now, and a
      // teardown under an in-flight load doubles the children when it lands.
      if (!node.tree) {
        const raw = this.rawTree();
        node = rel === "" ? raw?.root : raw?.findFirst((n) => this.relPath(n) === rel);
        if (!node || node.isLoading?.()) return;
      }
      // The expansion to put back is read AFTER the fetch, right before the
      // teardown: the fetch is the long wait, and what the tree looks like on
      // the far side of it is what the operator has. Read before it, the
      // snapshot missed every folder a FILES search opened meanwhile, and the
      // reload put a filtered tree back collapsed — every match hidden under
      // a closed parent, i.e. a blank panel (2026-09-15).
      const expandedRels = [];
      node.visit((n) => {
        if (this.isFolder(n) && n.expanded) expandedRels.push(this.relPath(n));
      });
      const activeRel = this.relPath(this.rawTree()?.getActiveNode?.() || null) || null;
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
        const f = this.rawTree()?.findFirst((n) => this.relPath(n) === r);
        if (f && !f.expanded) await f.setExpanded(true);
      }
      const target = (this._revealSeq || 0) > seq ? this._revealedRel : activeRel;
      if (target) await this.revealRel(target, { restore: true });
      // A search is on: the reloaded level has no match marks yet, and in
      // `hide` mode that is a blank level. Same fix as the `load` hook — on
      // the raw instance, for the reason `rawTree` gives.
      const raw = this.rawTree();
      if (raw?.isFilterActive?.()) raw.updateFilter();
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
        // The nudge is the mounted tree's; a tab pinned to another checkout of
        // the same project (#406) holds bytes this nudge says nothing about.
        if ((t.checkout ?? null) !== (this._treeCheckout ?? null)) continue;
        // An image tab re-reads through its OWN verb: `file.read` would refuse
        // its bytes, and the drop-on-failure rule below would then make an image
        // the one viewer that never refreshes (ADR-0049 §1).
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
    // `opts.restore` marks the reconcile path's own re-activation, which replays
    // an existing intent rather than expressing a new one — it must NOT bump
    // `_revealSeq`, or a stale pass would date its restore as newer than the
    // reveal it is about to undo.
    async revealRel(rel, opts = {}) {
      const tree = this.rawTree();
      if (!tree || typeof rel !== "string" || rel === "") return null;
      if (!opts.restore) {
        this._revealSeq = (this._revealSeq || 0) + 1;
        this._revealedRel = rel;
      }
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
      return this.rawTree()?.findFirst((n) => this.isFolder(n) && this.relPath(n) === rel) || null;
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
      this._treeCheckout = null;
      try {
        this._tree?.destroy?.();
      } catch {}
      this._tree = null;
      document.querySelectorAll(".wb-host").forEach((h) => (h.innerHTML = ""));
      // Both states describe a tree that no longer exists; carrying either into
      // the next project would report ITS read wrongly.
      this.treeLoading = false;
      this.treeError = "";
      // A search describes THIS tree: its hits, its note and the expansion it
      // took are gone with it. The row stays open if the operator left it so —
      // the next project's tree is searchable from the first keystroke.
      this.resetFileSearch();
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
        this._flashAction?.("Cannot open binary files.");
        return;
      }
      // Opened out of a CONTENT search: the tab lands on the first occurrence
      // and the find widget carries the term (ADR-0036 amendment 2026-09-15).
      const find = this.fileSearchFindTerm();
      this.openTab({ project: this.openSlug, path, title: node.title, ftype, find });
    },

    // The term a tab opened from the tree should land on: the live query, and
    // only while the CONTENT filter is on — a name search says nothing about
    // what is inside the file.
    fileSearchFindTerm() {
      const fs = this.fileSearch;
      if (!fs.open || fs.mode !== "content" || !fs.hits.length) return null;
      const q = String(fs.query ?? "").trim();
      return q || null;
    },

    // A rendered markdown link to another repo file: the viewer only asked, the
    // shell decides — same viewer choice and same binary refusal as a click in
    // the tree, minus the tree node (a link names a path, not a loaded node).
    // `checkout` is the SOURCE pane's pin (#406): a link inside a document
    // shown from a worktree names that worktree's file, whatever the project's
    // selection is by the time it is clicked; an older relay without the key
    // falls back to the selection.
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

    // `content` is optional: opening from the tree synthesises it, re-attaching
    // a detached popup passes the current (possibly edited) bytes back in.
    // `fragment` is a `#heading` to land on once the bytes are shown — a link
    // into a document carries one; the tree never does.
    // `find` is a term to land on (a content-search open); like `fragment`, it
    // is applied once the bytes are shown, and on an already-open tab at once.
    // `checkout` PINS the tab to the tree it was opened in (#406): the default
    // is the project's current selection, and the tab keeps it for its own
    // reads and its Save whatever the selection does afterwards — a Save from a
    // tab showing worktree bytes must never land on the primary's file.
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
        // A re-attach passes its (possibly edited) bytes in; a fresh open fetches
        // the real file via the daemon (`file.read`), falling back to the seed.
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
          // NOT `setActive(id)`: the read that just landed does not get to own
          // the screen. `restoreView` opens N tabs in one burst and only THEN
          // activates the stored one, so N reads resolve after it — with
          // `setActive(id)` the last one to answer showed its pane while
          // `active` named another tab. The divergence was visible: with
          // `active === "consoles"` the workspace and its toolbar stayed up
          // (they are `x-show`-bound) and a file pane painted over the live
          // consoles, until any tab click ran `activate` and reconciled it.
          this.syncViewer();
          window.lucide?.createIcons();
          if (fragment) WBViewer.jumpTo(id, fragment);
          if (find) WBViewer.find(id, find);
        });
      });
    },

    // --- opening a Changes row into a diff tab ----------------------------
    // HEAD on one side, the working tree on the other, so reviewing what the
    // agent wrote is one gesture away from noticing it (#311). Read-only: no
    // commit, no discard, no staging. Monaco computes the diff from the two
    // texts, so nothing here — and nothing in the daemon — produces a patch.
    openDiff(project, entry) {
      // Pinned to the selection at open, like a file tab (#407): both sides
      // read from `t.checkout`, so a later switch of the picker never makes
      // the pane diff one tree's HEAD against the other's file.
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
              checkout: t.checkout,
              label: this.projectLabel(project),
              path: t.workingPath,
              ftype: "diff",
              content: work,
              original: head,
            });
            // Same rule as `openTab`: the pane follows the CURRENT active tab,
            // never the id of the read that just resolved.
            this.syncViewer();
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
      return WBDaemon.observe(
        "file.read",
        WBDaemon.withCheckout({ repo: project, path: t.workingPath }, t.checkout),
      ).then((reply) => {
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
      const id = fileTabId(desc.project, desc.path, desc.checkout);
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
      // The Spend tab's subject is the OPEN PROJECT, which can change while the
      // tab sits in the background — so coming back to it re-reads rather than
      // showing the cost of whatever was open last time.
      if (id === "spend" && this.spend.slug !== this.openSlug) this.refreshSpend();
      this.$nextTick(() => {
        this.syncViewer();
        window.lucide?.createIcons();
        // A console opened/reattached while another tab was active measured 0×0
        // (its tab was display:none); refit now that the Consoles tab is visible.
        if (id === "consoles") window.WBConsole?.refitAll?.();
      });
      this.persistView();
    },

    // The ONE place that tells the viewer which pane is on screen. `setActive`
    // reconciles every record against the tab that is active NOW, so an async
    // opener calling it late can only ever converge — the three callers
    // (`activate`, `openTab`, `openDiff`) hand it no id of their own.
    // The tabs that own no pane — Consoles and Spend, both `x-show` sections of
    // their own — map to `null`, i.e. show none.
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
        .map((t) => ({
          project: t.project,
          path: t.path,
          title: t.title,
          kind: t.kind,
          // The pin (#406) survives a reload: a restored tab must read the tree
          // it was opened in, not whatever is selected when the restore runs.
          checkout: t.checkout ?? null,
        }));
      // A stored `active` naming a tab this store does not carry (a diff tab, or
      // one that just closed) would restore to a tab that never opens, leaving
      // the canvas blank — degrade to Consoles instead.
      const alive =
        this.active === "consoles" ||
        files.some((f) => fileTabId(f.project, f.path, f.checkout) === this.active);
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
      // Always the primary: a console is born in `current` and only its own
      // title switcher moves it (ADR-0063, amendment 2026-09-16 b). The
      // Files chip's selection is what the panels SHOW, never where an agent
      // is launched.
      WBConsole.open({ repo: this.openSlug, agent, checkout: null });
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
      if (this.whatsNewOpen) return true;
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
      return `Maximum of ${window.WBConsole.FENCE_MAX} fences. Remove one to add another.`;
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
        // For folders too: moving a whole directory is the gesture that made the
        // explorer's tree editable rather than only its leaves. Dropped entirely
        // for a node inside `.git`/`.ralphy`, whose every move the daemon
        // refuses on the SOURCE — offering it would only promise a dead end.
        node && !underProtectedDir(this.relPath(node)) && {
          label: "Move to…",
          icon: "bi-arrow-right-square",
          run: () => this.moveNode(node),
        },
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
        // Windows by SHAPE (a drive letter or a UNC lead), not by "contains a
        // backslash" — a POSIX directory may legitimately have one in its name.
        const sep = /^[A-Za-z]:/.test(root) || root.startsWith("\\\\") ? "\\" : "/";
        // Trim a trailing separator so a drive root (`C:\`) does not double it.
        const base = root.replace(/[\\/]+$/, "");
        path = base + sep + rel.split("/").join(sep);
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

      // The tree's gestures speak the tree's checkout (#406): the listing that
      // picks the free name, and the copy itself, aim at the selected tree —
      // the daemon refuses a worktree write for now, and a refusal is the
      // honest answer where a primary-aimed copy would be a silent misfire.
      const checkout = this.checkoutOf(this.openSlug);
      const listing = await WBDaemon.observe(
        "tree.list",
        WBDaemon.withCheckout({ repo: this.openSlug, path: parent }, checkout),
      ).catch(() => null);
      // A refused or dropped listing must NOT degrade to an empty `taken` set:
      // that proposes `<stem> copy<ext>` blindly and turns a readable "couldn't
      // list the folder" into the daemon's flat `exists`.
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
    // Move a file or a folder to another directory. The destination is PICKED,
    // never typed: the picker browses real directories through `tree.list`, so
    // an operator cannot compose a path that does not exist.
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
      // A listing is a round trip, so a superseded or cancelled navigation can
      // still land. Stamp the request and drop a reply that is no longer the
      // one being awaited — otherwise two quick clicks settle out of order.
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
      // A refused or dropped listing surfaces as a REASON, not as an empty
      // folder: "nothing in here" and "we could not look" are different answers
      // and only one of them means the operator should pick elsewhere.
      if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
        this.movePick.entries = [];
        this.movePick.error = WBFail.message(listing, "couldn't list the folder");
        return;
      }
      const from = this.movePick.from;
      this.movePick.dir = dir;
      this.movePick.entries = listing.entries
        .filter((e) => e.dir)
        // A folder cannot move into itself or its own subtree: `fs::rename`
        // fails there as a flat `Io`, so the picker refuses by not offering it.
        .filter((e) => {
          const rel = dir ? `${dir}/${e.name}` : e.name;
          return rel !== from && !rel.startsWith(`${from}/`);
        })
        // …and never offer a destination the Write path refuses anyway
        // (`fswrite::PROTECTED_DIRS`). `tree` deliberately still LISTS `.ralphy`
        // so the plan and the run artifacts stay watchable, so without this the
        // picker would advertise a destination every move dead-ends on.
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

    // "Move here" is dead while the browsed directory IS the source's current
    // parent (the move would be a no-op the daemon reports as a flat `exists`),
    // and while the listing FAILED — `dir` still names the last directory that
    // loaded, so a live button there would move into somewhere the operator was
    // just told could not be read.
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
      // A move into the directory it already sits in is a no-op the daemon would
      // report as a bare `exists` — name the reason here instead.
      if (dir === parentRel(from)) {
        this.movePick.error = "it is already in that folder";
        return;
      }
      this.movePick.open = false;
      return this.performMove(from, dir ? `${dir}/${leaf}` : leaf);
    },

    // The move itself. Written through `WBDaemon.write` rather than the
    // fire-and-forget `WB.emit("rename")` path because the reveal, the refusal
    // flash and the tab re-path all need the reply, which `call()` discards.
    // INVARIANT: no tab is re-pathed and no reveal happens on a refusal — the
    // two early returns below are the only exits before the tab/reveal block.
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
      // The reveal goes LAST so its `setActive()` is the final write and a
      // concurrent watcher nudge cannot deselect what was just moved.
      await this.revealRel(to);
    },

    // Re-point every open tab under the moved path. A tab's id IS its path
    // (`file:<project>:<rel>`), so a move that left the ids alone would leave
    // saves writing to the old location.
    repathTabs(from, to) {
      // Snapshot: the collision branch CLOSES a tab, which mutates `this.tabs`.
      for (const t of [...this.tabs]) {
        if (t.kind === "diff" || t.project !== this.openSlug) continue;
        if (t.path !== from && !t.path?.startsWith(`${from}/`)) continue;
        const newPath = to + t.path.slice(from.length);
        const newId = fileTabId(t.project, newPath, t.checkout);
        if (this.tabs.some((x) => x.id === newId)) {
          // The destination is ALREADY open in another tab. Leaving this one on
          // the old path would leave a live editor whose next save recreates the
          // file at the location the move just emptied — close it instead; the
          // bytes are reachable through the tab that already holds them.
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

// The live Alpine component instance (Alpine stores it on the x-data element).
//
// On `window` explicitly, not as a bare function declaration. Two other served
// modules call it, and a bare declaration only reaches them because this file is
// evaluated at global scope — the first wrapper, IIFE or module around app.js
// takes the name away.
//
// NOT a silent failure, to be exact: the optional chain in `getShell()?.x` sits
// after the CALL, so a missing binding throws rather than yielding undefined.
// The point is that the binding is then a property nobody declared, reachable
// only by accident of scope; naming it on `window` is what makes the two
// cross-module callers legible as callers.
window.getShell = function getShell() {
  const root = document.querySelector("[x-data]");
  return root && root._x_dataStack ? root._x_dataStack[0] : null;
};

// Keep the Alpine mirror of the live console count fresh (windows can close
// themselves via their own chrome, outside the New-console button).
document.addEventListener("workbench:consoles-changed", (e) => {
  const c = window.getShell();
  if (c) c.consoleCount = e.detail.count;
});

// …and of the stage extent, for the frame's second footer pill (issue #338).
// `wb-console.js` only emits this when a number actually changed — a drag folds
// the extent per mousemove.
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
  } else if (m.type === "wb-open-request" && m.detail) {
    // A link clicked inside a detached pane; the same guards above vouch for
    // the sender, and `openLink` re-classifies the path as it would for the
    // in-shell event, so the popup decides nothing about what opens.
    window.getShell()?.openLink({
      project: m.detail.project,
      path: m.detail.path,
      fragment: m.detail.fragment,
      checkout: m.detail.checkout ?? null,
    });
  } else if (m.type === "wb-reattach" && m.desc) {
    // The pin comes home with the bytes (#406): an explicit `null` is the
    // primary, never "whatever is selected now".
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

// --- Write byte-ops (#197): route the workspace-mutating seam actions to the
// daemon's confined `file.*` verbs. Daemon-backed only (a `file://` standalone
// demo keeps its synthesised behaviour); a confinement/conflict refusal comes
// back as `{status:"error",reason}` and is flashed. The browser composes the
// full rel path from the tree node — the daemon verbs take a complete rel path.
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
    // tab's PIN (`d.checkout`, `null` for the primary — an explicit null must
    // not fall through to the selection), a tree gesture says the project's
    // current selection. The daemon refuses a write under a worktree for now;
    // what this guarantees is that it never lands on the primary's file by
    // silently dropping the key.
    const checkout =
      d.checkout !== undefined ? d.checkout : (window.getShell()?.checkoutOf?.(repo) ?? null);
    const aimed = (payload) => WBDaemon.withCheckout(payload, checkout);
    switch (d.action) {
      case "save":
        call("file.write", aimed({ repo, path: d.path, content: d.content || "" }));
        break;
      case "worktree-created": {
        // A console's switcher cut a worktree: the shell's listing (the Files
        // chip, the branch chip's dirty dot) re-reads, and what the add had
        // to say — the carry-over entries it skipped — is flashed verbatim.
        const c = window.getShell();
        c?.ensureWorktreeListing?.(repo, true);
        if (d.message) c?._flashAction?.(d.message);
        break;
      }
      case "create": {
        // The tree emits `create` carrying the target DIRECTORY and no name
        // (`emitCreate` already resolved a file node to its parent, and no node
        // at all to the repo root ""). Ask for the name, compose the full rel
        // path the daemon verb expects, and — for a file — open it once the
        // write lands, so creating a file leaves the operator in it.
        const folder = d.kind === "folder";
        const where = d.path || "repo root";
        const c = window.getShell();
        const name = c
          ? await c.askPrompt({
              // The title carries the destination; the field below it asks for
              // the name, so no message. No placeholder either: a plausible
              // filename sitting in an empty field reads as a name already
              // chosen, and operators pressed Enter on it.
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
        // `from`/`to` are FULL rel paths: this listener is shared with the move
        // gesture, whose destination is in another directory entirely. The one
        // caller that renames in place (the inline edit) composes its own parent.
        call("file.rename", aimed({ repo, path: d.from, to: d.to }));
        break;
      }
      case "delete": {
        // Deleting is irreversible (a folder removes recursively, server-side);
        // confirm through the design-system dialog before the verb leaves the
        // browser. Fall back to the native confirm if the shell is unreachable.
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
        // "not found" on a delete says the ROW is the lie, not the disk: the
        // operator is trying to remove something that is already gone. Left
        // alone, the gesture is a dialog that changes nothing and the row is
        // unkillable — the state a cache a rename invalidated used to leave
        // behind. Re-list the parent so the ghost still ends up off the screen.
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

// Alt+Shift+<digit> → the menu row carrying that digit, invoking the SAME row
// action as clicking it (reach a live session, else launch): one code path, so a
// digit can never launch the duplicate its row refuses to. The digits come from
// the daemon's roster; digit 0 is the plain console. Matched on the physical key
// (e.code) so layout / macOS Option glyphs don't matter; guarded so it never
// hijacks a text field, modal, or the login.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  if (!/^Digit\d$/.test(e.code)) return;
  const c = window.getShell();
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
  const c = window.getShell();
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
  const c = window.getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  if (!c.jumpFenceAt(Number(e.code.slice(1)))) return;
  e.preventDefault();
});

// `/` → the search that is on screen: the FILES search while a project is
// open (the project box is hidden then — it filters rows `has-open` already
// hides), the project search otherwise. Reuses consoleShortcutsBlocked so it
// never hijacks a text field, modal, or the login.
document.addEventListener("keydown", (e) => {
  if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
  const c = window.getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  e.preventDefault();
  if (c.openSlug) c.openFileSearch();
  else c.focusProjectSearch();
});

// Ctrl/Cmd+Shift+F → the FILES search of the open project. NOT
// `consoleShortcutsBlocked`: that guard yields to any text field, and the
// editor is exactly where an operator reaches for "find in files" from. Only
// the login and a modal keep it out.
document.addEventListener("keydown", (e) => {
  if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey) return;
  if (e.key !== "F" && e.key !== "f") return;
  const c = window.getShell();
  if (!c || !c.authed || !c.openSlug) return;
  if (c.settingsOpen || c.securityOpen || c.runOpen || c.branchOpen || c.whatsNewOpen) return;
  e.preventDefault();
  c.openFileSearch();
});

// Inbound run events, `file://` demo ONLY (#300): the fold that advances the
// panel from a `{ type, runid, data }` detail is the demo's stand-in for the live
// feed. In daemon mode the panel advances by snapshot replacement instead, so the
// listener is gated here AND in `applyRunEvent` (the method is also called
// directly by `demoTick`). `window.WBRuns.emit(evt)` is the console door.
document.addEventListener("ralphy:run-event", (e) => {
  if (!window.WBMode.seedAllowed()) return;
  window.getShell()?.applyRunEvent(e.detail);
});
window.WBRuns = {
  emit(evt) {
    document.dispatchEvent(new CustomEvent("ralphy:run-event", { detail: evt }));
  },
  // Phase 1: append a raw output chunk from a daemon-spawned run into the panel,
  // capping the buffer so a long run never grows the DOM unbounded.
  output(text) {
    const c = window.getShell();
    if (c) c.rawFeed = (c.rawFeed + text).slice(-8000);
  },
};
