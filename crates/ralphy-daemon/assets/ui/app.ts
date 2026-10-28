/* ---------------------------------------------------------------------------
   ralphy workbench shell — shell behaviour

   The sidebar is a project accordion (Alpine); the file tree is a Wunderbaum
   instance. The canvas is a tabbed workspace: "Consoles" is fixed and hosts
   the floating console windows (wb-console.ts); every opened file is its own
   closable tab rendered by a viewer (wb-file-viewer.ts).

   Every user gesture becomes one CustomEvent, `workbench:action`, on
   `document`. That event IS the seam: a backend subscribes and does the work.
--------------------------------------------------------------------------- */
import type { AlpineMagics } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { WBChanges } from "./wb-changes.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import { sendWindow } from "./wb-events.ts";
import { phoneLayout } from "./wb-viewport.ts";
import { agentStateTitle } from "./wb-console-session.ts";
import type { Group, Project, ShellLate } from "./wb-shell-types.d.ts";

// The `this` of `shell()`'s members: its own members, the Alpine magics, and
// the fields a method sets on first use (`ShellLate`). The type a
// component checks its `uses` against is the literal itself (`Shell`).
function shellData<T extends object>(data: T & ThisType<T & AlpineMagics & ShellLate>): T {
  return data;
}

export function shell() {
  return shellData({
    // Why the daemon cannot read the saved desk, or "" (ADR-0070 D4); a copy
    // of `WBConsole.deskFailure()` so the page can show it.
    deskFailure: "",

    // Alpine lifecycle. The session, the presence socket and the tab's
    // visibility start in the Alpine store `connection`
    // (wb-connection-store.ts), which `main.ts` registers first; the
    // projects, the fleet, the agents and the sessions in the store
    // `projects` (wb-projects-store.ts); the release view in the store
    // `release` (wb-release-store.ts); the modal stack, the confirm, the
    // prompt and the flash line in the store `modals` (wb-modals-store.ts);
    // the canvas tabs, the slot and the split in the store `tabs`
    // (wb-tabs-store.ts).
    // The account menu and the login gate are the components `wbAccount` and
    // `wbLogin`.
    init() {
      // The daemon said whether this browser is authorized (the store
      // `connection`): the shell restores the view (the store `tabs`).
      window.addEventListener("workbench:auth-read", (e) => {
        // Gated on `authed`: a pre-login restore would have every tab refused
        // and closed, persisting the loss (#339). The `workbench:logged-in`
        // listener below is the other end.
        if (e.detail.authed) this.$store.tabs.restoreView();
      });
      // The daemon accepted a login (wb-login.ts).
      window.addEventListener("workbench:logged-in", () => {
        // `/api/desk` is gated too: unread AND unwritable until re-read (#327);
        // the selected checkouts ride it (#406).
        window.WBConsole?.afterLogin()?.then(() => {
          this.$store.checkouts.adoptDeskCheckouts();
          this.syncDeskFailure();
        });
        // Only now is `file.read` allowed (#339).
        this.$store.tabs.restoreView();
      });
      // A push, a reopened socket or the tab coming back (the store
      // `connection`) asks to read these facts again.
      window.addEventListener("workbench:desk-reread", () => this.rereadDesk());
      // The runs socket said the snapshots changed (the Alpine store `runs`).
      // A snapshot change means the tracker may have moved, so the same push
      // nudges the board (#301); the predicate coalesces it.
      window.addEventListener("workbench:runs-dirty", () => {
        if (this.$store.connection.tabHidden()) return;
        this.$store.runs.hydrateRuns();
        sendWindow(window, "workbench:board-refresh", { trigger: "runs" });
      });
      // The Alpine store `checkouts` hears `unknown checkout` and copies the
      // desk's checkouts once the boot desk lands (wb-checkouts-store.ts).
      window.WBConsole?.whenDeskLoaded?.().then(() => {
        this.syncDeskFailure();
      });
      // A flush can be the read that finds the desk unreadable; no push says so.
      window.WBConsole?.setDeskFailureHook?.(() => this.syncDeskFailure());
      // The phase clock's tick: one assignment a second while the panel is
      // open; the run document is NOT re-read. The presence half of the clock
      // is the store `connection`'s.
      this._clockTick = setInterval(() => {
        if (this.runsOpen) this.nowMs = Date.now();
      }, 1000);
    },

    // Read the desk again. The console module puts it on the stage, and tells
    // the columns (wb-consoles-tab.ts) which consoles left it; the selected
    // checkouts are copied, since another client may have picked a tree.
    rereadDesk() {
      const read = window.WBConsole?.reloadDesk?.();
      if (!read?.then) return;
      read.then(() => {
        this.syncDeskFailure();
        this.$store.checkouts.adoptDeskCheckouts();
      });
    },
    syncDeskFailure() {
      this.deskFailure = window.WBConsole?.deskFailure?.() || "";
    },
    // The desk failure's one action (ADR-0070 D4).
    startNewDesk() {
      window.WBConsole?.startNewDesk?.()
        .then(() => this.syncDeskFailure())
        .catch((e) => {
          const why = String(e?.message || "").startsWith("the daemon") ? e.message : "the daemon did not answer";
          this.$store.modals.flash(`Could not start a new desk: ${why}.`);
        });
    },

    // --- the open project -------------------------------------------------
    // The fact itself is the Alpine store `projects` (wb-projects-store.ts).
    toggle(ref: string, row?: Project) {
      const previous = this.$store.projects.openSlug;
      // A row on a host that cannot answer stays closed. The click still wakes
      // a sleeping host: that is the act the operator asked for.
      if (previous !== ref && !this.$store.projects.refAvailable(ref)) {
        this.$store.projects.wakePeerFor(ref);
        return;
      }
      this.$store.projects.setOpen(previous === ref ? null : ref);
      const slug = this.$store.projects.openSlug;
      // NOT awaited: the accordion must not sit behind a cold WSL boot.
      if (slug === ref) this.$store.projects.wakePeerFor(ref);
      this.$store.projects.loadAgents(slug);
      // The chip's `<branch> · <name>` needs the listing (#406).
      if (slug === ref) this.$store.checkouts.ensureWorktreeListing(ref);
      sendWindow(window, "workbench:spend-refresh");
      this.projectChanged(previous);
    },
    // The files, the board and the git part each drop what was scoped to the
    // project that WAS open (ADR-0073 amendment of 2026-10-08, decision 4).
    projectChanged(previous: string | null) {
      const slug = this.$store.projects.openSlug;
      sendWindow(window, "workbench:project-changed", { slug, previous });
    },
    // A sleeping peer's wake button. Its two sentences keep their order here,
    // not in a `+` chain inside the markup (ADR-0065 §9).
    wakeTitle(g: Group) {
      if (this.$store.projects.waking[g.daemon]) return `Waking ${g.environment}…`;
      return `Wake ${g.environment}. ${g.diagnosis}`;
    },
    // The update button of an SSH host that speaks another protocol.
    updateTitle(g: Group) {
      const host = WBFleet.peerName(g);
      if (this.$store.projects.updating[g.daemon]) return `Updating Ralphy on ${host}…`;
      return `${g.diagnosis} Click to install this computer's Ralphy on ${host}.`;
    },
    // A row on a host that cannot answer: why it does not open.
    unavailableTitle(g: Group) {
      const host = WBFleet.peerName(g);
      const head = `${host} is not available (${this.peerStateWord(g.state)}).`;
      if (WBFleet.wakeable(g)) return `${head} Click to wake it.`;
      return `${head} Its projects open when it connects again.`;
    },
    rowTitle(p: Project) {
      return WBProject.rowTitle(p);
    },
    // Drop a project from the daemon's registry (#363); the disk is NOT
    // touched. The confirm is awaited BEFORE any `WBDaemon` call: cancel must
    // open no socket.
    async removeProject(p: Project) {
      const ref = this.$store.projects.repoRef(p);
      const ok = await this.$store.modals.askConfirm({
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
          this.$store.modals.flash(WBFail.failed(reply, "Could not remove the project: the daemon gave no reason."));
          return;
        }
        // Identity is `repoRef`, not the slug: a peer can list the same slug.
        this.$store.projects.setProjects(this.$store.projects.projects.filter((x) => this.$store.projects.repoRef(x) !== ref));
        if (this.$store.projects.openSlug === ref) {
          this.$store.projects.setOpen(null);
          this.projectChanged(ref);
        }
        this.$store.projects.loadRepos();
      } catch {
        this.$store.modals.flash("remove unavailable: no daemon");
      }
    },
    // The status dot: live → green, idle → grey, offline → red (unreachable
    // path), waiting → yellow (an agent is asking for you, ADR-0059).
    // Orthogonal to `remote`.
    // The project dot's tooltip, in words; the class keeps the state code.
    dotTitle(state: string, detail?: string) {
      if (state === "silent") return agentStateTitle(state, detail);
      return (
        ({
          live: "A console is open",
          waiting: "An agent is waiting for you",
          offline: "The folder cannot be reached",
        } as Record<string, string>)[state] || "No console is open"
      );
    },
    dotClass(state: string) {
      return ["live", "waiting", "silent", "offline"].includes(state) ? state : "";
    },
    peerStateWord(state: string | null | undefined) {
      return WBFleet.stateWord(state);
    },

    // --- chrome panels ----------------------------------------------------
    // Sidebar, Runs panel and Kanban board: each a layout flip on a body class.
    // A phone opens with the sidebar closed: there it floats over the canvas.
    sideOpen: !phoneLayout(window),
    // `projects` or `changes` (#317). Changes is a VIEW scoped to `openSlug`.
    sideView: "projects",
    runsOpen: false,
    kanbanOpen: false,
    projectQuery: "",

    phone() {
      return phoneLayout(window);
    },

    // Clicking the rail button of the view already showing collapses the sidebar.
    showSideView(view: string) {
      if (this.sideOpen && this.sideView === view) {
        this.sideOpen = false;
        return;
      }
      this.sideView = view;
      this.sideOpen = true;
      // Opening Changes IS a read trigger: the rows were last read when the
      // project was opened.
      sendWindow(window, "workbench:changes-refresh");
    },

    // The change indicator for one row. Only slugs whose count was READ render
    // one: a `changes.list` per repo would be N git subprocesses on open.
    projectBadge(slug: string) {
      return WBChanges.projectBadge(this.$store.checkouts.changesCount, slug);
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
    repoLabel(p: Project) {
      return WBProject.repoLabel(p);
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
      // Closing drops the board-arrival marker (wb-runs-panel.ts): a
      // same-numbered issue in another run would inherit it.
      if (!this.runsOpen) sendWindow(window, "workbench:runs-closed");
      if (this.runsOpen) {
        // `nowMs` is as stale as the panel has been closed; re-anchor before
        // the first paint.
        this.nowMs = Date.now();
        this.$store.runs.hydrateRuns();
      }
    },
    toggleKanban() {
      // The tasks board (wb-kanban.ts): an overlay flip over the canvas.
      this.kanbanOpen = !this.kanbanOpen;
      if (this.kanbanOpen) {
        // Lazy-load the tracker for the open project when the board opens
        // (wb-board.ts).
        sendWindow(window, "workbench:board-opened");
      }
      window.WB.emit("kanban-toggle", { open: this.kanbanOpen });
    },

    // --- branch switcher --------------------------------------------------
    // The dialog is the component `wbBranchDialog` (wb-branch-dialog.ts); the
    // selected checkouts are the Alpine store `checkouts` (wb-checkouts-store.ts).
    // The row's branch chip. Collapsed it is only the change count, and the
    // click falls through to `.project-head`'s toggle; open it is the switcher,
    // and must not ALSO collapse the row it sits on. Switching is gated on
    // reachability, not on remote (`canSwitchBranch`): an unreachable chip
    // stays inert — informational — but still swallows the click.
    branchChipClick(p: Project, ev: Event) {
      if (!this.$store.projects.rowOpen(p)) return;
      ev.stopPropagation();
      sendWindow(window, "workbench:branch-open", { project: p });
    },
    // The checkout chip of the Files bar and of the Changes view: its menu is
    // the branch dialog's.
    checkoutChipClick(p: Project, anchor: HTMLElement) {
      sendWindow(window, "workbench:checkout-menu", { project: p, anchor });
    },

    // --- Runs panel -------------------------------------------------------
    // The runs, their read state and the selected run are the Alpine store
    // `runs` (wb-runs-store.ts). The panel, the plan section and the run
    // modal are the component `wbRuns` (wb-runs-panel.ts).
    // The clock's "now", advanced by the tick in `init` while the panel is
    // open. STATE, not `Date.now()` in the getter: Alpine only re-renders what
    // it can observe changing.
    nowMs: Date.now(),
  });
}

/** The `shell()` component: a component's `uses` names come from it (ADR-0075 D6). */
export type Shell = ReturnType<typeof shell>;
