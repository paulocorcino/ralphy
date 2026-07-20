# Auditoria UI/UX — Workbench mini-IDE

- **Data:** 2026-07-13
- **Branch auditada:** `feat/mini-ide` (HEAD `173071e` — "chore(daemon): promote workbench to /, delete legacy UI, prune vendors (#200)")
- **Escopo:** `crates/ralphy-daemon/assets/ui/` (frontend embarcado) × `crates/ralphy-daemon/src/` (superfície HTTP/WS real do daemon)
- **Natureza:** somente leitura — nenhuma linha de código foi alterada por esta auditoria.
- **Contexto:** o backlog do workbench-revamp (PRD #185, issues #186–#199) foi todo executado e fechado; #200 (promover o workbench a `/`) está aberto aguardando revisão humana, mas o commit correspondente já está nesta branch. Esta auditoria cataloga o que **restou** de mock/fake após esse backlog e os defeitos observados em uso real.

---

## 1. Método

1. Leitura integral do frontend não-vendor: `index.html` (1092 linhas), `app.js` (1719), `wb-daemon.js` (156), `wb-console.js` (397), `wb-runs.js` (249), `wb-kanban.js` (780, parcial), `wb-viewer.js` (756, via grep), `wb-settings.js` (316, via grep), `styles.css` (pontos críticos), `detached.html`.
2. Mapeamento exaustivo da superfície server-side (`crates/ralphy-daemon/src/lib.rs`, `dispatch.rs`, `tree.rs`, `fswrite.rs`, `confine.rs`, `auth.rs`, `totp.rs`, `password.rs`, `cookie.rs`, `session.rs`, `watch.rs`, `protocol.rs`) — rotas HTTP, endpoints WS, registro de verbos, guard de auth.
3. Cross-reference: cada chamada do JS → handler real; cada handler real → consumidor no JS.
4. Verificação dos sintomas relatados pelo operador (kanban vazio, login/TOTP inoperante, console sobre o kanban, dot-folders ausentes, "mock" espalhado) contra causa-raiz no código.
5. Cruzamento com o backlog GitHub (`gh issue list`, termo "workbench") para não recapitular trabalho já entregue.

---

## 2. Veredito geral

A entrega está **~70% real, ~30% encenação**. Os sistemas estruturais funcionam de ponta a ponta (consoles PTY, árvore de arquivos viva, leitura/escrita confinada, board, branches, settings de projeto, endpoints de auth). O que resta de fake se divide em três categorias:

1. **Um painel inteiro simulado** — o painel Runs (a única lacuna que exige backend novo).
2. **Fallbacks de mock que vazam para o modo real** — o defeito mais grave da entrega: erro de transporte vira dado fabricado apresentado como real.
3. **Chrome decorativo ou quebrado** — uptime fixo, busca morta, copy "mock", z-index, etc.

---

## 3. Inventário — o que é REAL (verificado handler a handler)

### 3.1 Rotas HTTP do daemon (todas em `crates/ralphy-daemon/src/lib.rs`; router em lib.rs:181–369)

| Rota / método | Handler | O que faz de verdade |
|---|---|---|
| `GET /api/identity` | `identity_route` (lib.rs:1387) | Identidade do daemon `{name, avatar}`; 404 se não batizado. **Sem consumidor no JS.** |
| `GET /api/repos` | `repos_route` (lib.rs:1287) | Lê `repos.toml` fresco por request; `[{slug, path, reachable, branch}]`; branch lida ao vivo de `.git/HEAD`. |
| `GET /api/usage[?since=…]` | `usage_route` (lib.rs:1325) | Ledger de tokens + scan interativo (Claude/Codex/OpenCode/Kimi). **Sem consumidor no JS.** |
| `GET /api/sessions` | `sessions_route` (lib.rs:1368) | Sessões vivas `[{id, repo, agent, kind, started_at}]` para reattach. |
| `POST /api/sessions/close?id=` | `close_session_route` (lib.rs:1374) | Tree-kill do filho da sessão; 200/404. |
| `POST /api/login` | `login_submit` (lib.rs:1416) | Valida TOTP (+password) **apenas sob política `Session`**; senão `404 "login not enabled"`. Seta cookie `HttpOnly`. |
| `GET /api/session` | `session_state_route` (lib.rs:1465) | Oráculo `{authed, password}` que alimenta o gate da SPA. |
| `POST /api/logout` | `logout_route` (lib.rs:1446) | `Set-Cookie` com `Max-Age=0` (cookie é HttpOnly, JS não limpa). |
| `GET /api/security/state` | lib.rs:1562 | Estado real `{token_set, password_set, totp_enrolled, require_login}`. |
| `POST /api/security/totp/enroll` | lib.rs:1574 | Seed TOTP mint-once; URI `otpauth://` mostrada uma única vez. |
| `POST /api/security/totp/revoke` | lib.rs:1588 | Apaga o arquivo de seed. |
| `POST /api/security/password` | lib.rs:1606 | Seta (não-vazio) / limpa (vazio) o fator senha. |
| `POST /api/security/token/remint` | lib.rs:1619 | Rotaciona o token em disco (nunca ecoado). Só tem efeito no próximo boot (política capturada no boot, ADR-0032 §4). |
| `POST /api/security/require-login` | lib.rs:1638 | Gate server-side: habilitar sem seed TOTP → 400. Estado derivado da seed, não é flag armazenada. |
| `GET /*` (fallback) | `ui_asset` (lib.rs:1431) | Serve a UI embutida (`include_dir!` de `assets/ui`, lib.rs:47). Zero leitura de disco em runtime. |

**Guard de auth** (`require_auth`, lib.rs:384–423): envolve TODAS as rotas, incluindo os upgrades WS e o fallback estático. `Localhost` passa tudo; `Bearer`/perna-máquina de `Session` exigem `Authorization: Bearer`. Sob `Session`, cookie válido passa; allowlist de login = `/api/login`, `/api/session`, `/api/logout`; um `GET` top-level não-`/api`/não-`/ws` serve a SPA sem cookie (ela renderiza o próprio gate opaco); resto → 401. Token é removido do env do processo antes de qualquer spawn de filho (lib.rs:118–134).

### 3.2 Endpoints WebSocket

Codec de frame binário taggeado (`protocol.rs`, espelhado em `wb-daemon.js:14`): `0x01`=Terminal `[tag][session u64 BE][bytes]`, `0x02`=Command `[tag][JSON {id, verb, payload}]`, `0x03`=Presence.

| Rota WS | Handler | Protocolo |
|---|---|---|
| `GET /ws` | `ws_presence_loop` (lib.rs:450) | Push de `Presence{name, avatar, uptime_secs}` a cada 2s. **Sem consumidor no JS** (o `TAG_PRESENCE` é declarado em wb-daemon.js:17 mas nenhum socket abre `/ws`). |
| `GET /ws/session?…` | `session_ws` (lib.rs:624) | Bridge PTY. Três formas: `?id=<u64>[&takeover=1]` (reattach; 404 desconhecida, 409 ocupada), `?repo=<slug>&agent=<claude\|codex\|opencode>` (launch de agente), `?console=1[&repo=]` (console livre, #167). Replay de scrollback na conexão; input do cliente → stdin do PTY; `Command{verb:"resize"}` → resize. Modelo tmux (`session.rs`): drop do WS **detacha** (o filho sobrevive); só `POST /api/sessions/close` ou exit do filho encerram. |
| `GET /ws/command` | `command_ws` (lib.rs:763) | Um comando por conexão. Primeiro frame `Command{id, verb, payload}` → despachado por `Verb::from_query` + `effect_class`. Verbo desconhecido / repo não registrado → um frame `{status:"error"}`. |
| `GET /ws/tree` | `tree_ws` (lib.rs:1139) | Assinatura persistente (#196): cliente manda `watch`/`unwatch {repo, path}`; servidor empurra `tree.dirty {repo, path}` em mudanças de FS assentadas. Backing: `watch::WatcherManager`. |

### 3.3 Registro de verbos de `/ws/command` (dispatch.rs:128–240)

`Verb::from_query` (dispatch.rs:173) aceita SOMENTE estas 18 strings; qualquer outra → `{status:"error","unknown verb"}`.

| Verbo | EffectClass | Tratado em (lib.rs) | O que faz |
|---|---|---|---|
| `run` | Spawn | 990–1126 | Spawna `ralphy run --if-idle --agent … [--plan-agent …] --branch-mode …` destacado (argv por `spawn_argv`, dispatch.rs:248 — parâmetros closed-enum, o cliente nunca compõe linha de comando). Streaming `spawned`/`output`/`exited`. O run sobrevive ao daemon. |
| `triage` | Spawn | ″ | `ralphy triage --if-idle --yes`. |
| `push` | Spawn | ″ | `ralphy issues --push`. |
| `tree.list` | Observe | 822–850 | **In-daemon** `tree::list(root, rel)` — um nível de diretório, confinado ao root do repo. |
| `file.read` | Observe | ″ | **In-daemon** `tree::read` — texto do arquivo; erros `binary`/`too large`/`not found` (cap 2 MiB; sniff de NUL em 8 KiB; escape mascarado como miss). |
| `config.get` | Query | 911–948 | Spawn-e-coleta `ralphy config get --json`; reply aninhada no campo `config`. |
| `board.list` | Query | ″ | `ralphy issues --format json --board`; reply no campo `board`. |
| `issue.show` | Query | ″ | `ralphy issues show <n> --format json`; reply no campo `issue`. |
| `branch.list` | Query | ″ | `ralphy branch list --format json`; reply no campo `branches` (#199). |
| `config.set` / `config.unset` | Mutate | 953–986 | `ralphy config set/unset -- <key> [<value>]` (run-lock-aware; key `^[a-z0-9_.]+$`). |
| `branch.switch` / `branch.create` | Mutate | ″ | `ralphy branch switch/create -- <name>` (run-lock-aware). |
| `label.set` | Mutate | ″ | `ralphy label set <n> --add=/--remove=<label>` (run-lock-aware). |
| `file.write` / `file.create` / `file.rename` / `file.delete` | Write | 857–903 | **In-daemon** `fswrite::*` confinado; erros `refused` (escape), `exists`, `not found`, `io error`. Sem run-lock (byte-writes, ADR-0036 emenda #187). |

**Não existe** verbo genérico de git (`git status`, diff, stage) — só branch list/switch/create. Não existe verbo de descoberta de runs nem feed de eventos de run.

### 3.4 Funcionalidades do frontend confirmadas reais

- **Consoles flutuantes** (wb-console.js): xterm.js + FitAddon + WebGL (fallback DOM) sobre `/ws/session`; reattach de todas as sessões vivas no load (`/api/sessions` → uma janela por sessão, com scrollback); takeover explícito de sessão ocupada (confirm → reconecta com `takeover=1`); close chama `POST /api/sessions/close` antes de derrubar a janela; drag/resize/tile (`arrange`) com clamp por ResizeObserver; atalhos Alt+Shift+1/2/3/0 (por `e.code`, layout-agnóstico), bloqueados em input/modal/login.
- **Árvore de arquivos** (app.js:1251–1375): Wunderbaum com raiz de `tree.list`, pastas `lazy` (fetch por nível no expand); watch/unwatch por pasta expandida via `/ws/tree`; `tree.dirty` re-lista só o nível visível (nudge para pasta colapsada é descartado — ADR-0036 §4); rename via F2 (intent + otimista); menu de contexto (open/rename/copy-path/new file/new folder/delete).
- **Viewer/editor** (wb-viewer.js + app.js): `file.read` real com recusa apresentada (binary/too large/not found fecham a aba); CodeMirror para código, markdown renderizado (marked+DOMPurify+mermaid); save/create/rename/delete roteados a `file.*` (app.js:1631–1680); detach de aba para popup (`detached.html`) com postMessage de volta.
- **Kanban** (app.js:578–809): `board.list` no abrir do board e na troca de projeto; ordem das colunas Ready preservada do servidor (ordem de grafo do fold — deliberado, comentário app.js:658-664); `issue.show` hidrata body/comments/blockers do drawer; `label.set` com update otimista + revert e flash na recusa `{status:"error"}` (run-lock, ADR-0036 §6); cores de label reais do repo com fallback ao vocabulário seed.
- **Branch switcher** (app.js:138–273): `branch.list` (reply aninhada lida corretamente em `reply.branches.{current,branches}`), switch/create otimistas com revert+flash em recusa.
- **Settings de projeto** (app.js:811–864): `config.get` merge sobre defaults do schema; máscara de `events.token` nunca round-trip; `config.set`/`config.unset` no change.
- **Login/segurança** (app.js:866–1060): todos os endpoints `/api/security/*` chamados; QR do TOTP renderizado da URI real; toggles refletem `GET /api/security/state`.
- **run/triage/push** (wb-daemon.js): spawn real via `/ws/command`; output cru do run alimenta `runsActionMsg`/`rawFeed`.

**Conclusão do cross-reference:** todo verbo/endpoint que o JS chama tem backend. O inverso não: `GET /ws` (presence), `GET /api/usage` e `GET /api/identity` são código servidor real e testado (`tests/ws_presence.rs`, `tests/observe_read.rs`, `tests/session_ws.rs`, `tests/security_routes.rs`) **sem nenhum consumidor** no JS embarcado.

---

## 4. Defeitos e mocks — catálogo completo

Severidade: **C**rítico / **A**lto / **M**édio / **B**aixo. "Evidência" = arquivo:linha na branch auditada.

### C1 — Conteúdo de arquivo FABRICADO em erro de transporte (risco de corrupção de dados)

- **Evidência:** `app.js:1341` (`.catch(() => fakeContent(path, ftype))`), `wb-viewer.js:120–129` (mesmo padrão no refresh), gerador em `wb-viewer.js:538–751` (`fakeContent`, `fakeMarkdown`, `fakeTs`, `fakeRs`, `fakeJson`, …).
- **Comportamento:** servido pelo daemon, se o `file.read` falha por **transporte** (WS caiu, timeout), a UI renderiza um arquivo sintético plausível (ex.: um `.rs` genérico com o nome do arquivo) **sem qualquer indicação de que é falso**. O usuário pode editar e salvar: o `file.write` grava o conteúdo fabricado por cima do arquivo real.
- **Nota:** a recusa *explícita* do daemon (`{status:"error"}`) é tratada corretamente (fecha a aba com o motivo); só o caminho de exceção vaza para o mock.
- **Correção esperada:** em modo daemon (`location.protocol !== "file:"`), o catch deve apresentar erro e fechar a aba; `fakeContent` deve ser inalcançável fora de `file://`.

### C2 — Kanban vazio: erro do verbo engolido, indistinguível de "sem issues" (sintoma relatado pelo operador)

- **Evidência:** `app.js:606–625` — `loadBoard()` faz `if (!reply || reply.status !== "ok") return;` e `catch { /* leave it empty */ }`.
- **Causa-raiz:** `board.list` não lê nada in-daemon — spawna `ralphy issues --format json --board` no cwd do repo (lib.rs:911–948). Se esse CLI falha (`gh` não autenticado no ambiente do daemon, tracker não configurado, exit ≠ 0, repo sem issues.md/GitHub), a reply é `{status:"error", message}` — e a UI silencia, mostrando o empty-state "No issues in <slug>".
- **Correção esperada:** estado de erro visual distinto do vazio, exibindo a `message` verbatim do verbo (mesmo padrão do flash de recusa do `label.set`). Isso também é pré-requisito de diagnóstico: sem ele não dá para saber a causa concreta no ambiente do operador.

### C3 — Login/TOTP inoperante em localhost e modal Security enganoso (sintoma relatado)

- **Evidência:** lib.rs:123–129 (`upgrade_with_session`: promoção a `Session` só em bind de **rede** com seed TOTP; `Localhost` intocada), lib.rs:1416–1418 (`login_submit`: política ≠ `Session` → `404 "login not enabled"`), app.js:978–1001 (toggle require-login), app.js:1010–1022 (`logOff` seta `authed=false` incondicionalmente).
- **Cadeia do defeito em loopback:** (1) o operador enrola TOTP e liga "Require login" — o endpoint aceita (a seed existe), mas a política viva continua `Localhost` (capturada no boot; e mesmo após restart, loopback **nunca** vira `Session`); (2) "Log off" derruba a SPA para o gate opaco; (3) todo submit do form bate em `/api/login` → 404 → "Invalid code or password", **sempre**; (4) o operador fica preso até F5 (o `probeSession` no reload lê `authed:true` de novo).
- **Leitura:** o backend está coerente com a postura opt-in do ADR-0032/#179 (localhost aberto por design). O defeito é de **honestidade da UI**: o modal Security permite ligar um gate que nunca terá efeito naquele bind, sem aviso, e o Log off cria um beco sem saída.
- **Correção esperada:** `/api/session` (ou `/api/security/state`) expor a política ativa (`localhost|bearer|session`); o modal desabilitar/explicar "Require login" fora de `Session`; `logOff` sob `Localhost` não trancar (ou re-probar). Alternativa maior (decisão de produto): suportar `Session` em loopback — mexe no ADR-0032.

### A1 — Painel Runs 100% teatro (a única lacuna estrutural de backend)

- **Evidência:** seed `WB_RUNS` em `wb-runs.js:151–249` (runs inventados para fincal/ralphy/lingopilot com runids `01JR-…`, faces 🦊🐼🦉🐙); 4 blocos `<script type="text/markdown">` de plan.md fake em `index.html:929–1045`; botão ⚡ `demoTick()` (`app.js:558–576`) que sintetiza eventos; `githubUrl` do drawer hardcoded.
- **O que é real nele:** o **fold** está pronto e correto — `applyRunEvent` (app.js:497–550) trata os CloudEvents load-bearing (`plan.step`, `issue.closed/skipped/started`, `run.sleep_started/ended`, `run.heartbeat`) com tolerância a tipos desconhecidos; a porta de entrada existe (`ralphy:run-event` / `window.WBRuns.emit`, app.js:1703–1718); os helpers de vocabulário (`WBRun`: glifos por `IssueStatus` de state.rs, slicing de `##` do plan.md, formatação de sleep) são fiéis ao core. O Phase-1 `rawFeed` (stdout+stderr cru de um run spawnado **por este browser**) é real.
- **O que falta (backend):** descoberta de runs ativos (runstate on-disk) e retransmissão dos eventos do run para o browser (ex.: `/ws/events` ou push estilo `/ws/tree`). Runs iniciados fora do browser são invisíveis; reload perde até o rawFeed.
- **Peças a aposentar quando ligar:** `WB_RUNS`, os 4 seed-plans do index.html, `demoTick` e o botão ⚡ (`index.html:392–396`), `initRuns()` lendo dos `<script>`.

### A2 — Consoles flutuam ACIMA do overlay Kanban (sintoma relatado; confirmado)

- **Evidência:** `.kanban { z-index: 30 }` (styles.css:2514–2517), `.kanban-detail { z-index: 40 }` (styles.css:2801–2804), janelas de console começam em `z = 60` e **incrementam a cada foco** (`wb-console.js:18`, `focusWin` em :60–67).
- **Comportamento:** o Kanban é `position:absolute; inset:0` sobre o canvas, mas qualquer console focado (z ≥ 61) fura o board.
- **Correção esperada:** elevar o kanban acima da faixa dos consoles (ela é ilimitada — cresce a cada foco — então melhor: esconder/inert o `#workspace` enquanto o board está aberto, ou impor `isolation: isolate`/stacking context no `.stage` de modo que o overlay ganhe).

### A3 — Settings gravam a cada tecla digitada (um spawn de CLI por keystroke)

- **Evidência:** `app.js:850–864` (`saveSetting`) chamado por `@input` nos campos text/password/number (`index.html:620–631`); cada chamada faz `WBDaemon.spawn("config.set", …)` → um `/ws/command` novo → um processo `ralphy config set` (Mutate, run-lock-aware).
- **Impacto:** digitar "origin/main" = 11 spawns sequenciais de CLI; sob run-lock cada um pode recusar; valores intermediários ("o", "or", "ori"…) são persistidos.
- **Correção esperada:** commit em `@change`/blur (ou debounce ≥ 500 ms), e feedback de recusa (hoje o callback `() => {}` descarta a reply — uma recusa por run-lock é silenciosa).

### A4 — Dot-folders ausentes na árvore (sintoma relatado; parte por design, parte a confirmar)

- **Evidência:** `tree.rs:16` — `HARD_EXCLUDE = ["node_modules", "target", ".git", ".ralphy"]`; `tree.rs:29–56` — walker com `.git_ignore(true).git_exclude(true).hidden(false)`.
- **Análise:** hidden em si **não** é filtrado (`.hidden(false)` desliga o filtro); o que some é: (a) `.git` e `.ralphy` por hard-exclude deliberado; (b) qualquer dot-folder **gitignorado** (`.claude`, `.vscode`, `.env`…), pelo filtro de gitignore (UX cleanliness, ADR-0036 §5). Um `.github` commitado deveria aparecer — se no teste do operador não apareceu, é bug novo e merece teste de integração dedicado.
- **Questão de produto:** esconder `.ralphy` num IDE do ralphy é questionável — é onde vive o `plan.md` que o próprio painel Runs quer exibir. Sugestão: hard-exclude só de `.git`/`node_modules`/`target`; exibir `.ralphy`; opcionalmente exibir gitignorados esmaecidos (padrão de IDEs).

### A5 — Árvore de arquivos DUPLICA a cada save (sintoma relatado; confirmado)

- **Evidência:** `app.js:1348–1356` (`onTreeDirty`) — `node.load(this.loadTreeLevel(rel))`; vendor `wunderbaum.umd.min.js`: `WunderbaumNode.load()` → `_loadSourceObject()` → `this.addChildren(e.children)` **sem limpar os filhos existentes**. Só o `Wunderbaum.load()` de topo faz `this.clear()` antes; o `node.load()` de nó é pensado para lazy-load de nó *vazio* e **anexa**.
- **Cadeia do defeito:** salvar um arquivo → `file.write` → o watcher (#196) emite `tree.dirty` para o diretório pai → `onTreeDirty` re-lista o nível chamando `node.load()` num nó **já populado** → cada entrada do nível é anexada de novo. Um save de arquivo na raiz duplica o nível raiz inteiro; N saves = N cópias (o screenshot do operador mostra a raiz triplicada). O mesmo vale para qualquer `tree.dirty` (criar/renomear/deletar, mudanças externas), não só save.
- **Correção esperada:** reconciliar em vez de anexar — `node.removeChildren()` antes do `load()` (ou `tree.load()` quando `rel === ""`), preservando expansão/seleção; um teste de UI que salve duas vezes e conte os nós.

### A6 — Arquivo aberto não atualiza quando muda em disco (sintoma relatado; gap confirmado)

- **Evidência:** `wb-viewer.js:119–131` — o único caminho de refresh é o botão **Reload** manual (`reloadFile`); nenhum listener liga `tree.dirty` (que já chega no browser, mas só com path de *diretório*) aos viewers abertos; não existe verbo de watch de conteúdo de arquivo.
- **Impacto:** exatamente o caso de uso central do workbench — assistir um agente editar o repo — não funciona: o agente grava, a árvore até é nudged (e duplica, A5), mas a aba aberta continua mostrando bytes velhos até o operador clicar Reload. O save local também não confirma round-trip (o viewer confia no que enviou; nunca relê o que o daemon gravou).
- **Nuance de design:** auto-reload cego sobrescreveria edição local não salva — a política precisa ser dirty-aware (limpo ⇒ re-lê no `tree.dirty` do dir pai; sujo ⇒ badge "changed on disk" com escolha). O transporte já existe (`/ws/tree`); não precisa de verbo novo, só de granularidade: ao receber nudge do dir pai, re-`file.read` dos arquivos abertos daquele dir e comparar.

### M1 — Topbar decorativo: uptime fixo, identidade não buscada

- **Evidência:** `index.html:25` — `<span class="stat uptime">… daemon up 2h 14m</span>` (string literal); `/ws` presence e `/api/identity` sem consumidor (§3.4); "signed in · daemon" e avatar são estáticos. O próprio backlog seed do kanban tem um card "Surface the daemon uptime as a real heartbeat age" (wb-kanban.js:434).

### M2 — Estado dos projetos parcial: `dirty` nunca real, `live` nunca setado, `remote` adivinhado

- **Evidência:** `app.js:90–106` (`loadRepos`): `dirty: false` fixo; `state` só `idle|offline` (comentário admite: "live means an active session, not yet tracked here"); `remote` inferido do formato do slug (`path-` ⇒ local).
- **Consequências:** o aviso "uncommitted changes" do branch modal (`index.html:776`, :793–796) nunca dispara com dado real; o dot verde nunca acende; um repo GitHub clonado sem remote configurado seria classificado errado.
- **Backend necessário (pequeno):** estender `/api/repos` com `dirty` (`git status --porcelain` barato) e URL do remote; cruzar `/api/sessions` para `live`.

### M3 — "Open on GitHub" hardcoded para um único repo

- **Evidência:** `app.js:752` — `githubUrl(number)` retorna sempre `https://github.com/paulocorcino/ralphy/issues/${number}`, para **qualquer** projeto aberto. Links do drawer (cabeçalho e blockers) apontam para o repo errado em todos os outros projetos.

### M4 — Fallback de login local "qualquer código de 6 dígitos"

- **Evidência:** `app.js:1045–1060` — se o fetch de `/api/login` **lança** (sem daemon), cai na checagem local: regex `[0-9]{6}` + password em memória. Pensado para `file://`, mas é um caminho de bypass que não distingue "estático" de "daemon momentaneamente inacessível".

### M5 — Seeds fake sobrevivem como fallback silencioso em modo daemon

- **Evidência:** 4 projetos fake (lingopilot/fincal/ralphy/bioledger com árvores e branches inventados) em `app.js:1076–1171` — exibidos se `/api/repos` falhar (o catch de `loadRepos` engole); branches seed no modal se `branch.list` falhar (app.js:191–193); mesmo padrão em board (C2). Em todos os casos o operador vê dado plausível e falso sem aviso.

### M6 — Settings de escopo daemon não persistem em lugar nenhum

- **Evidência:** `wb-settings.js` define seções `scope: "daemon"` (bind, porta, identidade…); `saveSetting` só roteia `config.set` **por repo** (`if (this.openSlug)`), então mudanças de escopo daemon apenas emitem o intent `setting-change` no vácuo. Não existe verbo/arquivo de config machine-wide do daemon.

### M7 — `/api/usage` implementado e testado, sem nenhuma tela

- **Evidência:** `usage_route` (lib.rs:1325) devolve ledger + scan interativo (ADR-0033); nenhum fetch no JS. Funcionalidade paga (spike inteiro do token-tracking) invisível ao operador.

### M8 — Tradução on-device falha com "Other generic failures occurred." (sintoma relatado)

- **Evidência:** `wb-translate.js:50–67` (`translate`) e o consumo em app.js:410–433 / wb-viewer.js (preview markdown).
- **Análise:** a mensagem é o erro genérico do **Chromium** (Translator API) borbulhando cru até o operador — a UI apenas exibe `e?.message` e reverte o toggle (honesto, mas indecifrável). Causas típicas do genérico: (1) o par de idiomas está `downloadable`/`downloading` e o download do modelo on-device falha (offline, política corporativa, requisitos de hardware/disco, falta de user-activation para iniciar download em algumas versões) — o código trata só `unavailable` e segue direto para `Translator.create()`, que é quem dispara o download; (2) fonte mal-detectada: `detect()` cai em `"en"` fixo quando `LanguageDetector` está ausente ou falha (wb-translate.js:52), pedindo um par de modelo errado para texto PT; (3) o monitor de `downloadprogress` existe mas é vazio (wb-translate.js:60–62) — nenhum feedback de "baixando modelo X%".
- **Correção esperada:** tratar `downloadable`/`downloading` explicitamente (mostrar progresso e estado "baixando modelo"); mapear o erro genérico para mensagem acionável ("o navegador não conseguiu baixar o modelo pt→en — verifique conexão/espaço"); não chutar `"en"` como fonte (se a detecção falhar, perguntar ou desistir com mensagem clara). Vale registrar: a feature é inerentemente dependente de Chrome/Edge 138+ com download de modelo — em qualquer outro browser o botão já se esconde (`xlateSupported`), esse caminho está correto.

### B1 — Copy "mock" espalhado, inclusive afirmações hoje FALSAS

- **Evidência:** `<title>ralphy · workbench shell (mock)</title>` (index.html:6); rodapé do Settings "Mock — settings emit… **nothing is written to disk**" (index.html:641) — falso: `config.set` grava; rodapé do Security "no secrets are stored for real" (index.html:751) — falso: TOTP/password/token são reais; rodapé do branch modal e do run modal "Mock — emits…" (index.html:819, :892) — os verbos são reais; hint do login "mock — any 6-digit code works" (index.html:919); rodapé do drawer Kanban "Read-only mock" (index.html:374); cabeçalhos de comentário "(mock)" em todos os wb-*.js.

### B2 — Busca de projetos morta e ícone órfão

- **Evidência:** `index.html:69–72` — o input "Search projects… /" não tem `x-model` nem handler; o atalho "/" sugerido no placeholder não existe; `index.html:66` — ícone `ellipsis` no header da sidebar sem ação.

### B3 — Intents de teatro restantes

- `run-issue-focus` (app.js:353–355) — clicar num nó do trail só emite intent; nada abre o plano/log daquele issue.
- `rawFeed` (app.js:444, index.html:417–420) — cap de 8000 chars num `<pre>`; some no reload; é o stand-in explícito da Fase 1.
- `demoTick`/⚡ — ver A1.
- `consoleItems()` hardcoded (app.js:1466–1473) — a lista de agentes poderia vir de capabilities do daemon (hoje coincide com os 3 adapters reais, então é benigno).

---

## 5. Sintomas relatados pelo operador × diagnóstico

| Sintoma relatado | Diagnóstico | Item |
|---|---|---|
| "Kanban não traz visibilidade, tudo vazio" | Erro do `board.list` engolido; empty-state mente | **C2** |
| "Sem login e validação do TOTP não funciona" | Política `Localhost` nunca vira `Session` em loopback; `/api/login` → 404; UI não avisa e Log off tranca | **C3** |
| "Console fica acima do kanban, não dentro do canvas" | z-index 60+ (crescente) vs 30/40 do overlay | **A2** |
| "Não lista as pastas com ponto" | `.git`/`.ralphy` hard-exclude deliberado + gitignore filter; hidden NÃO é filtrado — se dot-folder commitado sumiu, é bug a confirmar com teste | **A4** |
| "Palavra mock espalhada" | Título, 5 rodapés, hints, comentários — incluindo afirmações hoje falsas | **B1** |
| "Toda vez que salva, a árvore de arquivos é duplicada" | `node.load()` do Wunderbaum anexa sem limpar; `onTreeDirty` chama em nó populado | **A5** |
| "O arquivo aberto não é atualizado automaticamente" | Único refresh é o botão Reload manual; nada liga `tree.dirty` aos viewers | **A6** |
| "Tradução não funciona — Other generic failures occurred." | Erro genérico do Chromium (download do modelo / fonte chutada em "en") exibido cru | **M8** |

---

## 6. Plano de trabalho proposto (não iniciado — auditoria é somente leitura)

### Etapa 0 — Honestidade e correções fechadas (só frontend + 1 ajuste de CSS; sem verbo novo)

1. **C1**: em modo daemon, erro de transporte no `file.read` → mensagem + fechar aba; `fakeContent` inalcançável fora de `file://`.
2. **C2**: estado de erro visual no Kanban com a `message` verbatim do verbo (distinto de vazio).
3. **A2**: kanban acima dos consoles (esconder/inert `#workspace` com board aberto, ou stacking context no `.stage`).
4. **A3**: settings commit em change/blur + flash de recusa (reply do `config.set` não pode ser descartada).
5. **M4/M5**: fallbacks locais (login 6 dígitos, projetos/branches/board seed) restritos a `file://`; em modo daemon, falha → erro visível.
6. **A5**: `onTreeDirty` reconcilia em vez de anexar (`removeChildren()` antes do `node.load()`, preservando expansão) — corrige a duplicação da árvore a cada save.
7. **B1/B2**: remover "(mock)" e as afirmações falsas; implementar (filtro client-side trivial + atalho "/") ou remover a busca; remover o ícone morto.
- *Critério de aceite:* nenhuma informação fabricada alcançável quando servido pelo daemon; cada falha de verbo/endpoint tem apresentação visual própria; dois saves seguidos não alteram a contagem de nós da árvore.

### Etapa 1 — Presença, identidade e estado reais (backend pequeno + frontend)

1. **M1**: consumir `/ws` (uptime vivo no topbar, com idade do heartbeat) e `/api/identity` (brand/avatar).
2. **M2**: estender `/api/repos` com `dirty` e URL do remote; cruzar `/api/sessions` para o estado `live`; com isso **M3** (githubUrl) passa a derivar do remote real.
3. **C3**: expor a política ativa em `/api/session` (`localhost|bearer|session`); modal Security desabilita/explica "Require login" fora de `Session`; `logOff` não tranca sob `Localhost`. (Se a decisão for suportar `Session` em loopback, virar emenda do ADR-0032 antes.)
4. **A4**: decisão de produto do hard-exclude (recomendação: manter `.git`/`node_modules`/`target`, exibir `.ralphy`) + teste de integração para dot-folders commitados.
5. **A6**: refresh dirty-aware dos viewers abertos no `tree.dirty` do dir pai (limpo ⇒ re-lê; sujo ⇒ badge "changed on disk") — transporte já existe, sem verbo novo.
6. **M8**: tratar `downloadable`/`downloading` do Translator com progresso e mensagens acionáveis; eliminar o fallback de fonte `"en"` chutada.

### Etapa 2 — Painel Runs real (única lacuna estrutural; PRD/ADR antes de codar)

- Backend: descoberta de runs ativos (runstate on-disk) + retransmissão dos CloudEvents do run ao browser (candidatos: `/ws/events` dedicado, ou push no padrão do `/ws/tree`). Decidir a fonte (tail do sink de eventos vs. leitura do runstate) e o contrato de replay/catch-up no reattach.
- Frontend: hidratar `runsByProject` do real; `plan.md` vivo via `file.read` de `.ralphy/plan.md` (depende da decisão do A4) ou via payload de evento; aposentar `WB_RUNS`, seed-plans, `demoTick`/⚡; ligar `run-issue-focus` a algo útil (abrir o plano do issue).
- *Racional de rito:* mesmo padrão do #187 — a emenda de contrato veio antes do código.

### Etapa 3 — Completar o operador

1. **M7**: tela de usage consumindo `/api/usage` (ledger + interativo, ADR-0033).
2. **M6**: persistência dos settings de escopo daemon (exige decisão: verbo novo + arquivo de config do daemon).
3. **B3**: varrer os intents de teatro restantes.

### Ordem recomendada

Etapa 0 inteira primeiro (destrava o diagnóstico do C2 no ambiente real e elimina o risco de corrupção do C1); Etapa 1 em seguida (tudo fechado e verificável, `ready-for-agent`); Etapa 2 começa pelo PRD/ADR em paralelo à Etapa 1; Etapa 3 por último.

---

## 7. Apêndice — mapa de arquivos do frontend

| Arquivo | Linhas | Papel | Estado |
|---|---|---|---|
| `index.html` | 1092 | Shell Alpine.js: topbar, rail, sidebar, tabs, kanban, runs, 5 modais, login gate, 4 seed-plans | Real com ilhas fake (seed-plans, uptime, copy) |
| `app.js` | 1719 | Componente `shell()`: accordion, árvore, tabs, kanban, runs fold, settings, security, login, seam `workbench:action` | Real com fallbacks fake (C1, M4, M5) |
| `wb-daemon.js` | 156 | Adapter do seam → `/ws/command` (spawn/observe/write) + `/ws/tree` (subscribe) | Real |
| `wb-console.js` | 397 | Janelas flutuantes + xterm real sobre `/ws/session`, reattach, takeover | Real (bug A2 é do CSS) |
| `wb-viewer.js` | 756 | Viewers CodeMirror/markdown, find, save, detach | Real + gerador `fakeContent` (C1) |
| `wb-runs.js` | 249 | Helpers `WBRun` (fiéis ao vocabulário do core) + seed `WB_RUNS` | Helpers reais; dados 100% fake (A1) |
| `wb-kanban.js` | 780 | Helpers de coluna/grafo/labels + seed `WB_KANBAN` | Helpers reais; seed só fallback |
| `wb-settings.js` | 316 | Schema data-driven dos settings + helpers de security | Real (persistência: só escopo projeto — M6) |
| `wb-translate.js` | 68 | Tradução on-device (Translator API, Chrome/Edge 138+) | Real, degrada bem |
| `detached.html` | 109 | Popup de viewer destacado (postMessage) | Real |
| `styles.css` | 3155 | Tema warm-dark (ADR-0035) | Real (bug A2: z-index) |
| `vendor/` | — | alpine, xterm(+fit/webgl/links), codemirror, wunderbaum, marked, dompurify, mermaid, lucide, bootstrap-icons, devicon, qrcode | Vendorizado, sem CDN |
