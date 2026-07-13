# Workbench shell — build guide

**What this document is.** A *living implementation guide* for the model that
turns this mock into the real daemon web workbench. It is **not an ADR**: ADRs
freeze one decision (see [ADR-0032](../../docs/adr/0032-daemon-mode-supervised-launcher.md)
for daemon mode, [ADR-0035](../../docs/adr/0035-daemon-ui-visual-language.md) for
the visual language). This guide is meant to change alongside the mock, and it
records the *idea behind* the shell so the build keeps its intent.

**How to keep it updated.** When a new interaction or panel is added to the mock,
add its intent + its backend mapping here (or, if it's a module, point at that
file's header comment — don't duplicate). When a decision hardens, promote it to
a real ADR and link it from here.

> **Scope of this file.** It documents the **shell foundation** — the pieces
> established first: the layout, the project accordion, the file tree, and the
> `workbench:action` seam. The later modules (file viewers, floating consoles,
> Runs panel, run verbs, branch switcher, on-device translation, Settings/Security)
> each carry their own header comment describing intent and backend sources; this
> guide points at them rather than restating.

---

## The core idea: the UI only *intents*

The mock performs **nothing** destructive. Every gesture — open, rename, delete,
create, save, console-open, branch-switch, setting-change… — is turned into a
single browser event and nothing else. A backend engine subscribes and does the
real work (touch the filesystem, spawn an agent, run `git checkout`).

This is the seam the real product must preserve: **the web UI is a pure intent
surface; the daemon is the executor.** It mirrors ralphy's own ethos (it proposes,
a human/backend disposes) and keeps the browser incapable of harm on its own.

### The seam contract

`window.WB.emit(action, detail)` (in [app.js](app.js)) is the one exit point:

```js
document.dispatchEvent(new CustomEvent("workbench:action", {
  detail: { action, ...detail, at: "<ISO timestamp>" }
}));
```

A backend integration is therefore just:

```js
document.addEventListener("workbench:action", (e) => {
  const { action, ...rest } = e.detail;   // e.g. "rename", { project, path, ... }
  // route to the daemon (WebSocket / fetch) and perform the real action
});
```

**Discover the live event catalogue by grepping** — it grows with the mock, so a
static list here would rot:

```
grep -rn "WB.emit(" mocks/workbench-shell/*.js
```

Each call site names the `action` and the payload keys it carries (project, path,
etc.). Treat those keys as the wire contract when wiring the backend.

---

## Foundation pieces (what this guide covers in full)

### Layout & chrome
A CSS-grid shell of **four columns**: **icon rail · sidebar · canvas · Runs panel**
(with the topbar spanning the top row). The chrome panels (topbar/rail/sidebar/runs)
sit on `--chrome`, one subtle step above the canvas ground `--bg`, so the dotted
canvas reads as the "floor" and the panels as framing. The **sidebar and Runs tracks
collapse to `0`** (they're `overflow:hidden`) — showing or hiding either is a pure
`grid-template-columns` flip driven by body classes (`side-collapsed`, `runs-open`),
and the whole thing animates. All colours are tokens from
[ADR-0035](../../docs/adr/0035-daemon-ui-visual-language.md), declared once in
[styles.css](styles.css) `:root`. **Do not hand-pick hex values** — use the tokens;
a genuinely new colour is an amendment to ADR-0035.

### Rail toggles, account menu & the auth gate
The icon rail is **interactive chrome** (handlers in [app.js](app.js)): **Projects**
(`toggleSide`) shows/hides the sidebar, **Runs** (`toggleRuns`) reveals the
right-hand panel, **Kanban** (`toggleKanban`) opens the tasks board as a canvas
overlay (module: [wb-kanban.js](wb-kanban.js) — see its section below), and a
**Settings gear** pinned to the rail's bottom (`.rail-spacer` +
`openSettings`) opens the Settings modal. There is **no "Sessions" button** — live
sessions surface as the floating consoles on the Agents tab.

