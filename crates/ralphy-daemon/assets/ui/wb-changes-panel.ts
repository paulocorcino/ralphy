/* ---------------------------------------------------------------------------
   The Changes view of the open project: an Alpine component on the element
   that holds it (`div.changes-view` in the side panel; #621, ADR-0073 D2).

   It owns the change set and the branch (sync) of each project, their reads
   and their read state, the remote acts (fetch, pull, push), the rows and
   their acts (stage, unstage, discard), the commit box, the refusal line,
   the run-completion nudge socket and the slow backstop.
   The write locks are the Alpine store `connection`'s (wb-connection-store.ts):
   they also read the runs and the page build, and the board and the Runs
   panel ask them too. This view tells the store whether its reads are
   current (`_reportCurrent`). The change count of each project is the
   Alpine store `checkouts`'s (wb-checkouts-store.ts), because the project
   rows show it too: this view writes it through `changeCountRead`.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4),
   and the page reaches it only through `workbench:*` events, which `init()`
   hears.

   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component, emptyRecord } from "./wb-alpine.ts";
import { WBChanges } from "./wb-changes.ts";
import { WBFail } from "./wb-fail.ts";
import type { ProjectsStore } from "./wb-projects-store.ts";
import type { ChangeEntry, Read, Subscription, Sync, Timer } from "./wb-shell-types.d.ts";

export function wbChanges() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["sideOpen", "sideView", "checkoutChipClick"], {
    // The slow backstop: the `/ws/tree` nudge (#310) only reports what a RUN
    // did; an operator's own editor produces no event. Two git subprocesses per
    // minute, only while the panel is open and the tab in front.
    CHANGES_POLL_MS: 50000,
    _changesBackstop: null as Timer,
    // --- the change set and the branch ------------------------------------
    // Per slug, named apart from the panel-wide `changesError` below: a
    // duplicate key in this literal is a silent no-op.
    changesReadError: emptyRecord<string>(),
    // Per slug, the read state of the change set, the branch (sync), the board
    // and the runs (`WBFail.readFold`, ADR-0070 D3). A write is locked while
    // its fact is not current.
    changesRead: emptyRecord<Read>(),
    syncRead: emptyRecord<Read>(),
    // The two rendered groups (#315). INVARIANT: every path that sets one must
    // set the OTHER in the SAME statement — a stale group left behind renders
    // rows under a headline while the badge already reads `—`.
    changesStaged: emptyRecord<ChangeEntry[]>(),
    changesUnstaged: emptyRecord<ChangeEntry[]>(),
    // The sync row per project (#316): the fold of `sync.status`. Same three
    // triggers as the change set, never a timer.
    syncByProject: emptyRecord<Sync>(),
    // The remote act in flight (`"fetch"` | `"pull"` | `"push"`), or null. One
    // slot for the whole bar: the three acts share the upstream, so a second
    // click while one is out would race it against the first — and a push's
    // round trip is long enough that a silent button reads as a dead one.
    syncBusy: null as "fetch" | "pull" | "push" | null,
    // Same token for the Changes count (#310's nudge overlaps the open's read)
    // and for the sync row.
    _changesSeq: 0,
    _syncSeq: 0,
    // The last refusal from the Changes panel, held until the next act. NOT
    // `runsActionMsg`: that renders only inside `aside.runs`, which is closed
    // by default. One string, not per-project: switching projects is itself
    // the next act.
    changesError: "",
    // The change count per slug is the store `checkouts`'s (`changesCount`);
    // `changesReadError` carries the reason of a failed read into the Changes
    // view's title.
    // The commit message being composed (#318). One box, but it belongs to
    // `commitMsgSlug` ONLY: a message typed for repo A must never land as repo
    // B's commit. Cleared on success only.
    commitMsg: "",
    commitMsgSlug: null as string | null,

    // Re-read the working tree, only while the Changes panel is on screen: both
    // reads are local but each is a subprocess.
    refreshChanges() {
      if (!this.sideOpen || this.sideView !== "changes" || !this.$store.projects.openSlug) return;
      if (document.visibilityState !== "visible") return;
      void this.loadChanges(this.$store.projects.openSlug);
      void this.loadSync(this.$store.projects.openSlug);
    },

    // The open project's change count (#307) via `changes.list`: reloads on
    // open, sidebar refresh and run-completion nudge (#310), never on a
    // repo-wide watch. The SELECTED checkout's (#407, ADR-0063 §2).
    async loadChanges(slug: string | null | undefined) {
      if (!slug) return;
      // Overlapping reads can return OUT OF ORDER (as `_runsSeq`).
      const seq = ++this._changesSeq;
      try {
        const reply = await window.WBDaemon.observe(
          "changes.list",
          window.WBDaemon.withCheckout({ repo: slug }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (seq !== this._changesSeq) return; // superseded → the newer read owns it
        if (!reply || reply.status !== "ok") {
          this.changesFailed(slug, WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        const folded = WBChanges.fold(reply);
        this.$store.checkouts.changeCountRead(slug, folded.count);
        this.changesStaged[slug] = folded.staged;
        this.changesUnstaged[slug] = folded.unstaged;
        this.changesReadError[slug] = "";
        this.changesRead[slug] = WBFail.readFold(this.changesRead[slug], { ok: true, value: true, at: Date.now() });
        this._reportCurrent(slug);
      } catch {
        if (seq === this._changesSeq) {
          this.changesFailed(slug, "the daemon did not answer");
        }
      }
    },
    // A failed `changes.list` (ADR-0070 D3). After a good read the groups and
    // the count stay, marked not current; before one, the count is absent
    // (`—`), never another repo's number or a clean tree.
    changesFailed(slug: string, reason: string) {
      const read = WBFail.readFold(this.changesRead[slug], { ok: false, reason, at: Date.now() });
      this.changesRead[slug] = read;
      this._reportCurrent(slug);
      if (read.goodAt) {
        this.changesReadError[slug] = WBFail.notCurrent(read, (ms) => this.$store.connection.fmtClock(ms));
        return;
      }
      this.$store.checkouts.changeCountRead(slug, null);
      this.changesReadError[slug] = `Could not read the changes: ${reason}`;
      this.changesStaged[slug] = [];
      this.changesUnstaged[slug] = [];
    },

    // The open project's sync state (#316) via `sync.status`, which makes NO
    // network call. No timer: a launcher holding N repos must never become a
    // scheduled network client.
    async loadSync(slug: string | null | undefined) {
      if (!slug) return;
      const seq = ++this._syncSeq;
      try {
        const reply = await window.WBDaemon.observe(
          "sync.status",
          window.WBDaemon.withCheckout({ repo: slug }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (seq !== this._syncSeq) return; // superseded → the newer read owns it
        if (!reply || reply.status !== "ok") {
          this.syncFailed(slug, WBFail.why(reply, "the daemon gave no reason"));
          return;
        }
        const sync = WBChanges.foldSync(reply);
        this.syncByProject[slug] = sync;
        this.syncRead[slug] = WBFail.readFold(this.syncRead[slug], { ok: true, value: true, at: Date.now() });
        this._reportCurrent(slug);
        // Under a selected worktree the read is THAT tree's HEAD, and
        // `p.branch` is the primary's (#407), so only a primary read moves it.
        if (!this.$store.checkouts.checkoutOf(slug)) primaryReadMovesRow(this.$store.projects, slug, sync);
      } catch {
        if (seq === this._syncSeq) {
          this.syncFailed(slug, "the daemon did not answer");
        }
      }
    },
    // A failed `sync.status` (ADR-0070 D3). After a good read the row stays,
    // and its note says it is not current; before one, the state is unknown.
    syncFailed(slug: string, reason: string) {
      const read = WBFail.readFold(this.syncRead[slug], { ok: false, reason, at: Date.now() });
      this.syncRead[slug] = read;
      this._reportCurrent(slug);
      const prev = this.syncByProject[slug];
      if (read.goodAt && prev && prev.state !== "unknown") {
        this.syncByProject[slug] = { ...prev, note: WBFail.notCurrent(read, (ms) => this.$store.connection.fmtClock(ms)) };
        return;
      }
      this.syncByProject[slug] = { ...WBChanges.foldSync(null), note: `Could not read the branch: ${reason}` };
    },

    // Fetch from the upstream — the operator's act, never a timer's. A refusal
    // is `{status:"error"}` whose message IS the core's prose.
    async syncFetch(slug: string) {
      if (this.syncBusy || this.$store.connection.writeLocked()) return;
      this.syncBusy = "fetch";
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "sync.fetch",
          window.WBDaemon.withCheckout({ repo: slug }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not fetch: the daemon gave no reason."),
          );
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered.
        this._changesRefused("Could not fetch: the daemon did not answer.");
      } finally {
        this.syncBusy = null;
      }
      void this.loadSync(slug);
    },

    // Fast-forward from the upstream. A successful pull moves the working tree,
    // so the change set is reloaded beside the counts.
    async syncPull(slug: string) {
      if (this.syncBusy || this.$store.connection.writeLocked()) return;
      this.syncBusy = "pull";
      this.changesError = "";
      let moved = false;
      try {
        const reply = await window.WBDaemon.observe(
          "sync.pull",
          window.WBDaemon.withCheckout({ repo: slug }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not pull: the daemon gave no reason."),
          );
        } else {
          moved = true;
        }
      } catch {
        this._changesRefused("Could not pull: the daemon did not answer.");
      } finally {
        this.syncBusy = null;
      }
      void this.loadSync(slug);
      if (moved) void this.loadChanges(slug);
    },

    // Publish the branch (#320). The OPERATOR's click is the whole consent (no
    // opt-in flag, ADR-0046 amendment); a refusal's message IS the core's
    // prose. No credential UI, by decision. Push moves no file, so only the
    // counts reload.
    async syncPush(slug: string) {
      if (this.syncBusy || this.$store.connection.writeLocked()) return;
      this.syncBusy = "push";
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "sync.push",
          window.WBDaemon.withCheckout({ repo: slug }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not push: the daemon gave no reason."),
          );
        }
      } catch {
        this._changesRefused("Could not push: the daemon did not answer.");
      } finally {
        this.syncBusy = null;
      }
      void this.loadSync(slug);
    },

    _changesSub: null as Subscription | null, // the run-completion nudge subscription for the open project (#310)
    // The run-completion subscription (#310, ADR-0036 amendment). The socket
    // carries EVERY repo's nudge, so the filter is here.
    mountChangesSub() {
      if (!window.WBDaemon?.subscribeChanges || !this.$store.projects.openSlug) return;
      this._changesSub = window.WBDaemon.subscribeChanges(this.$store.projects.openSlug, (frame) => {
        if (this.$store.connection.tabHidden()) return;
        // Optional-chained: a frame without wb-changes.ts must not throw
        // inside `onmessage`.
        if (WBChanges?.shouldReload?.(frame, this.$store.projects.openSlug)) {
          void this.loadChanges(this.$store.projects.openSlug);
          void this.loadSync(this.$store.projects.openSlug);
        }
      });
    },
    destroyChangesSub() {
      try {
        this._changesSub?.close();
      } catch {}
      this._changesSub = null;
    },
    // On `workbench:project-changed`. Refusal notes name an act against the
    // project that WAS open, and an unsent commit message (#318) is dropped.
    // After the paint, the changes-nudge socket (#310) follows the tree's
    // open/close path, and the open project's changes are read.
    gitFollowProject(slug: string | null) {
      this.changesError = "";
      if (this.commitMsgSlug !== slug) {
        this.commitMsg = "";
        this.commitMsgSlug = slug;
      }
      void this.$nextTick(() => {
        this.destroyChangesSub();
        this.mountChangesSub();
        if (this.$store.projects.openSlug) void this.loadChanges(this.$store.projects.openSlug);
        if (this.$store.projects.openSlug) void this.loadSync(this.$store.projects.openSlug);
      });
    },
    // On `workbench:head-moved` (the files' socket heard `head.dirty`): re-read
    // what the branch drives: the chip, the sync row and the Changes count.
    gitFollowHead(ref: string) {
      void this.loadChanges(ref);
      void this.loadSync(ref);
      // Under a selection the chip reads the worktree's branch from the
      // listing, not `p.branch`.
      if (this.$store.checkouts.checkoutOf(ref)) void this.$store.checkouts.ensureWorktreeListing(ref, true);
    },

    // --- the rows and the commit box -----------------------------------------
    // `all` is the group head's button, which acts on every row of the group.
    rowActTitle(verb: string, all = false) {
      const locked = this.$store.connection.writeLockReason();
      if (locked) return locked;
      if (verb === "stage") return all ? "Stage all changes" : "Stage changes";
      if (verb === "discard") return "Discard changes";
      return all ? "Unstage all changes" : "Unstage changes";
    },
    // Push's title (#320) states the run-lock reason, as `rowActTitle` does.
    pushTitle() {
      return (
        this.syncBusyTitle("push") ||
        this.$store.connection.writeLockReason() ||
        this.pushAct().title
      );
    },
    pushAct() {
      return WBChanges.pushAct(this.syncByProject[this.$store.projects.openKey()]);
    },
    pullBlocked() {
      return WBChanges.pullBlocked(this.syncByProject[this.$store.projects.openKey()]);
    },
    // The remote bar's title while an act is out: the busy act names itself,
    // the other two name what they are waiting on.
    syncBusyTitle(verb: string) {
      if (!this.syncBusy) return "";
      if (this.syncBusy !== verb) return `Waiting for the ${this.syncBusy} to finish`;
      return ({ fetch: "Fetching…", pull: "Pulling…", push: "Pushing…" } as Record<string, string>)[verb] || "";
    },
    groupNote(group: string) {
      return WBChanges.groupDiscardNote(group);
    },
    commitTarget() {
      return WBChanges.commitTarget(this.syncByProject[this.$store.projects.openKey()]);
    },
    // `withOriginal` only on the UNSTAGE direction — see `wb-changes.ts`.
    groupPaths(list: ChangeEntry[], withOriginal: boolean) {
      return WBChanges.groupPaths(list, withOriginal);
    },
    commitTitle() {
      const locked = this.$store.connection.writeLockReason();
      if (locked) return locked;
      if (!(this.changesStaged[this.$store.projects.openKey()] || []).length) {
        return "Stage a change first";
      }
      if (!this.commitMsg.trim()) return "Write a commit message first";
      return this.commitTarget().label;
    },
    canCommit() {
      return (
        !this.$store.connection.writeLocked() &&
        this.commitMsgSlug === this.$store.projects.openSlug &&
        !!this.commitMsg.trim() &&
        !!(this.changesStaged[this.$store.projects.openKey()] || []).length
      );
    },

    // Stage / unstage / commit, each in `syncFetch`'s shape, re-reading the
    // list on EVERY path. The list is never moved optimistically.
    async stagePaths(slug: string, paths: string[]) {
      if (!slug || !paths || !paths.length || this.$store.connection.writeLocked()) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.stage",
          window.WBDaemon.withCheckout({ repo: slug, paths }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not stage: the daemon gave no reason."),
          );
        }
      } catch {
        // A transport throw is NOT a refusal: the repo never answered.
        this._changesRefused("Could not stage: the daemon did not answer.");
      }
      void this.loadChanges(slug);
      void this.loadSync(slug);
    },

    async unstagePaths(slug: string, paths: string[]) {
      if (!slug || !paths || !paths.length || this.$store.connection.writeLocked()) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.unstage",
          window.WBDaemon.withCheckout({ repo: slug, paths }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not unstage: the daemon gave no reason."),
          );
        }
      } catch {
        this._changesRefused("Could not unstage: the daemon did not answer.");
      }
      void this.loadChanges(slug);
      void this.loadSync(slug);
    },

    // Discard ONE row's changes (#319) — the only irreversible act here, so the
    // only one confirmed (`discardConfirm`). A cancel makes NO daemon call.
    async discardRow(slug: string, entry: ChangeEntry) {
      if (!slug || !entry || !entry.path || this.$store.connection.writeLocked()) return;
      const c = WBChanges.discardConfirm(entry);
      const ok = await this.$store.modals.askConfirm({
        title: c.title,
        message: c.message,
        confirmLabel: c.confirmLabel,
        danger: true,
      });
      // A read may have failed while the dialog was open: a discard cannot
      // be undone, so the lock is asked again.
      if (!ok || this.$store.connection.writeLocked()) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.discard",
          window.WBDaemon.withCheckout({ repo: slug, paths: [entry.path] }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not discard: the daemon gave no reason."),
          );
        }
      } catch {
        this._changesRefused("Could not discard: the daemon did not answer.");
      }
      void this.loadChanges(slug);
      void this.loadSync(slug);
    },

    async commitStaged(slug: string) {
      // Never commit a draft composed for another project.
      if (this.commitMsgSlug !== slug || this.$store.connection.writeLocked()) return;
      const message = this.commitMsg.trim();
      if (!slug || !message) return;
      this.changesError = "";
      try {
        const reply = await window.WBDaemon.observe(
          "changes.commit",
          window.WBDaemon.withCheckout({ repo: slug, message }, this.$store.checkouts.checkoutOf(slug)),
        );
        if (WBFail.isError(reply)) {
          this._changesRefused(
            WBFail.failed(reply, "Could not commit: the daemon gave no reason."),
          );
        } else {
          // Cleared on success ONLY: a refused commit must not eat the message.
          this.commitMsg = "";
        }
      } catch {
        this._changesRefused("Could not commit: the daemon did not answer.");
      }
      void this.loadChanges(slug);
      void this.loadSync(slug);
    },

    // A refusal from the CHANGES panel lands in that panel and STAYS; the flash
    // is kept beside it. `runs-verb-error`'s counterpart (#331).
    _changesRefused(msg: string) {
      this.changesError = msg || "";
      this.$store.modals.flash(msg);
    },

    // The write lock (the Alpine store `connection`) hears whether the change
    // set and the branch shown for `slug` are current.
    _reportCurrent(slug: string) {
      this.$store.connection.changesCurrent(slug, !(this.changesRead[slug]?.current === false || this.syncRead[slug]?.current === false));
    },

    init() {
      // The open project changed (ADR-0073 amendment of 2026-10-08, decision 4).
      window.addEventListener("workbench:project-changed", (e) => this.gitFollowProject(e.detail.slug));
      // The open checkout's HEAD moved (the files' `/ws/tree` socket says so).
      window.addEventListener("workbench:head-moved", (e) => this.gitFollowHead(e.detail.ref));
      // The page asks for a read of one project's change set and branch: the
      // sidebar refresh, a branch act, a new or restored checkout.
      window.addEventListener("workbench:changes-read", (e) => {
        void this.loadChanges(e.detail.ref);
        void this.loadSync(e.detail.ref);
      });
      // A new checkout of `ref`: another tree's change set is not this tree's
      // "last value" (ADR-0070 D3), so its reads start over.
      window.addEventListener("workbench:changes-forget", (e) => {
        delete this.changesRead[e.detail.ref];
        delete this.syncRead[e.detail.ref];
        this._reportCurrent(e.detail.ref);
      });
      // Opening the Changes view is a read trigger.
      window.addEventListener("workbench:changes-refresh", () => this.refreshChanges());
      // A resume of the page's sockets (`resumeSockets`).
      window.addEventListener("workbench:sockets-resume", (e) => this._changesSub?.resume?.(e.detail.stale));
      // Registered once; the tick asks whether the panel is open.
      this._changesBackstop = setInterval(() => this.refreshChanges(), this.CHANGES_POLL_MS);
    },
  });
}

// A sync read of the primary tree of `slug` moves the branch and the head of
// its project row.
function primaryReadMovesRow(projects: ProjectsStore, slug: string, sync: Sync) {
  const p = projects.projects.find((x) => projects.repoRef(x) === slug);
  if (!p) return;
  const branch = WBChanges.headBranch(sync);
  if (branch !== null && p.branch !== branch) projects.setRowGit(p, { branch });
  const head = WBChanges.headOf(sync);
  if (head !== null) projects.setRowGit(p, { head });
}
