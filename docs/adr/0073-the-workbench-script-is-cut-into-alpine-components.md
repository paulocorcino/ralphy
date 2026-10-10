# The workbench script is cut by feature into Alpine components and Alpine stores

Status: accepted
Kind: structural
Protects: testability, extensibility

Amended by ADR-0075, proposed (D1 is replaced: the script is TypeScript,
built into ES modules by `build.rs`).

## Context

Two files hold most of the workbench script, and they grow fast. Measured on
`main` on 2026-10-04 with `wc -l` and `git log --since=2026-09-04`:

| File | Lines | 30 days ago | Commits in 30 days |
|---|---|---|---|
| `crates/ralphy-daemon/assets/ui/app.js` | 7,289 | 4,931 | 147 |
| `crates/ralphy-daemon/assets/ui/wb-console.js` | 7,482 | 4,210 | 117 |
| the other 27 `wb-*.js` files | 42 to 2,792 each | | |

These two files are 58% of the 25,352 lines of first-party script. The
diagnosis of 2026-09-30 records them as gap G11, and no ADR decides how the
workbench script is structured.

The other files are not the problem. Each one is a classic script that sets
one `window.WB<Name>` namespace from an immediately invoked function. Most of
them are pure folds: they take arguments and return values, and they read no
DOM. In `node --test`, 29 test files run the source again for each load,
with `new Function(...)` and new stubs, so each test gets a new copy of the
module. This pattern works, and this ADR keeps it.

The two large files are large for two different reasons:

- **`app.js`** is one Alpine component, `shell()`. It is one object literal
  with 467 methods, 184 data fields and 5 getters. The page (`index.html`)
  has about 1,351 Alpine bindings, and every binding sees every member. The
  code is already grouped in 35 marked sections, one for each feature, but
  any section can read or write any field.
- **`wb-console.js`** does not use Alpine. It builds the DOM itself (90
  `createElement` calls). Its 255 functions share 38 `let` bindings at module
  scope (`desk`, `fences`, `notes`, `z`, `cascade`, `detached`, and others).
  Any function can read or write any of this state.

ADR-0057 made a split of these files safe to check: the tag cross-check, the
tree sweeps and the `node --test` layer. It did not decide what the parts are.
A cut without a rule about how the parts talk moves the coupling into more
files and does not remove it.

## Decision

**D1. The workbench script stays classic scripts, with no build step.** A
first-party file in `assets/ui/` has no `import` or `export` statement, and no
`<script type="module">` loads it. It sets its namespace on `window`, as
today. Data that Alpine wraps (a component or an Alpine store) has no class
with `#private` fields, because Alpine reads it through a Proxy, and a private
field read through a Proxy throws a `TypeError`.

**D2. The `shell()` component owns the layout only.** That is the canvas
tabs, the side views, the route, the connection to the daemon, the modal
stack (`scrim()`), and the per-client view. A feature that has its own state
leaves `shell()` as an **Alpine component**: a factory registered with
`Alpine.data` and bound in `index.html` with `x-data="<name>"` on the element
that holds the feature's markup. New feature state is not added to `shell()`.

**D3. A component lives in its own file and can be built without Alpine.**
The file is `wb-<feature>-<part>.js` (for example `wb-hosts-dialog.js`). A
pure fold keeps its own file (`wb-hosts.js`) and does not touch the DOM or
Alpine. The component file registers the factory in an `alpine:init`
listener, and it also sets the factory on `window.WB<Name>`, so that
`node --test` can call it without Alpine.

**D4. A component reaches `shell()` only through the names it lists.** Each
component factory has a `uses` array with every `shell()` member that its
code or its markup reads or calls. It never assigns a `shell()` field. When it
must change one, it calls a `shell()` method that it lists in `uses`. In the
browser, Alpine lets a nested component see every member of the outer one.
This rule exists to stop that, because without it the component is the same
coupling in a new file.