The **topbar avatar** is an account menu (`avatarMenu`): **Security settings**
(`openSecurity`) and **Log off** (`logOff`). Auth is modelled **opt-in**, faithful to
[ADR-0032 §4](../../docs/adr/0032-daemon-mode-supervised-launcher.md): the Security
modal (content + real config sources in [wb-settings.js](wb-settings.js)) covers the
access token, an optional PBKDF2 password, and TOTP 2FA — enroll shows a **one-time**
QR + `otpauth://` provisioning URI (vendored `qrcode`, offline), and **revoke =
delete the seed** because ralphy has no rotate verb today. "Require login" is gated
on TOTP being enrolled (the session factor).

**The login gate** (`.login-gate`, shown on `!authed`, with `body.locked` blanking the
chrome) is a **fully opaque** overlay — deliberately, not a dim scrim: the real
daemon never renders the app until `/api/login` succeeds, so there is nothing behind
to peek at. The mock form mirrors `crates/ralphy-daemon/assets/ui/login.html`
(6-digit code + optional password). Backend wiring: `authed` becomes "holds a valid
`ralphy_session` cookie"; the form POSTs `/api/login`.

**One reflow gotcha (already paid for):** floating consoles are sized as a percentage
of `#workspace`, so a panel toggle that resizes the canvas could clip them under
`overflow:hidden`. `clampAll()` in [wb-console.js](wb-console.js) — a `ResizeObserver`
on `#workspace` — resizes/repositions every window back inside the box, so consoles
reflow for **both** the sidebar and the Runs panel.

