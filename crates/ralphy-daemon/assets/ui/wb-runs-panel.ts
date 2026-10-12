/* ---------------------------------------------------------------------------
   The Runs panel of the open project: an Alpine component on the element that
   holds the panel (`aside.runs`) and the run modal (#621; ADR-0073 D2).

   It owns the run picker, the issue trail, the plan section (the steps and
   the prose of `.ralphy/plan.md`), the daemon verbs (run, stop, triage,
   push), the run modal, the verb refusal and the raw output feed. The runs
   themselves are the Alpine store `runs` (wb-runs-store.ts): the panel reads
   them, and changes them only through the store's methods.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4),
   and the page reaches it only through `workbench:*` events, which `init()`
   hears.

   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { sendWindow } from "./wb-events.ts";
import { WBRun } from "./wb-runs.ts";
import type { Run, RunIssue } from "./wb-shell-types.d.ts";

export function wbRuns() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["runsOpen", "toggleRuns", "nowMs", "kanbanOpen", "toggleKanban"], {
    // The trail node the operator arrived at from the board (#301): a marker,
    // not a selection.
    trailFocus: null as number | null,
    runMenu: false,
    planSection: "",

    // Read the selected run's plan via `file.read` (the document carries its
    // PATH, never its text). A refusal KEEPS the last good text and only flags
    // it (#330); `runsError` is untouched. Reads the PRIMARY tree — no
    // `checkout` (#406): a run takes the primary tree (ADR-0063 §7). Same for
    // `loadPlan`, `diffWorkSide` and the git-backed panels.
    async loadRunPlan() {
      const run = this.$store.runs.currentRun();
      if (!run?.planPath) return;
      try {
        const reply = await window.WBDaemon.observe("file.read", {
          repo: this.$store.projects.openSlug,
          path: run.planPath,
        });
        if (reply?.status === "ok") {
          this.$store.runs.setRunPlan(run, reply.content || "");
        } else {
          this.$store.runs.setRunPlan(run, null);
        }
      } catch {
        this.$store.runs.setRunPlan(run, null);
      }
      // A replacement mints new run objects: a `run` no longer current belongs
      // to a superseded hydration.
      if (this.$store.runs.currentRun() !== run) return;
      // Reassign the section only when the chosen heading is gone.
      const hs = this.planHeadings(run);
      if (!hs.includes(this.planSection)) this.planSection = hs[0] || "";
    },

    selectRun(runid: string) {
      this.$store.runs.setCurrentRun(runid);
      this.trailFocus = null; // the arrival marker belonged to the run we left
      // reset the section dropdown to the new run's first non-Steps heading
      this.planSection = this.planHeadings(this.$store.runs.currentRun())[0] || "";
      // each run has its own plan; the viewer follows the selection.
      this.loadRunPlan();
    },

    // Thin delegations to the faithful helpers in wb-runs.ts.
    runPhaseLabel(run: Run | null) {
      return run ? WBRun.runPhaseLabel(run) : "";
    },
    runTitle(run: Run) {
      return WBRun.runTitle(run);
    },
    runIdentity(run: Run) {
      return WBRun.runIdentity(run);
    },
    // Reading `nowMs` subscribes this binding to the 1 s tick.
    runClock(run: Run) {
      return WBRun.phaseClock(run, this.nowMs);
    },
    // When this phase began, and the run's whole elapsed time.
    clockTitle(run: Run | null) {
      if (!run) return "";
      const parts: string[] = [];
      const at = (iso: string) =>
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
    issueState(run: Run, iss: RunIssue) {
      return WBRun.issueState(run, iss);
    },
    issueGlyph(run: Run, iss: RunIssue) {
      return WBRun.glyph(run, iss);
    },
    sleepLabel(run: Run | null) {
      return WBRun.sleepText(run?.sleep);
    },
    nodeTitle(run: Run | null, iss: RunIssue | null) {
      if (!run || !iss) return "";
      const st = WBRun.issueState(run, iss);
      let t = `#${iss.number} — ${iss.title} · ${WBRun.LABEL[st] || st}`;
      // Per-issue: tier routing gives two issues of one run different models.
      const seg = WBRun.modelEffort(iss.model, iss.effort);
      if (seg) t += ` · ${seg}`;
      if (iss.blockedBy?.length) t += ` (blocked by ${iss.blockedBy.map((n) => "#" + n).join(", ")})`;
      return t;
    },
    // Run → board (#301): a trail node opens that issue's detail. The Runs
    // panel closes first (`z-index: 150`, sharing the drawer's right edge).
    // `toggleKanban()` resets `kanbanSel`, so it runs BEFORE the issue opens
    // (wb-board.ts).
    focusIssue(number: number) {
      window.WB.emit("run-issue-focus", { project: this.$store.projects.openSlug, runid: this.$store.runs.currentRun()?.runid, issue: number });
      if (this.runsOpen) this.toggleRuns();
      this.trailFocus = null;
      if (!this.kanbanOpen) this.toggleKanban();
      sendWindow(window, "workbench:issue-open", { issue: number });
    },

    // Board → run (#301): the card's run pill opens the Runs panel on THAT run,
    // marking the issue in the trail. The board stays open behind it.
    openRunFor(number: number) {
      const hit = window.WBKanban.runningFor(number, this.$store.runs.projectRuns());
      if (!hit) return;
      this.$store.runs.setCurrentRun(hit.runid);
      this.planSection = this.planHeadings(this.$store.runs.currentRun())[0] || "";
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

    // The runid whose stop is in flight: a double-click must not dispatch two
    // `ralphy stop` children.
    runStopping: null as string | null,
    // Whether the open project has a live run: flips the toolbar between `run`
    // and `stop` (ADR-0054). Derived from the SAME list `writeLockReason`
    // reads, so the two agree, and self-clearing via `runs.dirty`.
    runIsLive() {
      return this.$store.runs.projectRuns().length > 0;
    },
    // Ask a live run to stop (ADR-0054). This does NOT kill anything: it
    // dispatches `ralphy stop`, which writes a request the run acts on; the
    // daemon never signals a dispatched child (ADR-0032 §5/§6). No wait: the
    // reply says the request was written; the run leaves the panel via
    // `runs.dirty` when it exits.
    async stopRun(runid: string | null | undefined) {
      // No runid: the run left the panel between the render and the click.
      if (!runid || this.runStopping) return;
      // ADR-0032 §6 asks for a strong confirmation. The shell's OWN dialog,
      // not `window.confirm` (blocks the page, ignores the theme).
      const ok = await this.$store.modals.askConfirm({
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
          repo: this.$store.projects.openSlug,
          runid,
        });
        if (WBFail.isError(reply)) {
          this.runVerbFailed(WBFail.failed(reply, "Could not stop the run."));
        } else {
          this.$store.modals.flash("Stop requested. The run is stopping.");
        }
      } catch {
        this.$store.modals.flash("Could not stop the run: the daemon is not connected.");
      } finally {
        this.runStopping = null;
      }
    },

    // --- plan viewer ------------------------------------------------------
    // The issue whose plan this panel is showing: the snapshot's `plan.issue`
    // when it has one, else the run's active issue.
    planIssueWanted(run: Run | null) {
      return run?.planIssue ?? run?.active ?? null;
    },
    // The issue the PROSE belongs to, from the plan's own trailer. The steps
    // are keyed by issue (ADR-0047 A1) but the prose is a `file.read` of
    // `.ralphy/plan.md`, and a failed read KEEPS the last text (#330): without
    // the key the block would render the PREVIOUS issue's plan. Unkeyed prose
    // (a half-written plan) stays empty.
    planProseIssue(run: Run | null) {
      return WBRun.planTrailerIssue(run?.planMd);
    },
    planProseIsCurrent(run: Run | null) {
      return WBRun.planBelongsTo(run?.planMd, this.planIssueWanted(run));
    },
    // Every `##` section except Steps (its own block); none while the prose
    // belongs to another issue.
    planHeadings(run: Run | null) {
      if (!this.planProseIsCurrent(run)) return [];
      return WBRun.headings(run?.planMd).filter((h) => h.toLowerCase() !== "steps");
    },
    // Render one `##` section as sanitized HTML. Steps render from the
    // snapshot document, not from here (#330).
    renderPlanSection(run: Run | null, name: string) {
      if (!run || !name || !this.planProseIsCurrent(run)) return "";
      const body = WBRun.section(run?.planMd, name);
      return DOMPurify.sanitize(marked.parse(body || "_(empty)_"));
    },

    // --- the step list (the plan block is state, #330) ---------------------
    planSteps() {
      return this.$store.runs.currentRun()?.steps || [];
    },
    stepGlyph(status: string) {
      return WBRun.stepGlyph(status);
    },
    stepLabel(status: string) {
      return WBRun.stepLabel(status);
    },
    stepClass(status: string) {
      return WBRun.stepClass(status);
    },
    // Why the step list is empty — an unexplained blank block reads as a bug.
    stepsNote() {
      const run = this.$store.runs.currentRun();
      if (this.planSteps().length) return "";
      if (run?.phase === "planning") return "Writing the plan…";
      if (run?.planIssue != null) return "This plan has no steps.";
      return "No plan for this issue yet.";
    },
    // Why the prose block is empty: unreadable, not written yet, or written
    // for another issue are DIFFERENT facts.
    proseNote() {
      const run = this.$store.runs.currentRun();
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
    // the --plan-agent and --branch-mode new|current flags.
    runOpen: false,
    // A CLI refusal, held until the next verb click (#331). Distinct from the
    // 2.6 s `runsActionMsg` flash.
    verbError: "",
    // Phase 1 raw merged output of the last daemon-spawned run (wb-daemon.ts).
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
        project: this.$store.projects.openSlug,
        agent: c.agent,
        planAgent,
        branchMode: c.branchMode,
        command: this.runCommandPreview(),
      });
      this.$store.modals.flash("Run started.");
      this.closeRunModal();
    },
    // triage / push: the verb name is the whole intent; the client never
    // composes a command line.
    fireVerb(verb: string) {
      this._resetVerbSurface();
      window.WB.emit("command", { project: this.$store.projects.openSlug, verb });
      this.$store.modals.flash(this.$store.connection.verbRequestedText(verb));
    },
    // From wb-daemon.ts on a TERMINAL frame only; an empty note is a no-op.
    runVerbFailed(msg: string) {
      if (msg) this.verbError = msg;
    },

    // On `workbench:project-changed`. The trail marker and a verb refusal
    // (#331, whose terminal frame can land long after the click) of the
    // project that WAS open are dropped. The selected run follows in the
    // Alpine store `runs`, whose listener runs first: after the paint, the
    // section is the new run's.
    runsPanelFollowProject() {
      this.trailFocus = null;
      this.verbError = "";
      this.$nextTick(() => {
        this.planSection = this.planHeadings(this.$store.runs.currentRun())[0] || "";
      });
    },

    // The listeners, added once when Alpine builds the panel.
    init() {
      window.addEventListener("workbench:project-changed", () => this.runsPanelFollowProject());
      // A read of the open project's runs was kept. Only while showing: a
      // whole-plan `file.read` nobody can see is cost.
      window.addEventListener("workbench:runs-read", () => {
        if (this.runsOpen) this.loadRunPlan();
      });
      // The panel closed (`toggleRuns`): the board-arrival marker goes.
      window.addEventListener("workbench:runs-closed", () => (this.trailFocus = null));
      // The run pill of a board card (#301).
      window.addEventListener("workbench:run-open", (e) => this.openRunFor(e.detail.issue));
      // A raw output chunk (`window.WBRuns.output`), capped so the DOM never
      // grows unbounded.
      window.addEventListener("workbench:run-output", (e) => (this.rawFeed = (this.rawFeed + e.detail.text).slice(-8000)));
      // A terminal verb frame (wb-daemon.ts).
      window.addEventListener("workbench:run-verb-failed", (e) => this.runVerbFailed(e.detail.message));
    },
  });
}
