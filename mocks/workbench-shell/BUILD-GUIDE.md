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
> Runs panel, Settings/Security) each carry their own header comment describing
> intent and backend sources; this guide points at them rather than restating.

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
right-hand panel, **Kanban** (`toggleKanban`, a stub → `kanban-toggle`) is the future
tasks board, and a **Settings gear** pinned to the rail's bottom (`.rail-spacer` +
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
Sidebar and the Wunderbaum viewport are themed to the warm-dark palette
(`--border` thumb, `--border-focus` on hover). Wunderbaum's viewport needs its own
rule because it lost the page default.

### Vendored libraries (loaded locally, no CDN at runtime)
`alpine.min.js` (reactivity), `lucide.min.js` (chrome icons — prune to a subset
when the icon set stabilises), `wunderbaum/` (tree), `devicon/` +
`bootstrap-icons/` (file icons). Later modules added `codemirror/`, `marked`,
`mermaid`, `dompurify`, `qrcode` — see their modules.

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
| Rail toggles | `toggleSide` / `toggleRuns` / `toggleKanban` | show/hide sidebar · Runs panel · Kanban stub | `kanban-toggle {open}` |
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

## The rest of the mock (documented at each file's head)

These were built out after the foundation; read the top-of-file comment in each
for intent + the real ralphy sources it mirrors:

- **[wb-viewer.js](wb-viewer.js)** — the closable file tabs: source via CodeMirror
  (highlight + edit + find), Markdown via marked + DOMPurify + mermaid, with a
  heading outline and in-page find. Per-file toolbar is **Find · Reload · Edit ·
  Save · Detach**; editing emits `save`, Reload reloads from source (`reload`),
  binaries are refused (`open-refused`).
- **[wb-console.js](wb-console.js)** — the floating agent consoles on the Agents
  tab; mirrors the real daemon window chrome (`crates/ralphy-daemon/assets/ui/`).
  An agent picker spawns a console per repo (`console-open`) and **Arrange** tiles
  the open windows. In the mock the terminal is a faux local echo — swap for a real
  xterm.js over a WebSocket.
- **[wb-runs.js](wb-runs.js)** — the Runs panel; data is what a backend folds from
  the CloudEvents bus (ADR-0019), faithful to ralphy's run/issue vocabulary.
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
