/* ---------------------------------------------------------------------------
   ralphy workbench shell — Runs model

   The Runs panel shows what's *running* in ralphy for the open project. Its data
   is exactly what a backend would fold from the CloudEvents bus (ADR-0019): one
   entry per `runid` (the correlation key), carrying the ordered issue queue with
   per-issue status, the live phase, and the current issue's `plan.md`.

   A project can host more than one concurrent run (two `ralphy run` processes),
   so the panel offers a run picker. Below it, the issue *trail* (`#71 — #72 …`)
   renders each issue with its status glyph; below that, the plan viewer shows the
   fixed `## Steps` block plus a dropdown to read any other `##` section.

   This file holds:
     • WB_RUNS       — the seed (a backend replaces it live),
     • WBRun         — pure helpers (status → glyph/label, plan slicing, sleep),
   both faithful to ralphy's real vocabulary:
     - IssueStatus (crates/ralphy-cli/src/runstate/state.rs): planning, executing,
       done, skipped, blocked, infeasible, needs_split, non_green, hitl.
     - plan steps are `- [ ]`/`- [x]` checkboxes (open/checked).
     - "sleep" = a usage-limit reset wait (run.sleep_started/ended; target_epoch
       is the wake anchor), a run-level phase that overlays the active issue.
   Glyphs are the union of the Telegram sink + terminal presenter tables.
--------------------------------------------------------------------------- */

