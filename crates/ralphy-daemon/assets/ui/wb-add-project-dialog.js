/* ---------------------------------------------------------------------------
   The Add a project dialog: a folder on this computer or on a peer becomes a
   project. The daemon that owns the folder lists it (`dir.list`) and runs
   `ralphy daemon add` (`project.add`), and `wb-add-project.js` is the fold that
   holds the dialog's state.

   `addProjectDialog` is the Alpine component `wbAddProjectDialog`. It reaches
   `shell()` only through the names in `uses` (ADR-0073 D4), and it opens on the
   `workbench:add-project-open` event (ADR-0073 D5).

   Load order: after `wb-add-project.js`, before `app.js` and before Alpine, on
   `index.html` only.
   --------------------------------------------------------------------------- */
function addProjectDialog() {
  return {
    // Every `shell()` member this component's code or markup reads or calls.
    // `loadComponent` in ui-tests/harness.mjs fails on any other name.
    uses: ["fleetPeers", "loadFleet", "loadRepos", "openSlug", "projects", "repoRef", "toggle", "scrim"],
    // The Add a project dialog (#501): its whole state is the wb-add-project.js fold.
    addProject: window.WBAddProject.initial(),
    // The debounce and "Loading…" timers of its folder list.
    _addProjectTimer: null,
    _addProjectSlow: null,
    _addProjectSeq: 0,
    // The last press on the folder list was not a mouse.
    _addProjectTouch: false,
    // --- Add a project (#501) -----------------------------------------------
    // Thin calls: every state change goes through `WBAddProject.next`, and
    // the daemon that owns the folder lists it and runs `ralphy daemon add`.
    addProjectStep(ev) {
      this.addProject = window.WBAddProject.next(this.addProject, ev);
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
      return window.WBAddProject.places(this.fleetPeers);
    },
    addProjectEntries() {
      return window.WBAddProject.entries(this.addProject);
    },
    addProjectPrimary() {
      return window.WBAddProject.primary(this.addProject);
    },
    addProjectHelp() {
      return window.WBAddProject.help(this.addProject);
    },
    addProjectListable() {
      return window.WBAddProject.listable(this.addProject);
    },
    addProjectWhere(daemon) {
      this.addProjectStep({ type: "where", daemon });
      this.addProjectList(0);
    },
    addProjectText(text) {
      this.addProjectStep({ type: "text", text, peers: this.fleetPeers });
      // A WSL path with no peer calls no verb.
      if (this.addProject.wslMissing) return;
      this.addProjectList(150);
    },
    // `ev` is the click, absent for a key. The second click of a double click
    // is dropped: by then the list may show the folder the first one opened.
    // A touch does not focus the field: on a phone that opens the keyboard,
    // and iOS Safari zooms into an input with text under 16px.
    addProjectPick(entry, ev) {
      if (entry.error || (ev && ev.detail > 1)) return;
      this.addProjectStep({ type: "pick", name: entry.name, up: !!entry.up });
      this.addProjectList(0);
      if (!this._addProjectTouch) this.$refs.addProjectFolder?.focus();
    },
    // Arrows move in the list; Enter or Tab on a highlighted folder goes down
    // one level; Enter with none highlighted adds.
    addProjectKey(ev) {
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
        if (window.WBAddProject.enterAdds(this.addProject)) this.addProjectSubmit();
      }
    },
    // Ask the daemon for the folder list after `delay` ms. Each request has a
    // sequence number, and the fold drops a reply that is not the newest.
    addProjectList(delay) {
      clearTimeout(this._addProjectTimer);
      this._addProjectTimer = setTimeout(() => {
        const seq = ++this._addProjectSeq;
        const payload = window.WBAddProject.request(this.addProject);
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
      const payload = window.WBAddProject.addPayload(this.addProject);
      this.addProjectStep({ type: "adding" });
      let reply;
      try {
        reply = await window.WBDaemon.observe("project.add", payload);
      } catch (e) {
        reply = { status: "error", message: String(e.message || e) };
      }
      if (reply?.status !== "ok") {
        this.addProjectStep({
          type: "addFailed",
          message: window.WBFail.failed(reply, "Could not add the project: the daemon gave no reason."),
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
    selectAddedProject(ref) {
      if (!this.projects.some((p) => this.repoRef(p) === ref)) return;
      if (this.openSlug !== ref) this.toggle(ref);
      this.$nextTick(() => {
        const head = document.querySelector("li.project.open .project-head");
        if (!head) return;
        head.scrollIntoView({ block: "nearest" });
        head.focus();
      });
    },
  };
}

window.WBAddProjectDialog = { component: addProjectDialog };

if (typeof document !== "undefined" && document.addEventListener) {
  document.addEventListener("alpine:init", () => window.Alpine.data("wbAddProjectDialog", addProjectDialog));
}