### Tabbed canvas (Agents tab + file tabs)
The canvas is a **tabbed workspace**, not a single view. A tab strip (`.tabbar`)
runs across the top: tab 0 is the fixed **Agents** tab (never closes) and hosts the
floating agent consoles; every opened file rides in after it as a **closable** tab.
The console controls (**New console ▾**, **Arrange**) are pinned at the strip's
right edge, above the workspace, so a floating console can never cover them. Tab
state + lifecycle live in [app.js](app.js) (`tabs`, `active`, `activate`,
`openTab`, `closeTab`); the panes are owned by the viewer / console modules. On
open, the pane is chosen by extension (`classify`): markdown → rendered, binaries →
refused (`open-refused`), everything else → source. This shape ("tabbed workspace,
Agents fixed") is a candidate to harden into an ADR.

### Project accordion (sidebar)
`projects` is the daemon's repo list (a mirror of `/api/repos`): each has a
`slug`, `branch`, `state` (daemon reachability: live/idle/offline), `remote`
(github/local), and a file `tree`. The list is a **single-open accordion**:
opening a project lifts it to the top (`order:-1`), hides the siblings
(`.projects.has-open`), and mounts its tree in the freed column. Backend wiring:
replace the seeded `projects` array with the repo list from the daemon.

Two per-row indicators are **orthogonal** and must not be conflated: the **status
dot** (`.dot`, `dotClass`) is daemon-reachability *right now* (green live / grey
idle / red offline), while the **provenance icon** (`.remote`, `bi-github` vs
`bi-hdd`, before the name) is *where the repo lives* (GitHub-backed vs local-only).
A local-only repo can be live; a GitHub repo can be offline. The header also shows a
**count badge** (`.count` = `projects.length`) — how many repos the daemon located.

**Switching branch.** The current branch renders as a clickable chip
(`.branch-chip`) on the project row → it opens the **branch switcher**
(`.branch-modal`, `openBranchModal`): a filtered list of local branches (current
pinned + ticked) plus a *create-from-current* row when the typed name is new.
Switching or creating emits `branch-switch {branch}` / `branch-create {name,from}`;
the daemon runs the real `git checkout`. The chip is **gated on reachability, not
remote** (`canSwitchBranch` → `state !== "offline"`): a local-only repo is still a
git checkout with branches, so only an *unreachable* repo makes the chip inert.

### File tree (Wunderbaum)
The tree is a real, mature, dependency-free library — **[Wunderbaum](https://github.com/mar10/wunderbaum)**
(mar10; the jQuery-free successor to Fancytree) — chosen over jsTree specifically
to avoid jQuery. It loads from a **nested JSON `source`** (`folder`/`children`),
which is exactly the shape a backend should deliver. Backend wiring: `fetch` the
repo tree and **lazy-load subfolders** (Wunderbaum supports it); the gestures
already emit on the seam.

Gotchas already paid for (keep them):
- Wunderbaum puts the `wunderbaum` class **on the host element itself** → theme
  selectors are compound: `.wb-host.wunderbaum`, not descendant.
- A node has **no `isFolder()`** — use `node.folder || node.children`.
- The tree is **virtualized** → the host needs a real height (the flex-column
  chain in styles.css provides it).
- `mar10.Wunderbaum.getNode(event)` resolves a node from a DOM event (right-click).

### File-type icons
Resolved by extension in [app.js](app.js): coloured **Devicon** font glyphs for
known types (ts/js/json/rs/prisma/css/html…), **Bootstrap Icons** for folders and
the neutral fallback. Brand colours that go near-black on the dark ground
(Markdown, Rust) get a light-tone class override in styles.css.

### Right-click context menu
Built in [app.js](app.js) (`#ctxmenu`): Open, Rename (inline, also F2), Copy
relative path (also writes clipboard), New file/folder, Delete. Every item calls
`WB.emit(...)` — the menu is the clearest example of "gesture → intent".

### Themed scrollbars
Every scroll surface is themed to the warm-dark palette (`--border` thumb,
`--border-focus` on hover, transparent track): the sidebar and the Wunderbaum
viewport (which needs its own rule — it lost the page default), the **file viewers**
(CodeMirror's own `.CodeMirror-vscrollbar`/`-hscrollbar` + the markdown `.md-scroll`
pane), and the **Runs panel** scroll areas (`.plan-md`, `.branch-list`). No native
scrollbar is left unstyled.

### Typography
The chrome is monospace (terminal feel). **Rendered prose** — the plan.md in the
Runs panel — uses a UI sans (`--font-ui`) so it reads well, while code spans/blocks
stay monospace (`--font-mono`). Both tokens live in [styles.css](styles.css) `:root`.

One Wunderbaum gotcha worth its own line: the tree swaps to its `--wb-*-grayscale`
vars when it **loses focus**, and their defaults are near-white — the theme
overrides them to the same warm tone so a selected row doesn't flash white on blur.

### Vendored libraries (loaded locally, no CDN at runtime)
`alpine.min.js` (reactivity), `lucide.min.js` (chrome icons — prune to a subset
when the icon set stabilises), `wunderbaum/` (tree), `devicon/` +
`bootstrap-icons/` (file icons). Later modules added `codemirror/`, `marked`,
`mermaid`, `dompurify`, `qrcode` — see their modules. **Translation adds no
library** — it uses the browser's built-in Translator/LanguageDetector APIs
(see [wb-translate.js](wb-translate.js)).

---

## Element catalogue (foundation — created here, safe to evolve)

Every element below was created as part of the foundation. Identifiers are DOM
`id`/`class` or the Alpine method in [app.js](app.js). Another agent updating a
piece should keep its identifier and its emitted `action` stable (they're the
contract), or update this table when they change.

| Element | Identifier | Purpose | Events / actions |
|---|---|---|---|
| Topbar | `.topbar` | brand + crumb + stats/account | — |
| Icon rail | `.rail` | switches sidebar/panels | — |
| Sidebar | `.side` | hosts the project accordion | — |
| Project count | `.count` | repos located (`projects.length`) | — |
| Provenance icon | `.remote` (`bi-github`/`bi-hdd`) | GitHub-backed vs local-only, before the name | — |
| Canvas | `.canvas` / `.stage` | tabbed workspace: tab strip + dotted stage | — |
| Tab strip | `.tabbar` / `.tabstrip` / `.tab` | Agents (fixed) + closable file tabs | — |
| Tab lifecycle | `openTab` / `activate` / `closeTab` | open / switch / close a file tab | — |
| Console tools | `.canvas-tools` | New-console picker + Arrange, pinned right | `console-open` / `console-close` |
| Chrome tone | `--chrome` token | panels one step above `--bg` | — |
| Project accordion | `.projects` (+ `.has-open`) | single-open list; open hides siblings | — |
| Project row | `.project` (+ `.open`, `order:-1`) | one repo; open rises to top | — |
| Project header | `.project-head` | click = open/close | — |
| Accordion toggle | `toggle(slug)` | mounts/destroys the tree on open | — |
| Status dot | `dotClass(state)` | live / idle / offline colour | — |
| File tree host | `.wb-host` | Wunderbaum mount point | — |
| Tree theming | `.wb-host.wunderbaum` (compound!) | warm-dark `--wb-*` overrides | — |
| Tree mount | `mountTree()` / `destroyTree()` | build/tear the Wunderbaum from JSON `source` | — |
| Folder test | `isFolder(node)` | `node.folder \|\| node.children` (no `isFolder()` on node) | — |
| Icon inject | `withIcons(nodes)` | attach a file-type icon per node | — |
| Icon resolver | `fileIcon(title)` | ext → Devicon/Bootstrap class | — |
| Context menu | `#ctxmenu`, `.ctx-item` / `.ctx-sep` | right-click actions | see below |
| Menu show/hide | `showMenu(x,y,node)` / `hideMenu()` | build + place the menu | — |
| Path helper | `relPath(node)` | repo-relative path from parent titles | — |
| Copy path | `copyPath(node)` | clipboard + emit | `copy-path {path}` |
| **The seam** | `WB.emit` / `emit(action,node,extra)` | dispatch `workbench:action` (+ `console.log`) | **event `workbench:action`** |
| Scrollbars | `.projects` / `.wb-host` `::-webkit-scrollbar` | warm-dark themed | — |

**Actions emitted by the foundation** (payload always includes
`{project, path, title, isFolder, at}`; extras noted):
`copy-path {path}` · `create {kind:"file"|"folder"}` · `delete` ·
`rename {from,to}` (via the tree edit-apply). Rename edits inline via Wunderbaum's
edit extension; `Open` was foundational too but has since evolved into
`openFile(node)` (tabbed viewer — see wb-viewer.js).

---

## Chrome interactions & panels (added after the foundation)

The rail became interactive and the shell grew an account menu, a Settings modal,
a Security modal, and an opaque login gate (all wired in [app.js](app.js) /
[index.html](index.html); the two modals' *content* is data-driven from
[wb-settings.js](wb-settings.js)). Keep each identifier + emitted `action` stable —
they're the contract.

| Element | Identifier | Purpose | Events / actions |
|---|---|---|---|
| Rail toggles | `toggleSide` / `toggleRuns` / `toggleKanban` | show/hide sidebar · Runs panel · Kanban board | `kanban-toggle {open}` |
| Settings gear | `.rail-spacer` + `openSettings` | pinned to rail bottom; opens Settings | — |
| Runs panel | `.runs` (+ `body.runs-open`) | right-hand column; collapses to 0 | — |
| Account menu | `.avatar-btn` / `.account-menu` | Security settings · Log off | `logoff` / `login` |
| Settings modal | `.settings-modal` / `settings` / `settingsSection` | daemon + per-project config, data-driven | `setting-change {key,value}` |
| Settings scoping | `scope: "daemon"\|"project"` (wb-settings.js) | daemon group is machine-wide; project group follows `openSlug`, disabled when none open | — |
| Security modal | `.security-modal` / `security` | access token · password · TOTP 2FA | `totp-enroll` · `totp-revoke` · `password-set` · `password-clear` · `token-remint` · `require-login {on}` · `require-login-blocked` |
| TOTP QR | `wbQr(uri)` (wb-settings.js) | one-time `otpauth://` QR via vendored `qrcode` | — |
| Login gate | `.login-gate` (`!authed` + `body.locked`) | fully opaque lock; chrome blanked, nothing rendered behind | `login` |
| Console reflow | `clampAll` (ResizeObserver on `#workspace`) | keeps consoles inside the box on any panel resize | — |

Auth posture note (mock-faithful to current ralphy): token and TOTP are **mint-once**;
"revoke" is a **file deletion**, not a rotate command — there is no rotate/disable verb
in the tree today (a per-daemon revocable credential is ADR-0032 §8, Phase 2, unbuilt).

---

## Runs panel, run verbs & translation (added after the foundation)

The Runs panel became a real surface (module: [wb-runs.js](wb-runs.js)). It is
project-scoped and shows what's *running* in ralphy for the open repo. A project can
host **several concurrent runs** (one per `runid`), so a **run picker** chooses which
to inspect; below it an **issue trail** renders the run's queue, each node glyph-coded
by ralphy's real `IssueStatus` (done/skipped/blocked/infeasible/needs_split/non_green/
hitl, plus the active node's live phase or 🌙 sleep); below that a **plan viewer**
renders the active issue's `plan.md` — `## Steps` pinned on top, a dropdown reading
any other `##` section. The panel is **fed by events**: `applyRunEvent` folds a
CloudEvents-shaped `ralphy:run-event {type,runid,data}` to advance the run live, and
`window.WBRuns.emit(evt)` is the same door (a ⚡ demo button synthesizes the next
event to prove the path). This is the backend seam **into** the UI — the mirror of
`WB.emit` going out.

The three **daemon verbs** (`crates/ralphy-daemon/src/dispatch.rs`) live in the
panel's action bar (`.runs-actions`): `triage` and `push` are blessed no-arg
invocations fired straight onto the seam (`command {verb}`), the same "client never
composes a command line" contract as the daemon UI. `run` opens a **modal**
(`.run-modal`) that enriches it: pick the agent (executor, default claude), a checkbox
reveals a second picker to **plan with a different agent** (`--plan-agent`), and a
branch-mode segmented control — with a live `ralphy run …` preview. It emits
`run-start {agent,planAgent,branchMode,command}`.

**On-device translation** (module: [wb-translate.js](wb-translate.js)) reads plan/doc
prose in another language using the browser's built-in **Translator + LanguageDetector
APIs** — free, on-device, no network, no key (Chrome/Edge 138+). It's wired into two
places: each **Runs plan block** and the **markdown viewer** (preview only, never the
editor). A per-block target picker (PT/EN/ES/…) drives it; a same-language target says
"already X" instead of silently doing nothing; results are cached per target. Where the
API is absent the control is **hidden entirely** (not disabled).

| Element | Identifier | Purpose | Events / actions |
|---|---|---|---|
| Branch chip | `.branch-chip` + `canSwitchBranch(p)` | project-row chip → branch switcher; inert when unreachable | — |
| Branch switcher | `.branch-modal` / `openBranchModal` / `branchList` | filtered local branches (current pinned) + create-from-current | `branch-switch {branch}` · `branch-create {name,from}` |
| Runs toolbar | `.runs-actions` (run/triage/push) | the daemon verbs, scoped to the open project | `command {verb:"triage"\|"push"}` |
| Run modal | `.run-modal` / `runCfg` / `startRun` | agent (+ optional `--plan-agent` split) + branch mode + live preview | `run-start {agent,planAgent,branchMode,command}` |
| Run picker | `.run-select` / `currentRun` / `selectRun` | choose among concurrent runs (one per `runid`) | — |
| Issue trail | `.trail` / `.trail-node.st-*` | run queue, glyph-coded by `IssueStatus`; active node = phase / 🌙 sleep | `run-issue-focus {runid,issue}` |
| Plan viewer | `.plan-block` (`Steps` fixed + section dropdown) | render the active issue's plan.md | — |
| Inbound run events | `applyRunEvent` / `window.WBRuns.emit` | seam **into** the panel; folds CloudEvents to advance the run | consumes `ralphy:run-event {type,runid,data}` |
| Translate toggle | `.plan-xlate` (Runs) · `[data-act="xlate"]` (md viewer) | on-device translate of rendered prose; hidden where the API is absent | — |
| Console shortcuts | `consoleItems()` + Alt+Shift+`1/2/3/0` | New-console accelerators, matched by physical key (`e.code`), guarded off inputs/modals | `console-open {agent,plain}` |

Faithful sources: run/issue vocabulary and glyphs mirror `ralphy-cli/src/runstate/`
+ the Telegram/presenter tables; the verbs' argv is `dispatch.rs`; run flags
(`--agent` default claude, `--plan-agent`, `--branch-mode`) are `ralphy-cli/src/cli.rs`.

## Kanban board (added after the foundation)

The **Kanban** rail button opens the tasks board as an **overlay over the canvas**
(module: [wb-kanban.js](wb-kanban.js)) — the open project's GitHub issues placed by
**ralphy's own judgment**, project-scoped like the Runs panel. It is a **read-only
lens on the tracker**; the daemon never edits an issue's prose here. The *one*
mutation the board allows is **changing labels** (which is how a card moves between
columns) — everything else routes to GitHub via an **Open on GitHub** link on the
detail drawer.

**Four columns**, an issue landing in exactly one (precedence top-down, mirroring the
runner's queue precedence): **Closed** (grouped by `stateReason` — completed / not
planned) · **Ready for human** (`ready-for-human`/HITL — the human gate outranks
agent-eligibility) · **Ready for agent** (`ready-for-agent` **or** `AFK` — same intent)
· **Backlog** (everything else still open). The two **Ready** columns are ordered by the
**dependency graph** — a JS port of `sort_queue_in_graph` (`crates/ralphy-core/src/
blocked.rs`): **Kahn's algorithm** over `## Blocked by` edges, ascending issue number as
the tie-break, blockers walked transparently through open out-of-queue nodes, **closed
blockers pruned** (satisfied), a retired bundle's `## Parent` children standing in. The
order shown IS the order the runner would execute. **Backlog** is a flat list in issue
order with a board-wide **search**, a **label filter** (incl. *no label*), and a **sort**
control (newest / oldest / recently updated / title).

**Assignee scope (business rule — not yet applied in the mock seed).** The board is
**not** the whole tracker: it shows only issues an AFK agent may act on, scoped by
assignee. By default that is issues with **empty Assignees** (unassigned = up for
grabs); plus, when the operator sets one, issues matching the **configured
`queue.assignee`** (the same knob `ralphy run --assignee` / `queue.assignee` uses,
ADR-0021). So the effective set is *unassigned* **OR** *assignee = config value* —
anything assigned to someone else is hidden. Note this is deliberately a **union**,
which differs from the raw `gh --assignee <login>` semantics the CLI uses (that scopes
to the login *only*, excluding unassigned) and from the runner's unfiltered default
when `queue.assignee` is unset — the board's default is the stricter *unassigned-only*.
The real backend applies this when folding the tracker; the mock seed is small and
left unfiltered on purpose (design unchanged), so this rule is registered here for the
build, not enforced in `WB_KANBAN`. The detail drawer still shows each issue's
Assignees verbatim.

A card shows the number, title, label chips (in the repo's **real label colors**), a
close-reason badge, an assignee glyph, and a **lock** when it has an open blocker. The
**running signal**: an issue that is the *actively-worked* node of a live run (cross-ref
into `WB_RUNS` via `window.WBRun`) carries a **run pill** — the agent's face + the live
status glyph + phase — in whichever column it sits. Clicking a card opens the **detail
drawer** (slides from the right): state pill, Open-on-GitHub, the running banner, a meta
grid (column / assignees / opened / updated), **Blocked by** (each blocker with its live
open/closed state), the **editable labels** row, the rendered issue **body** and
**comments** (marked + DOMPurify), and the read-only footer.

The drawer's selection (`kanbanSel`) is held **by issue number**, so a label move that
re-columns the card keeps the drawer pointed at the same issue. It is **cleared whenever
the project opens/closes/switches** (`toggle()` resets `kanbanSel`) — a selection belongs
to the project that was open — and the drawer only takes its `.open` class when
`selectedIssue()` actually resolves, so a stale or empty selection can never leave an
empty strip on the right.

| Element | Identifier | Purpose | Events / actions |
|---|---|---|---|
| Board overlay | `.kanban` / `toggleKanban` / `kanbanOpen` | canvas overlay; project-scoped issue board | `kanban-toggle {open}` |
| Column classify | `WBKanban.columnOf(iss)` | closed → human → agent(`ready-for-agent`\|`AFK`) → backlog | — |
| Graph order | `WBKanban.orderGraph(queue, all)` | Kahn port of `blocked.rs`; orders the two Ready columns | — |
| Board filters | `kanbanFilter` · `kanbanLabel` · `kanbanSort` | search / label filter / Backlog sort | — |
| Card | `.kanban-card` (`.running`, `.closed`, `.sel`) | one issue; labels, blocker lock, close badge, run pill | `openIssue(number)` |
| Running pill | `WBKanban.runningFor(n, projectRuns)` | flags the active node of a live run (`window.WBRun`) | — |
| Detail drawer | `.kanban-detail` / `selectedIssue` / `kanbanSel` | GitHub-style read-only view + Open-on-GitHub; selection is by number, reset on project open/close/switch, `.open` requires `selectedIssue()` | `openIssue` · `closeIssue` |
| Label editor | `.kd-label-menu` / `toggleLabel(iss,label)` | the sole mutation; moves the card between columns | `issue-label-change {number,label,op}` |

Faithful sources: the label vocabulary + colors are the repo's `gh label list`; close
reasons are GitHub `stateReason`; the graph order is `ralphy-core/src/blocked.rs`; run
glyphs come from `window.WBRun` (wb-runs.js). **Backend gaps this exposes**: the core has
no *list-issues-with-bodies/comments* query yet (the board would fold it from the tracker
or an events snapshot), and `issue-label-change` maps to a `gh` label call the core does
own.

## The rest of the mock (documented at each file's head)

These were built out after the foundation; read the top-of-file comment in each
for intent + the real ralphy sources it mirrors:

- **[wb-viewer.js](wb-viewer.js)** — the closable file tabs: source via CodeMirror
  (highlight + edit + find), Markdown via marked + DOMPurify + mermaid, with a
  heading outline and in-page find. Per-file toolbar is **Find · Reload · Edit ·
  **Translate** · Save · Detach**; editing emits `save`, Reload reloads from source
  (`reload`), binaries are refused (`open-refused`). **Translate** (preview only —
  hidden in Edit) runs the shared on-device translator over the rendered markdown.
- **[wb-console.js](wb-console.js)** — the floating agent consoles on the Agents
  tab; mirrors the real daemon window chrome (`crates/ralphy-daemon/assets/ui/`).
  The New-console picker lists the agents and, **last**, a plain **console** (no
  agent — a shell in the repo dir, `plain:true`); each row has an **Alt+Shift+digit**
  accelerator (`1/2/3` agents, `0` console). Spawns a window per repo (`console-open`)
  and **Arrange** tiles them. The terminal is a faux local echo — swap for a real
  xterm.js over a WebSocket.
- **[wb-runs.js](wb-runs.js)** — the Runs panel model: the seeded runs, the pure
  helpers (status→glyph, plan.md section slicing, sleep countdown), and the inbound
  event fold. Data is what a backend folds from the CloudEvents bus (ADR-0019),
  faithful to ralphy's run/issue vocabulary. See the section above.
- **[wb-kanban.js](wb-kanban.js)** — the Kanban board model: the per-project issue
  seed (`WB_KANBAN`) and the pure helpers (`WBKanban`) — column classification, the
  Kahn graph-order port of `blocked.rs`, the running cross-ref into `WB_RUNS`, label
  metadata/colors, and filter/sort. Read-only except labels; see the section above.
- **[wb-translate.js](wb-translate.js)** — shared on-device translation
  (`window.WBTranslate`) over the browser's Translator + LanguageDetector APIs; used
  by both the Runs plan blocks and the markdown viewer. Free, on-device, degrades
  where the API is absent.
- **[wb-settings.js](wb-settings.js)** — data-driven Settings + Security (TOTP);
  its header lists the exact real config sources per key.
- **[detached.html](detached.html)** — a torn-off file viewer in its own popup
  window (read a file while watching an agent in the main window). Reuses
  wb-viewer verbatim; the file descriptor rides in the URL hash and Save / Reload /
  Re-attach talk back via **postMessage** (a `file://` opaque origin blocks reading
  shared globals off `window.opener`), re-emitted on the opener's seam.

---

## Starting points for the real build

1. **Stand up the seam listener** — subscribe to `workbench:action`, route each
   `action` to a daemon call. This unlocks everything else incrementally.
2. **Feed real data** — replace the seeded `projects` and file `tree` with
   `/api/repos` + a repo-tree endpoint (lazy-loaded).
3. **Promote hardened decisions to ADRs** — e.g. "canvas is a tabbed workspace,
   Agents tab fixed", "branch switching gated on reachability not remote" — and
   link them back here.
