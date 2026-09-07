// Demo seed: the Runs panel — fabricated runs and the `plan.md` each one is
// working through.
//
// Lives OUTSIDE `assets/ui` on purpose: `lib.rs` embeds that whole directory via
// `include_dir!`, so a daemon would carry and serve every byte of this. Here it
// is loaded only by the `file://` demo (index.html gates the tag on
// `WBMode.isDemo()`), which makes the #300 / ADR-0036 invariant structural:
// synthetic data is not merely inert in daemon mode, it is absent.
//
// The plans are arrays of lines rather than template literals because the
// markdown is full of backticks and `${}`. That is the same reason they used to
// be inline `<script type="text/markdown">` blocks in index.html; joining lines
// keeps them escape-free AND diffable one line at a time.

const PLAN_FINCAL_71 = [
  "# Plan for #71: Walking skeleton: casca deployável ponta a ponta",
  "",
  "## Feasible: yes",
  "Greenfield repo. The spec is concrete: scaffold Next.js App Router + Prisma + SQLite + Auth.js, render the UI shell (sidebar with 6 destinations + topbar) behind a protected route, single `Dockerfile`. Machine-verifiable via lint + render tests + `prisma migrate deploy` + `next build` + `docker build`.",
  "",
  "## Done when",
  "- `npx prisma migrate deploy` exits 0 applying the init migration on a fresh `dev.db`.",
  "- `npm run test` passes `sidebar.test.tsx` (6 destinations in `ui-ux.md` order) and `empty-state.test.tsx`.",
  "- `npm run build` exits 0 end to end; `docker build -t fincal-skeleton .` exits 0.",
  "- `.env.example` carries the four placeholders `DATABASE_URL`, `AUTH_SECRET`, `OPENAI_API_KEY`, `OPENAI_MODEL`.",
  "",
  "## Verify",
  "```",
  "npm run lint",
  "npm run test",
  "npx prisma migrate deploy",
  "npm run build",
  "docker build -t fincal-skeleton .",
  "```",
  "",
  "## Decisions",
  "- Auth.js v5 infra wired now (config + Prisma adapter models + proxy + minimal `/login`), NOT full auth UX (that is slice 02).",
  "- \"schema base\" = Auth.js adapter models only (`User`, `Account`, `Session`, `VerificationToken`); domain entities arrive later.",
  "- Scaffold via `create-next-app` into a temp dir, then merge into repo root preserving `docs/`, `.env`, `CONTEXT.md`.",
  "",
  "## Caveats",
  "- `.env` holds REAL secrets and is gitignored — the executor never overwrites, commits, or logs it.",
  "- \"Sem scroll horizontal até ~360px\" is visual — machine-verifiable only via Playwright (arrives slice 14); tagged [review-only].",
  "",
  "## Steps",
  "- [x] Scaffold Next.js num diretório temporário e mover os arquivos gerados para a raiz do repo, preservando `docs/`, `.env`, `CONTEXT.md`. Mergear `.gitignore`.",
  "- [x] Inicializar shadcn/ui e adicionar primitivos: `button`, `card`, `input`, `sheet`, `dropdown-menu`, `separator`, `tooltip`, `avatar`. Tema claro único.",
  "- [x] Adicionar Prisma: `schema.prisma` com os models do adapter Auth.js, `src/lib/prisma.ts` singleton, `prisma migrate dev --name init`.",
  "- [ ] Wire Auth.js v5: edge-split (`auth.config.ts` + `auth.ts`), proxy, route, seed. Confirmar `npm run build` verde.",
  "- [ ] Construir a casca de UI: `nav.ts`, `sidebar.tsx`, `topbar.tsx`, `app-shell.tsx`.",
  "- [ ] Rotas protegidas + `EmptyState` + login.",
  "- [ ] Build config + Docker: `next.config.ts` standalone, `Dockerfile` multi-stage, `.dockerignore`.",
  "- [ ] Adicionar Vitest + testes de renderização (`sidebar.test.tsx`, `empty-state.test.tsx`).",
  "- [ ] Provar ambiente + gate local: lint, test, migrate, build, docker build — todos exit 0.",
  "- [ ] Self-review via subagent; resolver MEDIUMs.",
  "",
  "## Notes & decisions",
  "- Pinned Prisma to `^6` (v6 keeps `url = env(\"DATABASE_URL\")`, no driver adapter for SQLite).",
  "- Next.js 16 renamed `middleware.ts` to `proxy.ts` (same API).",
  "",
  "## Handoff",
  "- Delivered so far: repo scaffolded, shadcn primitives, Prisma schema + init migration. Next: Auth.js edge-split and the UI shell.",
  "- Traps: `@vitejs/plugin-react@6` conflicts with shadcn's babel chain — install test deps with `--legacy-peer-deps`.",
  "",
  "<!-- ralphy-plan: issue=71 -->",
].join("\n");

