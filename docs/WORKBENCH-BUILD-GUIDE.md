# Workbench build guide

Read this before you change `crates/ralphy-daemon/assets/ui/`. It holds what
the code and the ADRs do not: the procedure for each vendored library, the
rules for touch input, and the rules a change to the page keeps. The UI gate
commands are in [AGENTS.md](../AGENTS.md). Traps for a browser test of the page
are in [TESTING-TRAPS.md](TESTING-TRAPS.md#the-workbench-page-in-a-browser).

## Who owns each fact

| Fact | Owner |
|---|---|
| The contract between the page and the daemon: verbs, effect classes, the tree watch | [ADR-0036](adr/0036-workbench-daemon-integration-protocol.md) |
| Colours and type. The tokens are in `styles/01-base.css` `:root`; a new colour is an amendment | [ADR-0035](adr/0035-daemon-ui-visual-language.md) |
| The tabbed canvas, with the Consoles tab fixed | [ADR-0037](adr/0037-workbench-canvas-tabbed-workspace.md) |
| Desk layout and locks are daemon state | [ADR-0050](adr/0050-desk-layout-is-daemon-state.md) |
| The stage is a plane; fences; maximize | [ADR-0051](adr/0051-consoles-stage-plane-and-fences.md) |
| The Runs panel feed (the run snapshot) | [ADR-0047](adr/0047-run-state-snapshot-channel.md) |
| Which fact the page may show, and a failed read shows `—`, never `0` | [ADR-0070](adr/0070-the-workbench-shows-only-what-it-has-read.md) |
| UI text | [ADR-0065](adr/0065-the-workbench-written-voice.md) |
| How the assets are gated | [ADR-0057](adr/0057-the-workbench-asset-contract.md) |
| The intent of one module and the Ralphy sources it mirrors | the header comment of that `wb-*.js` file |

## The seam: the page states intent, the daemon acts

A gesture (open, rename, delete, save, console-open, branch-switch,
setting-change) becomes one `workbench:action` event through `WB.emit(action,
detail)` in `app.js`. The page itself does not touch the file system, git or an
agent. `wb-daemon.js` turns an action into a daemon verb (`ACTION_TO_VERB`) and
routes the daemon's pushes back into the page.

The live list of actions is the code:

```sh
grep -rn "WB.emit(" crates/ralphy-daemon/assets/ui/*.js
```

An action name and its payload keys are a wire contract. Change both sides in
the same commit, or leave both unchanged.

## Vendored libraries

Every library is loaded from `assets/ui/vendor/`; the page loads nothing from a
CDN. `include_dir!` embeds and serves everything under `assets/ui/`, so a build
input (a `package.json`, a build script) lives outside that directory.

**The manifest.** `assets/ui/vendor/manifest.json` records each vendored
library: its name, npm package, version, source, and the SHA-256 of each of
its files. The test `vendored_files_match_the_manifest` recomputes every hash
and fails on a file that the manifest does not name. A library is updated by
hand and reviewed: the bump changes the files and the manifest in the same
commit. The daily `osv-scanner` job in `.github/workflows/security.yml` checks
the manifest's versions against the advisory databases (`cargo run -q -p xtask
-- vendor-lock <dir>/package-lock.json` writes the lockfile it reads; the file
must have that name). `vendor/**` is `-text` in `.gitattributes`, so a
checkout never changes the line endings of a hashed file.

**Load order.** `vendor/monaco/vs/loader.js` installs a global `define` with
`define.amd`. It must load AFTER every UMD library on the page (marked,
DOMPurify, mermaid, Wunderbaum, xterm and its addons). Otherwise they register
as anonymous AMD modules and never set their globals.

### Monaco (pinned `0.56.0`)

Monaco is the one editor engine of the workbench. `vendor/monaco/vs/` is the
`min/vs/**` directory of the `monaco-editor@0.56.0` npm tarball (the minified
AMD build), with these exclusions:

| Excluded | Why |
| --- | --- |
| `language/**` | a second, 7.7 MB copy of the four LSP modes |
| `nls/**` | localizations |
| `*.d.ts`, `*.map` | types and sourcemaps |
| `assets/{css,html,json,ts}.worker-*.js` | the four language workers (8.8 MB, `ts.worker` alone 7.0 MB). Language services are out of scope, and `wb-monaco.js` disables all four mode configurations at boot |

The embed-pin test `monaco_replaced_codemirror_in_the_embedded_ui`
(`crates/ralphy-daemon/src/tests.rs`) checks these rows by prefix.

- `assets/editor.worker-*.js` and `assets/editorWebWorkerMain-*.js` stay: they
  are the base editor worker. No page sets `globalThis.MonacoEnvironment`, so
  `vs/workers-*.js` falls through to `createWorker()`, which loads them. Keep
  `MonacoEnvironment.getWorkerUrl` unset: setting it sends the four remaining
  200-byte language-worker shims to the worker files that are not vendored.
- Keep every `basic-languages` grammar. They are chunks loaded on demand
  through a map inside `basic-languages/monaco.contribution.js`, so a deleted
  grammar stays registered and returns 404 on open. All ~90 are ~600 KB.
- 0.56 names its chunks by content hash (`editor-KLE6jdfb.js`). To bump Monaco,
  derive the file set again from the new tarball with the rules above; a diff
  of file names shows nothing useful.
- Measured at 0.56: 113 files, 5.31 MiB; the daemon crate's clean build grew
  8.5% (8.47s → 9.19s).
- A diff editor does not dispose its models. On every path, dispose both
  models before the editor: `m.original.dispose(); m.modified.dispose();
  ed.dispose()`.

### Crepe (pinned `7.22.2`), the one library that is built

The note card's editor is Milkdown Crepe, built lean into
`vendor/crepe/{crepe.js,crepe.css,LICENSE}` (715 KB + 20 KB, seven features,
no CodeMirror, no KaTeX). A tarball copy cannot reproduce it, so it has a
recipe in `crates/ralphy-daemon/vendor-build/crepe/`: `npm ci && node
build.mjs`, run by hand, never in CI. The first line of each output file names
the versions and the feature list; the test `vendored_crepe_states_its_recipe`
checks it.

### xterm (`@xterm/xterm` 6.0.0)

`vendor/xterm.js` and `xterm.css` are `@xterm/xterm` 6.0.0. The addons are
`@xterm/addon-fit` 0.11.0, `@xterm/addon-web-links` 0.12.0 and
`@xterm/addon-webgl` 0.19.0, copied from the `lib/` directory of each tarball
under the old `xterm-addon-*.js` names. OSC 52 is written by hand in
`wb-console.js` (`term.parser.registerOscHandler(52, …)`) instead of with
`@xterm/addon-clipboard`.

After a bump, check the four upstream behaviours the console depends on:
`parser.registerOscHandler`, `attachCustomKeyEventHandler`, the `contextmenu` →
`rightClickHandler` path (it moves the hidden textarea under the pointer with
the selection in it, so the browser's own **Copy** works over the WebGL
renderer), and the `copy` listener on the element.

### Wunderbaum

Wunderbaum (the file tree) was chosen because it has no jQuery.

- The `wunderbaum` class is on the host element itself, so theme selectors
  are compound: `.wb-host.wunderbaum`.
- A node has no `isFolder()`. Use `node.folder || node.children`.
- The tree is virtualized, so its host needs a real height. The flex-column
  chain in `styles/02-rail-sidebar.css` gives it one.
- When the tree loses focus it switches to its `--wb-*-grayscale` variables,
  whose defaults are near-white. The theme sets them to the same warm tone, so
  a selected row keeps its colour on blur.
- Monaco paints its own scrollbars. Colour them through
  `.monaco-scrollable-element > .scrollbar > .slider`; the
  `::-webkit-scrollbar` rules do not reach them.

### The others

`alpine`, `lucide`, `devicon`, `bootstrap-icons`, `marked`, `mermaid`,
`dompurify` and `qrcode` are single files or directories copied from upstream,
with no build step. The manifest has their versions. Two files have a local
change: `bootstrap-icons.min.css` keeps only the `woff2` font in its
`@font-face` list, and `devicon.min.css` keeps only the `woff` font, because
only those font files are vendored.

## The console clipboard

The clipboard rules protect the operator from a remote agent.

- **Copy** is the browser's context-menu Copy (the xterm path above) or
  `Ctrl+Insert`. `Ctrl+Shift+C` opens the DevTools inspector on Chrome and Edge,
  and a page cannot take that key back.
- **Paste** is the native `Ctrl+V` into xterm's hidden textarea, or the key
  bar's paste key. The paste key is the ONE call to
  `navigator.clipboard.readText()`: inside the operator's tap (Safari allows
  the read only there), feeding `term.paste`. Both paths reach `term.onData`,
  so a watching window refuses a paste as it refuses a keystroke. Code that an
  agent can trigger never reads the clipboard; a Rust test counts the call
  sites. On an insecure origin the key is disabled (`pasteOffered`).
- **OSC 52** only writes, and it is refused in two cases: during the replay of
  the console history on attach (the replay is raw bytes, so an old copy would
  rewrite the clipboard on every reconnect, takeover and reattach), and in a
  watching window (the window that holds the baton owns the clipboard). Control
  bytes and a trailing newline are removed from the text, so a wrong paste
  cannot run a command.

`tests/browser/console/wb_console_clipboard.py` covers this end to end.

## Touch: tablets and phones

The workbench is used from iPads and Android tablets and phones.

### Rules for a new gesture or control

- **Listen to Pointer Events, not mouse events.** iOS sends mouse events only
  after a tap ends and never during a drag. Each handle takes
  `touch-action: none` (or the browser claims the first pixels as a scroll and
  fires `pointercancel`) and `-webkit-user-select` /
  `-webkit-touch-callout: none`. Track one `pointerId`.
- **A press becomes a drag only past `dragThreshold`** (4px for a mouse, 10px
  for anything else). Take the grab offset at `pointerdown`, so the first move
  covers the whole distance. A press that never becomes a drag saves nothing:
  the desk fold keeps the newest `ts`, so a saved tap would win over a real
  move made on another device.
- **Refuse on a lock.** A gesture that changes a rect checks
  `isLocked(win)` / `fenceLocked(id)`. Maximize, fullscreen and close still
  work on a locked console, because they do not change the rect.
- **Size for a finger.** Buttons are at least 44px and take
  `touch-action: manipulation` (this removes the 300ms wait for a double tap).
  Under `(any-pointer: coarse)` the resize bands are 26px at corners and 14px
  at edges.
- **Send keys through `sendInput`**, so a watching window refuses them.

### Behaviour measured on devices

- **The finger scrolls the terminal.** xterm has no native scroller (it scrolls
  a synthetic viewport by transform), so a drag panned the whole page.
  `touch-action: none` on `.session-body` takes the drag back, and
  `touchScrollLines` turns it into `scrollLines`, with a `flingStep` glide. It
  is `none`, not `pinch-zoom`, because WebKit treats `pinch-zoom` as `auto`.
- **The drag has three owners** (`touchScrollTarget`): the application when it
  asked for mouse events (Claude Code and every full-screen TUI), arrow keys in
  the alternate buffer, and xterm's viewport otherwise. The application's
  share goes through xterm's own `wheel` listener, one line-mode `WheelEvent`
  per line. Scrolling the viewport under a TUI shows the app's old frames: an
  iPad report of "ghost text" was this.
- **Two fingers pan the canvas** (`touchGesture`), through the same scroll
  writes as the mouse pan. Under a maximized console two fingers do nothing.
- **The WebGL renderer is off on WebKit** (`prefersDomRenderer`): it draws
  scrolled rows twice on Safari and iPadOS (xterm.js #3357, #5816).
- **Fullscreen is not offered on WebKit** (`fullscreenOffered`): iOS leaves
  fullscreen when a text field takes focus, and the console focuses one on
  every tap. The PWA install (`display: standalone`) is the full-window mode
  on an iPad. `syncFullState` reads `document.fullscreenElement` every time,
  because a browser can leave fullscreen without telling the page.
- **The on-screen keyboard** (`keyboardInset`) publishes `--kb-inset` from
  `visualViewport` for maximized and fullscreen consoles. Chrome on Android
  does not need it: `interactive-widget=resizes-content` in the viewport meta
  shrinks the layout viewport.
- **On a phone, a maximized console hides the chrome** (`body.console-max`
  with the 560px media query in `01-base.css`). The gate is width, not pointer,
  so an iPad keeps its chrome.
- **A restored maximized console is raised last** (`raiseMaximized`), because
  windows are spawned in record order. A fixed `z-index` would block raising
  another window over it on purpose.
- **Resume.** A suspended tab comes back with dead sockets that still report
  OPEN; `visibilitychange` and `online` call `resumeAll`. The term is
  CONTEXT.md → *Resume*.

The pure rules are tested in `ui-tests/wb-console.test.mjs`; the browser
checks are `tests/browser/console/wb_console_touch.py` and `tests/browser/console/wb_console_phone.py`.

## Kanban: the assignee scope is not applied yet

The board must show only the issues an AFK agent may act on: issues with no
assignee, plus, when `queue.assignee` is set, issues assigned to that login.
This is a union. It differs from `ralphy run --assignee`, which shows only that
login, and from the runner's default, which does not filter. `wb-kanban.js`
does not apply it yet.

## Keep this guide current

Change a vendored library and its section here in the same commit. A decision
goes to an ADR, a test trap to TESTING-TRAPS.md, and the intent of a module to
its header comment.
