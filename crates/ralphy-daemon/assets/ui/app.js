/* ---------------------------------------------------------------------------
   ralphy workbench shell — shell behaviour

   The sidebar is a project accordion (Alpine). The file tree inside the open
   project is a real Wunderbaum instance (mar10/wunderbaum) — a mature,
   dependency-free tree lib — loaded from a JSON tree the backend would send.

   The canvas is a tabbed workspace:
     • the first tab, "Agents", is fixed (never closes) and hosts the floating
       console windows (see wb-console.js);
     • every opened file rides in as its own closable tab, rendered by a viewer
       (source code via CodeMirror, Markdown rendered with mermaid — see
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

// Files whose bytes aren't source we can render — refuse to open them.
const BINARY_EXT = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "pdf", "zip", "gz",
  "tar", "rar", "7z", "exe", "dll", "so", "dylib", "bin", "class", "jar", "wasm",
  "mp3", "wav", "flac", "ogg", "mp4", "mov", "avi", "mkv", "webm", "woff",
  "woff2", "ttf", "eot", "otf",
]);

function extOf(name) {
  const n = name.toLowerCase();
  return n.includes(".") ? n.split(".").pop() : "";
}

// What kind of viewer a file gets: markdown gets the rendered pane, binaries
// are refused, everything else opens as source code.
function classify(name) {
  const ext = extOf(name);
  if (ext === "md" || ext === "markdown") return "markdown";
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
    // Live presence + identity (#204): the topbar uptime is the `/ws` heartbeat's
    // age, and the brand/avatar are `/api/identity`. Empty until the first tick /
    // a baptized daemon; `_lastHeartbeat` (epoch ms) drives the stale indicator.
    uptimeText: "",
    identityName: "",
    identityAvatar: "",
    _lastHeartbeat: 0,
    _tree: null, // the live Wunderbaum instance, if any
    _treeSub: null, // the live `/ws/tree` subscription for the open project, if any

    // Alpine lifecycle: hydrate the Runs seed once the DOM (incl. the hidden
    // plan <script> blocks) is present.
    init() {
      this.initRuns();
      this.currentRunId = this.projectRuns()[0]?.runid || null;
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      this.probeSession();
      this.loadRepos();
      this.subscribePresence();
      this.loadIdentity();
    },

    // The daemon's real identity (name + avatar), shown in the topbar brand. A
    // 404 (un-baptized daemon) or a thrown fetch (file:// demo) leaves the
    // fields empty and the markup falls back to `ralphy` / no avatar.
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
        }
      } catch {}
    },

    // Hydrate the accordion from the daemon's real repo registry. A thrown
    // fetch (file:// standalone, no daemon) is swallowed so `projects` keeps
    // its seed — same offline-navigable contract as `probeSession()`. `state`
    // maps only to idle/offline this slice ("live" means an active session,
    // not yet tracked here); `remote` is inferred from the slug shape
    // (`git::project_slug`'s only `path-<hash>` fallback is a remoteless repo).
    async loadRepos() {
      try {
        const r = await fetch("/api/repos");
        if (r.ok) {
          const repos = await r.json();
          this.projects = repos.map((x) => ({
            slug: x.slug,
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
      }
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
        for (const p of this.projects) {
          if (p.state === "offline") continue;
          p.state = sessions.some((s) => s.repo === p.slug) ? "live" : "idle";
        }
      } catch {}
    },

    // --- chrome panels ----------------------------------------------------
    // Projects sidebar visibility (rail Projects button), the right-hand Runs
    // panel (rail Runs button), and the Kanban/tasks board (rail Kanban button,
    // a stub for now). Each is a pure layout flip driven by a body class.
    sideOpen: true,
    runsOpen: false,
    kanbanOpen: false,
    projectQuery: "",

    toggleSide() {
      this.sideOpen = !this.sideOpen;
    },

    // Case-insensitive slug/branch filter over the sidebar project list. The
    // sidebar count keeps showing `projects.length` (total located) — the
    // filter is a view concern, not a change to what's located.
    filteredProjects() {
      const q = this.projectQuery.trim().toLowerCase();
      if (!q) return this.projects;
      return this.projects.filter(
        (p) => p.slug.toLowerCase().includes(q) || p.branch.toLowerCase().includes(q)
      );
    },

    // Opens the sidebar (if collapsed) and focuses the project search input —
    // the target of the global `/` shortcut.
    focusProjectSearch() {
      this.sideOpen = true;
      this.$nextTick(() => this.$refs.projectSearch?.focus());
    },
    toggleRuns() {
      this.runsOpen = !this.runsOpen;
      // the panel's lucide icons mount on open (they live inside x-if)
      if (this.runsOpen) this.$nextTick(() => window.lucide?.createIcons());
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

    branchChipTitle(p) {
      if (!this.canSwitchBranch(p)) return "repo unreachable — branch switching unavailable";
      return (p.dirty ? "switch branch (uncommitted changes) — " : "switch branch — ") + p.branch;
    },

    openBranchModal(p) {
      if (!this.canSwitchBranch(p)) return;
      this.branchModal = {
        slug: p.slug,
        filter: "",
        branches: [...(p.branches || [p.branch])],
        current: p.branch,
        dirty: !!p.dirty,
      };
      this.branchOpen = true;
      this.loadBranches(p.slug);
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
        const p = this.projects.find((x) => x.slug === slug);
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
      const p = this.projects.find((x) => x.slug === slug);
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
    currentRunId: null,
    runMenu: false,
    planSection: "",

    // On-device translation of a plan block via the browser's built-in
    // Translator API (Chrome/Edge 138+), with LanguageDetector for the source.
    // No network, no key. Per-block toggle; results cached by run/section/target.
    // Degrades to a disabled button where the API is absent.
    xlate: {
      on: {}, // block id ("steps" | "more") -> translating?
      busy: {}, // block id -> in-flight?
      err: {}, // block id -> last error message
      note: {}, // block id -> hint (e.g. "already PT")
      target: window.WBTranslate.browserLang(),
      cache: {}, // `${runid}::${name}::${target}` -> translated markdown
    },
    xlateLangs: window.WBTranslate.LANGS,

    // Hydrate runs from the seed: copy each run's plan.md out of its hidden
    // <script> block into a live, mutable `planMd` the fold can update.
    initRuns() {
      const src = window.WB_RUNS || {};
      const out = {};
      for (const [proj, runs] of Object.entries(src)) {
        out[proj] = runs.map((r) => ({
          ...r,
          planMd: (document.getElementById(r.planEl)?.textContent || "").trim(),
        }));
      }
      this.runsByProject = out;
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
      // reset the section dropdown to the new run's first non-Steps heading
      this.planSection = this.planHeadings(this.currentRun())[0] || "";
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // Thin delegations to the faithful helpers in wb-runs.js.
    runPhaseLabel(run) {
      return run ? window.WBRun.runPhaseLabel(run) : "";
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
      if (iss.blockedBy?.length) t += ` (blocked by ${iss.blockedBy.map((n) => "#" + n).join(", ")})`;
      return t;
    },
    // Clicking an issue node is a read intent — a backend could scroll its log or
    // surface that issue's plan; today it only announces it.
    focusIssue(number) {
      WB.emit("run-issue-focus", { project: this.openSlug, runid: this.currentRun()?.runid, issue: number });
    },

    // --- plan viewer ------------------------------------------------------
    // Every `##` section except Steps (which is pinned in its own block above).
    planHeadings(run) {
      return window.WBRun.headings(run?.planMd).filter((h) => h.toLowerCase() !== "steps");
    },
    // Render one `##` section as sanitized HTML. When the block is toggled to
    // translate, the cached translation is shown once ready (original until then).
    // Steps render as glyph bullets so the checkbox state survives sanitising.
    renderPlanSection(run, name, block) {
      if (!run || !name) return "";
      let body = window.WBRun.section(run.planMd, name);
      if (block && this.xlate.on[block]) {
        const hit = this.xlate.cache[this.xlateKey(run, name)];
        if (hit != null) body = hit;
      }
      if (name.toLowerCase() === "steps") body = window.WBRun.stepsToGlyphs(body);
      return DOMPurify.sanitize(marked.parse(body || "_(empty)_"));
    },

    // --- on-device translation (shared helper: window.WBTranslate) --------
    xlateSupported() {
      return window.WBTranslate.supported();
    },
    xlateTitle() {
      return this.xlateSupported()
        ? "translate this block on-device (browser Translator API)"
        : "translation needs Chrome/Edge 138+ (built-in Translator API)";
    },
    xlateKey(run, name) {
      return `${run.runid}::${name}::${this.xlate.target}`;
    },
    toggleXlate(block, name) {
      if (!this.xlateSupported()) return;
      this.xlate.on = { ...this.xlate.on, [block]: !this.xlate.on[block] };
      if (this.xlate.on[block]) this.ensureXlate(block, name);
      this.$nextTick(() => window.lucide?.createIcons());
    },
    // the section dropdown changed → re-translate that block if it's on
    onSectionChange() {
      if (this.xlate.on.more) {
        this.ensureXlate("more", this.planSection || this.planHeadings(this.currentRun())[0]);
      }
    },
    // target language changed → refresh every active block
    retranslate() {
      if (this.xlate.on.steps) this.ensureXlate("steps", "Steps");
      if (this.xlate.on.more) {
        this.ensureXlate("more", this.planSection || this.planHeadings(this.currentRun())[0]);
      }
    },
    // Fetch (and cache) the translation for one block. Detects the source
    // language, then runs the on-device Translator; a same-language target is a
    // clean no-op. Reverts the toggle on failure so the UI stays honest.
    async ensureXlate(block, name) {
      const run = this.currentRun();
      if (!run || !name || !this.xlateSupported()) return;
      const src = window.WBRun.section(run.planMd, name);
      if (!src) return;
      const key = this.xlateKey(run, name);
      if (this.xlate.cache[key] != null) return; // already translated
      this.xlate.busy = { ...this.xlate.busy, [block]: true };
      this.xlate.err = { ...this.xlate.err, [block]: "" };
      this.xlate.note = { ...this.xlate.note, [block]: "" };
      try {
        const res = await window.WBTranslate.translate(src, this.xlate.target, (msg) => {
          this.xlate.note = { ...this.xlate.note, [block]: msg };
        });
        this.xlate.cache = { ...this.xlate.cache, [key]: res.text };
        // a same-language target changes nothing — say so, so it doesn't look broken
        if (res.same) {
          this.xlate.note = { ...this.xlate.note, [block]: `already ${this.xlate.target.toUpperCase()}` };
        } else {
          this.xlate.note = { ...this.xlate.note, [block]: "" }; // clear the download-progress note
        }
      } catch (e) {
        this.xlate.err = { ...this.xlate.err, [block]: e?.message || "translate failed" };
        this.xlate.on = { ...this.xlate.on, [block]: false }; // revert on failure
      } finally {
        this.xlate.busy = { ...this.xlate.busy, [block]: false };
      }
    },

    // --- run / triage / push (the daemon verbs) ---------------------------
    // The three remote-trigger verbs (ralphy-daemon dispatch.rs), scoped to the
    // open project. `triage`/`push` are blessed no-arg invocations fired straight
    // onto the seam; `run` opens a modal to enrich it with the agent(s) + branch
    // mode. Faithful flags: --agent (executor, default claude), --plan-agent
    // (optional planner), --branch-mode new|current.
    runOpen: false,
    runsActionMsg: "",
    // Phase 1 raw merged output of the last daemon-spawned run (wb-daemon.js).
    rawFeed: "",
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
      return this.projects.find((p) => p.slug === this.openSlug)?.branch || "current";
    },
    // The faithful `ralphy run …` line the chosen options map to.
    runCommandPreview() {
      const c = this.runCfg;
      let s = `run --agent ${c.agent}`;
      if (c.split && c.planAgent !== c.agent) s += ` --plan-agent ${c.planAgent}`;
      s += ` --branch-mode ${c.branchMode}`;
      return s;
    },
    startRun() {
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
      WB.emit("command", { project: this.openSlug, verb });
      this._flashAction(`${verb} requested`);
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
        case "dev.ralphy.plan.step":
          // tick the next open checkbox (the panel just advances a step)
          run.planMd = run.planMd.replace(/-\s+\[ \]/, "- [x]");
          break;
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
    kanbanSel: null, // the selected issue number → opens the detail drawer
    kanbanFilter: "", // search box (title / #num / body / label)
    kanbanLabel: "__all", // label filter: __all | __none | <label>
    kanbanSort: "num-desc", // Backlog sort (Ready columns keep graph order)

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
      try {
        const reply = await window.WBDaemon.observe("board.list", { repo: slug });
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
      } catch {
        // Daemon mode: transport error → distinct error state + flash; drop
        // any stale board (see the isError branch above).
        this.boardIssues[slug] = [];
        if (window.WBMode.isDaemon()) {
          this.boardError[slug] = "could not load board";
          this._flashAction?.("could not load board");
        }
        // Demo (static shell): leave it empty, no throw.
      }
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
      try {
        const reply = await window.WBDaemon.observe("issue.show", { repo: slug, number });
        if (!reply || reply.status !== "ok" || !reply.issue || typeof reply.issue !== "object") return;
        const detail = reply.issue;
        const iss = (this.boardIssues[slug] || []).find((i) => i.number === number);
        if (!iss) return;
        if (typeof detail.body === "string") iss.body = detail.body;
        if (Array.isArray(detail.comments)) iss.comments = detail.comments;
        if (Array.isArray(detail.blocked_by)) iss.blockedBy = detail.blocked_by;
      } catch {
        // Leave the board row's empty body on any failure.
      }
    },
    closeIssue() {
      this.kanbanSel = null;
    },
    // The real GitHub URL of an issue on the OPEN project — the drawer's editing
    // door (read-only here; edits happen on GitHub). Rebuilt from the project's
    // real `remoteUrl` (#204): parse `owner/repo` from an `https://github.com/o/r`
    // or `git@github.com:o/r` origin (`.git` stripped). `null` when the open
    // project has no GitHub remote, so the markup can hide the link.
    githubUrl(number) {
      const p = this.projects.find((x) => x.slug === this.openSlug);
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
    hasLabel(iss, label) {
      return !!iss && (iss.labels || []).includes(label);
    },
    toggleLabel(iss, label) {
      if (!iss) return;
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
          }
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

    openSettings() {
      this.settingsOpen = true;
      this.avatarMenu = false;
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
    usage: { records: [], interactive: [], error: "" },
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
        } else if (window.WBMode.isDaemon()) {
          this.usage.records = [];
          this.usage.interactive = [];
          this.usage.error = "could not load usage from the daemon";
        }
      } catch {
        if (window.WBMode.isDaemon()) {
          this.usage.records = [];
          this.usage.interactive = [];
          this.usage.error = "could not load usage from the daemon";
        }
      }
      this.$nextTick(() => window.lucide?.createIcons());
    },
    closeUsage() {
      this.usageOpen = false;
    },
    // Sum a run record's token buckets into one total for the compact list.
    usageTokens(rec) {
      const t = (rec && rec.tokens) || {};
      return (t.input || 0) + (t.output || 0) + (t.cache_read || 0) + (t.cache_creation || 0);
    },

    async saveSetting(key, value) {
      this.settings[key] = value;
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
      totpEnrolled: false,
      // set only in the one moment after enrolling — the real daemon prints the
      // secret/QR a single time and never again.
      secret: "",
      otpauthUri: "",
      qrHtml: "",
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
    },

    async enrollTotp() {
      // POST /api/security/totp/enroll returns the REAL one-time provisioning
      // URI (mint-once); the QR is rendered from THAT uri, not a client secret.
      try {
        const r = await fetch("/api/security/totp/enroll", { method: "POST" });
        if (!r.ok) return;
        const { uri } = await r.json();
        this.security.totpEnrolled = true;
        this.security.otpauthUri = uri;
        this.security.secret = (uri.split("secret=")[1] || "").split("&")[0];
        this.security.qrHtml = window.wbQr(uri);
      } catch {}
    },

    async revokeTotp() {
      // POST /api/security/totp/revoke deletes the seed file (mint-once posture).
      try {
        await fetch("/api/security/totp/revoke", { method: "POST" });
      } catch {}
      this.security.totpEnrolled = false;
      this.security.secret = "";
      this.security.otpauthUri = "";
      this.security.qrHtml = "";
      // revoking the seed removes the session factor → login can't be required
      this.security.requireLogin = false;
    },

    async savePassword() {
      const pw = this.security.passwordDraft.trim();
      if (!pw) return;
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
    },
    async remintToken() {
      // POST /api/security/token/remint rotates the on-disk token. The live
      // AuthPolicy is captured at boot (ADR-0032 §4), so the rotation takes effect
      // on the next daemon restart — it does NOT invalidate the current cookie in
      // this process. Log off locally so the operator re-authenticates once it does.
      try {
        await fetch("/api/security/token/remint", { method: "POST" });
      } catch {}
      if (this.security.requireLogin) this.logOff();
    },

    async toggleRequireLogin(ev) {
      // Requiring login is only meaningful once TOTP is enrolled (the session
      // factor). Hit the server-side gate (POST /api/security/require-login), which
      // refuses (400) an enable with no seed — the authoritative AC4 check; the
      // client guard just avoids a doomed round-trip.
      const want = !this.security.requireLogin;
      if (want && !this.security.totpEnrolled) {
        this.security.requireLogin = false;
      } else {
        try {
          const r = await fetch("/api/security/require-login", {
            method: "POST",
            headers: { "Content-Type": "application/x-www-form-urlencoded" },
            body: "enable=" + want,
          });
          this.security.requireLogin = r.ok ? want : false;
        } catch {
          this.security.requireLogin = false;
        }
      }
      // The checkbox's :checked binding won't re-sync when the bound value
      // didn't actually change (blocked case), so force the DOM to match state.
      if (ev?.target) ev.target.checked = this.security.requireLogin;
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
      WB.emit("login", {});
      this.$nextTick(() => window.lucide?.createIcons());
    },

    // --- canvas tabs ------------------------------------------------------
    // The Agents tab is permanent; file tabs are appended and closable.
    agents: ["claude", "codex", "opencode"],
    agentMenu: false,
    consoleCount: 0,
    tabs: [{ id: "agents", kind: "agents", title: "Agents", icon: "bi bi-robot", closable: false }],
    active: "agents",

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
    toggle(slug) {
      this.openSlug = this.openSlug === slug ? null : slug;
      // a selected issue belongs to the project that was open — closing or
      // switching projects must drop the Kanban detail drawer (its selection is
      // now stale/absent), else the empty drawer lingers on the right.
      this.kanbanSel = null;
      this.$nextTick(() => {
        this.destroyTree();
        if (this.openSlug) this.mountTree();
        // Refresh the board fold for the newly-open project (issue #198) so the
        // Kanban + drawer read this project's live tracker, not a stale slug.
        if (this.openSlug) this.loadBoard();
        // point the Runs panel at this project's first run + its first section
        this.currentRunId = this.projectRuns()[0]?.runid || null;
        this.planSection = this.planHeadings(this.currentRun())[0] || "";
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

    // Wunderbaum marks folders via the source `folder:true` flag / a children
    // array; there is no isFolder() method on the node.
    isFolder(node) {
      return !!(node.folder || node.children);
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
      const project = this.projects.find((p) => p.slug === this.openSlug);
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

      // Right-click anywhere in the tree → our own context menu.
      host.addEventListener("contextmenu", (ev) => {
        const node = mar10.Wunderbaum.getNode(ev);
        if (!node) return;
        ev.preventDefault();
        node.setActive();
        this.showMenu(ev.clientX, ev.clientY, node);
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
      if (activeRel) {
        this._tree.findFirst((n) => this.relPath(n) === activeRel)?.setActive();
      }
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
        reads.push(
          WBDaemon.observe("file.read", { repo: t.project, path: t.path })
            .then((reply) => {
              if (reply?.status === "ok") WBViewer.externalChange(t.id, reply.content);
            })
            .catch(() => {}),
        );
      }
      // Return the settled batch so a caller (a test, a chained nudge) can await
      // a fully-refreshed set of viewers rather than racing the reads.
      return Promise.all(reads);
    },

    // The expanded folder node whose rel path is `rel`, or `null` if none is
    // mounted (so a nudge for an off-screen dir drops).
    findFolderByRel(rel) {
      return this._tree?.findFirst((n) => this.isFolder(n) && this.relPath(n) === rel) || null;
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
        WB.emit("open-refused", { project: this.openSlug, path, reason: "binary" });
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
      const icon = ftype === "markdown" ? "bi bi-file-earmark-text" : "bi bi-file-earmark-code";
      this.tabs.push({ id, kind: ftype, title, path, project, icon, closable: true });
      this.active = id;
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

    // Pop a file tab out into a standalone browser popup, so it can be read
    // side-by-side with an agent console in the main window. The descriptor is
    // handed over via a shared same-origin global (no serialisation limits); the
    // in-app tab then closes and we drop back to the Agents workspace.
    detachFile(desc) {
      const id = `file:${desc.project}:${desc.path}`;
      // file:// windows get opaque origins, so a shared global on window.opener
      // is unreadable (SecurityError). Hand the descriptor over in the URL hash
      // instead; the popup talks back over postMessage (see the listener below).
      const payload = encodeURIComponent(JSON.stringify(desc));
      const win = window.open("detached.html#" + payload, "_blank", "popup,width=920,height=760");
      if (!win) {
        WB.emit("detach-blocked", { project: desc.project, path: desc.path });
        return;
      }
      WB.emit("detach", { project: desc.project, path: desc.path });
      this.closeTab(id);
      this.activate("agents");
    },

    activate(id) {
      this.active = id;
      this.$nextTick(() => {
        WBViewer.setActive(this.active === "agents" ? null : this.active);
        window.lucide?.createIcons();
        // A console opened/reattached while another tab was active measured 0×0
        // (its tab was display:none); refit now that the Agents tab is visible.
        if (id === "agents") window.WBConsole?.refitAll?.();
      });
    },

    closeTab(id) {
      const idx = this.tabs.findIndex((t) => t.id === id);
      const tab = this.tabs[idx];
      if (!tab || !tab.closable) return; // Agents never closes
      WBViewer.close(id);
      this.tabs.splice(idx, 1);
      if (this.active === id) {
        // fall back to the neighbour, else the Agents tab
        const next = this.tabs[idx] || this.tabs[idx - 1] || this.tabs[0];
        this.activate(next.id);
      }
    },

    // --- consoles (the Agents tab) ----------------------------------------
    // The "New console" menu: an agent adapter per row, plus a plain console
    // (no agent — a shell in the repo dir) pinned LAST, mirroring the daemon UI.
    // Each has an Alt+Shift+<digit> accelerator: Alt+Shift lives outside the
    // browser's reserved combos on Windows/Linux/macOS, and the digits are
    // matched by physical key (e.code), so they fire regardless of layout or the
    // glyph macOS' Option produces. Console is Alt+Shift+0 (last, the "zero").
    consoleItems() {
      return [
        { kind: "claude", label: "claude", plain: false, digit: "1" },
        { kind: "codex", label: "codex", plain: false, digit: "2" },
        { kind: "opencode", label: "opencode", plain: false, digit: "3" },
        { kind: "console", label: "console", plain: true, digit: "0" },
      ];
    },
    isMac: /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || ""),
    shortcutLabel(digit) {
      return this.isMac ? `⌥⇧${digit}` : `Alt+Shift+${digit}`;
    },
    openConsoleItem(item) {
      if (item.plain) this.newPlainConsole();
      else this.newConsole(item.kind);
      this.agentMenu = false;
    },

    newConsole(agent) {
      if (this.active !== "agents") this.activate("agents");
      WBConsole.open({ repo: this.openSlug, agent });
      this.consoleCount = WBConsole.count();
    },
    // a bare shell in the repo dir (no agent) — the daemon's per-repo console
    newPlainConsole() {
      if (this.active !== "agents") this.activate("agents");
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
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable || el.closest(".CodeMirror"))
      );
    },

    arrangeConsoles() {
      WBConsole.arrange();
    },

    // --- context menu -----------------------------------------------------
    showMenu(x, y, node) {
      const menu = document.getElementById("ctxmenu");
      const isFolder = this.isFolder(node);
      const items = [
        !isFolder && { label: "Open", icon: "bi-box-arrow-up-right", run: () => this.openFile(node) },
        { label: "Rename…", icon: "bi-pencil", run: () => node.startEditTitle() },
        { label: "Copy relative path", icon: "bi-clipboard", run: () => this.copyPath(node) },
        isFolder && { label: "New file…", icon: "bi-file-earmark-plus", run: () => this.emit("create", node, { kind: "file" }) },
        isFolder && { label: "New folder…", icon: "bi-folder-plus", run: () => this.emit("create", node, { kind: "folder" }) },
        { sep: true },
        { label: "Delete", icon: "bi-trash", danger: true, run: () => this.emit("delete", node) },
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

    copyPath(node) {
      const path = this.relPath(node);
      navigator.clipboard?.writeText(path).catch(() => {});
      this.emit("copy-path", node, { path });
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

// A viewer asked to detach → open the popup and close the tab.
document.addEventListener("workbench:detach-request", (e) => {
  getShell()?.detachFile(e.detail);
});

// Messages from detached popups (postMessage, since file:// blocks shared-global
// access): re-emit their save/reload intents on our seam so the backend sees
// them in one place, and fold a re-attached file back into the shell.
window.addEventListener("message", (e) => {
  const m = e.data;
  if (!m || typeof m !== "object") return;
  if (m.type === "wb-emit") {
    WB.emit(m.action, m.detail || {});
  } else if (m.type === "wb-reattach" && m.desc) {
    getShell()?.openTab({
      project: m.desc.project,
      path: m.desc.path,
      title: m.desc.path.split("/").pop(),
      ftype: m.desc.ftype,
      content: m.desc.content,
    });
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

  document.addEventListener("workbench:action", (e) => {
    if (!daemonBacked()) return;
    const d = e.detail || {};
    const repo = d.project;
    if (!repo) return;
    switch (d.action) {
      case "save":
        call("file.write", { repo, path: d.path, content: d.content || "" });
        break;
      case "create": {
        // The tree emits `create` on the parent folder with no name; prompt for
        // it and compose the full rel path the daemon verb expects.
        const name = window.prompt(d.kind === "folder" ? "New folder name" : "New file name");
        if (!name) return;
        const path = d.path ? `${d.path}/${name}` : name;
        call("file.create", { repo, path, dir: d.kind === "folder" }, `created ${name}`);
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
      case "delete":
        call("file.delete", { repo, path: d.path }, "deleted");
        break;
      default:
        break;
    }
  });
})();

// Dismiss the context menu on any outside interaction.
document.addEventListener("click", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu").style.display = "none"));
document.addEventListener("scroll", () => document.getElementById("ctxmenu") && (document.getElementById("ctxmenu").style.display = "none"), true);

document.addEventListener("alpine:initialized", () => window.lucide?.createIcons());

// Alt+Shift+<digit> → open a console: 1 claude · 2 codex · 3 opencode · 0 plain
// console. Matched on the physical key (e.code) so layout / macOS Option glyphs
// don't matter; guarded so it never hijacks a text field, modal, or the login.
document.addEventListener("keydown", (e) => {
  if (!e.altKey || !e.shiftKey || e.ctrlKey || e.metaKey) return;
  const map = { Digit1: "claude", Digit2: "codex", Digit3: "opencode", Digit0: "__plain" };
  const kind = map[e.code];
  if (!kind) return;
  const c = getShell();
  if (!c || c.consoleShortcutsBlocked()) return;
  e.preventDefault();
  if (kind === "__plain") c.newPlainConsole();
  else c.newConsole(kind);
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

// Inbound run events (the backend seam): a live CloudEvents feed dispatches
// `ralphy:run-event` with a `{ type, runid, data }` detail; the shell folds it
// into the Runs panel. `window.WBRuns.emit(evt)` is the same door for console
// testing, e.g. WBRuns.emit({ type: "dev.ralphy.issue.closed", runid, data }).
document.addEventListener("ralphy:run-event", (e) => getShell()?.applyRunEvent(e.detail));
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
