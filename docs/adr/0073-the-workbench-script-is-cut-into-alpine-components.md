# The workbench script is cut by feature into Alpine components and Alpine stores

Status: accepted
Kind: structural
Protects: testability, extensibility

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
  (`every_alpine_component_is_on_window_and_has_a_test`, added by the pilot):
  every first-party file that calls `Alpine.data` also assigns a `window.WB*`
  name and has a `ui-tests/<file>.test.mjs`.
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
