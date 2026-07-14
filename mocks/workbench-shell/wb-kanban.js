/* ---------------------------------------------------------------------------
   ralphy workbench shell — Kanban model (mock)

   The Kanban is the backlog as a board: every GitHub issue of the open project,
   placed in one of four columns by the SAME judgment ralphy uses. It is a
   read-only lens on the tracker — the daemon never edits an issue's prose. The
   ONE mutation the board allows is changing labels (which is how an issue moves
   between columns); everything else routes to GitHub via an "Open on GitHub"
   link for real editing.

   Columns (an issue lands in exactly one, precedence top-down):
     • Closed          — state = closed, grouped by close reason (completed /
                         not planned), mirroring GitHub's `stateReason`.
     • Ready for human — carries `ready-for-human` (HITL): parked on a person
                         (ADR-0014/0016). Human gate outranks agent-eligibility,
                         exactly as the runner's queue precedence does.
     • Ready for agent — carries `ready-for-agent` OR `AFK` (same intent: fully
                         specified, an AFK agent can pick it up).
     • Backlog         — everything else still open (needs-triage, needs-split,
                         bug/enhancement, or no label at all).

   Ordering:
     • The two Ready columns are ordered by the dependency graph — a JS port of
       `sort_queue_in_graph` (crates/ralphy-core/src/blocked.rs): Kahn's
       algorithm over `## Blocked by` edges, ascending issue number as the
       tie-break, blockers walked transparently through open out-of-queue nodes,
       closed blockers pruned (satisfied), a retired bundle's `## Parent`
       children standing in for it. The order shown IS the order the runner would
       execute — the board and the queue can never disagree.
     • Backlog stays in issue order (newest-first by default) with a search box,
       a label filter, and a sort control — a flat, compact list.

   Running signal: an issue that is the *active* node of a live run (see
   wb-runs.js / WB_RUNS) carries a run pill on its card — the agent's face + the
   live status glyph + the phase — so "what's executing right now" reads at a
   glance, in whichever column the issue sits.

   This file holds:
     • WB_KANBAN  — the seed (a backend replaces it live from the tracker),
     • WBKanban   — pure helpers (column classification, graph order, running
                    cross-ref, label metadata, filter/sort).
   Faithful sources: labels + colors = the repo's `gh label list`; close reasons
   = GitHub `stateReason`; graph order = ralphy-core/src/blocked.rs; run glyphs =
   window.WBRun (wb-runs.js).
--------------------------------------------------------------------------- */