**D5. A part of the page asks a component to act with a `workbench:<verb>`
event.** For example, the sidebar button that opens the Hosts dialog
dispatches `workbench:hosts-open`, and the component listens for it. No code
outside a component reads or writes the component's state.

**D6. A shown fact that several parts of the page read moves into an Alpine
store, as a whole.** The Alpine store is named for the fact. It follows
ADR-0070: it reads on the events of D2 there and shows a failed read as in D3
there. A fact moves only when all of its readers move with it. Until then, a
component reads the fact from `shell()` through its `uses` list. Code and
documents always say "Alpine store", never only "store", because in this
repository a store is a file the daemon owns (ADR-0070 D4, **Session store**).

**D7. `wb-console.js` is cut into factories that share one state object.**
Its module-scope `let` bindings move into one object. Each part of the file
becomes a factory that receives that object, in its own file. The public
`WBConsole` namespace does not change. This rule applies when the first cut of
`wb-console.js` ships, after its own pilot.

**D8. The two large files may only get smaller.** After the first cut of a
file ships, a ratchet holds its line count. A change that adds lines to
`app.js` or `wb-console.js` fails, and a change that removes lines lowers the
baseline in the same change.

**D9. `index.html` stays one file.** ADR-0057 D6 holds. A component changes
only the markup of its own feature: its `x-data` element, and the names its
bindings read.

## Consequences

The work is done in phases. A phase starts only when the phase before it
meets its exit criteria. If the pilot fails its criteria, this ADR is amended
or rejected, and no other feature moves.

1. **Pilot: the Hosts dialog.** It is about 290 lines of `shell()` (41
   members, from `addHostStep` to `copyHostCommand`) and about 280 lines of
   markup (the dialog, and its opener button in the sidebar). It reaches
   five `shell()` members, and it writes three `shell()` fields when a host is
   removed. It already has a fold (`wb-hosts.js`) and browser checks
   (`tests/browser/hosts/`). The pilot checks every mechanism in this ADR on a
   small feature: registration with classic scripts, a nested scope in the
   page, the opener event, the shared modal stack and Escape, the `uses`
   check, and one write replaced by a method. The pilot passes when all of
   these are true:
   - The behaviour does not change. `node --test` passes, and so do the
     browser checks `tests/browser/hosts/wb_hosts_497.py`,
     `tests/browser/hosts/wb_hosts_scroll.py` and
     `tests/browser/workbench/wb_modals_487.py`.
   - The `uses` list has 12 names or fewer.
   - The nested scope is supported by one shared harness helper. No test file
     outside the Hosts tests and the modal tests needs a change.
   - The component reaches `shell()` only through `uses`: no `Alpine.raw`,
     `$root` or `$data` that reads `shell()` state.
   - `app.js` gets smaller by at least the number of lines that moved out.

   The pilot pull request reports these numbers, and the count of asset pins
   before and after the cut.
2. **The other dialogs**, one for each pull request: Add a project, Settings,
   account and security (with the TOTP boxes), the login gate, About, What's
   new and update. The ratchet of D8 starts on `app.js` with the first of
   them.
3. **Alpine stores for shown facts**, one fact at a time, in the order of
   the fact index in `docs/ARCHITECTURE.md` §7, and only when a task already
   changes that fact.
4. **The panels with more state**: Runs, Kanban, Spend, the Files tree, and
   the Consoles tab bridge. The Files tree keeps `rawTree()`: Wunderbaum must
   not be changed through Alpine's Proxy.

The work on `wb-console.js` (D7) is a separate track, with its own pilot.
The pilot takes a part with little shared state first (the GPU budget,
dormant consoles, or the keyboard walk), and the desk layout last. Moving the
`let` bindings into one object needs a rename that follows scope, because
many functions have locals with the same names (16 declarations named
`fences`, 8 named `desk`). That rename is done once, with a tool that follows
scope. The tool is not added to the repository.

