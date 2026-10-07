/* ---------------------------------------------------------------------------
   ralphy workbench shell — shell behaviour

   The sidebar is a project accordion (Alpine); the file tree is a Wunderbaum
   instance. The canvas is a tabbed workspace: "Consoles" is fixed and hosts
   the floating console windows (wb-console.js); every opened file is its own
   closable tab rendered by a viewer (wb-viewer.js).

   Every user gesture becomes one CustomEvent, `workbench:action`, on
   `document`. That event IS the seam: a backend subscribes and does the work.
--------------------------------------------------------------------------- */
import type { AlpineMagics } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";

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

function extOf(name: any) {
  const n = name.toLowerCase();
  return n.includes(".") ? n.split(".").pop() : "";
}

// The directories the daemon's Write path refuses (`fswrite::PROTECTED_DIRS`),
// mirrored so the UI never offers a gesture that can only be refused.
// Case-insensitive: NTFS resolves `.GIT` to `.git`.
const PROTECTED_DIRS = [".git", ".ralphy"];

function isProtectedDir(name: any) {
  return PROTECTED_DIRS.some((p) => name.toLowerCase() === p);
}

// Whether `rel` names or traverses a protected directory (the daemon's own
// component test), with the daemon's one carve-out: a note in the notes
// landing directory IS writable (ADR-0064 §5), so the tree may offer rename
// and delete on it. Exactly `.ralphy/notes/<name>.note`, spelled that way —
// the same narrow shape `fswrite::is_note_in_notes_dir` opens.
function isNoteInNotesDir(rel: any) {
  // `.` and empty segments are dropped first: `Path::components()` on the
  // daemon's side collapses them, so `.ralphy/./notes/x.note` is one path
  // there and would be two different answers here.
  const parts = rel.split("/").filter((p: any) => p && p !== ".");
  return (
    parts.length === 3 &&
    parts[0] === ".ralphy" &&
    parts[1] === "notes" &&
    parts[2].length > ".note".length &&
    parts[2].endsWith(".note")
  );
}

function underProtectedDir(rel: any) {
  if (isNoteInNotesDir(rel)) return false;
  return rel.split("/").some(isProtectedDir);
}

// The title of a create gesture, one sentence with its word order kept
// whole (ADR-0065 §9). `dir` is "" for the top of the project.
function newEntryTitle(kind: any, dir: any) {
  return `New ${kind} in ${dir || "the project root"}`;
}

