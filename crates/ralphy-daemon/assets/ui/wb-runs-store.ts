/* ---------------------------------------------------------------------------
   The runs of each project: the Alpine store `runs` (#621; ADR-0073 D6 and
   the amendment of 2026-10-08, decisions 3 and 6).

   The store is the one owner of the run snapshots of each project, their read
   state, the selected run, and the runs socket of the open project. Its
   fields change only through its own methods. `shell()` code reads
   `this.$store.runs`, the markup reads `$store.runs`, and code outside Alpine
   reads `Alpine.store("runs")`.

   It reads no `shell()` member. It tells the page two facts with an event on
   `window`, and the page decides what each one means for it:
   - `workbench:runs-read`: a read of the open project's runs was kept;
   - `workbench:runs-dirty`: the runs socket says the snapshots changed.

   `main.ts` registers it as `runs`, after `projects`. `runsStore` builds it
   without Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBRun } from "./wb-runs.ts";
import { sendWindow } from "./wb-events.ts";
import type { Messages } from "./wb-messages.ts";
import type { Read, Run, Subscription } from "./wb-shell-types.d.ts";

/** What `main.ts` gives the store. */
export type RunsStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
};

// `deps` is the shape every store takes; the runs tell the operator nothing
// themselves yet (the panel words a failed read from `runsError`).
export function runsStore(_deps: RunsStoreDeps) {
  return {
    // Monotonic hydration token: overlapping `runs.list` replies can land OUT
    // OF ORDER; only the newest hydration commits.
    _runsSeq: 0,
    // One entry per `runid`: issue queue + per-issue status, live phase, the
    // current issue's plan.md (helpers in wb-runs.ts, `WBRun`).
    runsByProject: {} as Record<string, Run[]>,
    // An error must never render as "No active runs": an empty project and an
    // unreadable one are different facts (ADR-0047 §6).
    runsError: "",
    currentRunId: null as string | null,
    runsRead: {} as Record<string, Read>,
    _runsSub: null as Subscription | null, // the live run-snapshot subscription for the open project, if any

    // Alpine calls this once, when `main.ts` registers the store.
    init() {
      // The open project changed (ADR-0073 amendment of 2026-10-08, decision 4).
      window.addEventListener("workbench:project-changed", () => this.runsFollowProject());
    },

    // Hydrate from `runs.list` (ADR-0047 §9), by REPLACEMENT — a snapshot is
    // state, not a log. On panel open and project change.
    async hydrateRuns() {
      const slug = window.Alpine.store("projects").openSlug;
      // Clear FIRST: a stale error must not outlive its project.
      this.runsError = "";
      if (!slug) return;
      const prevRuns = this.runsByProject[slug] || [];
      const seq = ++this._runsSeq;
      try {
        const reply = await window.WBDaemon.observe("runs.list", { repo: slug });
        // Superseded while in flight: the newer hydration owns the state.
        if (seq !== this._runsSeq || window.Alpine.store("projects").openSlug !== slug) return;
        if (reply?.status !== "ok") {
          this.runsFailed(slug, WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        this.runsRead[slug] = WBFail.readFold(this.runsRead[slug], { ok: true, value: true, at: Date.now() });
        this.runsByProject[slug] = (reply.runs || []).map((d) => {
          const run = WBRun.fromSnapshot(d);
          // A push arrives on every snapshot write (~every few hundred ms);
          // re-fetching an unchanged plan would blank the viewer each time.
          const prev = prevRuns.find((p) => p.runid === run.runid);
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
        this.currentRunId = listed.some((r) => r.runid === this.currentRunId)
          ? this.currentRunId
          : listed[0]?.runid || null;
        // The panel reads the selected run's plan, if it shows (wb-runs-panel.ts).
        sendWindow(window, "workbench:runs-read");
      } catch (err) {
        if (seq !== this._runsSeq || window.Alpine.store("projects").openSlug !== slug) return;
        // A transport failure is a read failure, not an idle project.
        this.runsFailed(slug, WBFail.why({ message: (err as Error | undefined)?.message }, "the daemon did not answer"));
      }
    },
    // A failed `runs.list` (ADR-0070 D3). After a good read the runs stay,
    // marked not current; before one, there are none and the panel says why.
    runsFailed(slug: string, reason: string) {
      const read = WBFail.readFold(this.runsRead[slug], { ok: false, reason, at: Date.now() });
      this.runsRead[slug] = read;
      if (read.goodAt) {
        this.runsError = WBFail.notCurrent(read, (ms) => WBFail.clock(ms));
        return;
      }
      this.runsByProject[slug] = [];
      this.runsError = `Could not read the runs: ${reason}.`;
    },

    // The open project's runs (the panel is project-scoped).
    projectRuns() {
      return this.runsByProject[window.Alpine.store("projects").openKey()] || [];
    },
    // The selected run, falling back to the first when the id is stale (e.g. the
    // project changed).
    currentRun() {
      const runs = this.projectRuns();
      return runs.find((r) => r.runid === this.currentRunId) || runs[0] || null;
    },
    // The operator picked a run (the run menu, a board pill).
    setCurrentRun(runid: string | null) {
      this.currentRunId = runid;
    },
    // The Runs panel read the plan of `run`: its text, or `null` when the read
    // failed. A failed read KEEPS the last good text and only flags it (#330).
    setRunPlan(run: Run, planMd: string | null) {
      if (planMd === null) {
        run.planReadFailed = true;
        return;
      }
      run.planMd = planMd;
      run.planReadFailed = false;
    },

    // The open project's run-snapshot subscription (#300, ADR-0047 §9).
    mountRunsSub() {
      const slug = window.Alpine.store("projects").openSlug;
      if (!window.WBDaemon?.subscribeRuns || !slug) return;
      // What a push means for the page (a hidden tab, the board) is the
      // page's: `app.ts` hears it.
      this._runsSub = window.WBDaemon.subscribeRuns(slug, () =>
        sendWindow(window, "workbench:runs-dirty"),
      );
    },
    destroyRunsSub() {
      try {
        this._runsSub?.close();
      } catch {}
      this._runsSub = null;
    },
    // On `workbench:project-changed`. After the paint, the runs socket (#300)
    // follows the tree's open/close path, and the selection falls back to the
    // first run of the project now open.
    runsFollowProject() {
      window.Alpine.nextTick(() => {
        this.destroyRunsSub();
        this.mountRunsSub();
        this.currentRunId = this.projectRuns()[0]?.runid || null;
        if (window.Alpine.store("projects").openSlug) this.hydrateRuns();
      });
    },
  };
}

/** The Alpine store `runs`. */
export type RunsStore = ReturnType<typeof runsStore>;