New costs:

- The `node --test` harness must evaluate a nested `x-data` scope. This is
  one shared helper, not one copy in each test file.
- Each cut moves the Rust pins on the moved text. ADR-0057 D1 says how. Run
  `cargo run -q -p xtask -- asset-pins` before and after the cut.
- Each cut runs the browser checks of its feature. Focus and Escape faults
  show only in a browser.
- `git blame` follows moved lines only with `-C`.

ES module syntax is decided again only if the script load order causes a
defect after phase 2. When every file uses the `window.WB<Name>` form, the
change to modules is mechanical.

## Considered options

- **ES modules (`import`/`export`) now.** The syntax does not make a file
  smaller. It also removes the new copy of a module that 29 test files make
  for each test, because Node runs a module and its imports once and then
  keeps them. And Alpine still needs a global for `x-data`.
- **Cut `shell()` into parts merged back into one object** (with
  `Object.defineProperties`). The files get smaller, but all 656 members stay
  in one `this`, so any part can still read or write any field.
- **The Alpine CSP build.** Its main gain would be to remove `'unsafe-eval'`,
  but `'unsafe-eval'` stays for Monaco (ADR-0072 D10). It needs Alpine 3.15 or
  later (we have 3.14.1), and about 34 expressions in `index.html` would need
  a rewrite, including the five `x-html` bindings that show Markdown.
- **A nested component that reads the outer scope with no list.** It is the
  default in Alpine, and it keeps the coupling hidden. D4 forbids it.
- **Another framework** (Lit, Preact with htm, petite-vue, web components).
  It means rewriting about 1,351 bindings and about 15,000 lines of tested
  behaviour, and it solves no problem that Alpine does not.
- **A codemod tool (jscodeshift) kept in the repository.** The cuts are moves
  of contiguous blocks, and a copy of the text that keeps every byte keeps
  `git blame -C`, the jscpd fingerprints and the pins. The one rename that
  needs a tool (D7) is done once.

## Compliance

- D1: not checked by code: reviewed in the PR. oxlint does not check the
  source type, and no test reads the `type` of a `<script>` tag.
- D2: not checked by code: reviewed in the PR. Which state is "layout" is a
  judgment. D8 limits how much `shell()` can grow.
- D3: checked by `crates/ralphy-daemon/src/tests.rs`
  (`every_alpine_component_can_be_built_and_has_a_test`, added by the pilot):
  every first-party file that calls `Alpine.data` also assigns a `window.WB*`
  name, or exports its factory when it is a module (ADR-0075), and has a
  `ui-tests/<file>.test.mjs`.
- D4: checked by `loadComponent` in `crates/ralphy-daemon/ui-tests/harness.mjs`
  (added by the pilot). It builds the component against a scope that has only
  its own members and its `uses` names. Reading any other name fails, and
  assigning a `shell()` field fails. A write to a property inside a `uses`
  value is not caught yet: the Security cut adds that (see the amendment). The
  component's test checks its markup with `componentMarkup` and
  `bindingNames` from the same file. The D3 check requires the test file that
  calls this helper.
- D5: not checked by code: reviewed in the PR.
- D6: not checked by code: reviewed in the PR, against the fact index.
- D7: not checked by code until its pilot ships. The pilot adds the check.
- D8: checked by `crates/xtask/tests/ratchets.rs` as a ratchet (added by the
  first cut of each file): the baseline is the line count of `app.js` and
  `wb-console.js`, and the check fails if a count goes up.
- D9: not checked by code: reviewed in the PR (as ADR-0057 D6).

## Amendment (2026-10-05): the pilot result, and phase 2 as measured

**The pilot passed on every mechanism and missed two exit criteria by their
wording.** The Hosts dialog moved in #544 (issue #542). `app.js` went from
7,443 to 7,155 lines, and 289 of its lines moved, with their text, into
`wb-hosts-dialog.js`. `uses` has 9 names. The asset pins were 801 before and
801 after, with the same count in each shape. The browser checks found no
fault that `node --test` missed. Two criteria were not met:

