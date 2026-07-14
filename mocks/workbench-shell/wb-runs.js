/* ---------------------------------------------------------------------------
   ralphy workbench shell — Runs model (mock)

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
  // terminal per-issue statuses (state.rs:29-40) — these won't change further.
  TERMINAL: new Set(["done", "skipped", "blocked", "infeasible", "needs_split", "non_green", "hitl"]),

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
  // The body under one `## Heading` (heading line excluded), up to the next `##`.
  section(md, name) {
    if (!md || !name) return "";
    const want = name.trim().toLowerCase();
    const out = [];
    let inSec = false;
    for (const ln of md.split("\n")) {
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
  // Steps render more robustly as glyphs than as (sanitiser-fragile) checkboxes.
  stepsToGlyphs(body) {
    return body
      .replace(/^(\s*)-\s+\[x\]/gim, "$1- ✅")
      .replace(/^(\s*)-\s+\[ \]/gim, "$1- ⬜");
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
};

// Wake anchors for the seeded sleep state (relative to load time so the live
// countdown reads sensibly).
const _in = (mins) => Math.floor(Date.now() / 1000) + mins * 60;

// Seed: runs keyed by project slug. `planEl` points at a hidden <script> in
// index.html holding that run's plan.md (kept out of JS so backticks/${} in the
// markdown need no escaping); app.js hydrates `planMd` from it at init.
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
