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
import type { Group, Project, SavePayload, ShellLate, WireDeps } from "./wb-shell-types.d.ts";

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
  document.addEventListener("workbench:detach-request", (e) => window.Alpine.store("tabs").detachFile(e.detail));

  // A rendered markdown link asked for a repo file → open (or focus) its tab.
  document.addEventListener("workbench:open-request", (e) => window.Alpine.store("tabs").openLink(e.detail));

  // The divider between the two panes was dragged: the ratio is view state.
  document.addEventListener("workbench:split-ratio", (e) => {
    const sh = window.Alpine.store("tabs");
    sh.splitRatioChanged(e.detail.ratio);
    // Re-told, not only stored: the viewer repaints `--wb-split` from the ratio
    // it was last given, and a repaint can come before the next activation.
    sh.syncViewer();
    sh.persistView();
  });

  // The canvas resized. Only a crossing of the split's width floor changes what
  // is painted, so the fold reruns on the crossing alone — not per pixel.
  let wbCanvasWide: boolean | null = null;
  document.addEventListener("workbench:canvas-resize", (e) => {
    const sh = window.Alpine.store("tabs");
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

  const detached = { watch: watchDetached, dirty: detachedDirty, close: closeDetached };

  // The one way a detached file comes home: the button, the popup's unload and
  // the poll all end here. Closing the popup matters after an F5 inside it: the
  // unload sent the file home, and the reloaded page has nothing left to show.
  function reattachFile(win: Window, desc: FileDescriptor) {
    // The pin comes home with the bytes (#406): explicit `null` is the primary.
    window.Alpine.store("tabs").openTab({
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
      window.Alpine.store("tabs").openLink({
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
            ? await window.Alpine.store("modals").askPrompt({
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
          if (!folder && c) window.Alpine.store("tabs").openTab({ project: repo, path, title: name, ftype: classify(name) });
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
    if (!c || window.Alpine.store("modals").consoleShortcutsBlocked()) return;
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
    const modals = window.Alpine.store("modals");
    if (modals.modalOpen(WBSettingsDialog.openFlag) || modals.modalOpen(WBSecurityDialog.openFlag) || modals.modalOpen("runOpen") || modals.modalOpen("branchOpen") || modals.modalOpen(WBReleaseDialogs.whatsNewFlag)) return;
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