- A test file outside the Hosts tests and the modal tests changed:
  `wb-add-project.test.mjs`, 1 line. It pinned the opener's text
  `@click="openAddHost()"`, which D5 replaces.
- `app.js` got 288 lines smaller, not 289. The `shell()` method that D4
  requires (`hostRemoved`) added its own first and last lines.

Both misses come from D4 and D5, so every later cut would miss them in the
same way. From now on the two criteria read:

- A test that pins the text of a feature's opener counts as one of the
  feature's tests.
- `app.js` gets smaller by at least the number of lines that moved out, minus
  the lines of the `shell()` methods that D4 adds. The pull request lists those
  methods.

With this wording the pilot meets every criterion, and this ADR is accepted.

**D4 covers a write inside a field.** "Never assigns a `shell()` field" also
means no write to a property inside it (`this.security.x = …`). The pilot's
check does not catch that write yet, because the Hosts dialog makes none. The
first cut that reads an object from `shell()` adds it to `loadComponent`:
a `uses` value that is an object is given to the component read-only.

**An opener in the account menu closes the menu itself.** The account menu
(`avatarMenu`) is part of the header, so it stays in `shell()` (D2). Today
`openSettings`, `openSecurity`, `openWhatsNew` and `openAbout` close it. After
their cuts, the opener's own markup, which is in `shell()` scope, closes the
menu and sends the event.

**"Is a modal open" is asked of the modal stack.** `consoleShortcutsBlocked`
and the Ctrl+Shift+F listener in `app.js` read the open flags of several
dialogs. D5 forbids that once a flag moves into a component. The cut that moves
a flag replaces its read with a question to the modal stack, which `shell()`
owns (D2), and the behaviour does not change.

**What stays in `shell()` during phase 2.** Measured on `main` at `bde6f99c`:

- **The login gate and the TOTP digit boxes.** The login gate is part of the
  connection to the daemon (D2). After a login, `rehydrateAfterAuth` reads
  every panel again. The gate reads 20 `shell()` names and writes 4. The TOTP
  boxes are used only by the gate's markup, not by the Security dialog as the
  Consequences said.
- **The security fact:** `security.policy`, `passwordSet`, `tokenSet`,
  `totpEnrolled`, `requireLogin` and `remoteImages`. The boot, the login gate
  and log off read it. The form state of the Security dialog moves; the
  dialog changes the fact through one `shell()` method.
- **The release fact:** `release`, `releaseSeen`, `releaseRead`,
  `loadRelease` and the getters `releaseHasNews`, `releaseUnread` and
  `releaseSummary`. The sidebar reads it. It moves into an Alpine store (D6)
  only together with all of its readers.
- **`onTabVisible` and `rereadOpenPanels`.** They sit among the release code
  but are page layout.

**Phase 2, in this order.** Measured with code and markup on `main` at
`bde6f99c` (`uses` is the count of `shell()` names read; writes are writes to
`shell()` fields today, which the cut replaces with methods):

| Cut | `app.js` lines | Markup lines | `uses` | Writes today |
|---|---|---|---|---|
| Add a project | 133 | 54 | 8 | none |
| Security dialog | about 370 | 173 | 5 | `avatarMenu`, and fields inside `security` (about 60 writes) |
| Settings | 196 | 206 | 7 | `avatarMenu` |
| About, What's new and update | 186 | 178 | 12 | `avatarMenu`, `release`, `releaseSeen` |

Together this is about 890 lines of `app.js`. The ratchet of D8 starts with
Add a project.

