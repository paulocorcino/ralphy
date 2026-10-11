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
import { classify, fileTabId, newEntryTitle, parentRel } from "./wb-file-paths.ts";
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import { WBReleaseDialogs } from "./wb-release-dialogs.ts";
import { WBSecurityDialog } from "./wb-security-dialog.ts";
import { WBSettingsDialog } from "./wb-settings-dialog.ts";
import { WBSplit } from "./wb-split.ts";
import { isFilePopupMessage } from "./wb-detached.ts";
import { createEmitter, forwardAction, sendWindow } from "./wb-events.ts";
import type { CanvasTab, ChangeEntry, ConfirmAsk, DiffTarget, Group, MenuItem, ModalEntry, Project, PromptAsk, SavePayload, ShellLate, Slot, TabBody, TabOpen, WireDeps } from "./wb-shell-types.d.ts";

// A phone in either orientation: its SHORT side is under the workbench's phone
// breakpoint (560px). Landscape iPhone is ~750 wide but ~340 tall; an iPad's
// short side is 744+ and keeps its desktop chrome. 01-base.css and
// 05-workspace.css gate on the same query.
const PHONE_QUERY = "(max-width: 560px), (pointer: coarse) and (max-height: 560px)";

// The `this` of `shell()`'s members: its own members, the Alpine magics, and
// the fields a method sets on first use (`ShellLate`). The type a
// component checks its `uses` against is the literal itself (`Shell`).
function shellData<T extends object>(data: T & ThisType<T & AlpineMagics & ShellLate>): T {
  return data;
}