window.WBRun = {
  // per-status glyph, matching notifier.rs status_emoji + render.rs scroll glyphs.
  // `sleep`/`pending` are panel-only overlays (a sleeping active issue, and a
  // not-yet-started issue).
  GLYPH: {
    planning: "🧠",
    executing: "⚙️",
    // a plan-only pass superseded by the next `issue started` (a dry run):
    // terminal, and it reaches the panel via status_wire (state.rs).
    planned: "📝",
    done: "✅",
    skipped: "⏭️",
    blocked: "⛔",
    infeasible: "🤷",
    needs_split: "🧩",
    non_green: "❌",
    hitl: "🙋",
    sleep: "🌙",
    pending: "○",
  },
  LABEL: {
    planning: "planning",
    executing: "executing",
    planned: "planned only",
    done: "done",
    skipped: "skipped",
    blocked: "blocked",
    infeasible: "infeasible",
    needs_split: "needs split",
    non_green: "non-green",
    hitl: "waiting on human",
    sleep: "usage limit — sleeping",
    pending: "pending",
  },
  // per-step vocabulary (#330). The document ships `plan.steps[].status` as a
  // string in the ADR-0019 `plan.step` vocabulary; an unknown one falls back to
  // `open` rather than vanishing. Pinned from Rust by
  // `runstate::snapshot::tests::every_step_status_is_known_to_the_runs_panel`.
  STEP_GLYPH: {
    open: "⬜",
    checked: "✅",
    noticed: "⚠️",
  },
  STEP_LABEL: {
    open: "open",
    checked: "done",
    noticed: "noticed a problem",
  },
  // `Object.hasOwn`, never `in`/truthiness: a status of "toString" would
  // otherwise reach Object.prototype and defeat the fallback entirely.
  stepKey(status) {
    return Object.hasOwn(this.STEP_GLYPH, status) ? status : "open";
  },
  stepGlyph(status) {
    return this.STEP_GLYPH[this.stepKey(status)];
  },
  stepLabel(status) {
    return this.STEP_LABEL[this.stepKey(status)];
  },
  stepClass(status) {
    return "st-" + this.stepKey(status);
  },
  // Parse the three checkbox markers out of plan text — the same set the Rust
  // side parses (`ralphy_core::plan::count_open_steps` for the open ones). Used by
  // the `file://` demo's seed AND by `planSummary` over a plan read off disk.
  //
  // `/\r?\n/`, never `"\n"`: a plan.md written on Windows (or checked out with
  // `core.autocrlf`) leaves a trailing `\r` on every line, and `(.*)$` below
  // cannot match it — `.` excludes line terminators and `$` only matches the very
  // end — so EVERY step silently disappeared and the plan read as "0 steps", i.e.
  // infeasible. Measured on this host: a plan the operator could see in the editor
  // came back with `steps: 0`.
  parseSteps(md) {
    const out = [];
    (md || "").split(/\r?\n/).forEach((ln) => {
      const m = ln.replace(/^\s+/, "").match(/^- \[([ xX!])\](.*)$/);
      if (!m) return;
      const status = m[1] === " " ? "open" : m[1] === "!" ? "noticed" : "checked";
      out.push({ text: m[2].replace(/[*_`]/g, "").trim().replace(/\s+/g, " "), status });
    });
    return out;
  },

  // terminal per-issue statuses — these won't change further. MUST mirror
  // IssueStatus::is_terminal (crates/ralphy-cli/src/runstate/state.rs); every
  // name here is a `status_wire` string the run snapshot ships verbatim.
  TERMINAL: new Set(["planned", "done", "skipped", "blocked", "infeasible", "needs_split", "non_green", "hitl"]),

  // The visual state of one issue *within its run*: a terminal status as-is; the
  // active issue reflects the live phase (planning/executing, or sleep when the
  // whole run is parked on a usage limit); everything else is pending.
  issueState(run, iss) {
    // tolerate a null run / iss: the :class binding can re-run mid-transition
    // (project switch) after the run is gone but before the node unmounts.
    if (!run || !iss) return "pending";
    if (this.TERMINAL.has(iss.status)) return iss.status;
    if (iss.number === run.active) {
      if (run.phase === "sleeping") return "sleep";
      return iss.status === "planning" ? "planning" : "executing";
    }
    return "pending";
  },
  glyph(run, iss) {
    return this.GLYPH[this.issueState(run, iss)] || "○";
  },

  runPhaseLabel(run) {
    switch (run.phase) {
      case "starting":
        return "starting…";
      case "planning":
        return "planning #" + run.active;
      case "executing":
        return "executing #" + run.active;
      case "sleeping":
        return "sleeping · usage limit";
      case "consolidating":
        return "consolidating knowledge";
      default:
        return run.phase || "idle";
    }
  },

  // --- what is running, and for how long ---------------------------------
  // The model/effort segment, a mirror of `ui::render::model_effort_seg` — the
  // CLI's console has printed `model / effort` since ADR-0006 and this is the
  // same fact on the same run, so it must read the same way. An absent effort is
  // simply omitted; an absent model yields "" and the CALLER degrades.
  modelEffort(model, effort) {
    const m = (model || "").trim();
    const e = (effort || "").trim();
    if (!m) return "";
    return e ? `${m} / ${e}` : m;
  },
  // The issue the run is on. The document carries model/effort/budget PER ISSUE
  // (tier routing means #350 and #351 legitimately run different models), so
  // every render fact about "what is running now" is read off this one entry.
  activeIssue(run) {
    if (!run || run.active == null) return null;
    return (run.issues || []).find((i) => i.number === run.active) || null;
  },
  // The picker's headline: WHICH MODEL is running, falling back to the vendor.
  // The fallback is the honesty rule, not a convenience — a queued issue carries
  // `model: null`, and so does a run before its first phase event. Naming the
  // vendor there is a true statement; inventing a model would not be.
  runTitle(run) {
    if (!run) return "";
    return this.modelEffort(this.activeIssue(run)?.model, this.activeIssue(run)?.effort) || run.agent || "";
  },
  // The line under it: the vendor is not lost when the title takes the model,
  // it MOVES here, in front of the phase it is driving.
  runIdentity(run) {
    if (!run) return "";
    return [run.agent, this.runPhaseLabel(run)].filter(Boolean).join(" · ");
  },
  // `M:SS`, minutes unbounded — a verbatim mirror of `ui::render::fmt_clock`
  // (`72:05`, not `1:12:05`). Same unit as the budget it is compared against,
  // and the same string the console prints for the same phase.
  fmtClock(ms) {
    const secs = Math.floor(Math.max(0, ms || 0) / 1000);
    return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;
  },
  // How long the CURRENT phase has been running, from the document's own anchor
  // (`phase.since`, stamped by the snapshot writer on every phase change).
  //
  // The clock rule mirrors `render_active_line`: bare elapsed while planning,
  // `elapsed / budget` while executing under a real cap, and NEVER a `/ 0:00`
  // ceiling — a `budget_min` of 0 is a disabled cap, not a zero-minute one.
  //
  // `nowMs` is passed in rather than read here so the whole thing stays pure and
  // the caller owns the tick. A missing anchor renders "" (an older document, or
  // a run already in flight when this build shipped) — never a fabricated 0:00.
  phaseClock(run, nowMs) {
    if (!run?.since) return "";
    const since = Date.parse(run.since);
    if (Number.isNaN(since)) return "";
    // Clamped: the anchor is the RUN HOST's clock and this is the browser's, so
    // a few seconds of skew must read as 0:00, never as a negative clock.
    const elapsed = this.fmtClock(Math.max(0, (nowMs || 0) - since));
    const budget = run.phase === "executing" ? this.activeIssue(run)?.budgetMin : null;
    return budget > 0 ? `${elapsed} / ${this.fmtClock(budget * 60 * 1000)}` : elapsed;
  },

  // --- plan.md section slicing ------------------------------------------
  // Every `## Heading` in the plan, in order (e.g. "Feasible: yes", "Steps"…).
  headings(md) {
    const out = [];
    (md || "").split("\n").forEach((ln) => {
      const m = ln.match(/^##\s+(.+?)\s*$/);
      if (m) out.push(m[1]);
    });
    return out;
  },
  // --- whose plan is this? ------------------------------------------------
  // The plan file carries its own issue key: the planner writes the trailer
  // `<!-- ralphy-plan: issue=N -->` once every section is complete
  // (crates/ralphy-adapter-support/src/resume.rs → `plan_trailer`). The panel
  // needs it because the steps block is keyed by `plan.issue` from the snapshot
  // (ADR-0047 A1) while the PROSE is a `file.read` of `.ralphy/plan.md` — and
  // between issues that file still holds the previous issue's plan.
  //
  // NOT the Rust rule, deliberately: `plan_is_finalized_for` requires the
  // trailer to be the last non-empty line, because it answers "may I resume?".
  // This answers "which issue is this prose about?", and the executor appends
  // `## Notes & decisions` / `## Handoff` AFTER the trailer while it works — so
  // presence anywhere, not position. Do not "fix" this into the resume rule: the
  // prose would vanish the moment execution wrote its first note.
  PLAN_TRAILER_RE: /<!--\s*ralphy-plan:\s*issue=(\d+)\s*-->/g,
  planTrailerIssue(md) {
    if (!md) return null;
    let issue = null;
    // The LAST trailer wins: an appended section can quote an earlier plan, and
    // the planner's own trailer is written at the end of what it wrote.
    for (const m of md.matchAll(this.PLAN_TRAILER_RE)) issue = Number(m[1]);
    return Number.isFinite(issue) ? issue : null;
  },
  // Is this plan text the plan FOR `issue`? A plan with no trailer is not: it is
  // either mid-write (the planner has not finished) or not a ralphy plan at all,
  // and in both cases claiming it belongs to the active issue is the lie.
  planBelongsTo(md, issue) {
    if (issue == null) return false;
    return this.planTrailerIssue(md) === Number(issue);
  },

  // The body under one `## Heading` (heading line excluded), up to the next `##`.
  section(md, name) {
    if (!md || !name) return "";
    const want = name.trim().toLowerCase();
    const out = [];
    let inSec = false;
    // `/\r?\n/` for the same reason as `parseSteps`: a CRLF plan.md would
    // otherwise return a body with a carriage return on every line.
    for (const ln of md.split(/\r?\n/)) {
      const m = ln.match(/^##\s+(.+?)\s*$/);
      if (m) {
        const isTarget = m[1].trim().toLowerCase() === want;
        if (inSec && !isTarget) break;
        inSec = isTarget;
        continue; // drop the heading line; the panel chrome shows it
      }
      if (inSec) out.push(ln);
    }
    return out.join("\n").trim();
  },
  // --- the plan's verdict (the board's plan pill, #350-follow-up) ------------
  // A finalized plan is EXECUTED BY THE NEXT RUN (the trailer above is the resume
  // signal), so the board has to be able to say one exists, show it, and throw it
  // away. This fold is what the pill and the modal render.
  //
  // Every judgment below MIRRORS the Rust that actually decides, and must keep
  // mirroring it:
  //   • infeasible  ⇐ ZERO open `- [ ]` steps (ralphy-core `plan::count_open_steps`,
  //     read by runner/phases.rs — the `## Feasible:` heading is the human's
  //     reason, never the decision).
  //   • reason      ⇐ the body under `^##\s+Feasible\b.*$` (core `handoff::infeasible_reason`;
  //     the heading carries the verdict, so any tail after "Feasible" is accepted).
  //   • needsSplit  ⇐ that reason containing the literal word "bundle"
  //     (core `handoff::is_bundle_reason`, which the planning prompt requires).
  FEASIBLE_HEAD: /^##\s+Feasible\b.*$/im,
  // The `## Feasible` heading verbatim ("Feasible: no"), for the modal's banner.
  feasibleHeading(md) {
    return (md || "").match(this.FEASIBLE_HEAD)?.[0].replace(/^##\s+/, "").trim() || "";
  },
  // The planner's stated reason: the prose under that heading.
  feasibleReason(md) {
    const head = this.feasibleHeading(md);
    return head ? this.section(md, head) : "";
  },
  isBundleReason(reason) {
    return (reason || "").toLowerCase().includes("bundle");
  },
  // One plan.md → what the board needs to know about it. `issue` is null for prose
  // carrying no trailer, and the caller MUST treat that as "no plan for anyone":
  // an unfinished plan is not yet a plan (see `planBelongsTo`).
  planSummary(md) {
    const text = md || "";
    const steps = this.parseSteps(text);
    const openSteps = steps.filter((s) => s.status === "open").length;
    const reason = this.feasibleReason(text);
    return {
      issue: this.planTrailerIssue(text),
      heading: this.feasibleHeading(text),
      reason,
      steps: steps.length,
      openSteps,
      // A plan with no work left to do is the planner's refusal, whatever its
      // heading says — that is the same test the runner applies.
      infeasible: openSteps === 0,
      needsSplit: openSteps === 0 && this.isBundleReason(reason),
    };
  },
  // The pill's words. `open` is whether the ISSUE is still open: a plan left over
  // from a closed issue is not "ready", it is residue, and saying so is the
  // difference between an invitation and a warning.
  planPillLabel(summary, open = true) {
    if (!summary) return "";
    if (!open) return "leftover plan";
    if (summary.needsSplit) return "needs split";
    if (summary.infeasible) return "not feasible";
    return "plan ready";
  },
  // Whether the pill is a warning rather than an invitation — the one thing the
  // operator must not have to open a modal to notice.
  planPillWarns(summary, open = true) {
    return !!summary && (!open || summary.infeasible);
  },

  // A human sleep line from the wake anchor: "waiting for reset ~20:15 · resumes
  // in ~2h 3m" (mirrors notifier.rs sleep formatting).
  sleepText(sleep) {
    if (!sleep) return "waiting for reset";
    const rem = Math.max(0, (sleep.target_epoch || 0) - Math.floor(Date.now() / 1000));
    const h = Math.floor(rem / 3600);
    const m = Math.floor((rem % 3600) / 60);
    const eta = h > 0 ? `~${h}h ${m}m` : `~${m}m`;
    const at =
      sleep.reset ||
      new Date((sleep.target_epoch || 0) * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    return `waiting for reset ~${at} · resumes in ${eta}`;
  },

  // --- the run-snapshot wire shape (ADR-0047) ---------------------------
  // The daemon's `runs.list` answers with the snapshot documents verbatim. The
  // panel's run shape predates them (it was seeded), so one mapper bridges the
  // two — kept here, beside the vocabulary it speaks, rather than in app.js.

  // A run's avatar. The document carries no face (it is the panel's chrome, not
  // the run's state), so it is derived from the runid — deterministic, so a run
  // keeps its face across every re-hydration and across a page reload.
  FACES: ["🦊", "🐼", "🦉", "🐙", "🐸", "🦝", "🐻", "🐨", "🦄", "🐝", "🐧", "🦋"],
  face(runid) {
    let h = 0;
    for (const ch of String(runid || "")) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    return this.FACES[h % this.FACES.length];
  },

  // One `runs.list` document → the panel's run shape. `planMd` starts empty:
  // the document carries the plan's PATH, and the panel reads it through the
  // confined `file.read` verb (ADR-0047 §5).
  fromSnapshot(doc) {
    const issues = (doc.issues || []).map((i) => ({
      number: i.number,
      title: i.title || "",
      status: i.status,
      blockedBy: i.blocked_by || [],
      // The render facts, carried per issue. They have been on the wire since the
      // fold's `Planning`/`Executing` arms; this mapper used to drop them, which
      // is why the picker could only ever name the vendor. `?? null`, not `||`:
      // an empty model string is a fact the console renders, not an absence.
      model: i.model ?? null,
      effort: i.effort ?? null,
      budgetMin: i.budget_min ?? null,
    }));
    return {
      runid: doc.runid,
      face: this.face(doc.runid),
      agent: doc.exec_agent || "",
      branch: doc.branch || "",
      base: "",
      phase: doc.phase?.state || "starting",
      active: doc.phase?.active ?? null,
      // When the current phase began (ADR-0047 amendment): the anchor the panel's
      // clock counts from. Absent in a document written by an older build, and
      // `phaseClock` renders nothing rather than guessing.
      since: doc.phase?.since || "",
      startedAt: doc.started_at || "",
      completed: issues.filter((i) => this.TERMINAL.has(i.status)).length,
      // `??`, not `||`: a real total of 0 must stay 0, not be replaced.
      queueTotal: doc.queue?.total ?? issues.length,
      sleep: doc.phase?.sleep || null,
      planPath: doc.plan_path || "",
      planMd: "",
      // Steps come from the DOCUMENT, not from a plan.md read: they survive a
      // deleted/unreadable plan and are already accumulated when the panel opens.
      steps: (doc.plan?.steps || []).map((s) => ({ text: s.text || "", status: s.status || "open" })),
      planIssue: doc.plan?.issue ?? null,
      planReadFailed: false,
      issues,
    };
  },

  // --- verb chrome (#331) -------------------------------------------------
  // What a run verb's plain description is when nothing holds the lock. The
  // gate is a HINT (the CLI is the authority), so these stay the same strings
  // the enabled buttons have always carried.
  VERB_TITLE: {
    run: "start a run — choose agent & branch",
    triage: "triage the backlog — label + plan open issues (if idle)",
    push: "push the queue snapshot to the events sink",
  },
  // A disabled control that does not say why is just a control that stopped
  // working; the lock reason REPLACES the description rather than appending to
  // it, so the title answers the only question a dimmed button raises.
  // `hasOwn`, not a bare lookup: `verbLockTitle("constructor", "")` would
  // otherwise hand back Object's constructor instead of a string.
  verbLockTitle(verb, reason) {
    if (reason) return reason;
    return Object.hasOwn(this.VERB_TITLE, verb)
      ? this.VERB_TITLE[verb]
      : `${verb} on this project`;
  },
  // The panel's rendering of a terminal verb frame. Empty for a clean exit —
  // this is the whole guard against a success raising a refusal banner, so it
  // is the case the unit test pins first.
  // The last line is truncated: it is whatever the CLI happened to print, and
  // an unbounded string here becomes an unbounded box in the panel.
  EXIT_NOTE_TAIL: 200,
  exitNote(verb, code, lastLine) {
    if (code === 0) return "";
    const shown = code === null || code === undefined ? "unknown" : code;
    const note = `${verb} refused (exit ${shown})`;
    if (!lastLine) return note;
    const tail =
      lastLine.length > this.EXIT_NOTE_TAIL
        ? `${lastLine.slice(0, this.EXIT_NOTE_TAIL)}…`
        : lastLine;
    return `${note} — ${tail}`;
  },
};

// Wake anchors for the seeded sleep state (relative to load time so the live
// countdown reads sensibly).
const _in = (mins) => Math.floor(Date.now() / 1000) + mins * 60;

// Seed: runs keyed by project slug, reachable ONLY from the static `file://`
// demo (#300 — `initRuns` drops it in daemon mode, where the panel reads live
// snapshots). `planEl` points at a hidden `seed-plan-*` <script> in index.html
// holding that run's plan.md (kept out of JS so backticks/${} in the markdown
// need no escaping); app.js hydrates `planMd` from it at init.
window.WB_RUNS = {
  fincal: [
    {
      runid: "01JR-FIN-A",
      face: "🦊",
      agent: "opencode",
      branch: "feat/opencode",
      base: "main",
      phase: "executing",
      active: 71,
      completed: 0,
      queueTotal: 14,
      sleep: null,
      planEl: "seed-plan-fincal-71",
      issues: [
        { number: 71, title: "Walking skeleton: casca deployável ponta a ponta", status: "executing" },
        { number: 72, title: "Auth: registro + login + seed de categorias", status: "pending" },
        { number: 73, title: "Contas: CRUD + saldo", status: "pending" },
        { number: 74, title: "Categorias: árvore + cores", status: "pending" },
        { number: 75, title: "Lançamentos: entrada rápida", status: "pending" },
        { number: 76, title: "Agenda: recorrências", status: "pending" },
        { number: 77, title: "Dashboard: cards + gráfico", status: "pending" },
        { number: 78, title: "Importação assistida: CSV", status: "pending" },
        { number: 79, title: "Diagnóstico: regras", status: "pending" },
        { number: 80, title: "Exportação", status: "pending" },
        { number: 81, title: "Filtros salvos", status: "pending" },
        { number: 82, title: "Tema + acessibilidade", status: "pending" },
        { number: 83, title: "Telemetria mínima", status: "pending" },
        { number: 84, title: "Hardening + docs", status: "pending" },
      ],
    },
    {
      runid: "01JR-FIN-B",
      face: "🐼",
      agent: "claude",
      branch: "feat/reconcile",
      base: "main",
      phase: "sleeping",
      active: 92,
      completed: 2,
      queueTotal: 6,
      sleep: { reset: null, target_epoch: _in(131) },
      planEl: "seed-plan-b",
      issues: [
        { number: 90, title: "Reconcile: parser OFX", status: "done" },
        { number: 91, title: "Reconcile: matching heurístico", status: "skipped", blockedBy: [200] },
        { number: 92, title: "Reconcile: UI de conferência", status: "executing" },
        { number: 93, title: "Reconcile: desfazer", status: "pending" },
        { number: 94, title: "Reconcile: relatório", status: "pending" },
        { number: 95, title: "Reconcile: testes e2e", status: "pending" },
      ],
    },
  ],
  ralphy: [
    {
      runid: "01JR-RLP-C",
      face: "🦉",
      agent: "codex",
      branch: "feat/xterm-v6-webgl",
      base: "main",
      phase: "executing",
      active: 13,
      completed: 3,
      queueTotal: 5,
      sleep: null,
      planEl: "seed-plan-c",
      issues: [
        { number: 10, title: "xterm v6: bump + webgl addon", status: "done" },
        { number: 11, title: "xterm v6: sessão multiplexada", status: "needs_split" },
        { number: 12, title: "xterm v6: reflow no resize", status: "hitl" },
        { number: 13, title: "xterm v6: cores do tema", status: "executing" },
        { number: 14, title: "xterm v6: cleanup", status: "pending" },
      ],
    },
  ],
  lingopilot: [
    {
      runid: "01JR-LNG-D",
      face: "🐙",
      agent: "claude",
      branch: "feat/chat-streaming",
      base: "main",
      phase: "planning",
      active: 43,
      completed: 3,
      queueTotal: 6,
      sleep: null,
      planEl: "seed-plan-d",
      issues: [
        { number: 40, title: "Streaming: SSE transport", status: "done" },
        { number: 41, title: "Streaming: token counter", status: "infeasible" },
        { number: 42, title: "Streaming: cancelamento", status: "blocked" },
        { number: 43, title: "Streaming: UI incremental", status: "planning" },
        { number: 44, title: "Streaming: retry", status: "pending" },
        { number: 45, title: "Streaming: testes", status: "pending" },
      ],
    },
  ],
};
