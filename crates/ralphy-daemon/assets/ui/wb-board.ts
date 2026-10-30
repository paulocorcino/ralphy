/* ---------------------------------------------------------------------------
   The board of the open project: an Alpine component on the element that
   holds the board (`section.kanban`, with its issue view and label menu) and
   the plan modal (#621; ADR-0073 D2).

   It reads the board's issues and labels and gives each reply to the Alpine
   store `board` (wb-board-store.ts), which owns them and their read state.
   It owns the filters, the issue view, the label menu with its lock, and the ready
   plan of the next run. The runs are the Alpine store `runs`
   (wb-runs-store.ts): the board only reads them.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4),
   and the page reaches it only through `workbench:*` events, which `init()`
   hears.

   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component, emptyRecord } from "./wb-alpine.ts";
import { WBChanges } from "./wb-changes.ts";
import { WBFail } from "./wb-fail.ts";
import { WBProject } from "./wb-project.ts";
import { WBRun } from "./wb-runs.ts";
import type { BoardLabel } from "./wb-kanban.ts";
import type { BoardIssue, BoardRow, ReadyPlan, RunPill, Timer } from "./wb-shell-types.d.ts";

export function wbBoard() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component(["kanbanOpen", "toggleKanban"], {
    // The slow refresh backstop of the board, started in `init`.
    _boardBackstop: null as Timer,
    // Counts the `issue.show` reads; only the newest one is kept.
    _issueDetailGen: null as number | null,

    // --- Kanban board -----------------------------------------------------
    // The open project's issues in four columns (window.WBKanban). Read-only
    // except labels, the one mutation that moves a card.
    KANBAN: window.WBKanban,
    // Refresh bookkeeping (#301). `_boardLoadedAt` is stamped at fold START,
    // before any await, so the min-gap measures spacing between STARTS and an
    // erroring board throttles like a healthy one. `_boardPending` COALESCES a
    // trigger that arrived mid-fold into one follow-up load.
    _boardLoadedAt: 0,
    kanbanSel: null as number | null, // the selected issue number → opens the detail drawer

    // --- the repo's ready plan, on the board -------------------------------
    _boardPending: false,
    boardRefreshing: false,
    // The daemon awaits the board CLI with no timeout of its own; a wedged `gh`
    // must not disable the board for the page's life. Generous: a real fold
    // makes several calls.
    BOARD_FOLD_TIMEOUT_MS: 90000,
    kanbanFilter: "", // search box (title / #num / body / label)
    kanbanLabel: "__all", // label filter: __all | __none | <label>
    kanbanSort: "num-desc", // Backlog sort (Ready columns keep graph order)
    // A FINALIZED `.ralphy/plan.md` is executed by the next run (the trailer is
    // the resume signal, `WBRun.planTrailerIssue`), so the board shows it and
    // can throw it away. `planByProject[slug] = { md, summary }`, replaced on
    // every board load via the same confined `file.read`. Daemon-only (#300).
    planByProject: emptyRecord<ReadyPlan | null>(),
    planModal: { open: false, issue: null as number | null },

    async loadPlan(slug: string | null) {
      if (!slug) return;
      try {
        const reply = await window.WBDaemon.observe("file.read", {
          repo: slug,
          path: ".ralphy/plan.md",
        });
        // A refusal is the ORDINARY case (no plan sitting around): it clears
        // the entry rather than raising an error.
        const md = reply?.status === "ok" ? reply.content || "" : "";
        this.planByProject[slug] = md ? { md, summary: WBRun.planSummary(md) } : null;
      } catch {
        this.planByProject[slug] = null;
      }
    },
    // The open project's plan, or null. No trailer (`summary.issue` null) is a
    // plan still being written: not offered.
    openPlan() {
      const held = this.planByProject[this.$store.projects.openKey()];
      return held && held.summary.issue != null ? held : null;
    },
    // The plan for ONE card: only ever shown against the issue it names.
    planFor(number: number) {
      const held = this.openPlan();
      return held && held.summary.issue === number ? held : null;
    },
    // A plan left over from a closed issue is residue, and the pill says so.
    planIssueIsOpen() {
      const held = this.openPlan();
      if (!held) return true;
      const iss = this.projectIssues().find((i) => i.number === held.summary.issue);
      // Absent from the fold (filtered or cold): assume open.
      return !iss || iss.state !== "closed";
    },
    planPillLabel(number: number) {
      const held = this.planFor(number);
      return held ? WBRun.planPillLabel(held.summary, this.planIssueIsOpen()) : "";
    },
    planPillWarns(number: number) {
      const held = this.planFor(number);
      return !!held && WBRun.planPillWarns(held.summary, this.planIssueIsOpen());
    },
    // The head chip: for a plan whose issue is filtered out of the board or
    // absent from the fold, which would otherwise be invisible AND
    // undiscardable.
    planChipLabel() {
      const held = this.openPlan();
      if (!held) return "";
      return `#${held.summary.issue} · ${WBRun.planPillLabel(held.summary, this.planIssueIsOpen())}`;
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
        this.$store.connection.writeLockReason() ||
        "Delete this plan. The next run plans this issue again."
      );
    },
    // `plan.discard` carries no path (the daemon fixes the target). Gated
    // while a run holds the repo: a run owns the plan it is executing.
    async discardPlan() {
      const held = this.openPlan();
      if (!held || this.$store.connection.writeLocked()) return;
      const ok = await this.$store.modals.askConfirm({
        title: "Discard this plan?",
        message: `Deletes the plan for #${held.summary.issue}. The next run plans it again.`,
        confirmLabel: "Discard",
        danger: true,
      });
      if (!ok) return;
      const slug = this.$store.projects.openSlug;
      try {
        const reply = await window.WBDaemon.write("plan.discard", { repo: slug });
        if (WBFail.isError(reply)) {
          this.$store.modals.flash(WBFail.failed(reply, "Could not discard the plan."));
          return;
        }
        this.$store.modals.flash(`Plan for #${held.summary.issue} discarded.`);
        this.closePlanModal();
      } catch {
        this.$store.modals.flash("Could not discard the plan: the daemon is not connected.");
      } finally {
        // Re-read on EVERY path: the panel shows what is on disk now.
        await this.loadPlan(slug);
      }
    },

    // The open project's issues (#198). Empty until `loadBoard()` populates it.
    projectIssues() {
      return this.$store.board.openIssues();
    },

    // The whole-tracker board fold via `board.list`, cached under the slug. No
    // daemon or a transport error leaves the board empty.
    async loadBoard() {
      const slug = this.$store.projects.openSlug;
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
      void this.loadPlan(this.$store.projects.openSlug);
      try {
        const reply = await boardFold(slug, this.BOARD_FOLD_TIMEOUT_MS);
        if (WBFail.isError(reply)) {
          const msg = WBFail.failed(reply, "Could not load the board.");
          this.$store.board.boardReadFailed(slug, msg);
          this.$store.modals.flash(msg);
          return;
        }
        const fold = readFold(reply.board, (r) => this.boardRowToIssue(r));
        this.$store.board.boardReadOk(slug, fold.issues, fold.colors);
        // Fold rows carry `body: ""`: re-merge the open drawer's detail or it
        // goes blank on every refresh.
        if (this.kanbanSel != null) void this.loadIssueDetail(this.kanbanSel);
      } catch {
        this.$store.board.boardReadFailed(slug, "Could not load the board.");
        this.$store.modals.flash("Could not load the board.");
      } finally {
        this.boardRefreshing = false;
        // Exactly ONE follow-up for whatever was coalesced away, or for a
        // project that changed underneath this fold. `_boardPending` is cleared
        // by the recursive call before it awaits, so this settles.
        if (this._boardPending || this.$store.projects.openSlug !== slug) {
          this._boardPending = false;
          if (this.$store.projects.openSlug && this.kanbanOpen) void this.loadBoard();
        }
      }
    },

    // The one door every refresh trigger goes through (#301): the predicate
    // (wb-kanban.ts) decides.
    maybeRefreshBoard(trigger: string) {
      const ok = window.WBKanban.shouldRefresh({
        trigger,
        sinceMs: Date.now() - this._boardLoadedAt,
        boardOpen: this.kanbanOpen,
        docVisible: document.visibilityState === "visible",
        focused: document.hasFocus(),
      });
      if (ok) void this.loadBoard();
    },
    // The board head's refresh control.
    refreshBoard() {
      this.maybeRefreshBoard("manual");
    },
    // The slow backstop: the PREDICATE, not the timer, decides.
    boardBackstopTick() {
      this.maybeRefreshBoard("backstop");
    },

    // A CLI fold row → the issue shape `wb-kanban.ts` expects. Body + comments
    // are absent from the fold (`issue.show` fills them on open).
    boardRowToIssue(row: BoardRow) {
      return {
        number: row.number,
        title: row.title || "",
        state: row.state || "open",
        reason: row.reason ?? row.state_reason ?? null,
        labels: row.labels || [],
        assignees: row.assignees || [],
        blockedBy: rowBlockers(row),
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
      const shown = all.filter(
        (i) => K.matches(i, this.kanbanFilter) && K.hasLabelFilter(i, this.kanbanLabel),
      );
      const bucket: Record<ReturnType<typeof K.columnOf>, BoardIssue[]> = {
        backlog: [],
        agent: [],
        human: [],
        closed: [],
      };
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
    kanbanCount(colId: string) {
      const columns: Record<string, BoardIssue[]> = this.kanbanColumns();
      return columns[colId]?.length ?? 0;
    },
    // The label set present in the project, for the filter dropdown.
    kanbanLabelOptions() {
      const seen = new Set<string>();
      for (const i of this.projectIssues()) for (const l of i.labels || []) seen.add(l);
      // Distinct members, so no tie; `<` compares UTF-16 code units, as the sort without a compare function does.
      return [...seen].sort((a, b) => (a < b ? -1 : 1));
    },

    // The run pill for a card (the actively-worked issue of a live run).
    issueRunning(number: number) {
      return window.WBKanban.runningFor(number, this.$store.runs.projectRuns());
    },
    // The issue drawer's run line: `Running · executing (claude)`.
    issueRunningLabel(run: RunPill) {
      return `Running · ${this.runStateWord(run?.state)} (${run?.agent || ""})`;
    },
    // A run state as the Runs panel words it (`sleep` → "usage limit — sleeping").
    runStateWord(state: string | undefined) {
      return (state && WBRun?.LABEL?.[state]) || state || "";
    },

    // Thin delegations to the faithful helpers (used in the template).
    kanbanColumnOf(i: BoardIssue) {
      return window.WBKanban.columnOf(i);
    },
    labelColor(l: string) {
      // The repo's real label hex, else the seed vocabulary.
      return (
        this.$store.board.boardLabels[this.$store.projects.openKey()]?.get(l) ||
        window.WBKanban.labelColor(l)
      );
    },
    labelInk(l: string) {
      return window.WBKanban.labelInk(l);
    },
    labelShort(l: string) {
      return window.WBKanban.labelMeta(l).short;
    },
    closeLabel(i: BoardIssue) {
      return window.WBKanban.closeLabel(i);
    },
    kanbanColumnTitle(i: BoardIssue) {
      const id = window.WBKanban.columnOf(i);
      return (window.WBKanban.COLUMNS.find((c) => c.id === id) || {}).title || id;
    },
    kfmtDate(iso: string | null | undefined) {
      return window.WBKanban.fmtDate(iso);
    },

    // --- detail drawer ----------------------------------------------------
    // The open drawer's detail-fetch failure (#302). One string: exactly one
    // drawer is open at a time.
    issueError: null as string | null,
    issueLoading: false,
    // Selection is by number, so a label move (which can change the card's
    // column) keeps the drawer pointed at the same issue.
    selectedIssue() {
      if (this.kanbanSel == null) return null;
      return this.projectIssues().find((i) => i.number === this.kanbanSel) || null;
    },
    openIssue(number: number) {
      this.kanbanSel = number;
      // `issue.show` merges body + comments + blockers into the cached row.
      void this.loadIssueDetail(number);
    },

    async loadIssueDetail(number: number) {
      const slug = this.$store.projects.openSlug;
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
        gen !== this._issueDetailGen ||
        this.$store.projects.openSlug !== slug ||
        this.kanbanSel !== number;
      const fail = (msg: string) => {
        if (stale()) return;
        this.issueError = msg;
        this.$store.modals.flash(msg);
      };
      try {
        const reply = await window.WBDaemon.observe("issue.show", { repo: slug, number });
        if (WBFail.isError(reply)) {
          fail(WBFail.failed(reply, "Could not load the issue."));
          return;
        }
        const detail = shownIssue(reply);
        if (!detail) {
          fail("Could not load the issue.");
          return;
        }
        if (stale() || !slug) return;
        if (!this.$store.board.issueDetailRead(slug, number, detail)) return;
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
    githubUrl(number: number) {
      const p = this.$store.projects.projects.find(
        (x) => this.$store.projects.repoRef(x) === this.$store.projects.openSlug,
      );
      return WBProject.issueUrl(p && p.remoteUrl, number);
    },

    // The selected issue's blockers, each with its live open/closed state.
    issueBlockers(iss: BoardIssue | null) {
      if (!iss || !iss.blockedBy?.length) return [];
      const all = this.projectIssues();
      return iss.blockedBy.map((n) => {
        const b = all.find((x) => x.number === n);
        return {
          number: n,
          open: b ? b.state === "open" : false,
          known: !!b,
          title: b?.title || "",
        };
      });
    },

    // An issue body / comment as sanitized markdown.
    renderIssueMd(src: string | null | undefined) {
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
      void this.$nextTick(() =>
        document
          .querySelector(".kd-label-menu")
          ?.scrollIntoView({ block: "nearest", inline: "nearest" }),
      );
    },
    hasLabel(iss: BoardIssue | null, label: string) {
      return !!iss && (iss.labels || []).includes(label);
    },
    toggleLabel(iss: BoardIssue | null, label: string) {
      if (!iss) return;
      // Defence in depth: a `:disabled` button is still reachable by keyboard
      // in some browsers.
      if (this.labelsLocked()) return;
      const has = this.hasLabel(iss, label);
      const op = has ? "remove" : "add";
      const prev = [...(iss.labels || [])];
      const slug = this.$store.projects.openSlug;
      if (!slug) return;
      this.$store.board.issueLabelsSet(
        slug,
        iss.number,
        has ? prev.filter((l) => l !== label) : [...prev, label],
      );
      window.WB.emit("issue-label-change", { project: slug, number: iss.number, label, op });
      // The run-lock-aware `label.set` Mutate (#199): refusal → revert + flash.
      void (async () => {
        try {
          const reply = await window.WBDaemon.observe("label.set", {
            repo: slug,
            number: iss.number,
            label,
            op,
          });
          if (WBFail.isError(reply)) {
            this.$store.board.issueLabelsSet(slug, iss.number, prev);
            this.$store.modals.flash(
              WBFail.failed(reply, "Could not change the labels: the daemon gave no reason."),
            );
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

    // The board's label editor, under the SAME lock: `label set` is a
    // run-lock-aware Mutate (mutate.rs).
    labelsLocked() {
      return !!this.labelLockReason();
    },
    labelLockReason() {
      if (this.$store.connection.buildSkew) return this.$store.connection.BUILD_SKEW_LOCK;
      if (this.$store.board.boardRead[this.$store.projects.openKey()]?.current === false) {
        return "The board shown is not current. Wait for the next read.";
      }
      return WBChanges.writeLockReason(
        this.$store.runs.runsByProject[this.$store.projects.openKey()],
        "You can edit labels again when it finishes.",
      );
    },

    // On `workbench:project-changed`. The drawer selection of the project that
    // WAS open is dropped. The Runs panel (wb-runs-panel.ts) and the Alpine
    // store `runs` hear the same event.
    boardFollowProject() {
      this.kanbanSel = null;
      void this.$nextTick(() => {
        // Only when the board is OPEN (#301): the fold spawns a tracker CLI.
        if (this.$store.projects.openSlug && this.kanbanOpen) void this.loadBoard();
      });
    },

    init() {
      // The open project changed (ADR-0073 amendment of 2026-10-08, decision 4).
      window.addEventListener("workbench:project-changed", () => this.boardFollowProject());
      // A refresh trigger of the page (#301): the runs socket, a reopened
      // socket, the tab becoming visible, a login. The predicate decides.
      window.addEventListener("workbench:board-refresh", (e) =>
        this.maybeRefreshBoard(e.detail.trigger),
      );
      // The board opened (`toggleKanban`): lazy-load the tracker for the open
      // project.
      window.addEventListener("workbench:board-opened", () => {
        this.kanbanSel = null;
        void this.loadBoard();
      });
      // A trail node of the Runs panel (#301) opens that issue's detail.
      window.addEventListener("workbench:issue-open", (e) => this.openIssue(e.detail.issue));
      // Anchor the clock at page load: `_boardLoadedAt` at 0 would clear the
      // 120s floor on the first tick.
      this._boardLoadedAt = Date.now();
      this._boardBackstop = setInterval(() => this.boardBackstopTick(), 30000);
    },
  });
}

// The `board.list` reply for the repo `slug`, or a rejection after `timeoutMs`.
function boardFold(slug: string, timeoutMs: number) {
  return Promise.race([
    window.WBDaemon.observe("board.list", { repo: slug }),
    new Promise<never>((_, rej) =>
      setTimeout(() => rej(new Error("board fold timed out")), timeoutMs),
    ),
  ]);
}

// The issues of a board fold, in the shape that `toIssue` makes, and the
// colors of its labels.
function readFold<T>(board: DaemonReplies["board.list"]["board"], toIssue: (row: BoardRow) => T) {
  const b = board || {};
  return { issues: (b.issues || []).map(toIssue), colors: labelColors(b.labels) };
}

// The repo's label colors by label name. A Map, so a label named
// `constructor` reads as missing. A blank color is skipped: a bare "#" is
// truthy and masks `labelColor`'s fallback.
function labelColors(labels: BoardLabel[] | undefined) {
  const colors = new Map<string, string>();
  for (const l of labels || []) {
    if (!l.color) continue;
    colors.set(l.name, "#" + String(l.color).replace(/^#/, ""));
  }
  return colors;
}

// The issues that block a fold row, under either name the fold uses.
function rowBlockers(row: BoardRow) {
  return row.blocked_by || row.blockedBy || [];
}

// The issue of an `issue.show` reply, or null when the reply carries none.
function shownIssue(reply: ReplyOf<"issue.show">) {
  if (!reply || reply.status !== "ok" || !reply.issue || typeof reply.issue !== "object")
    return null;
  return reply.issue;
}