const PLAN_FINCAL_92 = [
  "# Plan for #92: Reconcile — UI de conferência",
  "",
  "## Feasible: yes",
  "The OFX parser (#90) lands normalized transactions; this slice renders the side-by-side conferência table and the accept/reject actions. Machine-verifiable via component tests.",
  "",
  "## Done when",
  "- `npm run test` passes `reconcile-table.test.tsx` (renders matched pairs + unmatched buckets).",
  "- Accepting a match writes a `Lançamento` linked to the imported row.",
  "",
  "## Steps",
  "- [x] Tabela de conferência: colunas extrato × lançamentos, agrupada por status de match.",
  "- [ ] Ações aceitar/rejeitar por linha, com atalhos de teclado.",
  "- [ ] Persistir a decisão (link `ImportedRow` → `Lançamento`).",
  "- [ ] Testes de render + fluxo de aceite.",
  "",
  "## Decisions",
  "- Reuse a heurística de matching de #91 (bloqueada por #200) apenas em modo read-only até desbloquear.",
  "",
  "## Handoff",
  "- Waiting on a provider usage-limit reset before resuming execution.",
  "",
  "<!-- ralphy-plan: issue=92 -->",
].join("\n");

const PLAN_RALPHY_13 = [
  "# Plan for #13: xterm v6 — cores do tema",
  "",
  "## Feasible: yes",
  "The webgl addon (#10) is in; this maps the ADR-0035 warm-dark palette onto xterm's 16-color + ANSI bright set and the cursor/selection colors.",
  "",
  "## Done when",
  "- The terminal theme object matches the palette tokens; a snapshot test pins the 16 colors.",
  "",
  "## Steps",
  "- [x] Extrair os tokens de cor do ADR-0035 para um objeto `ITheme`.",
  "- [ ] Mapear ANSI 0–15 + cursor + selection + background.",
  "- [ ] Snapshot test do tema aplicado.",
  "",
  "## Decisions",
  "- Bright variants derived by a fixed lightness bump, not hand-picked, to stay consistent.",
  "",
  "## Handoff",
  "- #11 (sessão multiplexada) needs a human split; #12 (reflow) is waiting on a human gate.",
  "",
  "<!-- ralphy-plan: issue=13 -->",
].join("\n");

const PLAN_LINGOPILOT_43 = [
  "# Plan for #43: Streaming — UI incremental",
  "",
  "## Feasible: yes",
  "SSE transport (#40) delivers token deltas; this renders them incrementally with a stable layout and a stop control. Planner is still writing the step list.",
  "",
  "## Done when",
  "- Deltas append without layout shift; the stop button aborts the stream cleanly.",
  "",
  "## Steps",
  "- [ ] Assinar o stream SSE e acumular deltas num buffer reativo.",
  "- [ ] Render incremental sem reflow (medir com Playwright).",
  "- [ ] Botão parar → aborta o fetch e marca a mensagem como interrompida.",
  "",
  "## Decisions",
  "- #41 (token counter) veio infeasible — 0 steps; #42 (cancelamento) está bloqueada.",
  "",
  "## Handoff",
  "- Planning in progress.",
  "",
  "<!-- ralphy-plan: issue=43 -->",
].join("\n");

const SEED_PLANS = {
  "seed-plan-fincal-71": PLAN_FINCAL_71,
  "seed-plan-b": PLAN_FINCAL_92,
  "seed-plan-c": PLAN_RALPHY_13,
  "seed-plan-d": PLAN_LINGOPILOT_43,
};

// The plan bodies are handed to the shell as the same hidden
// `<script type="text/markdown">` nodes they were authored as, so `initRuns()`
// (app.js) finds them by `getElementById(r.planEl)` exactly as before — this is
// a relocation, not a protocol change.
for (const [id, body] of Object.entries(SEED_PLANS)) {
  const el = document.createElement("script");
  el.type = "text/markdown";
  el.id = id;
  el.textContent = body;
  document.head.appendChild(el);
}

// Wake anchors for the seeded sleep state (relative to load time so the live
// countdown reads sensibly).
const _in = (mins) => Math.floor(Date.now() / 1000) + mins * 60;

// Seed: runs keyed by project slug, reachable ONLY from the static `file://`
// demo (#300 — `initRuns` drops it in daemon mode, where the panel reads live
// snapshots). `planEl` names the hidden `seed-plan-*` <script> this file
// appends above, holding that run's plan.md; app.js hydrates `planMd` from it
// at init.
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