window.WBKanban = {
  // GitHub label vocabulary → { color, short }. Colors are the repo's real label
  // hex (gh label list); `short` is a compact chip label where the full name is
  // long. Unknown labels fall back to a neutral chip.
  LABELS: {
    "ready-for-agent": { color: "#0E8A16", short: "ready · agent" },
    "ready-for-human": { color: "#5319E7", short: "ready · human" },
    AFK: { color: "#34A985", short: "AFK" },
    "needs-triage": { color: "#FBCA04", short: "needs-triage" },
    "needs-split": { color: "#D93F0B", short: "needs-split" },
    "needs-info": { color: "#D93F0B", short: "needs-info" },
    "triage-agent": { color: "#7F0A04", short: "triage-agent" },
    bug: { color: "#D73A4A", short: "bug" },
    enhancement: { color: "#A2EEEF", short: "enhancement" },
    documentation: { color: "#0075CA", short: "docs" },
    question: { color: "#D876E3", short: "question" },
    duplicate: { color: "#CFD3D7", short: "duplicate" },
    wontfix: { color: "#E8E2D9", short: "wontfix" },
    invalid: { color: "#E4E669", short: "invalid" },
    "good first issue": { color: "#7057FF", short: "good first issue" },
    "help wanted": { color: "#008672", short: "help wanted" },
  },
  // The four columns, in render order (left → right), with their heading + icon.
  COLUMNS: [
    { id: "backlog", title: "Backlog", lucide: "inbox" },
    { id: "agent", title: "Ready for agent", lucide: "bot" },
    { id: "human", title: "Ready for human", lucide: "user-round" },
    { id: "closed", title: "Closed", lucide: "check-check" },
  ],

  labelMeta(name) {
    return this.LABELS[name] || { color: "#6b5f52", short: name };
  },
  labelColor(name) {
    return this.labelMeta(name).color;
  },
  // A chip's text color: dark ink on a light chip, light ink on a dark one, so a
  // wontfix-white and a triage-maroon both stay legible.
  labelInk(name) {
    const hex = this.labelColor(name).replace("#", "");
    const r = parseInt(hex.slice(0, 2), 16),
      g = parseInt(hex.slice(2, 4), 16),
      b = parseInt(hex.slice(4, 6), 16);
    // perceived luminance
    return 0.299 * r + 0.587 * g + 0.114 * b > 150 ? "#1b1714" : "#f2ede4";
  },

  // Which column an issue belongs to — the runner's precedence, as a lens:
  // closed first, then the human gate, then agent-eligibility, else backlog.
  columnOf(iss) {
    if (iss.state === "closed") return "closed";
    const L = iss.labels || [];
    if (L.includes("ready-for-human")) return "human";
    if (L.includes("ready-for-agent") || L.includes("AFK")) return "agent";
    return "backlog";
  },

  // GitHub's close reason as a short badge label.
  closeLabel(iss) {
    if (iss.state !== "closed") return "";
    return iss.reason === "not_planned" ? "not planned" : "completed";
  },

  // The one open blocker check the board needs: is `#n` still open in this
  // project? (Mirrors the runtime gate — a closed blocker is satisfied.)
  openBlockers(iss, all) {
    const openSet = new Set(all.filter((i) => i.state === "open").map((i) => i.number));
    return (iss.blockedBy || []).filter((n) => openSet.has(n));
  },

  // --- graph order (port of sort_queue_in_graph / Kahn) -----------------
  // Order `queue` so every issue comes after the queue members it depends on,
  // ascending number as the tie-break. `all` is the project's full issue set
  // (for walking edges through open out-of-queue blockers, pruning closed ones,
  // and substituting a retired bundle's `## Parent` children).
  orderGraph(queue, all) {
    const inQueue = new Set(queue.map((i) => i.number));
    const openNums = new Set(all.filter((i) => i.state === "open").map((i) => i.number));
    const blockedOf = new Map();
    for (const i of all) blockedOf.set(i.number, i.blockedBy || []);
    for (const i of queue) if (!blockedOf.has(i.number)) blockedOf.set(i.number, i.blockedBy || []);
    // childrenOf[parent] = queue members declaring `## Parent` #parent (stand-ins
    // for a retired/closed bundle).
    const childrenOf = new Map();
    for (const i of queue) {
      if (i.parent == null) continue;
      if (!childrenOf.has(i.parent)) childrenOf.set(i.parent, []);
      childrenOf.get(i.parent).push(i.number);
    }
    // deps[x] = queue members that must precede x.
    const deps = new Map();
    for (const i of queue) {
      const acc = new Set();
      const seen = new Set();
      const stack = [i.number];
      while (stack.length) {
        const node = stack.pop();
        if (seen.has(node)) continue; // already expanded — also breaks cycles
        seen.add(node);
        for (const n of blockedOf.get(node) || []) {
          if (n === i.number) continue;
          if (inQueue.has(n)) acc.add(n); // terminal in-queue predecessor
          else if (openNums.has(n)) stack.push(n); // transparent: keep walking
          else {
            const ch = childrenOf.get(n); // closed bundle → its children stand in
            if (ch) for (const c of ch) if (c !== i.number) acc.add(c);
          }
        }
      }
      deps.set(i.number, acc);
    }
    // Kahn, smallest ready number first (ascending tie-break); a cycle's
    // remainder is appended ascending (the runtime gate owns correctness).
    const byNum = new Map(queue.map((i) => [i.number, i]));
    const placed = new Set();
    const out = [];
    while (byNum.size) {
      const keys = [...byNum.keys()].sort((a, b) => a - b);
      const ready = keys.find((n) => [...(deps.get(n) || [])].every((d) => placed.has(d)));
      if (ready == null) {
        for (const n of keys) out.push(byNum.get(n));
        break;
      }
      placed.add(ready);
      out.push(byNum.get(ready));
      byNum.delete(ready);
    }
    return out;
  },

  // --- running cross-ref (against WB_RUNS via window.WBRun) --------------
  // If `number` is the *active* node of one of the project's live runs, return a
  // descriptor for the card's run pill; else null. Only the actively-worked
  // issue (planning / executing / sleeping) is flagged — a run's pending or
  // already-terminal members don't clutter the board.
  runningFor(number, projectRuns) {
    for (const r of projectRuns || []) {
      const iss = (r.issues || []).find((x) => x.number === number);
      if (!iss) continue;
      const st = window.WBRun.issueState(r, iss);
      if (st === "planning" || st === "executing" || st === "sleep") {
        return { runid: r.runid, face: r.face, agent: r.agent, state: st, glyph: window.WBRun.GLYPH[st], phase: r.phase };
      }
    }
    return null;
  },

  // --- filter / sort (Backlog) ------------------------------------------
  matches(iss, q) {
    if (!q) return true;
    const s = q.trim().toLowerCase();
    if (!s) return true;
    return (
      String(iss.number).includes(s) ||
      (iss.title || "").toLowerCase().includes(s) ||
      (iss.body || "").toLowerCase().includes(s) ||
      (iss.labels || []).some((l) => l.toLowerCase().includes(s))
    );
  },
  hasLabelFilter(iss, label) {
    if (label === "__all") return true;
    if (label === "__none") return (iss.labels || []).length === 0;
    return (iss.labels || []).includes(label);
  },
  SORTS: [
    { id: "num-desc", label: "newest" },
    { id: "num-asc", label: "oldest" },
    { id: "updated", label: "recently updated" },
    { id: "title", label: "title A–Z" },
  ],
  sortBacklog(list, sort) {
    const a = list.slice();
    switch (sort) {
      case "num-asc":
        return a.sort((x, y) => x.number - y.number);
      case "updated":
        return a.sort((x, y) => (y.updated || "").localeCompare(x.updated || ""));
      case "title":
        return a.sort((x, y) => (x.title || "").localeCompare(y.title || ""));
      case "num-desc":
      default:
        return a.sort((x, y) => y.number - x.number);
    }
  },

  fmtDate(iso) {
    if (!iso) return "";
    const d = new Date(iso);
    if (isNaN(d)) return iso;
    return d.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" });
  },
};