// The directory containing `rel`; "" for a top-level entry (the repo root).
function parentRel(rel: any) {
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

// A file tab's identity (#406): project, path and — ONLY under a selected
// worktree — the checkout, so the same rel in two trees is two tabs (the
// primary's id is the pre-#406 spelling, byte for byte).
function fileTabId(project: any, path: any, checkout: any) {
  return checkout ? `file:${project}@${checkout}:${path}` : `file:${project}:${path}`;
}

// Which viewer a file gets: markdown → rendered pane, image → image pane,
// a note → its CARD on the consoles stage (ADR-0064 §11, never a tab: two
// editors over one file is the thing that decision exists to prevent), other
// binaries refused, everything else source code.
function classify(name: any) {
  const ext = extOf(name);
  if (ext === "note") return "note";
  if (ext === "md" || ext === "markdown") return "markdown";
  if (IMAGE_EXT.has(ext)) return "image";
  if (BINARY_EXT.has(ext)) return "binary";
  return "code";
}

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
    openSlug: null as any,
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
    // Working-tree change count per slug (#307). `null` until a load succeeds,
    // so a failed read never reads like a clean tree; `changesReadError`
    // carries the reason into the Changes view's title.
    changesCount: {} as Record<string, any>,
    // Per slug, named apart from the shell-wide `changesError` below: a
    // duplicate key in this literal is a silent no-op.
    changesReadError: {} as Record<string, any>,
    // Per slug, the read state of the change set, the branch (sync), the board
    // and the runs (`WBFail.readFold`, ADR-0070 D3). A write is locked while
    // its fact is not current.
    changesRead: {} as Record<string, any>,
    syncRead: {} as Record<string, any>,
    boardRead: {} as Record<string, any>,
    runsRead: {} as Record<string, any>,
    // The two rendered groups (#315). INVARIANT: every path that sets one must
    // set the OTHER in the SAME statement — a stale group left behind renders
    // rows under a headline while the badge already reads `—`.
    changesStaged: {} as Record<string, any>,
    changesUnstaged: {} as Record<string, any>,
    // The sync row per project (#316): the fold of `sync.status`. Same three
    // triggers as the change set, never a timer.
    syncByProject: {} as Record<string, any>,
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
    // The remote act in flight (`"fetch"` | `"pull"` | `"push"`), or null. One
    // slot for the whole bar: the three acts share the upstream, so a second
    // click while one is out would race it against the first — and a push's
    // round trip is long enough that a silent button reads as a dead one.
    syncBusy: null as any,
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
    // The daemon could not keep watching this tree: the rows are right now,
    // but a change on disk will not show until the project is opened again.
    treeNotLive: "",
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
      hits: [] as any[],
      truncated: false,
      note: "",
      expandedBefore: null as string[] | null,
    },
    _tree: null as any, // the live Wunderbaum instance, if any
    _treeSub: null as any, // the live `/ws/tree` subscription for the open project, if any
    // Tree memory, all three lazily created so they stay plain collections
    // outside Alpine's reactive data (a proxied Map is a trap):
    //   _treeCache     directory levels already shown, keyed `repo\nrel`.
    //                  Survives closing a project. Memory only.
    //   _treeValidated cached levels re-read against the disk during THIS open.
    //                  Cleared on every mount.
    //   _treeExpanded  folders expanded when a project was last closed, by repo.
    _runsSub: null as any, // the live run-snapshot subscription for the open project, if any
    _changesSub: null as any, // the run-completion nudge subscription for the open project (#310)
    _presenceSub: null as any, // the `/ws` heartbeat subscription, kept so a resume can re-open it
    // Monotonic hydration token: overlapping `runs.list` replies can land OUT
    // OF ORDER; only the newest hydration commits.
    _runsSeq: 0,
    // Same token for the Changes count (#310's nudge overlaps the open's read)
    // and for the sync row.
    _changesSeq: 0,
    _syncSeq: 0,

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
      // the predicate (wb-kanban.js) decides.
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible") this.onTabVisible();
      });
      // A tablet resumes on a different link; its sockets died without a close.
      window.addEventListener("online", () => this.resumeSockets(true));
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
      // A console whose record another client removed leaves the columns
      // before it leaves the stage.
      window.WBConsole?.setDeskGoneHook?.((ids: any) => this.checkColumnDesk(ids));
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
      this._treeSub?.resume?.(verdict);
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
        if (payload?.tab && payload.tab === window.WBDeskSink?.tabId?.()) return;
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
      if (this.openSlug) this.hydrateRuns();
    },

    // Read the desk again. The console module puts it on the stage, and says
    // which consoles left it (`checkColumnDesk`); the selected checkouts are
    // copied, since another client may have picked a tree.
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
      if (repo === undefined) repo = this.openSlug;
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
        const state = window.WBAgents.rosterState([], repo);
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
          const before = new Map(this.projects.filter((p) => !p.daemon).map((p) => [p.slug, p]));
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
            remote: window.WBProject.isGitHubRemote(x.remote) ? "github" : "local",
            remoteUrl: x.remote || "",
            tree: [],
          }));
          this.projects = local.concat(this._fleetRows);
          this.shareProjectNames();
          this.reposError = "";
          this.reposRead = window.WBFail.readFold(this.reposRead, { ok: true, value: true, at: Date.now() });
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
        if (git && this.openSlug) this.loadChanges(this.openSlug);
        if (git && this.openSlug) this.loadSync(this.openSlug);
      }
    },

    // A failed `/api/repos` (ADR-0070 D3). After a good read the list stays,
    // marked not current. Before one, the list is empty and says why.
    reposFailed(reason: any) {
      this.reposRead = window.WBFail.readFold(this.reposRead, { ok: false, reason, at: Date.now() });
      if (this.reposRead.goodAt) {
        this.reposError = window.WBFail.notCurrent(this.reposRead, (ms) => this.fmtClock(ms));
        return;
      }
      this.projects = [];
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
      const localRows = () => this.projects.filter((p) => !p.daemon);
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
          for (const p of this.projects) {
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
            remote: window.WBProject.isGitHubRemote(x.remote) ? "github" : "local",
            remoteUrl: x.remote || "",
            tree: [],
            // What makes this a peer row.
            daemon: x.daemon_id,
            daemonName: x.daemon_name || "",
            env: x.environment || "",
            os: x.os || "",
            peerState: x.peer_state || "",
          }));
        this.projects = localRows().concat(this._fleetRows);
        this.shareProjectNames();
        this.shareFleet();
        this.filesFollowFleet();
        this.fleetRead = window.WBFail.readFold(this.fleetRead, { ok: true, value: true, at: Date.now() });
        this.fleetError = "";
      } catch (e: any) {
        if (seq !== this._fleetSeq) return;
        // After a good read the peers and their rows stay, marked not current
        // (ADR-0070 D3); `loadRepos` rebuilt the list without them.
        const reason = String(e?.message || "").startsWith("the daemon") ? e.message : "the daemon did not answer";
        this.fleetRead = window.WBFail.readFold(this.fleetRead, { ok: false, reason, at: Date.now() });
        if (this.fleetRead.goodAt) {
          this.projects = localRows().concat(this._fleetRows);
          this.shareProjectNames();
        } else {
          this.fleetPeers = [];
        }
        this.fleetError = window.WBFail.notCurrent(this.fleetRead, (ms) => this.fmtClock(ms));
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
      this.projects = this.projects.filter((r) => r.daemon !== daemon);
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
            window.WBFail.failed({ message: reply.diagnosis || reply.error }, "Could not wake the peer: the peer did not answer."),
          );
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
        this._flashAction("Could not wake the peer: the daemon is not connected.");
        return false;
      } finally {
        delete this.waking[daemonId];
      }
    },

    // Opening a row on a sleeping peer wakes it. A no-op for every other row.
    wakePeerFor(ref: any) {
      const daemon = window.WBFleet.refDaemon(ref);
      if (!daemon) return;
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      if (window.WBFleet.wakeable(group)) this.wakePeer(daemon);
    },

    peerWakeable(g: any) {
      return window.WBFleet.wakeable(g);
    },
    peerAvailable(g: any) {
      return window.WBFleet.available(g);
    },
    refAvailable(ref: any) {
      const daemon = window.WBFleet.refDaemon(ref);
      if (!daemon) return true;
      return window.WBFleet.available(this.fleetGroups().find((g) => g.daemon === daemon));
    },
    peerIcon(g: any) {
      return window.WBFleet.stateIcon(g);
    },
    peerFault(g: any) {
      return window.WBFleet.stateFault(g);
    },
    groupTitle(g: any) {
      return window.WBFleet.groupTitle(g);
    },
    groupLabel(g: any) {
      return window.WBFleet.groupLabel(g);
    },
    groupHost(g: any) {
      return window.WBFleet.groupHost(g);
    },
    // `x` is a fleet group or a peer of `/api/fleet`: both carry `os` and
    // `environment`.
    osOf(x: any) {
      return window.WBFleet.system(x && x.os, x && x.environment);
    },

    // Local rows first, then one group per peer environment (wb-fleet.js).
    fleetGroups() {
      return window.WBFleet.fleetGroups(this.filteredProjects(), this.fleetPeers);
    },
    repoRef(p: any) {
      return window.WBFleet.repoRef(p);
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
        this.sessionsRead = window.WBFail.readFold(this.sessionsRead, { ok: true, value: true, at: Date.now() });
        // The console menu's fold reads this (#304).
        this.liveSessions = sessions;
        // The console windows read their own row off the same poll (ADR-0059).
        window.WBConsole?.ingestSessions?.(sessions);
        for (const p of this.projects) {
          if (p.state === "offline") continue;
          const mine = sessions.filter((s: any) =>
            window.WBSessionRoute.matchesRepo(s, this.repoRef(p)),
          );
          // A `waiting` agent outranks `live` on the dot (ADR-0059).
          p.state = !mine.length
            ? "idle"
            : window.WBProject.agentStateOf(mine) === "waiting"
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
      this.sessionsRead = window.WBFail.readFold(this.sessionsRead, { ok: false, reason, at: Date.now() });
    },
    sessionsError() {
      return window.WBFail.notCurrent(this.sessionsRead, (ms) => this.fmtClock(ms));
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
    projectBadge(slug: any) {
      return window.WBChanges.projectBadge(this.changesCount, slug);
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
    repoLabel(p: any) {
      return window.WBProject.repoLabel(p);
    },

    // What every surface OUTSIDE the sidebar prints for a repo ref. A peer ref
    // is `<daemon_id>/<owner>/<repo>`: the ULID is how the fleet ROUTES
    // (ADR-0052 §5), so the environment is printed in its place. The ref itself
    // is untouched on the wire, the desk and the tab ids. Row lookup by
    // `repoRef`, not slug: the same `owner/repo` on two daemons is two rows.
    projectLabel(ref: any) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return window.WBFleet.refLabel(ref);
      const name = window.WBProject.projectName(row);
      return row.daemon && row.env ? `${name} · ${row.env}` : name;
    },
    // The consoles name their project too (title, tooltip, default name), and
    // `wb-console.js` has no project list of its own.
    shareProjectNames() {
      window.WBConsole?.ingestProjects?.(
        this.projects.map((p) => ({
          ref: this.repoRef(p),
          name: window.WBProject.projectName(p),
          title: window.WBProject.projectTitle(p),
        })),
      );
    },
    // A console of a peer project says what its peer's state is, and comes
    // back when the peer does. Every project, not the filtered list: a search
    // in the sidebar must not change what a console says.
    shareFleet() {
      window.WBConsole?.ingestFleet?.(window.WBFleet.fleetGroups(this.projects, this.fleetPeers), {
        wake: (daemonId: any) => this.wakePeer(daemonId),
        read: () => this.readFleetNow(),
      });
    },
    // The tooltip twin of `projectLabel`: `owner/repo`, or the full folder of
    // a remoteless repo, plus the environment of a peer. Never a hash key or
    // a daemon id.
    projectTitle(ref: any) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return window.WBFleet.refLabel(ref);
      const who = window.WBProject.projectTitle(row);
      return row.daemon && row.env ? `${who} · ${row.env}` : who;
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
      // The tasks board (wb-kanban.js): an overlay flip over the canvas.
      this.kanbanOpen = !this.kanbanOpen;
      if (this.kanbanOpen) {
        this.kanbanSel = null;
        // Lazy-load the tracker for the open project when the board opens.
        this.loadBoard();
      }
      window.WB.emit("kanban-toggle", { open: this.kanbanOpen });
    },

    // --- branch switcher --------------------------------------------------
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
    // A `worktree.remove` in flight, per repo ref: the chip's menu greys the
    // row and a second click is ignored until the re-read lands.
    worktreeRemoving: {} as Record<string, any>,
    // The selected checkout per repo ref (#406, ADR-0063 §4): the REACTIVE copy
    // of `WBConsole`'s desk mirror (a closure variable there is invisible to
    // Alpine). `worktreeListings` is the last `worktree.list` reply per ref;
    // `_treeCheckout` is the checkout the mounted tree was built for.
    checkouts: {} as Record<string, any>,
    worktreeListings: {} as Record<string, any>,
    _treeCheckout: null,

    // Only when the daemon can reach the repo on disk. NOT gated on `remote`:
    // a local-only repo still has branches.
    canSwitchBranch(p: any) {
      return window.WBProject.canSwitchBranch(p);
    },

    // The branch chip lives on the Files bar (#332), which only the OPEN
    // project renders. `.project-slug` carries the ADR-0008 D7 identity in
    // `data-slug`, which is how the browser tests find a row.
    rowOpen(p: any) {
      return this.openSlug === this.repoRef(p);
    },
    // A sleeping peer's wake button. Its two sentences keep their order here,
    // not in a `+` chain inside the markup (ADR-0065 §9).
    wakeTitle(g: any) {
      if (this.waking[g.daemon]) return `Waking ${g.environment}…`;
      return `Wake ${g.environment}. ${g.diagnosis}`;
    },
    // A row on a host that cannot answer: why it does not open.
    unavailableTitle(g: any) {
      const host = window.WBFleet.peerName(g);
      const head = `${host} is not available (${this.peerStateWord(g.state)}).`;
      if (window.WBFleet.wakeable(g)) return `${head} Click to wake it.`;
      return `${head} Its projects open when it connects again.`;
    },
    // The checkout chip of a project row: which tree Files, Changes and
    // search read, and that a click chooses another.
    checkoutTitle(p: any) {
      const name = this.checkoutOf(this.repoRef(p));
      return name
        ? `Files, changes and search show worktree “${name}”. Click to choose another one.`
        : "Files, changes and search show the primary tree. Click to choose a worktree.";
    },

    // Drop a project from the daemon's registry (#363); the disk is NOT
    // touched. The confirm is awaited BEFORE any `WBDaemon` call: cancel must
    // open no socket.
    async removeProject(p: any) {
      const ref = this.repoRef(p);
      const ok = await this.askConfirm({
        title: "Remove project",
        message: `Remove “${window.WBProject.projectName(p)}” from Ralphy? Files on disk are kept.`,
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
          this._flashAction(window.WBFail.failed(reply, "Could not remove the project: the daemon gave no reason."));
          return;
        }
        // Identity is `repoRef`, not the slug: a peer can list the same slug.
        this.projects = this.projects.filter((x) => this.repoRef(x) !== ref);
        if (this.openSlug === ref) this.openSlug = null;
        this.loadRepos();
      } catch {
        this._flashAction("remove unavailable: no daemon");
      }
    },

    rowTitle(p: any) {
      return window.WBProject.rowTitle(p);
    },

    branchChipTitle(p: any) {
      const ref = this.repoRef(p);
      return window.WBProject.branchChipTitle(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    chipDirty(p: any) {
      const ref = this.repoRef(p);
      return window.WBProject.chipDirty(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    // The row's branch chip. Collapsed it is only the change count, and the
    // click falls through to `.project-head`'s toggle; open it is the switcher,
    // and must not ALSO collapse the row it sits on. Switching is gated on
    // reachability, not on remote (`canSwitchBranch`): an unreachable chip
    // stays inert — informational — but still swallows the click.
    branchChipClick(p: any, ev: any) {
      if (!this.rowOpen(p)) return;
      ev.stopPropagation();
      this.openBranchModal(p);
    },

    openBranchModal(p: any) {
      if (!this.canSwitchBranch(p)) return;
      // Reaching for the picker IS the next branch act.
      this.branchError = "";
      const ref = this.repoRef(p);
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
          this.changesFailed(slug, window.WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        const folded = window.WBChanges.fold(reply);
        this.changesCount[slug] = folded.count;
        this.changesStaged[slug] = folded.staged;
        this.changesUnstaged[slug] = folded.unstaged;
        this.changesReadError[slug] = "";
        this.changesRead[slug] = window.WBFail.readFold(this.changesRead[slug], { ok: true, value: true, at: Date.now() });
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
      const read = window.WBFail.readFold(this.changesRead[slug], { ok: false, reason, at: Date.now() });
      this.changesRead[slug] = read;
      if (read.goodAt) {
        this.changesReadError[slug] = window.WBFail.notCurrent(read, (ms) => this.fmtClock(ms));
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
          this.syncFailed(slug, window.WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        const sync = window.WBChanges.foldSync(reply);
        this.syncByProject[slug] = sync;
        this.syncRead[slug] = window.WBFail.readFold(this.syncRead[slug], { ok: true, value: true, at: Date.now() });
        // Under a selected worktree the read is THAT tree's HEAD, and
        // `p.branch` is the primary's (#407), so only a primary read moves it.
        const branch = window.WBChanges.headBranch(sync);
        const p = this.checkoutOf(slug) ? null : this.projects.find((x) => this.repoRef(x) === slug);
        if (p && branch !== null && p.branch !== branch) p.branch = branch;
        const head = window.WBChanges.headOf(sync);
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
      const read = window.WBFail.readFold(this.syncRead[slug], { ok: false, reason, at: Date.now() });
      this.syncRead[slug] = read;
      const prev = this.syncByProject[slug];
      if (read.goodAt && prev && prev.state !== "unknown") {
        this.syncByProject[slug] = { ...prev, note: window.WBFail.notCurrent(read, (ms) => this.fmtClock(ms)) };
        return;
      }
      this.syncByProject[slug] = { ...window.WBChanges.foldSync(null), note: `Could not read the branch: ${reason}` };
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
        if (window.WBFail.isError(reply)) {
          this._changesRefused(
            window.WBFail.failed(reply, "Could not fetch: the daemon gave no reason."),
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
        if (window.WBFail.isError(reply)) {
          this._changesRefused(
            window.WBFail.failed(reply, "Could not pull: the daemon gave no reason."),
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
        if (window.WBFail.isError(reply)) {
          this._changesRefused(
            window.WBFail.failed(reply, "Could not push: the daemon gave no reason."),
          );
        }
      } catch {
        this._changesRefused("Could not push: the daemon did not answer.");
      } finally {
        this.syncBusy = null;
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
          repo: this.openSlug,
          runid,
        });
        if (window.WBFail.isError(reply)) {
          this.runVerbFailed(window.WBFail.failed(reply, "Could not stop the run."));
        } else {
          this._flashAction("Stop requested. The run is stopping.");
        }
      } catch {
        this._flashAction("Could not stop the run: the daemon is not connected.");
      } finally {
        this.runStopping = null;
      }
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
      const slug = this.openSlug;
      if (this.buildSkew) return this.BUILD_SKEW_LOCK;
      if (this.changesRead[slug]?.current === false || this.syncRead[slug]?.current === false) {
        return "The changes shown are not current. Wait for the next read, or reload the page.";
      }
      if (this.runsRead[slug]?.current === false) {
        return "The runs shown are not current. Wait for the next read, or reload the page.";
      }
      return window.WBChanges.writeLockReason(this.runsByProject[slug]);
    },
    BUILD_SKEW_LOCK: "This page is older than Ralphy. Save your work, and the page loads the new version.",
    // The build this page was served with (`<meta name="ralphy-build">`);
    // "" with no such tag, and then the page never reloads for a build.
    pageBuild: document.querySelector<HTMLMetaElement>('meta[name="ralphy-build"]')?.content || "",
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
        window.WBDeskSink?.setHold?.(false);
        // A detached file window would outlive the reload with no tab that
        // hears its Save; closing it sends the file home as a tab.
        detached.close();
        window.location.reload();
        return;
      }
      // A hidden tab writes no desk until it reloads: its JavaScript may not
      // know the daemon's desk.
      if (!unsaved) {
        window.WBDeskSink?.setHold?.(true);
        return;
      }
      if (!this.buildSkew) {
        this.buildSkew = true;
        window.WBDeskSink?.setHold?.(true);
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
      if (this.boardRead[this.openSlug]?.current === false) {
        return "The board shown is not current. Wait for the next read.";
      }
      return window.WBChanges.writeLockReason(
        this.runsByProject[this.openSlug],
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
      return window.WBRun.verbLockTitle(verb, this.writeLockReason());
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
      return window.WBChanges.pushAct(this.syncByProject[this.openSlug]);
    },
    pullBlocked() {
      return window.WBChanges.pullBlocked(this.syncByProject[this.openSlug]);
    },
    // The remote bar's title while an act is out: the busy act names itself,
    // the other two name what they are waiting on.
    syncBusyTitle(verb: any) {
      if (!this.syncBusy) return "";
      if (this.syncBusy !== verb) return `Waiting for the ${this.syncBusy} to finish`;
      return ({ fetch: "Fetching…", pull: "Pulling…", push: "Pushing…" } as Record<string, string>)[verb] || "";
    },
    groupNote(group: any) {
      return window.WBChanges.groupDiscardNote(group);
    },
    commitTarget() {
      return window.WBChanges.commitTarget(this.syncByProject[this.openSlug]);
    },
    // `withOriginal` only on the UNSTAGE direction — see `wb-changes.js`.
    groupPaths(list: any, withOriginal: any) {
      return window.WBChanges.groupPaths(list, withOriginal);
    },
    commitTitle() {
      const locked = this.writeLockReason();
      if (locked) return locked;
      if (!(this.changesStaged[this.openSlug] || []).length) {
        return "Stage a change first";
      }
      if (!this.commitMsg.trim()) return "Write a commit message first";
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
    async stagePaths(slug: any, paths: any) {
      if (!slug || !paths || !paths.length || this.writeLocked()) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.stage",
          window.WBDaemon.withCheckout({ repo: slug, paths }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          this._changesRefused(
            window.WBFail.failed(reply, "Could not stage: the daemon gave no reason."),
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
        if (window.WBFail.isError(reply)) {
          this._changesRefused(
            window.WBFail.failed(reply, "Could not unstage: the daemon gave no reason."),
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
      const c = window.WBChanges.discardConfirm(entry);
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
        if (window.WBFail.isError(reply)) {
          this._changesRefused(
            window.WBFail.failed(reply, "Could not discard: the daemon gave no reason."),
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
        if (window.WBFail.isError(reply)) {
          this._changesRefused(
            window.WBFail.failed(reply, "Could not commit: the daemon gave no reason."),
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
      return window.WBProject.hasWorktrees(this.worktreeListings[this.repoRef(p)] || null);
    },
    openCheckoutChip(p: any, anchor: any) {
      const ref = this.repoRef(p);
      const listing = this.worktreeListings[ref] || null;
      const mine = (this.liveSessions || []).filter((s) => window.WBSessionRoute.matchesRepo(s, ref));
      window.WBConsole.checkoutMenu({
        anchor,
        host: document.body,
        rows: window.WBConsole.checkoutMenuRows(listing, this.checkoutOf(ref), mine, p.branch, !!p.dirty),
        onPick: (row: any) => this.setCheckout(ref, row.primary ? null : row.name),
        onRemove: (row: any) => this.removeWorktree(ref, row),
      });
    },

    // --- the selected checkout (#406, ADR-0063 §4) ----------------------------
    checkoutOf(ref: any) {
      return this.checkouts[ref] || null;
    },
    chipLabel(p: any) {
      const ref = this.repoRef(p);
      return window.WBProject.chipLabel(p, this.checkoutOf(ref), this.worktreeListings[ref] || null);
    },
    // The reactive map is REPLACED so Alpine sees it; persistence goes to the
    // desk mirror; an open tree is remounted (cache key, watch and rows are
    // per checkout).
    setCheckout(ref: any, name: any) {
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
      // Another tree's change set is not this tree's "last value" (ADR-0070
      // D3): its reads start over.
      delete this.changesRead[ref];
      delete this.syncRead[ref];
      if (this.openSlug === ref) {
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
    switchBranch(name: any) {
      if (this.writeLocked()) {
        this._flashAction(this.writeLockReason());
        this.closeBranchModal();
        return;
      }
      if (name !== this.branchModal.current) {
        const slug = this.branchModal.slug;
        const checkout = this.checkoutOf(slug);
        const p = checkout ? null : this.projects.find((x) => this.repoRef(x) === slug);
        const prev = p ? p.branch : null;
        if (p) p.branch = name; // optimistic — the chip updates immediately
        window.WB.emit("branch-switch", { project: slug, branch: name, checkout });
        // The run-lock-aware `branch.switch` Mutate (#199): refusal → revert.
        this._mutateBranch("branch.switch", slug, name, () => {
          if (p) p.branch = prev;
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
      const p = checkout ? null : this.projects.find((x) => this.repoRef(x) === slug);
      const prevBranch = p ? p.branch : null;
      const prevBranches = p ? [...(p.branches || [])] : null;
      if (p) {
        p.branches = [...(p.branches || []), name];
        p.branch = name; // a fresh branch is checked out onto
      }
      window.WB.emit("branch-create", { project: slug, name, from, checkout });
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
        if (window.WBFail.isError(reply)) {
          refused(window.WBFail.cause(reply, "The daemon refused to delete the worktree."));
        } else {
          this._flashAction(`Worktree ${w.name} deleted`);
        }
      } catch {
        refused("Could not reach the daemon. Check whether the worktree was deleted.");
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
    async _mutateBranch(verb: any, slug: any, name: any, revert: any) {
      try {
        const reply = await window.WBDaemon.observe(
          verb,
          WBDaemon.withCheckout({ repo: slug, name }, this.checkoutOf(slug)),
        );
        if (window.WBFail.isError(reply)) {
          revert();
          this._branchRefused(
            window.WBFail.failed(
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
    // One entry per `runid`: issue queue + per-issue status, live phase, the
    // current issue's plan.md (helpers in wb-runs.js, `window.WBRun`).
    runsByProject: {} as Record<string, any>,
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

    // Hydrate from `runs.list` (ADR-0047 §9), by REPLACEMENT — a snapshot is
    // state, not a log. On panel open and project change.
    async hydrateRuns() {
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
          this.runsFailed(slug, window.WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        this.runsRead[slug] = window.WBFail.readFold(this.runsRead[slug], { ok: true, value: true, at: Date.now() });
        this.runsByProject[slug] = (reply.runs || []).map((d: any) => {
          const run = window.WBRun.fromSnapshot(d);
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
        if (seq !== this._runsSeq || this.openSlug !== slug) return;
        // A transport failure is a read failure, not an idle project.
        this.runsFailed(slug, window.WBFail.why({ message: err?.message }, "the daemon did not answer"));
      }
    },
    // A failed `runs.list` (ADR-0070 D3). After a good read the runs stay,
    // marked not current; before one, there are none and the panel says why.
    runsFailed(slug: any, reason: any) {
      const read = window.WBFail.readFold(this.runsRead[slug], { ok: false, reason, at: Date.now() });
      this.runsRead[slug] = read;
      if (read.goodAt) {
        this.runsError = window.WBFail.notCurrent(read, (ms) => this.fmtClock(ms));
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

    // Thin delegations to the faithful helpers in wb-runs.js.
    runPhaseLabel(run: any) {
      return run ? window.WBRun.runPhaseLabel(run) : "";
    },
    runTitle(run: any) {
      return window.WBRun.runTitle(run);
    },
    runIdentity(run: any) {
      return window.WBRun.runIdentity(run);
    },
    // Reading `nowMs` subscribes this binding to the 1 s tick.
    runClock(run: any) {
      return window.WBRun.phaseClock(run, this.nowMs);
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
      return window.WBRun.issueState(run, iss);
    },
    issueGlyph(run: any, iss: any) {
      return window.WBRun.glyph(run, iss);
    },
    sleepLabel(run: any) {
      return window.WBRun.sleepText(run?.sleep);
    },
    nodeTitle(run: any, iss: any) {
      if (!run || !iss) return "";
      const st = window.WBRun.issueState(run, iss);
      let t = `#${iss.number} — ${iss.title} · ${window.WBRun.LABEL[st] || st}`;
      // Per-issue: tier routing gives two issues of one run different models.
      const seg = window.WBRun.modelEffort(iss.model, iss.effort);
      if (seg) t += ` · ${seg}`;
      if (iss.blockedBy?.length) t += ` (blocked by ${iss.blockedBy.map((n: any) => "#" + n).join(", ")})`;
      return t;
    },
    // Run → board (#301): a trail node opens that issue's detail. The Runs
    // panel closes first (`z-index: 150`, sharing the drawer's right edge).
    // `toggleKanban()` resets `kanbanSel`, so it runs BEFORE `openIssue`.
    focusIssue(number: any) {
      window.WB.emit("run-issue-focus", { project: this.openSlug, runid: this.currentRun()?.runid, issue: number });
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
      return window.WBRun.planTrailerIssue(run?.planMd);
    },
    planProseIsCurrent(run: any) {
      return window.WBRun.planBelongsTo(run?.planMd, this.planIssueWanted(run));
    },
    // Every `##` section except Steps (its own block); none while the prose
    // belongs to another issue.
    planHeadings(run: any) {
      if (!this.planProseIsCurrent(run)) return [];
      return window.WBRun.headings(run?.planMd).filter((h) => h.toLowerCase() !== "steps");
    },
    // Render one `##` section as sanitized HTML. Steps render from the
    // snapshot document, not from here (#330).
    renderPlanSection(run: any, name: any) {
      if (!run || !name || !this.planProseIsCurrent(run)) return "";
      const body = window.WBRun.section(run?.planMd, name);
      return DOMPurify.sanitize(marked.parse(body || "_(empty)_"));
    },

    // --- the step list (the plan block is state, #330) ---------------------
    planSteps() {
      return this.currentRun()?.steps || [];
    },
    stepGlyph(status: any) {
      return window.WBRun.stepGlyph(status);
    },
    stepLabel(status: any) {
      return window.WBRun.stepLabel(status);
    },
    stepClass(status: any) {
      return window.WBRun.stepClass(status);
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
    },
    closeRunModal() {
      this.runOpen = false;
    },
    // The current git branch of the open project (for the "current" mode blurb).
    // The open project's row, for the panels scoped to `openSlug` that reuse a
    // per-row control (the Changes head's checkout chip).
    openProject() {
      return this.projects.find((p) => this.repoRef(p) === this.openSlug) || null;
    },
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
      window.WB.emit("run-start", {
        project: this.openSlug,
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
      window.WB.emit("command", { project: this.openSlug, verb });
      this._flashAction(this.verbRequestedText(verb));
    },
    // From wb-daemon.js on a TERMINAL frame only; an empty note is a no-op.
    runVerbFailed(msg: any) {
      if (msg) this.verbError = msg;
    },
    _flashAction(msg: any) {
      this.runsActionMsg = msg;
      clearTimeout(this._actionTimer);
      this._actionTimer = setTimeout(() => (this.runsActionMsg = ""), 2600);
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
    boardLabels: {} as Record<string, Map<string, string>>,
    // A `board.list` failure (#207): a broken tracker connection must never
    // read as "no work to do".
    boardError: {} as Record<string, any>,
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
    _boardBackstop: null as any,
    _changesBackstop: null as any,
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
      return held ? window.WBRun.planPillLabel(held.summary, this.planIssueIsOpen()) : "";
    },
    planPillWarns(number: any) {
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
          this._flashAction(window.WBFail.failed(reply, "Could not discard the plan."));
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
        const reply: any = await Promise.race([
          window.WBDaemon.observe("board.list", { repo: slug }),
          new Promise((_, rej) =>
            setTimeout(() => rej(new Error("board fold timed out")), this.BOARD_FOLD_TIMEOUT_MS),
          ),
        ]);
        if (window.WBFail.isError(reply)) {
          const msg = window.WBFail.failed(reply, "Could not load the board.");
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
        this.boardRead[slug] = window.WBFail.readFold(this.boardRead[slug], { ok: true, value: true, at: Date.now() });
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
        if (this._boardPending || this.openSlug !== slug) {
          this._boardPending = false;
          if (this.openSlug && this.kanbanOpen) this.loadBoard();
        }
      }
    },

    // A failed `board.list` (ADR-0070 D3). After a good read the cards stay,
    // under a banner that says they are not current; before one, there are no
    // cards and the banner says why. Moving a card is locked meanwhile.
    boardFailed(slug: any, msg: any) {
      const read = window.WBFail.readFold(this.boardRead[slug], { ok: false, reason: msg, at: Date.now() });
      this.boardRead[slug] = read;
      if (read.goodAt) {
        this.boardError[slug] = window.WBFail.notCurrent(read, (ms) => this.fmtClock(ms));
        return;
      }
      this.boardIssues[slug] = [];
      this.boardError[slug] = msg;
    },

    // The one door every refresh trigger goes through (#301): the predicate
    // (wb-kanban.js) decides.
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

    // A CLI fold row → the issue shape `wb-kanban.js` expects. Body + comments
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
      return (state && window.WBRun?.LABEL?.[state]) || state || "";
    },
    peerStateWord(state: any) {
      return window.WBFleet.stateWord(state);
    },

    // Thin delegations to the faithful helpers (used in the template).
    kanbanColumnOf(i: any) {
      return window.WBKanban.columnOf(i);
    },
    labelColor(l: any) {
      // The repo's real label hex, else the seed vocabulary.
      return this.boardLabels[this.openSlug]?.get(l) || window.WBKanban.labelColor(l);
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

    // --- detail drawer ----------------------------------------------------
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
      const fail = (msg: any) => {
        if (stale()) return;
        this.issueError = msg;
        this._flashAction?.(msg);
      };
      try {
        const reply = await window.WBDaemon.observe("issue.show", { repo: slug, number });
        if (window.WBFail.isError(reply)) {
          fail(window.WBFail.failed(reply, "Could not load the issue."));
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
      const p = this.projects.find((x) => this.repoRef(x) === this.openSlug);
      return window.WBProject.issueUrl(p && p.remoteUrl, number);
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
      const slug = this.openSlug;
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
          if (window.WBFail.isError(reply)) {
            iss.labels = prev;
            this._flashAction(window.WBFail.failed(reply, "Could not change the labels: the daemon gave no reason."));
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
      const slug = this.openSlug;
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
      if (this.openSlug !== slug) return;
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
    },

    // --- release ----------------------------------------------------------
    // The release fact stays here, because the sidebar reads it. The About,
    // What's new and update dialogs are the component in
    // wb-release-dialogs.ts, and change it only through the two methods
    // after `loadRelease` (ADR-0073 D4).
    // The release view the daemon computed (ADR-0056 §7), seeded empty.
    release: (window.WBRelease && window.WBRelease.EMPTY) || {
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
      this._treeSub?.replay?.();
      this.resumeSockets();
      this.loadRelease();
    },
    // The panels whose facts have no push: read again on visible and after
    // login while they are open (fact index: settings 3, 4; usage 3, 4). The
    // Settings dialog hears the event and reads again only when it is open
    // (ADR-0073 D5).
    rereadOpenPanels() {
      window.dispatchEvent(new CustomEvent("workbench:panels-reread"));
      if (this.tabs.some((t) => t.id === "spend")) this.loadSpend();
    },
    releaseRead: null as any,
    releaseStale() {
      return window.WBFail.notCurrent(this.releaseRead, (ms) => this.fmtClock(ms));
    },
    async loadRelease() {
      if (!window.WBRelease) return;
      const view = await window.WBRelease.read();
      // A failed read keeps what the page last knew (ADR-0056 §6), marked
      // not current (ADR-0070 D3).
      this.releaseRead = window.WBFail.readFold(this.releaseRead, {
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

    // --- login gate -------------------------------------------------------
    // An opaque overlay covers the shell while locked (`body.locked`).
    authed: true,
    // `remember` is "keep me signed in" (ADR-0032 amendment 2026-09-16):
    // opt-in, reset on every log-off.
    login: { code: "", digits: ["", "", "", "", "", ""], password: "", remember: false, error: "", passwordRequired: false },

    async logOff() {
      this.avatarMenu = false;
      window.dispatchEvent(new CustomEvent("workbench:log-off"));
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
      this._treeSub?.replay?.();
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
    // The adapter roster comes from `/api/agents`, never a list here:
    // onboarding a vendor must not need a frontend change (#304).
    agents: [] as any[],
    roster: [] as any[],
    agentMenu: false,
    // The Go-to picker (#337) and the fence picker (#343): SNAPSHOTS taken
    // when the menu opens, because the windows and fences live in the DOM.
    windowMenu: false,
    windowList: [],
    fenceMenu: false,
    fenceItems: [],
    // Columns beside a maximized console (ADR-0051 §5): per-client view
    // state, never desk state. The ids left to right; empty whenever fewer
    // than two remain, so a lone survivor is an ordinary maximize again.
    columns: [],
    columnDir: "right",
    _columnsRestored: false,
    _paintedKey: "",
    columnMenu: false,
    columnGroups: [] as any[],
    columnFilter: "",
    columnFrom: null,
    columnMenuAt: { top: 0, right: 0, maxWidth: 400, maxHeight: 400 },
    // The note picker (ADR-0064 §§9–10): a SNAPSHOT on open, like the two
    // above — the cards live in the DOM and the desk, not in Alpine state.
    noteMenu: false,
    noteItems: [],
    // Which notes have their `##` sections open in the menu, by id. Collapsed
    // is the default: a note is a document, and every heading of every note at
    // once is a wall, not a map.
    consoleCount: 0,
    // The stage extent, for the footer pill (#338).
    stageW: 0,
    stageH: 0,
    // The open modals, oldest first: `{ path, opener }`. Only the last one
    // answers Escape, and each returns focus to its opener on close.
    _modalStack: [] as any[],
    // The Escape keydown a modal has already answered.
    _escapeEvent: null,
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
    // The move destination picker (#364): browses one level at a time through
    // `tree.list`. `from` is the FULL rel path; `dir` the browsed directory
    // ("" is the repo root).
    movePick: { open: false, from: "", isFolder: false, dir: "", entries: [], busy: false, error: "" } as any,
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
    splitRatio: null,
    lastLeft: null as any,

    // `loadRepos()` fills this at init.
    projects: [] as any[],

    // --- accordion --------------------------------------------------------
    toggle(ref: any, row?: any) {
      // A row on a host that cannot answer stays closed. The click still wakes
      // a sleeping host: that is the act the operator asked for.
      if (this.openSlug !== ref && !this.refAvailable(ref)) {
        this.wakePeerFor(ref);
        return;
      }
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
      });
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

    // Wunderbaum copies source keys it does not define into `node.data`, so
    // `folder:true` lands at `node.data.folder` and `node.folder` is forever
    // `undefined`; `node.children` is `null` on a lazy or empty folder. Either
    // alone made EVERY collapsed folder answer "file".
    isFolder(node: any) {
      if (!node) return false;
      return !!(node.data?.folder || node.lazy || Array.isArray(node.children));
    },

    // --- file-type icons (Devicon font; folders use Wunderbaum defaults) ---
    fileIcon(title: any) {
      const name = title.toLowerCase();
      if (name.endsWith("lock") || name === "package-lock.json") return "devicon-json-plain colored";
      const ext = name.includes(".") ? name.split(".").pop() : "";
      const map: Record<string, string> = {
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
        note: "bi bi-sticky",
      };
      return map[ext] || "bi bi-file-earmark";
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
      this.treeNotLive = "";
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
        // The root level from `tree.list`, folders `lazy`. A failed root read
        // says so and renders nothing.
        source: this.useDaemonTree()
          ? this.loadTreeLevel("").catch(() => {
              if (gen === this._treeGen) this.treeError = "Could not read the files of this project.";
              return [];
            })
          : [],
        // A failed level returns `false`: Wunderbaum then leaves the folder
        // unloaded, so the next expand reads it again, and draws no row for
        // the failure. A rethrow drew an "Error (…)" row inside the folder.
        // The footer says why; the folder closes, because it shows nothing.
        lazyLoad: (e: any) =>
          this.loadTreeLevel(this.relPath(e.node)).catch((err: any) => {
            this.treeWentStale(err);
            if (window.WBFleet.refDaemon(this.openSlug || "")) this.readFleetNow();
            setTimeout(() => e.node.setExpanded(false));
            return false;
          }),
        // While the fleet calls the open project's peer down, a folder with
        // no level read before does not open: its read would fail. A level
        // read before opens from memory. A restore of the expanded folders
        // stops here too.
        beforeExpand: (e: any) => {
          if (!e.flag || !this.openPeerDown() || e.node.children) return undefined;
          return this._treeCache.has(this.treeKey(this.relPath(e.node))) ? undefined : false;
        },
        // A level (re)loaded while a search is on carries no match marks, and
        // in `hide` mode an unmarked row is not painted.
        load: (e: any) => {
          if (e.tree.isFilterActive?.()) e.tree.updateFilter();
        },
        // The content-search badge: hit count beside the title; nothing once
        // the filter is gone (the map is empty by then).
        render: (e: any) => {
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
        init: (e: any) => {
          if (gen !== this._treeGen) return;
          this.treeLoading = false;
          if (e.error) this.treeError = "Could not read the files of this project.";
          else this.restoreExpansion();
        },
        edit: {
          trigger: ["F2", "macEnter"],
          apply: (e: any) => {
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
        expand: (e: any) => {
          if (!this.isFolder(e.node)) return;
          const rel = this.relPath(e.node);
          if (e.flag) this._treeSub?.watch(rel);
          else this._treeSub?.unwatch(rel);
          this.rememberExpansion();
        },
        // Double-click / Enter on a leaf = "open this file".
        dblclick: (e: any) => {
          if (!this.isFolder(e.node)) this.openFile(e.node);
          return false;
        },
      });

      // One `/ws/tree` subscription per open project; the root is always
      // watched. A `tree.dirty` push refetches only the affected subtree; a
      // `head.dirty` push re-reads the branch.
      if (this.useDaemonTree() && window.WBDaemon?.subscribeTree) {
        this._treeSub = WBDaemon.subscribeTree(
          this.openSlug,
          (rel: any) => {
            if (!this.tabHidden()) this.onTreeDirty(rel);
          },
          this._treeCheckout,
          () => {
            if (!this.tabHidden()) this.onHeadMoved();
          },
          (reason: any) => this.treeWatchFailed(reason),
        );
        this._treeSub.watch("");
      }

      // Right-click → our own context menu. Empty space below the rows is the
      // repo root: the only gesture that can create a top-level entry.
      host.addEventListener("contextmenu", (ev: any) => {
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
      const rels: any[] = [];
      this.rawTree().root.visit((n: any) => {
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
          const node = this.rawTree().findFirst((n: any) => this.relPath(n) === rel);
          if (node && !node.expanded) await node.setExpanded(true);
        }
      } finally {
        this._restoringExpansion = false;
      }
    },

    // False only when the daemon client script is not loaded (a unit test).
    useDaemonTree() {
      return !!window.WBDaemon?.observe;
    },

    // One directory level from `tree.list`, folders lazy. Cache-FIRST
    // (stale-while-revalidate): a level read once is painted from memory and
    // re-read in the BACKGROUND, touching the DOM only when the directory
    // changed. Nothing is cached on the daemon (ADR-0036); the revalidation
    // is not optional, since the watch is dropped when the project closes.
    loadTreeLevel(rel: any) {
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
    fetchTreeLevel(rel: any) {
      this.treeMem();
      const key = this.treeKey(rel);
      const payload = WBDaemon.withCheckout(
        { repo: this.openSlug, path: rel },
        this.checkoutOf(this.openSlug),
      );
      return WBDaemon.observe("tree.list", payload).then((reply) => {
        if (!reply || reply.status !== "ok" || !Array.isArray(reply.entries)) {
          throw new Error(window.WBFail.message(reply, ""));
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
    pruneTreeCache(rel: any, entries: any) {
      this.treeMem();
      const dirs = new Set(entries.filter((en: any) => en.dir).map((en: any) => en.name));
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

    // Mark every cached level BELOW `rel` as not validated, so the next
    // `loadTreeLevel` of each paints from the cache and re-reads it.
    forgetValidatedBelow(rel: any) {
      this.treeMem();
      const prefix = this.treeKey(rel === "" ? "" : `${rel}/`);
      const own = this.treeKey(rel);
      for (const key of [...this._treeValidated]) {
        if (key !== own && key.startsWith(prefix)) this._treeValidated.delete(key);
      }
    },

    // Cache key, scoped by REPO and by CHECKOUT (#406).
    treeKey(rel: any) {
      return `${this.openSlug}\n${this.checkoutOf(this.openSlug) || ""}\n${rel}`;
    },

    // Daemon entries → fresh Wunderbaum node specs, rebuilt on every call: the
    // tree OWNS and mutates the objects it is given. An ignored entry carries
    // `wb-ignored`, set once when the row is created. An older peer sends no
    // `ignored`, and its rows are simply not dimmed.
    treeNodes(entries: any) {
      return entries.map((en: any) => {
        const node: any = en.dir
          ? { title: en.name, folder: true, lazy: true }
          : { title: en.name, icon: this.fileIcon(en.name) };
        if (en.ignored) node.classes = "wb-ignored";
        return node;
      });
    },

    // Re-read a level painted from cache and reconcile ONLY if the directory
    // changed: the common case costs one read and zero DOM work.
    revalidateLevel(rel: any) {
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
        .catch((err: any) => this.treeWentStale(err));
    },

    // The tree shows a listing it could not confirm: record the reason, leave
    // every row alone. Cleared by `treeFresh`.
    treeWentStale(err: any) {
      if (!this.useDaemonTree()) return;
      const failure = window.WBFail.failed({ message: err?.message }, "Could not refresh the file list: the daemon gave no reason.");
      this.treeStale = `${failure} The list shown is the last one read.`;
    },

    // A read landed: whatever the tree is showing is confirmed again.
    treeFresh() {
      this.treeStale = "";
    },

    // The fleet group of the open project's peer while the fleet calls that
    // peer down, else null. FILES takes this fact from the fleet, its owner.
    openPeerDown() {
      const daemon = window.WBFleet.refDaemon(this.openSlug || "");
      if (!daemon) return null;
      const group = this.fleetGroups().find((g) => g.daemon === daemon);
      return group && !window.WBFleet.available(group) ? group : null;
    },
    peerDownText(g: any) {
      return `${window.WBFleet.peerName(g)} is not connected. The list shown is the last one read.`;
    },
    peerDownAction(g: any) {
      return window.WBFleet.wakeable(g) ? "Wake" : "Try again";
    },
    peerDownAct() {
      const g = this.openPeerDown();
      if (!g) return;
      if (window.WBFleet.wakeable(g)) this.wakePeer(g.daemon);
      else this.readFleetNow();
    },

    // After each good fleet read: the read that calls the open project's peer
    // back reads again the levels the tree shows.
    filesFollowFleet() {
      const down = !!this.openPeerDown();
      if (!down && this._filesPeerDown && this._filesPeerDown === this.openSlug && this._tree) {
        this.treeFresh();
        this.revalidateLevel("");
        this.rawTree().root.visit((n: any) => {
          if (this.isFolder(n) && n.expanded) this.revalidateLevel(this.relPath(n));
        });
      }
      this._filesPeerDown = down ? this.openSlug : null;
    },
    _filesPeerDown: null,

    // The daemon says it could not watch a dir of this tree (`reason`), or
    // `null` when a new socket holds every dir again.
    treeWatchFailed(reason: any) {
      this.treeNotLive = reason
        ? `The file list no longer updates by itself: ${reason}. Reopen the project to try again.`
        : "";
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

    setFileSearchMode(mode: any) {
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
            this.fileSearch.note = window.WBFail.failed(reply, "Could not search: the daemon gave no reason.");
            return;
          }
          return this.applyFileSearch(reply.hits, !!reply.truncated, seq);
        })
        .catch((err: any) => {
          if (seq !== this.fileSearch.seq) return;
          this.fileSearch.note = window.WBFail.failed({ message: err?.message }, "Could not search: the daemon did not answer.");
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
      const rels: any[] = [];
      this.rawTree()?.root?.visit((n: any) => {
        if (this.isFolder(n) && n.expanded) rels.push(this.relPath(n));
      });
      return rels;
    },

    // Narrow the tree to `hits`: snapshot the expansion once per search
    // session, load every ancestor level (shallow-first), then filter. The
    // expands run under `_restoringExpansion`: the remembered expansion must
    // not learn them.
    async applyFileSearch(hits: any, truncated: any, seq: any) {
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
          const f = tree.findFirst((n: any) => this.relPath(n) === dir);
          if (f && this.isFolder(f) && !f.expanded) await f.setExpanded(true);
        }
        if (seq !== this.fileSearch.seq || tree !== this.rawTree()) return;
        this._fileHits = window.WBFileSearch.hitMap(hits);
        // `autoExpand: false`: the extension would also open every MATCHED
        // folder, a burst of lazy loads.
        tree.filterNodes((n: any) => this._fileHits.has(this.relPath(n)), {
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
          const f = tree.findFirst((n: any) => this.relPath(n) === rel);
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

    // `file.read`, resolving to what the pane opens with: `{content, encoding,
    // bom}` (ADR-0036 amendment 2026-09-22), or `{refused: reason}` when the
    // daemon served nothing for a reason the pane can state (`binary`, `too
    // large`, …) — the tab KEEPS its place and says so — or `null` when there
    // is nothing to hold a tab open for (`not found`, a transport drop): the
    // tab closes. `checkout` is the tab's PINNED checkout (#406).
    fetchContent(project: any, path: any, ftype: any, checkout: any) {
      const refuse = (reason: any) => {
        window.WB.emit("open-refused", { project, path, reason });
        this._flashAction?.(window.WBFail.failed({ reason }, "Could not open the file: the daemon gave no reason."));
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
          if (!window.WBFail.isError(reply)) {
            return { content: reply.content, encoding: reply.encoding, bom: !!reply.bom };
          }
          return refuse(window.WBFail.message(reply, "refused"));
        })
        .catch(() => refuse("transport"));
    },

    // A `head.dirty` push: the open checkout's HEAD moved (a switch, a commit).
    // Re-read what the branch drives: the chip, the sync row and the Changes
    // count. The gitdir and its `logs/` push together for one move, so a short
    // trailing timer makes them one read.
    onHeadMoved() {
      clearTimeout(this._headTimer);
      this._headTimer = setTimeout(() => {
        const ref = this.openSlug;
        if (!ref) return;
        this.loadChanges(ref);
        this.loadSync(ref);
        // Under a selection the chip reads the worktree's branch from the
        // listing, not `p.branch`.
        if (this.checkoutOf(ref)) this.ensureWorktreeListing(ref, true);
      }, this.HEAD_SETTLE_MS);
    },
    HEAD_SETTLE_MS: 250,
    _headTimer: null as any,

    // A `tree.dirty` nudge for `rel`: refetch that level IF it is on screen. A
    // nudge for a collapsed/absent dir is DROPPED (ADR-0036 §4).
    onTreeDirty(rel: any) {
      const tree = this.rawTree();
      if (!tree) return;
      const node = rel === "" ? tree.root : this.findFolderByRel(rel);
      if (!node) return; // not in the tree → invisible, drop
      if (rel !== "" && !node.expanded) return; // collapsed → invisible, drop
      // Reconcile in place, then freshen the open tabs in this directory. A
      // reconcile failure leaves every row in place (`_reconcileOnce` resolves
      // the listing BEFORE touching the tree) and must still refresh viewers.
      return this.reconcileLevel(node, rel)
        .catch((err: any) => this.treeWentStale(err))
        .then((): any => this.refreshOpenViewers(rel));
    },

    // Re-list one level WITHOUT duplicating nodes, preserving descendant
    // expansion and the active selection. `node.load` appends, so
    // `removeChildren()` first, then re-expand and re-activate by captured rel.
    async reconcileLevel(node: any, rel: any) {
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

    async _reconcileOnce(nodeAtCall: any, rel: any) {
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
        node = rel === "" ? raw?.root : raw?.findFirst((n: any) => this.relPath(n) === rel);
        if (!node || node.isLoading?.()) return;
      }
      const hasGitignore = source.some((n: any) => !n.folder && n.title === ".gitignore");
      // A write to a file already listed nudges its directory too. When the
      // rows on screen already match the fresh listing, the teardown would
      // change nothing but the operator's scroll position.
      if (this.levelShows(node, source)) {
        if (hasGitignore) this.revalidateBelow(node, rel);
        return;
      }
      // Read AFTER the fetch, right before the teardown: read before it, the
      // snapshot missed every folder a FILES search opened meanwhile.
      const expandedRels: any[] = [];
      node.visit((n: any) => {
        if (this.isFolder(n) && n.expanded) expandedRels.push(this.relPath(n));
      });
      const activeRel = this.relPath(this.rawTree()?.getActiveNode?.() || null) || null;
      // The teardown shrinks the list under the viewport, the browser clamps
      // `scrollTop` to the shorter list, and the re-activation below scrolls
      // to the selected row. Nothing else puts the offset back.
      const scrollTop = this.rawTree()?.element?.scrollTop ?? 0;
      node.removeChildren();
      await node.load(source);
      // `load` leaves the reloaded node collapsed, and the NEXT nudge would
      // hit the `!expanded` drop guard.
      if (rel !== "" && !node.expanded) await node.setExpanded(true);
      // A level with a `.gitignore` may have changed the ignore marks of every
      // level below it, but the re-expansion below paints those from the
      // cache. Forgetting that they were validated makes `loadTreeLevel`
      // re-read each one in the background.
      if (hasGitignore) this.forgetValidatedBelow(rel);

      // Shallow-first. Match by rel path (NOT findFolderByRel): a freshly
      // reloaded folder has neither `folder` nor loaded `children` yet.
      expandedRels.sort((a, b) => a.split("/").length - b.split("/").length);
      for (const r of expandedRels) {
        const f = this.rawTree()?.findFirst((n: any) => this.relPath(n) === r);
        if (f && !f.expanded) await f.setExpanded(true);
      }
      const target = (this._revealSeq || 0) > seq ? this._revealedRel : activeRel;
      if (target) await this.revealRel(target, { restore: true });
      // A search is on: the reloaded level has no match marks yet.
      const raw = this.rawTree();
      if (raw?.isFilterActive?.()) raw.updateFilter();
      // A reveal that landed mid-pass owns the scroll position.
      if (raw?.element && (this._revealSeq || 0) === seq) {
        // The list must have its full height first, or the offset is clamped.
        raw.updatePendingModifications?.();
        raw.element.scrollTop = scrollTop;
      }
    },

    // Whether the rows under `node` already show `specs` (from `treeNodes`):
    // the same names, kinds and ignore marks, in the same order. A level still
    // loading has a status row, so it never matches.
    levelShows(node: any, specs: any) {
      const rows = node.children;
      if (!Array.isArray(rows) || rows.length !== specs.length) return false;
      return specs.every((spec: any, i: any) => {
        const row = rows[i];
        return (
          row.title === spec.title &&
          this.isFolder(row) === !!spec.folder &&
          !!row.hasClass?.("wb-ignored") === (spec.classes === "wb-ignored")
        );
      });
    },

    // An unchanged level with a `.gitignore` may still have changed the ignore
    // marks of the levels below it: re-read each expanded one, which repaints
    // only a level whose listing changed.
    revalidateBelow(node: any, rel: any) {
      this.forgetValidatedBelow(rel);
      node.visit((n: any) => {
        if (this.isFolder(n) && n.expanded) this.revalidateLevel(this.relPath(n));
      });
    },

    // After a directory nudge, re-read any open tab whose file lives in `rel`
    // and push the bytes to its viewer. A failure keeps the tab's bytes.
    refreshOpenViewers(rel: any) {
      if (!this.useDaemonTree()) return Promise.resolve();
      const dirOf = (p: any) => {
        if (typeof p !== "string") return null;
        const i = p.lastIndexOf("/");
        return i < 0 ? "" : p.slice(0, i);
      };
      const reads: any[] = [];
      for (const t of this.tabs) {
        if (t.project !== this.openSlug || dirOf(t.path) !== rel) continue;
        // A tab pinned to another checkout (#406) is not this nudge's.
        if ((t.checkout ?? null) !== (this._treeCheckout ?? null)) continue;
        // An image tab re-reads through its OWN verb (ADR-0049 §1). A text
        // tab re-reads with ITS encoding as the hint: the encoding is sticky
        // once opened, so a nudge never silently re-detects it.
        const payload = WBDaemon.withCheckout({ repo: t.project, path: t.path }, t.checkout);
        const enc = WBViewer.encodingOf?.(t.id)?.encoding;
        if (enc) payload.encoding = enc;
        const fresh =
          t.kind === "image"
            ? WBDaemon.readImage(t.project, t.path, undefined, t.checkout)
            : WBDaemon.observe("file.read", payload).then((reply) =>
                reply?.status === "ok" ? reply.content : null,
              );
        reads.push(
          fresh
            .then((content: any) => {
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
    async revealRel(rel: any, opts: any = {}) {
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
        const f = tree.findFirst((n: any) => this.relPath(n) === prefix);
        if (!f) return null; // an unmounted ancestor: nothing to reveal
        if (!f.expanded) await f.setExpanded(true);
      }
      const node = tree.findFirst((n: any) => this.relPath(n) === rel);
      if (!node) return null;
      node.setActive();
      return node;
    },

    // The folder node whose rel path is `rel`, or `null` if none is mounted.
    findFolderByRel(rel: any) {
      return this.rawTree()?.findFirst((n: any) => this.isFolder(n) && this.relPath(n) === rel) || null;
    },

    // The open project's run-snapshot subscription (#300, ADR-0047 §9).
    mountRunsSub() {
      if (!window.WBDaemon?.subscribeRuns || !this.openSlug) return;
      // A snapshot change means the tracker may have moved, so the same push
      // nudges the board (#301); the predicate coalesces it.
      this._runsSub = window.WBDaemon.subscribeRuns(this.openSlug, () => {
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

    // The run-completion subscription (#310, ADR-0036 amendment). The socket
    // carries EVERY repo's nudge, so the filter is here.
    mountChangesSub() {
      if (!window.WBDaemon?.subscribeChanges || !this.openSlug) return;
      this._changesSub = window.WBDaemon.subscribeChanges(this.openSlug, (frame: any) => {
        if (this.tabHidden()) return;
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
      // These states describe a tree that no longer exists.
      this.treeLoading = false;
      this.treeError = "";
      this.treeStale = "";
      this.treeNotLive = "";
      // A search describes THIS tree; the field stays open.
      this.resetFileSearch();
      this.hideMenu();
    },

    // --- opening a file into a tab ----------------------------------------
    openFile(node: any) {
      const path = this.relPath(node);
      const ftype = classify(node.title);
      this.emit("open", node, { ftype });
      // A note opens as a CARD, not as a tab (ADR-0064 §11): on the stage if
      // it is not there yet, and by a jump if it is.
      if (ftype === "note") {
        this.openNote(path);
        return;
      }
      if (ftype === "binary") {
        // Flash it too: a click that silently does nothing reads as a broken
        // tree.
        window.WB.emit("open-refused", { project: this.openSlug, path, reason: "binary" });
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
            label: this.projectLabel(project),
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
      window.WB.emit("open-diff", { project, path: t.workingPath, checkout: t.checkout });
      this.$nextTick(() => {
        // Latched: a path refused on BOTH sides would flash twice.
        let refused = false;
        const refuse = (reason: any) => {
          if (refused) return null;
          refused = true;
          this._flashAction?.(window.WBFail.failed({ message: reason }, "Could not open the diff: the daemon gave no reason."));
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
        if (window.WBFail.isError(reply)) return refuse(window.WBFail.message(reply, "refused"));
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
        if (!window.WBFail.isError(reply)) return reply.content;
        const reason = window.WBFail.message(reply, "refused");
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
      if (id === "spend" && this.spend.slug !== this.openSlug) this.refreshSpend();
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
      const r = window.WBSplit.resolve({
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
      return window.WBSplit.available(WBViewer.width());
    },

    closeTab(id: any) {
      const idx = this.tabs.findIndex((t) => t.id === id);
      const tab = this.tabs[idx];
      if (!tab || !tab.closable) return; // Consoles never closes
      // A closed tab takes its pin with it (a mirror follows the active tab).
      this.slot = window.WBSplit.afterClose(this.slot, id);
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
      window.WBView?.patch({
        tabs: files,
        active: alive ? this.active : "consoles",
        // ALWAYS written, null included: `patch` is read-modify-write, and a
        // key left out would let a stale pin outlive the tab it named.
        split: window.WBSplit.toStored(this.slot, this.splitRatio, this.tabs),
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
        this.slot = window.WBSplit.fromStored(stored.split, this.tabs);
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

    // --- consoles (the Consoles tab) ----------------------------------------
    // The "New console" menu (wb-agents.js): the roster folded against the
    // live sessions, plus a plain console pinned LAST. Each row carries an
    // Alt+Shift+<digit> accelerator, matched by physical key (e.code) so it
    // fires regardless of layout. Console is Alt+Shift+0; Alt+Shift+R opens the
    // menu with the console row's command field focused.
    liveSessions: [] as any[],
    // The console row's "Run…" field: one command line for ONE new console.
    // Never stored — the next console from the row or Alt+Shift+0 is a plain
    // shell again.
    consoleRunOpen: false,
    consoleRunText: "",
    consoleItems() {
      return window.WBAgents.menuRows({
        roster: this.roster,
        sessions: this.liveSessions,
        openSlug: this.openSlug,
      });
    },
    isMac: /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || ""),
    // The accelerator pattern, stated ONCE in the menu head (the Fence menu's
    // rule): each row carries only its digit.
    consoleShortcutHint() {
      return this.isMac ? "⌥⇧<n>" : "Alt+Shift+<n>";
    },
    // The menu head names the repo by its bare name — the owner and the
    // environment are the sidebar's to say, and neither changes which agent to
    // pick. The full ref rides the head's title.
    consoleMenuRepoName() {
      if (!this.openSlug) return "";
      const row = this.projects.find((p) => this.repoRef(p) === this.openSlug);
      const name = row ? window.WBProject.projectName(row) : window.WBFleet.refSlug(this.openSlug);
      return name.split("/").pop() || name;
    },
    // Every row is a launch (the menu is "New console"); `opts.tryAnyway` is
    // the unavailable row's escape hatch.
    openConsoleItem(item: any, opts: any = {}) {
      if (!window.WBAgents.consoleIntent(item, opts)) return;
      if (item.plain) this.newPlainConsole(item.command);
      else this.newConsole(item.kind);
      this.agentMenu = false;
    },

    newConsole(agent: any) {
      // The accelerator path calls this directly: refuse with no repo here too.
      if (!this.openSlug) return;
      if (this.active !== "consoles") this.activate("consoles");
      // Always the primary: only the console's own title switcher moves it
      // (ADR-0063, amendment 2026-09-16 b).
      WBConsole.open({ repo: this.openSlug, agent, checkout: null });
      this.consoleCount = WBConsole.count();
    },
    // a bare shell in the repo dir (no agent) — the daemon's per-repo console;
    // with `command`, the shell runs it instead of a prompt and the session
    // ends with it (the console row's "Run…" field)
    newPlainConsole(command: any) {
      if (this.active !== "consoles") this.activate("consoles");
      WBConsole.open({ repo: this.openSlug, plain: true, command: command || undefined });
      this.consoleCount = WBConsole.count();
    },
    openConsoleRun() {
      this.consoleRunOpen = true;
      this.$nextTick(() => this.$refs.consoleRun?.focus());
    },
    // Cancel keeps the menu open: Esc undoes only the field.
    closeConsoleRun() {
      this.consoleRunOpen = false;
      this.consoleRunText = "";
    },
    // A blank line is not a launch: the field stays open for the typing.
    runConsoleCommand() {
      const command = window.WBAgents.runCommand(this.consoleRunText);
      if (!command) return;
      this.newPlainConsole(command);
      this.agentMenu = false;
      this.closeConsoleRun();
    },
    // Alt+Shift+R: the menu, open (never toggled shut), with the field focused.
    // The menu lives in the Consoles tab's toolbar: on any other tab it would
    // open hidden.
    openConsoleRunMenu() {
      if (this.active !== "consoles") this.activate("consoles");
      this.closeMenus();
      this.agentMenu = true;
      this.openConsoleRun();
    },

    // Accelerators are ignored while typing or while a modal is up.
    // `allowTerminal`: a key that must also work from inside a terminal (the
    // column walk); xterm's input is a TEXTAREA.
    consoleShortcutsBlocked(allowTerminal = false) {
      if (!this.authed) return true;
      if (this.modalOpen(window.WBSettingsDialog.openFlag) || this.modalOpen(window.WBSecurityDialog.openFlag) || this.runOpen || this.branchOpen) return true;
      if (this.modalOpen(window.WBReleaseDialogs.whatsNewFlag)) return true;
      const el: any = document.activeElement;
      if (allowTerminal && el?.closest?.(".xterm")) return false;
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
      return this.fenceAtCap() ? this.fenceCapMessage() : "Draw a named fence on the stage";
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
      this.closeConsoleRun();
      this.windowMenu = false;
      this.fenceMenu = false;
      this.noteMenu = false;
      this.avatarMenu = false;
      this.columnMenu = false;
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
    revealWindow(id: any) {
      if (this.active !== "consoles") this.activate("consoles");
      this.windowMenu = false;
      // AFTER the tab is laid out: a `display:none` tab measures 0.
      this.$nextTick(() => WBConsole.reveal(id));
    },

    // The note cap, stated before the click — `fenceAtCap`'s shape, and for
    // the same reason: the module's array is not Alpine state, so a binding on
    // it would never re-evaluate.
    noteAtCap() {
      return this.noteItems.length >= 32;
    },
    noteCapReason() {
      if (!this.openSlug) return "Open a project first. A note is saved in its checkout.";
      if (this.noteAtCap()) return "Maximum of 32 notes. Close one to add another.";
      return "Write a note on the stage";
    },
    toggleNoteMenu() {
      this.noteItems = window.WBNotes.list();
      const was = this.noteMenu;
      this.closeMenus();
      this.noteMenu = !was;
    },
    // A new note (ADR-0064 §9): no dialog. The card lands in the middle of
    // what the operator is looking at, in the project's selected checkout —
    // where the file will be written is a field in the card's own footer.
    newNote() {
      if (!this.openSlug) return;
      if (this.active !== "consoles") this.activate("consoles");
      this.noteMenu = false;
      // AFTER the tab is laid out, as `revealWindow`: a `display:none` tab
      // measures a 0×0 viewport and every card would land at the origin.
      this.$nextTick(() => {
        const ws = document.getElementById("workspace");
        window.WBNotes.create({
          repo: this.openSlug,
          checkout: window.WBConsole.checkoutOf(this.openSlug),
          viewport: { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 },
          offset: { left: ws?.scrollLeft || 0, top: ws?.scrollTop || 0 },
        });
      });
    },

    // Open a `.note` as a card (ADR-0064 §11). The module decides whether this
    // is a jump to a card already on the plane or a new one; this layer only
    // puts the operator on the tab that holds the stage.
    openNote(path: any, project?: any, checkout?: any) {
      const repo = project || this.openSlug;
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

    // The note list is the map too (ADR-0064 §10): the row slides the plane to
    // the card.
    jumpNote(id: any) {
      if (this.active !== "consoles") this.activate("consoles");
      this.noteMenu = false;
      // As `revealWindow`: a `display:none` tab measures a 0×0 viewport.
      this.$nextTick(() => window.WBNotes.jump(id));
    },
    // Keep a card on top, or put it back (ADR-0064, 2026-09-26 amendment).
    // The menu closes on the way on top so the card is in view; putting back
    // keeps it open and redraws the rows.
    toggleOnTop(n: any) {
      if (n.away) return;
      if (n.onTop) {
        window.WBNotes.putBack();
        this.noteItems = window.WBNotes.list();
        return;
      }
      if (this.active !== "consoles") this.activate("consoles");
      this.noteMenu = false;
      // As `jumpNote`: a `display:none` tab measures a 0×0 viewport.
      this.$nextTick(() => window.WBNotes.keepOnTop(n.id));
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
    stepFence(step: any) {
      if (this.active !== "consoles") return null;
      return WBConsole.stepFence(step);
    },
    // Alt+Shift+arrows among the painted consoles: "x" across the columns,
    // "y" along the rows of one column. Returns whether it applied.
    stepColumn(axis: any, step: any) {
      if (this.active !== "consoles" || this.columnIds().length < 2) return false;
      const painted = WBColumns.painted(this.columns, this.columnCap());
      const to = WBColumns.focusMove(painted, WBConsole.focusedId(), axis, step);
      if (to) WBConsole.focusColumn(to);
      return true;
    },
    // The columns while they are open, the fences otherwise (ADR-0051 §5). The
    // fences have no "y": ↑/↓ with no columns open applies nothing.
    arrowStep(axis: any, step: any) {
      if (this.columnIds().length >= 2) return this.stepColumn(axis, step);
      return axis === "x" && !!this.stepFence(step);
    },
    jumpFence(id: any) {
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
    fenceShortcutLabel(n: any) {
      return `F${n}`;
    },
    fenceShortcutHint() {
      return this.isMac ? "⌥⇧F<n>" : "Alt+Shift+F<n>";
    },
    // --- columns (ADR-0051 §5) --------------------------------------------
    // INVARIANT: the shell never writes `max` itself. Each `applyColumns` call
    // passes `persist`, so `setMax` writes it for the first console in reading
    // order (`true`) or one that stopped being first (`false`). `columns` is
    // the grid: a list of columns, each a list of ids (`wb-columns.js`).
    columnIds() {
      return WBColumns.flat(this.columns);
    },
    columnCap() {
      return WBColumns.cap(WBConsole.columnMeasure().viewport, WBConsole.PHONE_MAX_WIDTH);
    },
    // The ONE writer of `columns`. The view store is written only when the grid
    // changes: `paintColumns` runs on every `consoles-changed` during boot with
    // an empty grid, and an unconditional write would erase the stored grid
    // before `restoreColumns` reads it.
    setColumns(next: any) {
      if (JSON.stringify(next) === JSON.stringify(this.columns)) return;
      this.columns = next;
      window.WBView?.patch({ columns: WBColumns.toStored(next) });
    },
    // Once, on `workbench:desk-restored`. A list the desk does not confirm is
    // ignored and cleared from the store; a kept one is painted with `persist`.
    restoreColumns() {
      if (this._columnsRestored) return;
      this._columnsRestored = true;
      const stored = window.WBView?.read();
      this.columnDir = WBColumns.dirOf(stored?.columnDir);
      const raw = stored?.columns ?? null;
      const next = WBColumns.fromStored(raw, WBConsole.deskRecords());
      this.setColumns(next);
      // `setColumns` writes only a change; an ignored grid meets an empty one.
      // A flat list stored before rows is written back as a grid.
      if (next.length && JSON.stringify(raw) !== JSON.stringify(next)) {
        window.WBView?.patch({ columns: WBColumns.toStored(next) });
      }
      if (!next.length && raw !== null) window.WBView?.patch({ columns: null });
      if (next.length) this.paintColumns({ raise: true });
    },
    effectiveColumns(fromId: any) {
      return this.columnIds().includes(fromId) ? this.columns : [[fromId]];
    },
    // The Right | Down choice at the top of the list, kept in this browser.
    setColumnDir(dir: any) {
      this.columnDir = WBColumns.dirOf(dir);
      window.WBView?.patch({ columnDir: this.columnDir });
    },
    // Re-derive what is painted from the list and the current cap. A console
    // that left the stage (closed, detached) leaves the list.
    paintColumns(opts?: any) {
      const byId = new Map(
        [...document.querySelectorAll<any>("#stage .session-window")].map((w) => [w._deskId, w]),
      );
      const head = this.columnIds()[0];
      const headWin = head ? byId.get(head) : null;
      // At a cap of 1 the first console is painted as a plain maximize, so its
      // Restore took the maximize path. It is still a column restore.
      if (headWin && !headWin.classList.contains("maximized") && !headWin.classList.contains("column")) {
        this.restoreColumn(head);
        return;
      }
      // A first console off the stage for a relaunch comes back under the same id.
      if (head && !headWin && WBConsole.isRelaunching(head)) return;
      const kept = WBColumns.keep(this.columns, new Set(byId.keys()));
      const keptIds = WBColumns.flat(kept);
      // The first console left the stage and one is left: it takes the maximize.
      if (head && !headWin && keptIds.length === 1) {
        const cap = this.columnCap();
        this.setColumns([]);
        WBConsole.applyColumns(WBColumns.painted(kept, cap), { cap, unmax: null, persist: true });
        return;
      }
      // The KEPT grid is stored, never the painted slice: a console the cap
      // hides comes back when the cap grows again.
      this.setColumns(keptIds.length >= 2 ? kept : []);
      const left =
        this.columnIds()[0] ?? document.querySelector<any>("#stage .session-window.maximized")?._deskId;
      // A hidden consoles tab measures 0 wide, which reads as a cap of 1: keep
      // the painted columns as they are until the tab shows again.
      if (left && !WBConsole.columnMeasure().viewport) return;
      const cap = left ? this.columnCap() : 1;
      const before = WBConsole.focusedId();
      const painted = WBColumns.painted(this.columns, cap);
      const ids = painted.map((p: any) => p.id);
      const key = ids.join(" ");
      // A column that stops being painted falls back to its plane rect with its
      // old z-index; raising the painted ones keeps it behind them.
      const moved = this.columnIds().length >= 2 && key !== this._paintedKey;
      this._paintedKey = key;
      WBConsole.applyColumns(painted, { cap, unmax: null, raise: !!opts?.raise || moved, persist: true });
      // Only when the keys are not somewhere else (a search box, a modal).
      const el = document.activeElement;
      const keysFree = !el || el === document.body || !!el.closest?.(".session-window");
      if (this.active === "consoles" && keysFree && this.columnIds().includes(before)) {
        const want = WBColumns.focusAfter(ids, before);
        if (want && (want !== before || moved)) WBConsole.focusColumn(want);
      }
    },
    // A fence detached to a popup takes its consoles out of the columns.
    leaveColumns(ids: any) {
      const r = WBColumns.external(this.columns, { type: "detached", ids });
      if (!r.changed) return;
      const cap = r.columns.length ? this.columnCap() : 1;
      this.setColumns(r.ended ? [] : r.columns);
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: r.unmax, raise: true, persist: true });
    },
    // Consoles whose records another client removed (`ids`, from the console
    // module after a desk read) leave the columns, then the stage. A session
    // that ended, a remote maximize and a remote rect or fence change need
    // nothing here: `WBColumns.external` names them as no-ops.
    checkColumnDesk(ids: any) {
      const gone = this.columnIds().filter((id: any) => ids.includes(id));
      const r = WBColumns.external(this.columns, { type: "closed", ids: gone });
      if (r.changed) {
        const cap = r.columns.length ? this.columnCap() : 1;
        this.setColumns(r.ended ? [] : r.columns);
        // Painted BEFORE the drops, so a lone survivor is maximized first.
        WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: null, raise: true, persist: true });
      }
      for (const id of ids) WBConsole.dropClosedElsewhere(id);
      if (r.changed) this.paintColumns();
    },
    toggleColumnMenu(id: any, rect: any) {
      const was = this.columnMenu && this.columnFrom === id;
      const ids = WBColumns.flat(this.effectiveColumns(id));
      this.columnGroups = WBColumns.listFold({
        ...WBConsole.columnRoster(),
        columns: ids,
        from: id,
        full: ids.length >= this.columnCap(),
      });
      this.columnFrom = id;
      const top = Math.round((rect?.bottom || 0) + 4);
      const right = Math.round(rect?.right || 0);
      // Right edge on the button's right edge, and no larger than the room
      // left of it and under it: the list grows with its longest row, and a
      // fixed guess at its width pushed it past the edge of the window.
      this.columnMenuAt = {
        top,
        right: Math.max(8, window.innerWidth - right),
        maxWidth: Math.max(200, right - 8),
        maxHeight: Math.max(120, window.innerHeight - top - 8),
      };
      this.columnFilter = "";
      this.closeMenus();
      this.columnMenu = !was;
      if (this.columnMenu) this.$nextTick(() => this.$refs.columnFilter?.focus());
    },
    columnFilterShown() {
      return this.columnGroups.reduce((n, g) => n + g.rows.length, 0) >= WBColumns.FILTER_MIN;
    },
    // `owner/repo` without the environment: the operator already knows where
    // each console runs, and the list is about telling the consoles apart.
    columnRepoLabel(ref: any) {
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      return row ? window.WBProject.projectName(row) : window.WBFleet.refLabel(ref);
    },
    columnView() {
      return WBColumns.filterGroups(this.columnGroups, this.columnFilter, (ref: any) => this.columnRepoLabel(ref));
    },
    columnRowLabel(r: any) {
      return WBColumns.rowLabel(r, window.WBConsoleName.consoleLabel);
    },
    // Enter in the filter opens the first row that can be opened; at the cap,
    // it swaps in the first row that can be swapped.
    openFirstColumn() {
      const rows = this.columnView().flatMap((g: any) => g.rows);
      const open = rows.find((r: any) => r.enabled);
      if (open) return this.openColumn(open.id);
      const swap = rows.find((r: any) => r.swappable);
      if (swap) this.swapColumn(swap.id);
    },
    openColumn(id: any) {
      const from = this.columnFrom;
      if (!from) return;
      const cols = this.effectiveColumns(from);
      const out = WBColumns.open(cols, from, id, this.columnCap(), this.columnDir);
      if (!out.ok) {
        if (out.reason) this._flashAction(out.reason);
        return;
      }
      this.setColumns(out.columns);
      this.columnMenu = false;
      this.paintColumns({ raise: true });
      WBConsole.focusColumn(id);
    },
    // Put `id` in the row that opened the list (ADR-0051 §5, swap).
    swapColumn(id: any) {
      const from = this.columnFrom;
      if (!from) return;
      const r = WBColumns.swap(this.effectiveColumns(from), from, id);
      if (!r.ok) return;
      const cap = this.columnCap();
      this.setColumns(r.ended ? [] : r.columns);
      this.columnMenu = false;
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: r.unmax, raise: true, persist: true });
      this.paintColumns();
      WBConsole.focusColumn(id);
    },
    restoreColumn(id: any) {
      const r = WBColumns.restore(this.columns, id);
      const cap = r.columns.length ? this.columnCap() : 1;
      this.setColumns(r.ended ? [] : r.columns);
      // It may promote a lone survivor to the maximize.
      WBConsole.applyColumns(WBColumns.painted(r.columns, cap), { cap, unmax: r.unmax, raise: true, persist: true });
      this.paintColumns();
    },
    // A fence opened as columns (ADR-0051 §5, 2026-09-30): the grid follows the
    // members' stage rects and replaces the columns that are open.
    columnsFromFence(items: any) {
      const grid = WBColumns.fromRects(items);
      const ids = WBColumns.flat(grid);
      if (!ids.length) return;
      const old =
        this.columnIds()[0] ?? document.querySelector<any>("#stage .session-window.maximized")?._deskId;
      const cap = ids.length >= 2 ? this.columnCap() : 1;
      this.setColumns(ids.length >= 2 ? grid : []);
      WBConsole.applyColumns(WBColumns.painted(grid, cap), {
        cap,
        unmax: old && !ids.includes(old) ? old : null,
        raise: true,
        persist: true,
      });
      this.paintColumns();
      WBConsole.focusColumn(ids[0]);
    },

    // Ordinal, not id: the row's position in `fenceList()`, read LIVE (the
    // menu's snapshot may be stale). Returns whether it landed.
    jumpFenceAt(n: any) {
      if (this.active !== "consoles") return false;
      const f = WBConsole.fenceList()[n - 1];
      if (!f) return false;
      this.fenceMenu = false;
      return !!WBConsole.jumpToFence(f.id);
    },

    // --- context menu -----------------------------------------------------
    // `node` is null for empty tree space, which addresses the repo root: the
    // create items apply, the per-node items drop out.
    showMenu(x: any, y: any, node: any) {
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
      this.renderMenu(x, y, items);
    },

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

    // --- the backend seam -------------------------------------------------
    // Build the repo-relative path by walking parent titles.
    relPath(node: any) {
      const parts: any[] = [];
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
    copyPath(node: any, full = false) {
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
    async duplicateNode(node: any) {
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
        this._flashAction?.(
          reply
            ? window.WBFail.failed(reply, "Could not duplicate the file: the daemon gave no reason.")
            : "Could not duplicate the file: the daemon did not answer.",
        );
        return;
      }
      await this.onTreeDirty(parent);
      await this.revealRel(to);
    },

    // --- move (issue #364) ------------------------------------------------
    // The destination is PICKED, never typed: the picker browses real
    // directories through `tree.list`.
    moveNode(node: any) {
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

    async movePickLoad(dir: any) {
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
        this.movePick.error = WBFail.failed(listing, "Could not list the folder: the daemon gave no reason.");
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

    movePickInto(name: any) {
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

    // Through `WBDaemon.write`, not the fire-and-forget `window.WB.emit("rename")`:
    // the reveal, the flash and the tab re-path need the reply. INVARIANT: no
    // tab is re-pathed and no reveal happens on a refusal.
    async performMove(from: any, to: any) {
      const reply = await WBDaemon.write(
        "file.rename",
        WBDaemon.withCheckout(
          { repo: this.openSlug, path: from, to },
          this.checkoutOf(this.openSlug),
        ),
      ).catch(() => null);
      if (!reply) {
        this._flashAction?.("Could not move: the daemon did not answer.");
        return;
      }
      if (WBFail.isError(reply)) {
        this._flashAction?.(WBFail.failed(reply, "Could not move: the daemon gave no reason."));
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
    repathTabs(from: any, to: any) {
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
        if (this.slot?.kind === "pin" && this.slot.id === t.id) this.slot = { kind: "pin", id: newId };
        if (this.lastLeft === t.id) this.lastLeft = newId;
        t.path = newPath;
        t.id = newId;
      }
      // The viewer's own "what is shown" is keyed by id too.
      this.$nextTick(() => this.syncViewer());
      this.persistView();
    },

    // A `create` intent carries the DIRECTORY, already resolved (`createDir`).
    emitCreate(node: any, kind: any) {
      window.WB.emit("create", { project: this.openSlug, path: this.createDir(node), kind, isFolder: true });
    },

    // The directory a create addressed at `node` lands in: the folder itself,
    // the folder CONTAINING a file, or the repo root ("") for no node at all.
    createDir(node: any) {
      const rel = node ? this.relPath(node) : "";
      return !node || this.isFolder(node) ? rel : parentRel(rel);
    },

    // The Files-header buttons create relative to the tree's active node.
    createHere(kind: any) {
      this.emitCreate(this.rawTree()?.getActiveNode() || null, kind);
    },

    // The header buttons' tooltip: the directory a create lands in.
    createTitle(kind: any) {
      return newEntryTitle(kind, this.createDir(this.rawTree()?.getActiveNode() || null));
    },

    // Node-shaped gestures funnel through the shared WB.emit.
    emit(action: any, node: any, extra: any = {}) {
      window.WB.emit(action, {
        project: this.openSlug,
        path: this.relPath(node),
        title: node.title,
        isFolder: this.isFolder(node),
        ...extra,
      });
    },

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
    promptRespond(name: any) {
      this.promptModal.open = false;
      const resolve = this._promptResolve;
      this._promptResolve = null;
      if (resolve) resolve(name);
    },
  });
}

// Everything the page wires at load: the window names classic scripts call,
// the document and window listeners, and the state of detached windows.
// `window` and `document` are parameters so a test passes its own stubs, and
// each call starts from fresh state (ADR-0075 D7).
export function wire(window: Window, document: Document) {
  // The one exit point: every gesture becomes a `workbench:action` event.
  // A classic script still reads these names (ADR-0075 D9).
  window.WB = {
    emit(action: any, detail: any = {}) {
      const full = { action, ...detail, at: new Date().toISOString() };
      document.dispatchEvent(new CustomEvent("workbench:action", { detail: full }));
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

  // The Alpine mirror of the live console count.
  document.addEventListener("workbench:consoles-changed", (e: any) => {
    const c = window.getShell();
    if (!c) return;
    c.consoleCount = e.detail.count;
    c.paintColumns();
  });

  // A console's title bar asked for the columns list, or to restore a column;
  // or something changed the cap (maximize, first measurable frame).
  document.addEventListener("workbench:column-open", (e: any) => {
    window.getShell()?.toggleColumnMenu(e.detail.id, e.detail.rect);
  });
  document.addEventListener("workbench:column-restore", (e: any) => {
    window.getShell()?.restoreColumn(e.detail.id);
  });
  document.addEventListener("workbench:columns-stale", () => {
    window.getShell()?.paintColumns();
  });
  document.addEventListener("workbench:columns-leave", (e: any) => {
    window.getShell()?.leaveColumns(e.detail.ids);
  });
  document.addEventListener("workbench:fence-columns", (e: any) => {
    window.getShell()?.columnsFromFence(e.detail.items);
  });
  document.addEventListener("workbench:desk-restored", () => {
    window.getShell()?.restoreColumns();
  });
  // A narrower or wider viewport changes the cap. One repaint per frame.
  let columnsFrame = 0;
  window.addEventListener("resize", () => {
    if (columnsFrame) return;
    columnsFrame = requestAnimationFrame(() => {
      columnsFrame = 0;
      window.getShell()?.paintColumns();
    });
  });

  // …and of the stage extent, for the footer pill (#338).
  document.addEventListener("workbench:stage-extent", (e: any) => {
    const c = window.getShell();
    if (!c) return;
    c.stageW = e.detail.width;
    c.stageH = e.detail.height;
  });

  // A viewer asked to detach → open the popup and close the tab.
  document.addEventListener("workbench:detach-request", (e: any) => {
    window.getShell()?.detachFile(e.detail);
  });

  // A rendered markdown link asked for a repo file → open (or focus) its tab.
  document.addEventListener("workbench:open-request", (e: any) => {
    window.getShell()?.openLink(e.detail);
  });

  // The divider between the two panes was dragged: the ratio is view state.
  document.addEventListener("workbench:split-ratio", (e: any) => {
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
  document.addEventListener("workbench:canvas-resize", (e: any) => {
    const sh = window.getShell();
    if (!sh) return;
    const wide = window.WBSplit.available(e.detail.width);
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
          if (window.WBFail.isError(reply)) flash(window.WBFail.failed(reply, "Could not rename: the daemon gave no reason."));
          else if (okMsg) flash(okMsg);
        })
        .catch(() => flash("Could not rename: the daemon did not answer."));
    };

    document.addEventListener("workbench:action", async (e: any) => {
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
          const send = (p: any) =>
            WBDaemon.write("file.write", aimed(p))
              .then((reply: any) => {
                if (!window.WBFail.isError(reply)) return viewer()?.saveDone?.(id);
                const reason = window.WBFail.message(reply, "the daemon gave no reason");
                viewer()?.saveFailed?.(id, reason, reply);
                // UTF-8 represents everything; a refusal under it is not a
                // conversion question, and asking again would loop.
                if (reason === "unencodable" && !/^utf-?8$/i.test(p.encoding || "utf-8")) {
                  return offerUtf8(p, reply);
                }
                flash(window.WBFail.failed(reply, "Could not save: the daemon gave no reason."));
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
          if (d.message) c?._flashAction?.(d.message.split("\n").map(window.WBFail.sentence).filter(Boolean).join(" "));
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
          if (window.WBFail.isError(reply)) return flash(window.WBFail.failed(reply, `Could not create ${name}: the daemon gave no reason.`));
          flash(`${name} created.`);
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
          if (!reply) return flash("Could not delete: the daemon did not answer.");
          if (!window.WBFail.isError(reply)) return flash(`${name} deleted.`);
          const reason = window.WBFail.message(reply, "the daemon gave no reason");
          flash(window.WBFail.failed(reply, "Could not delete: the daemon gave no reason."));
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
  document.addEventListener("click", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu")!.style.display = "none"));
  document.addEventListener("scroll", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu")!.style.display = "none"), true);

  // Alt+Shift+<digit> → the menu row carrying that digit, through the SAME row
  // action as a click. Matched on `e.code` so layout does not matter. R is no
  // row: it opens the menu on the console row's command field, so the digits
  // stay a sequence of rows. They work from inside a terminal too: its xterm
  // hands them over (wb-console.js).
  document.addEventListener("keydown", (e) => {
    if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
    if (!/^(?:Digit\d|KeyR)$/.test(e.code)) return;
    const c = window.getShell();
    if (!c || c.consoleShortcutsBlocked(true)) return;
    if (e.code === "KeyR") {
      e.preventDefault();
      c.openConsoleRunMenu();
      return;
    }
    const row = c.consoleItems().find((it: any) => e.code === "Digit" + it.digit);
    // No row, or a disabled one: inert, and the key is not swallowed.
    if (!row || row.disabled) return;
    e.preventDefault();
    c.openConsoleItem(row);
  });

  // Alt+Shift+arrows → walk the columns (←/→) and the rows of a column (↑/↓)
  // while two or more consoles are open, from inside a column's terminal too;
  // otherwise ←/→ walk the fences in reading order (`fenceCycle`) — ADR-0051 §5.
  // With nothing to walk the key is left UNSWALLOWED.
  const ARROW_STEPS = {
    ArrowRight: ["x", 1],
    ArrowLeft: ["x", -1],
    ArrowDown: ["y", 1],
    ArrowUp: ["y", -1],
  };
  document.addEventListener("keydown", (e) => {
    if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
    const move = ARROW_STEPS[e.code as keyof typeof ARROW_STEPS];
    if (!move) return;
    const c = window.getShell();
    if (!c || c.consoleShortcutsBlocked(c.columnIds().length >= 2)) return;
    if (!c.arrowStep(...move)) return;
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
    if (c.modalOpen(window.WBSettingsDialog.openFlag) || c.modalOpen(window.WBSecurityDialog.openFlag) || c.runOpen || c.branchOpen || c.modalOpen(window.WBReleaseDialogs.whatsNewFlag)) return;
    e.preventDefault();
    c.openFileSearch();
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
