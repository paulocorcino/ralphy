/* ---------------------------------------------------------------------------
   The Add a project dialog: a folder on this computer or on a peer becomes a
   project. The daemon that owns the folder lists it (`dir.list`) and runs
   `ralphy daemon add` (`project.add`), and `wb-add-project.ts` is the fold that
   holds the dialog's state.

   `addProjectDialog` is the Alpine component `wbAddProjectDialog`. It reaches
   `shell()` only through the names in `uses` (ADR-0073 D4), and it opens on the
   `workbench:add-project-open` event (ADR-0073 D5).

   `main.ts` registers it as `wbAddProjectDialog` (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBAddProject } from "./wb-add-project.ts";
import { WBFail } from "./wb-fail.ts";

export function addProjectDialog() {
  // Every `shell()` member this component's code or markup reads or calls.
  // `loadComponent` in ui-tests/harness.mjs fails on any other name, and the
  // type check fails on a name the code reads.
  return component(["fleetPeers", "loadFleet", "loadRepos", "toggle", "scrim"], {
    // The Add a project dialog (#501): its whole state is the wb-add-project.ts fold.
    addProject: WBAddProject.initial(),
    // The debounce and "Loading…" timers of its folder list.
    _addProjectTimer: undefined as number | undefined,
    _addProjectSlow: undefined as number | undefined,
    _addProjectSeq: 0,
    // The last press on the folder list was not a mouse.
    _addProjectTouch: false,
    // --- Add a project (#501) -----------------------------------------------
    // Thin calls: every state change goes through `WBAddProject.next`, and
    // the daemon that owns the folder lists it and runs `ralphy daemon add`.
    addProjectStep(ev: any) {
      this.addProject = WBAddProject.next(this.addProject, ev);
    },
    openAddProject() {
      this.addProjectStep({ type: "open" });
      this.addProjectList(0);
      this.$nextTick(() => this.$refs.addProjectFolder?.focus());
    },
    // Closing does not cancel an add in flight: the list reloads when it ends.
    closeAddProject() {
      this.addProjectStep({ type: "close" });
      clearTimeout(this._addProjectTimer);
      clearTimeout(this._addProjectSlow);
    },
    addProjectPlaces() {
      return WBAddProject.places(this.fleetPeers);
    },
    addProjectEntries() {
      return WBAddProject.entries(this.addProject);
    },
    addProjectPrimary() {
      return WBAddProject.primary(this.addProject);
    },
    addProjectHelp() {
      return WBAddProject.help(this.addProject);
    },
    addProjectListable() {
      return WBAddProject.listable(this.addProject);
    },
    addProjectWhere(daemon: string) {
      this.addProjectStep({ type: "where", daemon });
      this.addProjectList(0);
    },
    addProjectText(text: string) {
      this.addProjectStep({ type: "text", text, peers: this.fleetPeers });
      // A WSL path with no peer calls no verb.
      if (this.addProject.wslMissing) return;
      this.addProjectList(150);
    },
    // `ev` is the click, absent for a key. The second click of a double click
    // is dropped: by then the list may show the folder the first one opened.
    // A touch does not focus the field: on a phone that opens the keyboard,
    // and iOS Safari zooms into an input with text under 16px.
    addProjectPick(entry: any, ev?: any) {
      if (entry.error || (ev && ev.detail > 1)) return;
      this.addProjectStep({ type: "pick", name: entry.name, up: !!entry.up });
      this.addProjectList(0);
      if (!this._addProjectTouch) this.$refs.addProjectFolder?.focus();
    },
    // Arrows move in the list; Enter or Tab on a highlighted folder goes down
    // one level; Enter with none highlighted adds.
    addProjectKey(ev: any) {
      const list = this.addProjectEntries();
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        ev.preventDefault();
        this.addProjectStep({ type: "move", by: ev.key === "ArrowDown" ? 1 : -1 });
        return;
      }
      const picked = list[this.addProject.active];
      if ((ev.key === "Enter" || (ev.key === "Tab" && !ev.shiftKey)) && picked) {
        ev.preventDefault();
        this.addProjectPick(picked);
        return;
      }
      if (ev.key === "Enter") {
        ev.preventDefault();
        if (WBAddProject.enterAdds(this.addProject)) this.addProjectSubmit();
      }
    },
    // Ask the daemon for the folder list after `delay` ms. Each request has a
    // sequence number, and the fold drops a reply that is not the newest.
    addProjectList(delay: number) {
      clearTimeout(this._addProjectTimer);
      this._addProjectTimer = setTimeout(() => {
        const seq = ++this._addProjectSeq;
        const payload = WBAddProject.request(this.addProject);
        this.addProjectStep({ type: "sent", seq });
        clearTimeout(this._addProjectSlow);
        this._addProjectSlow = setTimeout(() => this.addProjectStep({ type: "slow", seq }), 300);
        window.WBDaemon.observe("dir.list", payload)
          .then((reply) => this.addProjectStep({ type: "reply", seq, reply }))
          .catch((e) => this.addProjectStep({ type: "reply", seq, reply: { status: "error", message: String(e.message || e) } }));
      }, delay);
    },
    async addProjectSubmit() {
      if (this.addProjectPrimary().disabled) return;
      const payload = WBAddProject.addPayload(this.addProject);
      this.addProjectStep({ type: "adding" });
      let reply;
      try {
        reply = await window.WBDaemon.observe("project.add", payload);
      } catch (e: any) {
        reply = { status: "error", message: String(e.message || e) };
      }
      if (reply?.status !== "ok") {
        this.addProjectStep({
          type: "addFailed",
          message: WBFail.failed(reply, "Could not add the project: the daemon gave no reason."),
        });
        this.loadRepos({ git: false });
        return;
      }
      const stillOpen = this.addProject.open;
      this.addProjectStep({ type: "added" });
      await this.loadRepos({ git: false });
      if (payload.daemon) await this.loadFleet();
      // A stated exception to returning focus to the opener: the new project
      // is selected, shown and focused, so work on it can start at once.
      if (stillOpen) this.selectAddedProject(payload.daemon ? `${payload.daemon}/${reply.slug}` : reply.slug);
    },
    selectAddedProject(ref: string | undefined) {
      if (!this.$store.projects.projects.some((p) => this.$store.projects.repoRef(p) === ref)) return;
      if (this.$store.projects.openSlug !== ref) this.toggle(ref);
      this.$nextTick(() => {
        const head = document.querySelector<HTMLElement>("li.project.open .project-head");
        if (!head) return;
        head.scrollIntoView({ block: "nearest" });
        head.focus();
      });
    },
  });
}