// A tiny markdown builder so seed bodies can hold code fences (backticks) and
// ${...} without template-literal escaping — plain double-quoted strings joined
// by newlines.
const md = (...lines) => lines.join("\n");

// Seed: issues keyed by project slug — a realistic backlog per project, faithful
// to ralphy's issue style (## What to build / ## Blocked by / ## Acceptance,
// decision-lock and handoff comments). Numbers overlap the run seed (wb-runs.js)
// where a run exists, so the "running" pill lights up on the active issue.
//
// Assignee scope (business rule): the real board shows only issues with EMPTY
// assignees OR whose assignee matches the configured `queue.assignee` (ADR-0021,
// same knob as `ralphy run --assignee`) — anything assigned to someone else is
// hidden. This seed is deliberately left UNFILTERED (small demo set); the rule is
// documented in BUILD-GUIDE.md and applied by the backend, not here.
window.WB_KANBAN = {
  ralphy: [
    {
      number: 10,
      title: "xterm v6: bump + webgl addon",
      state: "closed",
      reason: "completed",
      labels: ["AFK", "ready-for-agent"],
      assignees: [],
      created: "2026-06-28T09:12:00Z",
      updated: "2026-07-02T18:40:00Z",
      blockedBy: [],
      body: md(
        "## What to build",
        "Bump `xterm` to v6 and mount the `@xterm/addon-webgl` renderer behind a capability check, falling back to the canvas renderer where WebGL is unavailable.",
        "",
        "## Acceptance criteria",
        "- [x] `@xterm/xterm@^6` + `@xterm/addon-webgl` pinned; `npm run build` exits 0.",
        "- [x] WebGL addon loads when the context is available, canvas fallback otherwise (unit-tested behind a stub).",
      ),
      comments: [
        {
          author: "paulocorcino",
          at: "2026-07-02T18:40:00Z",
          body: md("## Verify (Ralphy run 20260702-101122)", "```", "✓ npm run build   exit 0", "✓ npm run test    exit 0", "```", "Gate green on the committed state — closing."),
        },
      ],
    },
    {
      number: 11,
      title: "xterm v6: sessão multiplexada (bundle)",
      state: "open",
      reason: null,
      labels: ["needs-split"],
      assignees: [],
      created: "2026-06-29T11:00:00Z",
      updated: "2026-07-05T08:10:00Z",
      blockedBy: [],
      body: md(
        "## What to build",
        "Multiplexar várias sessões PTY sobre uma única conexão WebSocket, com um seletor de sessão na barra da aba, detach/reattach e scrollback por sessão.",
        "",
        "> Provavelmente três tarefas independentes num número só (codec do protocolo, UI do seletor, persistência de scrollback) — candidato a split.",
      ),
      comments: [
        {
          author: "paulocorcino",
          at: "2026-07-05T08:10:00Z",
          body: md(
            "Ralphy run 20260705-060157 skipped this issue — the planner judged it a **bundle**.",
            "",
            "## Planner reasoning",
            "Packs the tunnel codec, the tab-strip session picker, and per-session scrollback persistence into one number. Park on `needs-split` until it is broken into children.",
          ),
        },
      ],
    },
    {
      number: 12,
      title: "xterm v6: reflow no resize (decisão de UX)",
      state: "open",
      reason: null,
      labels: ["ready-for-human"],
      assignees: ["paulocorcino"],
      created: "2026-06-30T14:22:00Z",
      updated: "2026-07-06T09:00:00Z",
      blockedBy: [],
      body: md(
        "## What to build",
        "Nothing yet — this is a human decision. On resize, xterm can hard-reflow (rewrap the scrollback) or clip. Reflow is correct but janky on WebGL at large scrollbacks; clipping is smooth but drops wrapped lines.",
        "",
        "Decide the default (reflow vs clip, and whether it's a per-session setting) before an agent implements.",
      ),
      comments: [],
    },
    {
      number: 13,
      title: "xterm v6: cores do tema",
      state: "open",
      reason: null,
      labels: ["AFK", "ready-for-agent"],
      assignees: [],
      created: "2026-07-01T10:05:00Z",
      updated: "2026-07-12T07:30:00Z",
      blockedBy: [10],
      body: md(
        "## What to build",
        "Map the ADR-0035 warm-dark palette onto xterm's 16-color + ANSI-bright set, cursor, and selection, exposed as an `ITheme` object.",
        "",
        "## Blocked by",
        "- #10",
        "",
        "## Acceptance criteria",
        "- [ ] `ITheme` derives the 16 colors + cursor/selection from the ADR-0035 tokens.",
        "- [ ] A snapshot test pins the applied theme.",
      ),
      comments: [
        { author: "paulocorcino", at: "2026-07-11T22:00:00Z", body: "**Decision locked for AFK execution:** derive bright variants by a fixed lightness bump (not hand-picked) so the set stays consistent. Ship the `ITheme` + snapshot; no palette redesign in this issue." },
      ],
    },
    {
      number: 14,
      title: "xterm v6: cleanup + dispose no unmount",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-07-01T10:20:00Z",
      updated: "2026-07-10T12:00:00Z",
      blockedBy: [],
      body: md(
        "## What to build",
        "Dispose the terminal, the WebGL addon, and the resize observer on unmount; no leaked GL contexts across a project switch (the workbench mounts/destroys terminals often).",
        "",
        "## Acceptance criteria",
        "- [ ] `terminal.dispose()` + addon dispose on teardown; no console warnings after 20 mount/unmount cycles.",
      ),
      comments: [],
    },
    {
      number: 20,
      title: "xterm v6: copy-on-select + bracketed paste",
      state: "open",
      reason: null,
      labels: ["AFK", "ready-for-agent"],
      assignees: [],
      created: "2026-07-02T09:00:00Z",
      updated: "2026-07-09T16:30:00Z",
      blockedBy: [14],
      body: md(
        "## What to build",
        "Wire copy-on-select and bracketed-paste guards, matching the daemon's console affordances.",
        "",
        "## Blocked by",
        "- #14",
      ),
      comments: [],
    },
    {
      number: 21,
      title: "xterm v6: e2e — render + resize under Playwright",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-07-02T09:30:00Z",
      updated: "2026-07-09T16:35:00Z",
      blockedBy: [20, 13],
      body: md(
        "## What to build",
        "A Playwright smoke: open a session, type, resize, assert no reflow jank and stable scrollback.",
        "",
        "## Blocked by",
        "- #20",
        "- #13",
      ),
      comments: [],
    },
    {
      number: 15,
      title: "Console flushes ANSI mid-escape on fast output",
      state: "open",
      reason: null,
      labels: ["bug"],
      assignees: [],
      created: "2026-07-04T20:11:00Z",
      updated: "2026-07-08T10:00:00Z",
      blockedBy: [],
      body: md(
        "## What happens",
        "Under a firehose of output (a noisy `cargo build`), the console occasionally renders a raw `\\x1b[` fragment — a write is flushed mid-escape-sequence instead of being buffered until the sequence completes.",
        "",
        "## Expected",
        "Escape sequences are never split across writes; the parser buffers a partial CSI until it terminates.",
      ),
      comments: [],
    },
    {
      number: 16,
      title: "Surface the daemon uptime as a real heartbeat age",
      state: "open",
      reason: null,
      labels: ["enhancement"],
      assignees: [],
      created: "2026-07-05T08:00:00Z",
      updated: "2026-07-05T08:00:00Z",
      blockedBy: [],
      body: md("## What to build", "The topbar shows a hard-coded `up 2h 14m`. Derive it from the WS heartbeat's first-seen timestamp so it's live."),
      comments: [],
    },
    {
      number: 17,
      title: "Persist the last-open project across daemon restarts",
      state: "open",
      reason: null,
      labels: [],
      assignees: [],
      created: "2026-07-06T13:40:00Z",
      updated: "2026-07-06T13:40:00Z",
      blockedBy: [],
      body: md("## What to build", "Remember which project was open (and its expanded tree paths) so a reconnect restores the workspace instead of dropping to the empty state."),
      comments: [],
    },
    {
      number: 18,
      title: "diagnosis.json occasionally written with a trailing comma",
      state: "open",
      reason: null,
      labels: ["needs-info"],
      assignees: [],
      created: "2026-07-07T09:15:00Z",
      updated: "2026-07-07T09:15:00Z",
      blockedBy: [],
      body: md("## What happens", "A `.ralphy/diagnosis.json` failed to parse once with a trailing comma. Not yet reproducible — need the offending file and the agent/version that wrote it before this can be triaged."),
      comments: [],
    },
    {
      number: 19,
      title: "Decision: should the workbench allow multiple projects open at once?",
      state: "open",
      reason: null,
      labels: ["ready-for-human"],
      assignees: ["paulocorcino"],
      created: "2026-07-08T11:00:00Z",
      updated: "2026-07-08T11:00:00Z",
      blockedBy: [],
      body: md("## Decide", "Today only one project opens at a time (accordion). Multi-open would allow side-by-side trees + consoles but complicates the tree mount lifecycle and the runs/kanban scoping. Human call before any agent work."),
      comments: [],
    },
    {
      number: 22,
      title: "Rewrite the tab strip with a virtualized list",
      state: "closed",
      reason: "not_planned",
      labels: ["wontfix"],
      assignees: [],
      created: "2026-06-25T15:00:00Z",
      updated: "2026-06-27T09:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Virtualize the tab strip for hundreds of open tabs.", "", "> Closed not-planned: real workflows open a handful of tabs; virtualization is complexity without a user. Revisit only if a real session hits the limit."),
      comments: [{ author: "paulocorcino", at: "2026-06-27T09:00:00Z", body: "Not planned — no evidence anyone opens enough tabs to matter. Keeping the simple strip." }],
    },
  ],

  fincal: [
    {
      number: 71,
      title: "Walking skeleton: casca deployável ponta a ponta",
      state: "open",
      reason: null,
      labels: ["AFK", "ready-for-agent"],
      assignees: [],
      created: "2026-07-01T08:00:00Z",
      updated: "2026-07-12T07:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Scaffold Next.js App Router + Prisma + SQLite + Auth.js, render the UI shell behind a protected route, single Dockerfile. Machine-verifiable via lint + render tests + `prisma migrate deploy` + `next build` + `docker build`."),
      comments: [{ author: "paulocorcino", at: "2026-07-11T20:00:00Z", body: "**Decision locked:** Auth.js v5 infra wired now (config + adapter models + `/login`), NOT full auth UX (that's slice 02)." }],
    },
    {
      number: 72,
      title: "Auth: registro + login + seed de categorias",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-07-01T08:05:00Z",
      updated: "2026-07-02T09:00:00Z",
      blockedBy: [71],
      body: md("## What to build", "Fluxo de registro/login sobre a infra do #71, com seed das categorias padrão no primeiro acesso.", "", "## Blocked by", "- #71"),
      comments: [],
    },
    {
      number: 73,
      title: "Contas: CRUD + saldo",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-07-01T08:06:00Z",
      updated: "2026-07-02T09:00:00Z",
      blockedBy: [72],
      body: md("## What to build", "CRUD de contas com saldo derivado dos lançamentos.", "", "## Blocked by", "- #72"),
      comments: [],
    },
    {
      number: 85,
      title: "Importador OFX aceita datas em dois formatos incompatíveis",
      state: "open",
      reason: null,
      labels: ["bug", "needs-triage"],
      assignees: [],
      created: "2026-07-08T10:00:00Z",
      updated: "2026-07-08T10:00:00Z",
      blockedBy: [],
      body: md("## What happens", "Alguns extratos OFX trazem `YYYYMMDD`, outros `YYYYMMDDHHMMSS`. O parser assume o primeiro e trunca o segundo, jogando lançamentos para a meia-noite errada."),
      comments: [],
    },
    {
      number: 86,
      title: "Sinalizar categorias sem cor com um placeholder acessível",
      state: "open",
      reason: null,
      labels: ["enhancement"],
      assignees: [],
      created: "2026-07-09T11:00:00Z",
      updated: "2026-07-09T11:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Categorias sem cor definida caem num cinza indistinguível. Mostrar um placeholder com contraste AA e um ícone de 'sem cor'."),
      comments: [],
    },
    {
      number: 87,
      title: "Definir a política de arredondamento dos relatórios",
      state: "open",
      reason: null,
      labels: ["ready-for-human"],
      assignees: ["paulocorcino"],
      created: "2026-07-09T12:00:00Z",
      updated: "2026-07-09T12:00:00Z",
      blockedBy: [],
      body: md("## Decide", "Arredondar por linha ou só no total? Afeta a conferência contra o extrato. Decisão humana antes de implementar."),
      comments: [],
    },
    {
      number: 88,
      title: "PRD: agenda de recorrências",
      state: "open",
      reason: null,
      labels: ["needs-triage"],
      assignees: [],
      created: "2026-07-10T09:00:00Z",
      updated: "2026-07-10T09:00:00Z",
      blockedBy: [],
      body: md("## Context", "Rascunho do PRD para a agenda de lançamentos recorrentes. Ainda não fatiado em issues acionáveis."),
      comments: [],
    },
    {
      number: 90,
      title: "Reconcile: parser OFX",
      state: "closed",
      reason: "completed",
      labels: ["AFK", "ready-for-agent"],
      assignees: [],
      created: "2026-06-20T08:00:00Z",
      updated: "2026-06-28T10:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Parser OFX → transações normalizadas para a conciliação."),
      comments: [],
    },
    {
      number: 92,
      title: "Reconcile: UI de conferência",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-06-22T08:00:00Z",
      updated: "2026-07-12T06:00:00Z",
      blockedBy: [90],
      body: md("## What to build", "Tabela lado a lado extrato × lançamentos com ações aceitar/rejeitar.", "", "## Blocked by", "- #90"),
      comments: [{ author: "paulocorcino", at: "2026-07-10T00:00:00Z", body: "Run parked on a provider usage-limit reset — resumes automatically (ADR-0003)." }],
    },
    {
      number: 60,
      title: "Espécie de import duplicado quando o CSV tem BOM",
      state: "closed",
      reason: "completed",
      labels: ["bug"],
      assignees: [],
      created: "2026-06-15T08:00:00Z",
      updated: "2026-06-19T09:00:00Z",
      blockedBy: [],
      body: md("## What happened", "Um BOM no início do CSV fazia a primeira coluna não casar com o header, duplicando a importação. Corrigido normalizando o BOM na leitura."),
      comments: [],
    },
  ],

  lingopilot: [
    {
      number: 40,
      title: "Streaming: SSE transport",
      state: "closed",
      reason: "completed",
      labels: ["AFK", "ready-for-agent"],
      assignees: [],
      created: "2026-06-18T08:00:00Z",
      updated: "2026-06-25T10:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Transporte SSE entregando deltas de token do provider ao cliente."),
      comments: [],
    },
    {
      number: 43,
      title: "Streaming: UI incremental",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-06-20T08:00:00Z",
      updated: "2026-07-12T07:00:00Z",
      blockedBy: [40],
      body: md("## What to build", "Render incremental dos deltas sem layout shift, com botão de parar.", "", "## Blocked by", "- #40"),
      comments: [],
    },
    {
      number: 44,
      title: "Streaming: retry com backoff",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-06-20T08:10:00Z",
      updated: "2026-06-21T09:00:00Z",
      blockedBy: [43],
      body: md("## What to build", "Retry com backoff exponencial quando o stream cai no meio.", "", "## Blocked by", "- #43"),
      comments: [],
    },
    {
      number: 41,
      title: "Streaming: token counter",
      state: "closed",
      reason: "not_planned",
      labels: ["enhancement"],
      assignees: [],
      created: "2026-06-19T08:00:00Z",
      updated: "2026-06-24T09:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Contador de tokens ao vivo no streaming.", "", "> Infeasible como especificado — o provider não expõe contagem incremental confiável; fechado não-planejado até haver um sinal utilizável."),
      comments: [{ author: "paulocorcino", at: "2026-06-24T09:00:00Z", body: "Planner returned 0 steps — sem fonte confiável de contagem incremental. Fechando não-planejado." }],
    },
    {
      number: 46,
      title: "Glossário: destacar termos traduzidos inconsistentemente",
      state: "open",
      reason: null,
      labels: ["needs-triage"],
      assignees: [],
      created: "2026-07-05T08:00:00Z",
      updated: "2026-07-05T08:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Sinalizar quando o mesmo termo-fonte foi traduzido de formas diferentes ao longo de um documento."),
      comments: [],
    },
    {
      number: 47,
      title: "Escolher o provider de detecção de idioma padrão",
      state: "open",
      reason: null,
      labels: ["ready-for-human"],
      assignees: [],
      created: "2026-07-06T08:00:00Z",
      updated: "2026-07-06T08:00:00Z",
      blockedBy: [],
      body: md("## Decide", "On-device (browser LanguageDetector) vs. servidor. Trade-off de privacidade × cobertura de idiomas. Decisão humana."),
      comments: [],
    },
    {
      number: 48,
      title: "Layout quebra em RTL no painel de comparação",
      state: "open",
      reason: null,
      labels: ["bug"],
      assignees: [],
      created: "2026-07-07T08:00:00Z",
      updated: "2026-07-07T08:00:00Z",
      blockedBy: [],
      body: md("## What happens", "Com um idioma-alvo RTL (árabe/hebraico), o painel de comparação não espelha, colando as colunas."),
      comments: [],
    },
  ],

  bioledger: [
    {
      number: 3,
      title: "OCR: tolerar páginas rotacionadas",
      state: "open",
      reason: null,
      labels: ["ready-for-agent"],
      assignees: [],
      created: "2026-07-01T08:00:00Z",
      updated: "2026-07-03T08:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Detectar e corrigir a rotação da página antes do OCR (o corpus tem scans a 90°/180°)."),
      comments: [],
    },
    {
      number: 4,
      title: "Reduzir o custo do ocr.test.mjs (verify gate ~198s)",
      state: "open",
      reason: null,
      labels: ["AFK", "ready-for-agent"],
      assignees: [],
      created: "2026-07-02T08:00:00Z",
      updated: "2026-07-04T08:00:00Z",
      blockedBy: [],
      body: md("## What to build", "O `ocr.test.mjs` domina o gate. Amostrar o corpus / cachear o passo caro para trazer o verify para baixo de 60s sem perder cobertura."),
      comments: [],
    },
    {
      number: 5,
      title: "Definir a fonte-verdade do dicionário de espécies",
      state: "open",
      reason: null,
      labels: ["ready-for-human"],
      assignees: [],
      created: "2026-07-03T08:00:00Z",
      updated: "2026-07-03T08:00:00Z",
      blockedBy: [],
      body: md("## Decide", "Manter o dicionário local versionado ou puxar de uma base externa? Afeta reprodutibilidade offline. Decisão humana."),
      comments: [],
    },
    {
      number: 6,
      title: "notes.md: normalizar a grafia dos nomes de coletor",
      state: "open",
      reason: null,
      labels: [],
      assignees: [],
      created: "2026-07-04T08:00:00Z",
      updated: "2026-07-04T08:00:00Z",
      blockedBy: [],
      body: md("## What to build", "Normalizar variações de grafia dos nomes de coletor para uma forma canônica no ledger."),
      comments: [],
    },
  ],
};