// The popups `wire` opened, as `shell()` reaches them. `wire` sets this, so
// each call starts with no popup.
let detached = { watch: (_win: Window, _desc: FileDescriptor) => {}, dirty: () => false, close: () => {} };

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
    // `release` (wb-release-store.ts). The account menu and the login gate
    // are the components `wbAccount` and `wbLogin`.
    init() {
      // The daemon said whether this browser is authorized (the store
      // `connection`): the restored view is the shell's.
      window.addEventListener("workbench:auth-read", (e) => {
        // Gated on `authed`: a pre-login restore would have every tab refused
        // and closed, persisting the loss (#339). The `workbench:logged-in`
        // listener below is the other end.
        if (e.detail.authed) this.restoreView();
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
        this.restoreView();
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
      // A canvas tab that a component asks for (the Spend tab): added once,
      // then brought to the front.
      window.addEventListener("workbench:tab-open", (e) => {
        const tab = e.detail.tab;
        if (!this.tabs.some((t) => t.id === tab.id)) this.tabs.push(tab);
        this.activate(tab.id);
      });
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
          this._flashAction(`Could not start a new desk: ${why}.`);
        });
    },

    _flashAction(msg: string) {
      this.runsActionMsg = msg;
      clearTimeout(this._actionTimer);
      this._actionTimer = setTimeout(() => (this.runsActionMsg = ""), 2600);
    },
    // The message door (`wb-messages.ts`) and the nested components flash here.
    flash(msg: string) { this._flashAction(msg); },

    // --- modal stack ------------------------------------------------------
    // The open modals, oldest first: `{ path, opener }`. Only the last one
    // answers Escape, and each returns focus to its opener on close.
    _modalStack: [] as ModalEntry[],
    // The confirm dialog (replaces window.confirm); `askConfirm` opens it.
    confirmModal: {
      open: false,
      title: "",
      message: "",
      confirmLabel: "Confirm",
      cancelLabel: "Cancel",
      danger: false,
    },
    _confirmResolve: null as ((ok: boolean) => void) | null,
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
    _promptResolve: null as ((name: string | null) => void) | null,
    // The Escape keydown a modal has already answered.
    _escapeEvent: null as Event | null,
    // The one binding every `.modal-scrim` in index.html uses:
    // `x-bind="scrim('runOpen', () => closeRunModal())"`. `path` names the open
    // flag, dotted for a nested one (`confirmModal.open`). Alpine evaluates the
    // object once per scrim, so `was` lives as long as the element.
    // A click on the scrim closes nothing: a stray click must not throw away
    // what a modal holds. Only its own buttons and Escape close it.
    scrim(path: string, close: () => void) {
      const isOpen = () => path.split(".").reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], this);
      let was = false;
      const self = this;
      return {
        "x-show": () => isOpen(),
        // Every open scrim hears the same window keydown; only the top one acts,
        // so a confirm raised over another modal closes alone. The event is
        // marked because the browser runs Alpine's effects between two
        // listeners: the close pops the stack before the next scrim is asked,
        // and the modal under it would read as the top.
        "@keydown.escape.window": (e: Event) => {
          if (self._escapeEvent === e) return;
          if (isOpen() && self.isTopModal(path)) {
            self._escapeEvent = e;
            close();
          }
        },
        // Watches the flag, not the close methods: `logOff()` clears flags
        // directly, and that close must still pop the stack.
        "x-effect"(this: { $el: Element }) {
          const open = !!isOpen();
          if (open === was) return;
          was = open;
          if (open) self.modalOpened(path, this.$el);
          else self.modalClosed(path);
        },
      };
    },
    isTopModal(path: string) {
      return this._modalStack.at(-1)?.path === path;
    },
    // Whether the modal with this open-flag path is open, at any depth. Code
    // outside a dialog's component asks this, never the flag (ADR-0073 D5).
    modalOpen(path: string) {
      return this._modalStack.some((m) => m.path === path);
    },
    modalOpened(path: string, scrimEl: Element) {
      this._modalStack.push({ path, opener: document.activeElement as HTMLElement | null });
      // One frame later: `x-show` has flipped by then, and a modal that focuses
      // its own field on open (Branch, Prompt) has already done so.
      window.requestAnimationFrame(() => {
        const dialog = scrimEl.querySelector('[role="dialog"], [role="alertdialog"]') || scrimEl;
        if (dialog.contains(document.activeElement)) return;
        const controls = Array.from(
          dialog.querySelectorAll<HTMLElement & { disabled?: boolean }>(
            'button, input, select, textarea, [href], [tabindex]:not([tabindex="-1"])',
          ),
        ).filter((el) => !el.disabled && el.getClientRects().length > 0);
        // The header ✕ is the last resort: it leads every modal, and the
        // operator came for the content.
        (controls.find((el) => !el.classList.contains("modal-x")) || controls[0])?.focus();
      });
    },
    modalClosed(path: string) {
      const i = this._modalStack.findLastIndex((m) => m.path === path);
      if (i < 0) return;
      const [{ opener }] = this._modalStack.splice(i, 1);
      if (opener?.isConnected) opener.focus();
    },
    // Accelerators are ignored while typing or while a modal is up.
    // `allowTerminal`: a key that must also work from inside a terminal (the
    // column walk); xterm's input is a TEXTAREA.
    consoleShortcutsBlocked(allowTerminal = false) {
      if (!this.$store.connection.authed) return true;
      if (this.modalOpen(WBSettingsDialog.openFlag) || this.modalOpen(WBSecurityDialog.openFlag) || this.modalOpen("runOpen") || this.modalOpen("branchOpen")) return true;
      if (this.modalOpen(WBReleaseDialogs.whatsNewFlag)) return true;
      const el = document.activeElement as HTMLElement | null;
      if (allowTerminal && el?.closest?.(".xterm")) return false;
      return !!(
        el &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable || el.closest(".monaco-editor"))
      );
    },
    // Resolve `true`/`false` on the operator's choice. A pending dialog is
    // settled `false` first so a second call never strands its promise.
    askConfirm(opts: ConfirmAsk = {}) {
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
    confirmRespond(ok: boolean) {
      this.confirmModal.open = false;
      const resolve = this._confirmResolve;
      this._confirmResolve = null;
      if (resolve) resolve(ok);
    },
    // Resolve the typed string, or `null`. Mirrors askConfirm.
    askPrompt(opts: PromptAsk = {}) {
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
    promptRespond(name: string | null) {
      this.promptModal.open = false;
      const resolve = this._promptResolve;
      this._promptResolve = null;
      if (resolve) resolve(name);
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
        this.$store.projects.loadRepos();
      } catch {
        this._flashAction("remove unavailable: no daemon");
      }
    },
    // The status dot: live → green, idle → grey, offline → red (unreachable
    // path), waiting → yellow (an agent is asking for you, ADR-0059).
    // Orthogonal to `remote`.
    // The project dot's tooltip, in words; the class keeps the state code.
    dotTitle(state: string) {
      return (
        ({
          live: "A console is open",
          waiting: "An agent is waiting for you",
          offline: "The folder cannot be reached",
        } as Record<string, string>)[state] || "No console is open"
      );
    },
    dotClass(state: string) {
      return state === "live"
        ? "live"
        : state === "waiting"
          ? "waiting"
          : state === "offline"
            ? "offline"
            : "";
    },
    peerStateWord(state: string | null | undefined) {
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
      // The counts are the Changes view's (wb-changes-panel.ts), asked here.
      const changes = { counts: {} as Record<string, number | null> };
      sendWindow(window, "workbench:changes-count", changes);
      return WBChanges.projectBadge(changes.counts, slug);
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

    // The Runs panel's flash line; `_flashAction` writes it.
    runsActionMsg: "",

    // --- canvas tabs ------------------------------------------------------
    // The SAME terminal glyph as the New-console button and its menu rows.
    tabs: [{ id: "consoles", kind: "consoles", title: "Consoles", icon: "bi bi-terminal", closable: false }] as CanvasTab[],
    active: "consoles",
    // The secondary pane (ADR-0037 §3c): `{ kind: "pin", id }` shows that tab
    // beside whichever is active, `{ kind: "mirror" }` shows the active code
    // tab twice. One slot, ever; `WBSplit.resolve` folds it against `active`
    // in `syncViewer`. `splitRatio` is the left column's share (null = half);
    // `lastLeft` the tab last read on the left, so activating the pinned tab
    // itself keeps its neighbour rather than emptying the canvas.
    slot: null as Slot,
    splitRatio: null as number | null,
    lastLeft: null as string | null,

    // Open a `.note` as a card (ADR-0064 §11). The module decides whether this
    // is a jump to a card already on the plane or a new one; this layer only
    // puts the operator on the tab that holds the stage.
    openNote(path: string, project?: string | null, checkout?: string | null) {
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
    fetchContent(project: string, path: string, ftype: string, checkout: string | null | undefined): Promise<TabBody> {
      const refuse = (reason: string) => {
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
        let refusal: TabBody = null;
        return WBDaemon.readImage(project, path, (reason) => (refusal = refuse(reason)), checkout)
          .then((url) => (url == null ? refusal : { content: url }))
          .catch(() => refuse("transport"));
      }
      return WBDaemon.observe("file.read", WBDaemon.withCheckout({ repo: project, path }, checkout))
        .then((reply) => {
          if (reply.status === "ok") {
            return { content: reply.content, encoding: reply.encoding, bom: reply.bom };
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
    openLink({ project, path, fragment, checkout, as }: OpenRequest) {
      const title = path.split("/").pop()!;
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
      this.openTab({ project: project!, path, title, ftype, fragment, checkout });
    },

    // `content`: a re-attached popup passes its (possibly edited) bytes back.
    // `fragment`/`find`: a `#heading` or search term to land on once the bytes
    // are shown. `checkout` PINS the tab to the tree it was opened in (#406):
    // a Save from a tab showing worktree bytes must never land on the primary.
    // `encoding`/`bom` come back with a re-attached popup's bytes, so the
    // reattached pane saves the way the detached one would have.
    openTab({ project, path, title, ftype, content, fragment, find, checkout, encoding, bom }: TabOpen) {
      const ck = checkout !== undefined ? checkout : this.$store.checkouts.checkoutOf(project);
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
        const bytes: Promise<TabBody> =
          content != null
            ? Promise.resolve({ content, encoding, bom })
            : this.fetchContent(project, path, ftype, ck);
        bytes.then((body) => {
          if (body == null) return; // nothing to show: fetchContent closed the tab
          WBViewer.open({
            id,
            project,
            label: this.$store.projects.projectLabel(project),
            path,
            ftype,
            checkout: ck,
            ...body,
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
    repathTabs(from: string, to: string) {
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
    openDiff(project: string, entry: ChangeEntry) {
      // Both diff sides read text, and a binary side closes the tab again: an
      // image or other binary path never reaches the diff.
      const ftype = classify(entry.path.split("/").pop()!);
      if (ftype === "image" || ftype === "binary") {
        this.openChangedBinary(project, entry, ftype);
        return;
      }
      // Pinned to the selection at open (#407): both sides read `t.checkout`.
      const t = WBChanges.diffTarget(entry, project, this.$store.checkouts.checkoutOf(project));
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
        const refuse = (reason: string) => {
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
    openChangedBinary(project: string, entry: ChangeEntry, ftype: string) {
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
      const title = path.split("/").pop()!;
      this.openTab({ project, path, title, ftype, checkout: this.$store.checkouts.checkoutOf(project) });
    },

    // The diff's HEAD side; an added/untracked path diffs against emptiness.
    diffHeadSide(project: string, t: DiffTarget, refuse: (reason: string) => null) {
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
    diffWorkSide(project: string, t: DiffTarget, refuse: (reason: string) => null) {
      if (t.workingAbsent) return Promise.resolve("");
      // NOT `fetchContent`: it collapses every refusal to `null` and closes
      // the `file:` tab id rather than this diff's.
      return WBDaemon.observe(
        "file.read",
        WBDaemon.withCheckout({ repo: project, path: t.workingPath }, t.checkout),
      ).then((reply) => {
        if (reply.status === "ok") return reply.content;
        const reason = WBFail.message(reply, "refused");
        return reason === "not found" ? "" : refuse(reason);
      });
    },

    // Pop a file tab out into a standalone popup; the in-app tab closes.
    detachFile(desc: FileDescriptor) {
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

    activate(id: string) {
      this.active = id;
      // The Spend tab's subject can change while it sits in the background.
      if (id === "spend") sendWindow(window, "workbench:spend-shown");
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
    pinTab(id: string) {
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

    closeTab(id: string) {
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
        files.some((f) => fileTabId(f.project!, f.path!, f.checkout) === this.active);
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
    showTabMenu(x: number, y: number, t: CanvasTab) {
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
        { sep: true as const },
        { label: "Close", icon: "bi-x-lg", run: () => this.closeTab(t.id) },
      ].filter(Boolean);
      this.renderMenu(x, y, items);
    },

    // Paint `items` into the one `#ctxmenu` and keep it on-screen. An item is
    // `{ label, icon, run }` with optional `sep`, `danger`, `disabled`, `title`.
    renderMenu(x: number, y: number, items: MenuItem[]) {
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
export function wire(window: Window, document: Document, deps: WireDeps) {
  const { messages } = deps;
  // The one exit point: every gesture becomes a `workbench:action` event.
  // Each page sets its own `window.WB`, and its modules read it (ADR-0075 D9).
  window.WB = createEmitter(document);
  window.shell = shell;

  // The live Alpine component instance. On `window` explicitly: two other
  // modules call it, and a bare declaration reaches them only by accident of
  // global scope.
  window.getShell = function getShell() {
    const root = document.querySelector<HTMLElement & { _x_dataStack?: Shell[] }>("[x-data]");
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
  let wbCanvasWide: boolean | null = null;
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
  const detachedWindows = new Map<Window, FileDescriptor>();

  // A popup that closes sends its bytes home on unload (`wb-reattach`). One that
  // dies without an unload event (a crashed or killed renderer) is found by this
  // poll and comes home with the descriptor it was detached with. The poll acts
  // on the SECOND tick that sees it closed: the unload message carries the
  // edited bytes and must win over the detach-time copy.
  const detachedClosedSeen = new Set<Window>();
  let detachedPoll: number | null = null;

  function watchDetached(win: Window, desc: FileDescriptor) {
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
  function reattachFile(win: Window, desc: FileDescriptor) {
    // The pin comes home with the bytes (#406): explicit `null` is the primary.
    window.getShell()?.openTab({
      project: desc.project,
      path: desc.path,
      title: desc.path.split("/").pop()!,
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
  window.addEventListener("message", (e) => {
    if (e.origin !== window.location.origin) return;
    if (!detachedWindows.has(e.source as Window)) return;
    const m: unknown = e.data;
    if (!isFilePopupMessage(m)) return;
    if (m.type === "wb-detach-ready") {
      // The popup booted and is asking for its file.
      (e.source as Window).postMessage({ type: "wb-detach-open", desc: detachedWindows.get(e.source as Window) }, window.location.origin);
    } else if (m.type === "wb-emit") {
      // `fromWindow` lets a save's answer reach the pane that sent it.
      forwardAction(window.WB, m.action, m.detail, e.source as Window);
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
      reattachFile(e.source as Window, m.desc);
    }
  });

  // --- Write byte-ops (#197): the workspace-mutating seam actions go to the
  // daemon's confined `file.*` verbs; a refusal is flashed. The browser composes
  // the full rel path.
  (function wireWriteVerbs() {
    const daemonBacked = () => !!window.WBDaemon?.write;
    const flash = (msg: string) => messages.flash(msg);
    const call = (verb: string, payload: CommandPayload, okMsg?: string) => {
      WBDaemon.write(verb, payload)
        .then((reply) => {
          if (WBFail.isError(reply)) flash(WBFail.failed(reply, "Could not rename: the daemon gave no reason."));
          else if (okMsg) flash(okMsg);
        })
        .catch(() => flash("Could not rename: the daemon did not answer."));
    };

    document.addEventListener("workbench:action", async (e) => {
      if (!daemonBacked()) return;
      const d = e.detail;
      if (!d || !("project" in d)) return;
      const repo = d.project;
      if (!repo) return;
      // Every Write carries the checkout it is aimed at (#406): a Save says its
      // tab's PIN (explicit `null` = the primary, never the selection), a tree
      // gesture says the current selection.
      const checkout =
        "checkout" in d && d.checkout !== undefined ? d.checkout : window.Alpine.store("checkouts").checkoutOf(repo);
      const aimed = (payload: CommandPayload) => WBDaemon.withCheckout(payload, checkout);
      switch (d.action) {
        case "save": {
          // The pane's encoding rides the write (ADR-0036 amendment 2026-09-22)
          // and the pane hears the answer: its dirty mark waits for the ack, and
          // a refusal is read where the bytes are, not in a flash elsewhere.
          // A detached window's pane is `detached` in its own viewer; a tab's is
          // its tab id in this one.
          const viewer = () => (d.fromWindow ? d.fromWindow.WBViewer : window.WBViewer);
          const id = d.fromWindow ? "detached" : fileTabId(repo, d.path, checkout);
          const payload: SavePayload = { repo, path: d.path, content: d.content || "" };
          if (d.encoding) payload.encoding = d.encoding;
          if (d.bom) payload.bom = true;
          const send = (p: SavePayload): Promise<void> =>
            WBDaemon.write("file.write", aimed(p))
              .then((reply) => {
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
          const offerUtf8 = (p: SavePayload, reply: WriteReply) => {
            const at = Number(reply?.char_index ?? 0) + 1;
            const ask = messages.askInShell({
              title: `Could not save as ${p.encoding}`,
              message: `Character ${at} is not representable in ${p.encoding}. Save the file as UTF-8 instead?`,
              confirmLabel: "Save as UTF-8",
            });
            // No shell, no dialog: the pane already says "Could not save" and why.
            if (!ask) return;
            return ask.then((ok) => {
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
          window.Alpine.store("checkouts").ensureWorktreeListing(repo, true);
          if (d.message) flash(d.message.split("\n").map(WBFail.sentence).filter(Boolean).join(" "));
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
          const name = d.title || d.path.split("/").pop() || d.path;
          const message = d.isFolder
            ? `Delete folder “${name}” and its contents? This cannot be undone.`
            : `Delete “${name}”? This cannot be undone.`;
          const ask = messages.askInShell({ title: "Delete", message, confirmLabel: "Delete", danger: true });
          const ok = ask ? await ask : window.confirm(message);
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
    if (!c || !window.Alpine.store("connection").authed || !window.Alpine.store("projects").openSlug) return;
    if (c.modalOpen(WBSettingsDialog.openFlag) || c.modalOpen(WBSecurityDialog.openFlag) || c.modalOpen("runOpen") || c.modalOpen("branchOpen") || c.modalOpen(WBReleaseDialogs.whatsNewFlag)) return;
    e.preventDefault();
    sendWindow(window, "workbench:file-search-open");
  });

  window.WBRuns = {
    // A raw output chunk: the Runs panel (wb-runs-panel.ts) appends it.
    output(text) {
      sendWindow(window, "workbench:run-output", { text });
    },
  };

  // The detached file windows: the Alpine store `connection` asks them about
  // unsaved work before a build reload (wb-connection-store.ts).
  return detached;
}

const pascal = (name: string) =>
  name.replace(/(\w)(\w*)(_|-|\s*)/g, (_: string, first: string, rest: string) => first.toUpperCase() + rest.toLowerCase());

// `x-icon="'name'"` draws a lucide icon INTO its own `<svg>`. It never swaps the
// element (lucide's `createIcons` replaces it), so it is still the node Alpine
// bound: the icon renders wherever Alpine initializes an element — page load, a
// new `x-if` branch, a new `x-for` row — and a changed name redraws it. The
// output matches `createIcons` (lucide 0.460.0): the icon's default attributes
// where the markup set none, `data-lucide` (the stylesheets select on it), and
// the `lucide lucide-<name>` classes.
export function iconDirective(el: HTMLElement, { expression }: AlpineDirective, { evaluateLater, effect }: AlpineDirectiveUtilities) {
  const read = evaluateLater(expression);
  const authored = new Set(el.getAttributeNames());
  let drawn: string | null = null;
  effect(() =>
    read((name) => {
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
      el.replaceChildren(...children.map((child) => window.lucide.createElement(child)));
    }),
  );
}

/** The `shell()` component: a component's `uses` names come from it (ADR-0075 D6). */
export type Shell = ReturnType<typeof shell>;