**`wb-console.js`: the pure functions leave before the D7 pilot.** Measured on
`main` at `bde6f99c`, `wb-console.js` has 7,864 lines and 280 top-level
functions. 164 of them (3,755 lines) read no module-scope `let`. 84 of them
(about 1,125 lines) read no module state and no DOM, and call only functions
of the same kind. Those 84 are pure folds, so by D3 they go to files of their
own, as `wb-geometry.js` did by ADR-0057. They need no shared state object,
so they move before the D7 pilot. There are four files, one for each theme:
input, session, desk, and plane geometry. The plane geometry goes into
`wb-geometry.js`. `WBConsole` exports them again, so its public namespace does
not change (D7). The ratchet of D8 starts on `wb-console.js` with the first of
these moves. The D7 pilot comes after them. It takes the GPU budget and the
dormant consoles (about 130 lines, with two module `let`s of their own).

## Amendment (2026-10-08): the order of the shell() cuts, the projects Alpine store, and the project-changed event

Phase 4 starts here. Moving code inside `app.ts` did not make it smaller
(issue #550), so a part leaves `shell()` only when the state it shares has one
owner. This amendment records the order of the cuts and the decisions they need
before code. It was measured on `8fa01411` (the section map is in the 2026-10-08
measure of #605; `cargo run -q -p xtask -- ui-groups` prints it again).
`app.ts` has 6,134 lines, and `shell()` has 544 members in 35 sections. In
`index.html`, 901 bindings are in `shell()` scope, and 635 of them name a
member. 65 of these bindings mix two groups, and none mixes two feature groups.
Members per group: core 188, git 77, files 82, board 120, consoles 77. (The
first measure had core 173 and files 97. The 15 members of the canvas tabs
are counted in core, by decision 2.)

1. **The order of the cuts.**
   1. The open project becomes the Alpine store `projects` (decision 3), with
      the event of decision 4.
   2. The consoles become a component.
   3. The board and the runs become components.
   4. Git becomes a component.
   5. Files are measured again, then planned.

   The cuts of `wb-console.ts` (#597) follow their own plan and can run in
   parallel. Each step is written after the step before it has merged, from a
   new measure.

2. **The canvas tabs stay in `shell()` as layout.** `tabs`, `active` and `slot`
   are read by the layout core, the consoles and the window bridge, so D2
   holds: they are layout. The group "files and tabs" is now called "files".
   They do not become an Alpine store.

3. **The open project is the Alpine store `projects`**, in
   `wb-projects-store.ts`. Its members are `openSlug`, `projects`, `repoRef`,
   `openProject` and the project-label helpers (`projectLabel`,
   `projectTitle`, `rowOpen`). `projectBadge` stays in `shell()`, because it
   also reads the change count of the git part. Measured, only the layout core
   writes this fact: `openSlug` only in `toggle` and `removeProject`, and
   `projects` in `loadRepos`, `reposFailed`, the fleet path and
   `removeProject`. Every group, the window bridge and four dialogs read it.
   - **One writer.** The store changes only through its own methods. No other
     code assigns its fields (D4 applies to the store too).
   - **How each part reads it.** `shell()` code reads
     `this.$store.projects`. Markup reads `$store.projects`. A component reads
     it through the Alpine `$store` magic, and its name leaves the `uses` list.
     Code outside Alpine (the window bridge) reads `Alpine.store("projects")`.
   - **A helper that needs `shell()` state too** stays in `shell()` and reads
     the store. The store holds only what it owns.
   - The store follows D6: it is named for the fact, and the test harness
     builds it without Alpine, as a component.

4. **`toggle` sends `workbench:project-changed`.** The event goes on `window`
   (D5) with the detail `{ slug, previous }`. Today `toggle` writes fields of other groups
   (git: `commitMsg`, `commitMsgSlug`, `branchError`; board: `kanbanSel`,
   `trailFocus`, `currentRunId`, `planSection`, `verbError`; and
   `changesError`, which only the Changes view reads) and restarts their
   sockets and reads. Git, board and files listen to the
   event and reset their own fields, restart their own sockets, and read again,
   in the order `toggle` uses today. `toggle` writes the store and sends the
   event; it writes no field of another group.

5. **Modal flags read across groups become `modalOpen()` questions** when
   their group moves, as the 2026-10-05 amendment says. `branchOpen` (git) and
   `runOpen` (board) are read by the consoles (`consoleShortcutsBlocked`) and by
   the window bridge. `planModal` (board) is read by the layout core. No other
   modal flag is read across groups in this measure.

6. **Small shared facts follow one rule.** The facts are the runs
   (`hydrateRuns`), the board read state (`loadBoard`), the change count
   (`loadChanges`), the agent roster (`loadAgents`), and the live sessions and
   the login (`refreshLive`, `probeSession`). A fact stays in `shell()` until
   its last reader outside its owner group has moved. Then it moves with its
   owner. If two components still read it, it moves into an Alpine store (D6)
   instead. The writer is always one method.

7. **Pins follow the text.** A cut runs `cargo run -q -p xtask -- asset-pins`
   before and after. Each pin on moved text reads the new file and protects the
   same claim, or a unit test or a browser check replaces it. A pin is never
   deleted without a replacement. The pull request shows the count before and
   after.

## Amendment (2026-10-09): app.ts ends near 500 lines

The goal of issue #621 (a part of #605) is that `app.ts` ends near 500 lines
and stops owning features. It only builds and connects the parts. This
amendment records the goal, the order of the cuts, and the decisions the cuts
need before code. It was measured on `f9d0aae3`: `app.ts` has 6,123 lines.
The consoles group is 77 members (about 554 lines), and the files group is 83
members (1,385 lines). `cargo run -q -p xtask -- ui-groups` prints the map
again.

1. **The goal.** `shell()` keeps only the side panel, the canvas tabs and the
   slot, with the tab operations (`openTab`, `openDiff`, `activate`,
   `repathTabs`, `syncViewer`, `detachFile`), the shared facts that still have
   readers in two parts, and the `init` that starts the parts. Every other
   part of the layout core becomes an Alpine component or an Alpine store.
   This narrows D2: layout is the canvas tabs and the slot, not the whole core.

2. **The order of the cuts.**
   1. Plan 1: the consoles (two components) and the files (one component and
      the move dialog).
   2. Plan 2: an Alpine store `runs` first, then the runs panel, the board with
      the plan and run dialogs, then git. The board and the runs wait for the
      store, because their markup is four separate regions with only `body` as
      the common parent.
   3. Plan 3: the core parts: the connection and presence, the modal stack,
      release, account, login and TOTP, spend and Ledger, the chrome panels,
      the context menu and the backend seam, `wire()` and the file helpers.

   Each plan is written after the plan before it has merged, from a new
   measure. The cuts of `wb-console.ts` follow
   [ADR-0075](./0075-the-workbench-script-is-written-in-typescript.md) and
   run in the same plans.

3. **A component's markup root is a `<div x-data="wbX">`.** A part whose
   markup sits inside an `x-for` row does not get one instance for each row.
   It gets one sibling element, shown for the open row. Files is the case: one
   `wbFiles` element in `ul.projects`, shown for the open project.

4. **The order of the `workbench:project-changed` listeners is not a
   contract.** Each part resets only its own state. The test that checks "the
   tree, the board, then git" changes to check that each part reacts.

5. **A browser check reads a component through its root.** The shell data
   stack does not hold a member that moved into a component, so a check reads
   it with `Alpine.$data(document.querySelector('[x-data="wbX"]'))`. A cut
   updates the checks that read the members it moves.

6. **A component declares every field it assigns.** In Alpine 3.14,
   `mergeProxies` writes a key that no scope owns to the last object in the
   data stack, which is the shell. A field created later is declared as
   `null` first. A component never assigns a shell field (D4): it calls a
   shell method listed in `uses`, or it sends a `workbench:<verb>` event.

## Amendment (2026-10-10): owners for the stage, the desk cards and operator messages; one prefix per domain

Issue #623 (a part of #605, next to #621) found three boundaries that the cuts
above do not cover: the **stage** has no owner, the cards reach the console
through `window.WBConsole`, and there are two ways to tell the operator
something. This amendment records where those boundaries are, and the file
names of each domain, before any code moves. It was measured on `823cc84c`:
`app.ts` has 4,209 lines, `wb-console.ts` 3,086, and `wb-notes.ts` 2,828.
`wb-notes.ts` names `WBConsole` 49 times in its code, for 22 members. Six
`wb-*.ts` modules name the shell's private `_flashAction` 19 times.

1. **One prefix per domain, and no subfolders.** A module's name starts with
   the domain it belongs to: `wb-stage-*` for the **stage**, `wb-desk-*` for
   the **desk layout**, `wb-notes-*` for the cards, and `wb-console-*` only for
   the console itself (`-terminal`, `-session`, `-input`, `-gpu`, `-name`,
   `-title`). This narrows decision 1 of the ADR-0075 amendment of 2026-10-09,
   which said that every theme of `createConsole` becomes a `wb-console-*.ts`
   factory: a factory still leaves `createConsole`, but its file takes the
   prefix of its domain. Nine modules are renamed in one change, before the
   cuts, so each cut moves code into a file that already has its final name:

   | Today | New name | Domain |
   |---|---|---|
   | `wb-console-chrome.ts` | `wb-stage-chrome.ts` | stage |
   | `wb-console-view.ts` | `wb-stage-view.ts` | stage |
   | `wb-console-fences.ts` | `wb-stage-fences.ts` | stage |
   | `wb-console-fence-list.ts` | `wb-stage-fence-list.ts` | stage |
   | `wb-console-desk.ts` | `wb-desk.ts` | desk layout |
   | `wb-console-popups.ts` | `wb-desk-popups.ts` | desk layout |
   | `wb-console-detach.ts` | `wb-desk-detach.ts` | desk layout |
   | `wb-view.ts` | `wb-client-view.ts` | per-client view |
   | `wb-viewer.ts` | `wb-file-viewer.ts` | canvas and files |

   A rename changes the file, its test file and the import lines. The names on
   `window` (`WBView`, `WBViewer`, and the others) do not change. Every module
   keeps the `wb-` prefix and stays directly in `assets/ui/`, because
   `xtask ui-copy` reads only those files. Subfolders need four build changes
   and an amendment of their own.

2. **The stage has two owners: one for each document, and one inside the
   console.**
   - **The z stack and the gestures are once per document.** Console windows,
     fences and cards share one z counter today, and `focusWin` orders
     `.session-window` and `.note-card` together. `createStack(document)` in
     `wb-stage-stack.ts` holds the z counter, `focusWin` and `stackWin`. The
     console's own work on a focus change (dormancy on blur, the GPU schedule)
     goes in through a hook on the instance. `createGestures()` is built next to
     it. Each entry module (`main.ts`, `detached-fence-main.ts`) builds both and
     passes them to the console options and to the cards. The popup has its own
     document, so it builds its own. Nothing is kept at module scope
     (ADR-0075 D7).
   - **The window states stay inside `createConsole`.** Maximize, lock, full
     screen, the extent, `restoreRect` and the column paint read the windows,
     `setWin`, the fences and the GPU budget. They become `wb-stage-*`
     factories with typed `deps`, built inside `createConsole`:
     `wb-stage-window.ts` (maximize, lock, full screen, extent, `restoreRect`)
     and `wb-stage-columns.ts` (the column paint). The column grid keeps its one
     writer, `wbColumns` in `wb-consoles-tab.ts`, and its fold, `wb-columns.ts`.

3. **The desk layout stays inside `createConsole`, and the desk module owns the
   record operations.** One document holds windows, fences and cards with one
   `WBDeskSync`. The popup is protected by the desk sink passed in the console
   options, as today. The record operations (create, set, forget, commit, save,
   checkouts, the caps, then the desk read, reload, converge, flush and
   `pagehide`) move from `wb-console.ts` to `wb-desk.ts`. The order of
   construction does not change: the desk read still starts while
   `createConsole` is built.

4. **The cards take the console as a typed dep.** `createNotes(window,
   document, deps)` gets `deps.console: CardHost | null`. `CardHost` is a
   `Pick` of the console instance type that lists the members a card uses. The
   entry modules pass `window.WBConsole`, and a unit test passes `null` or a
   stub. `wb-notes.ts` then names `WBConsole` 0 times. The cards get no new
   console member: the add, remove and cap logic that `wb-notes.ts` writes two
   and three times becomes pure folds in `wb-desk-folds.ts`, which the desk and
   the cards import. A card still writes through `saveNotes`.

5. **One door for operator messages, with two drawings.** An **operator
   message** (CONTEXT.md) is a confirm, a notice, a toast or a flash. One
   module, `wb-messages.ts`, owns all four, and every module calls only it.
   Each entry module builds it with `createMessages(window, document,
   { shell })`. `main.ts` passes `() => window.getShell?.() ?? null`, and the
   popup passes `() => null`. Inside, the door draws with the shell's Alpine
   modal when the shell exists, and with the DOM dialog in the detached-fence
   popup, which has no shell. The DOM dialog uses the shell's classes, so the
   operator sees no change. The console's `askConfirm`, `askNotice`, `toast`
   and `dismissToast` members stay, and pass the call to the door. No
   `wb-*.ts` module names `_flashAction` after this cut.

6. **The `WBConsole` and `WBNotes` member lists do not change.** Browser
   checks call `WBConsole.notes`, `saveNotes`, `focusWin`, `focusedId`,
   `WBNotes.cardEl`, `keepOnTop`, `onTopNow` and `flushAll` directly. A member
   whose last internal user leaves stays, as a delegate. The test
   `ui-tests/wb-api-members.test.mjs` compares both lists, with the type of
   each value, with the lists at `823cc84c` (159 and 62 members). This is the
   test that decision 2 of the ADR-0075 amendment of 2026-10-09 asks for. A
   member list changes only by a design decision.

7. **`wb-notes.ts` gets the D8 line ratchet, and is cut after the stage
   owner.** `the_notes_script_matches_the_line_baseline` holds it at 2,828
   lines from now on. It is cut in this order, after the cards take their
   typed deps and the card folds are shared: the folds and the front matter
   (`wb-notes-folds.ts`); the editor (`wb-notes-editor.ts`); then the card on
   top, the veil and the map (`wb-notes-on-top.ts`, `wb-notes-veil.ts`,
   `wb-notes-map.ts`).

8. **Three coupling ratchets hold the boundaries.** Each is exact, as the line
   ratchets are: a change that lowers a count lowers its baseline. They read
   the modules through the `ui-copy` lexer, so a comment does not count.

   | Ratchet | At `823cc84c` | Target |
   |---|---|---|
   | `NOTES_CONSOLE_REACH`: uses of `WBConsole` in `wb-notes.ts` | 49 | 0, when the cards take `CardHost` (decision 4) |
   | `SHELL_FLASH_REACH`: uses of `_flashAction` in each `wb-*.ts` module (`app.ts` owns it) | 19 in 6 modules | an empty table, when the door exists (decision 5) |
   | `the_column_paint_has_one_owner`: each column paint function is declared in one module | `wb-console.ts` | `wb-stage-columns.ts` (decision 2) |

The order of the work: the renames, the cards' typed deps, the message door,
the window states, the z stack and the gestures, the column paint, the desk
records, the card folds, then the cuts of `wb-notes.ts`. The `app.ts` cuts of
#621, other than the message door, are not part of this work.
