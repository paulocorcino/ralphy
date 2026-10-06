# The workbench script is written in TypeScript and built into JavaScript modules

Status: accepted
Kind: structural
Protects: extensibility, testability

## Context

The workbench script is 38 first-party classic scripts in
`crates/ralphy-daemon/assets/ui/` (26,934 lines on 2026-10-06, `wc -l`). Each
file sets one `window.WB<Name>` namespace, and `index.html` loads them with 53
`<script>` tags in an order that a reader must know. ADR-0073 cuts the two
large files into Alpine components, and its D1 keeps classic scripts with no
build step. ADR-0073 did not weigh TypeScript.

Most changes to this code are made by agents. An agent reads the shape of a
state object, of a daemon reply, and of the `shell()` members a component may
use, from the code around the change. Today nothing checks that shape when the
agent writes the code. A wrong name shows up only when a test reaches it, or in
the browser.

What was measured, with `tsc` 5.9.3 `--noEmit --allowJs --checkJs` on
2026-10-06:

- `wb-hosts.js`, `wb-hosts-dialog.js` and `wb-console.js` (8,658 lines): no
  defect that the tests miss. A checker on the current code is not worth its
  cost as a bug finder.
- In strict mode, 5 of the errors on `wb-hosts-dialog.js` are reads of
  `shell()` names from its `uses` list (ADR-0073 D4). A type can state that list, so the
  checker refuses a name outside it while the agent writes the code.

So the gain is a contract that the agent can read and that a tool checks, not
bugs found in old code.

Facts that fix the shape of the build:

- The daemon embeds the UI tree with `include_dir!`
  (`crates/ralphy-daemon/src/lib.rs:70`). Every `cargo build`, debug or
  release, and every `cargo install`, needs the JavaScript. `include_dir!`
  expands `$OUT_DIR` in its path (`include_dir_macros` 0.7.4,
  `src/lib.rs:170`), so `build.rs` can write the tree it embeds.
- `oxc_transformer` (0.153.0) removes TypeScript types in Rust, with
  `oxc_parser` and `oxc_codegen`. The gate already uses oxlint, from the same
  project.
- Node 22.18 and later run a `.ts` file by removing its types, with no flag
  (local: v22.22.2; CI: `node-version: "22"`, `ci.yml:72`). This works only for
  syntax that can be erased: no `enum`, no `namespace`, no parameter
  properties.
- A browser loads `<script type="module">` with a CORS request. Under
  `file://` the origin is `null`, and the module is refused. The static demo
  (`wb-mode.js:6`, `assets/ui-demo/`) runs under `file://`.
- Alpine's ES module build is started by the page with `Alpine.start()`, once,
  after every `Alpine.data` and `Alpine.store` call. The CDN build that is
  vendored today (`vendor/alpine.min.js`) starts itself.
- Module scripts and `defer` scripts run in document order after the page is
  parsed. Classic scripts without `defer` run before them, while the page is
  parsed. `vendor/alpine.min.js` is the last tag and has `defer`
  (`index.html:3300`).

## Decision

**D1. First-party workbench code is TypeScript.** A first-party script in
`assets/ui/` is a `.ts` file. It uses only syntax that can be erased:
`tsconfig.json` sets `erasableSyntaxOnly`, `strict` and `noEmit`. HTML, CSS and
the files under `vendor/` stay as they are.

**D2. A `.ts` file is an ES module.** It states what it needs with `import`
and what it gives with `export`. It does not read a `window.WB<Name>`
namespace of another first-party file, and it does not set one, except for the
names that the migration in D9 needs. An import path names the `.ts` file.

**D3. `build.rs` builds the served tree, and `cargo` needs no Node.** The
daemon's `build.rs` copies `assets/ui/` to `$OUT_DIR/ui`. It writes each `.ts`
file as a `.js` file with its types removed and its import paths changed from
`.ts` to `.js`, with `oxc_transformer`. It copies every other file unchanged.
`lib.rs` embeds `$OUT_DIR/ui`. The output is not committed. A `.ts` file that
`oxc_transformer` cannot parse fails the build.

**D4. The type check is part of the gate, and nothing is installed for it.**
The gate runs `tsc --noEmit -p crates/ralphy-daemon/assets/ui` with a pinned
version through `npx -y`, as it runs oxlint and jscpd. The types of a vendored
library come from a declaration file in the repository, not from an npm
package. `build.rs` does not check types: it only removes them.

**D5. The browser loads one entry module.** Each page loads one
`<script type="module">` (`main.ts` for `index.html`, and one entry for each
torn-off window page). The entry imports the parts of the page, registers each
`Alpine.data` and `Alpine.store`, and then calls `Alpine.start()` once. Alpine
is the ES module build, vendored and recorded in the manifest (ADR-0072 D12).
There is no bundler: the browser follows the imports.

**D6. A component states its `uses` list as a type.** The `this` of an Alpine
component is typed as its own members plus `Pick<Shell, …>` of the names it
lists in `uses` (ADR-0073 D4). The type check refuses any other `shell()`
name. The `loadComponent` check in the test harness stays until `index.html`
bindings are checked too, because `tsc` does not read the markup.

**D7. `node --test` runs the `.ts` source.** A test file imports the module
it tests. A test that needs a new copy of a module's state gets it from a
factory that the module exports, not by running the source again with
`new Function`. A module keeps no state at module scope that a test cannot
reset.

**D8. The `file://` demo is archived, not deleted.** The workbench runs only
when the daemon serves it. The demo leaves the source tree in its own pull
request, the first step of the migration, and that pull request does three
things:

- It tags the last commit where the demo runs, as the annotated tag
  `workbench-demo-archive`. A checkout of that tag opens `index.html` from
  disk, as before.
- It adds `docs/archive/workbench-demo.md`. The note says what the demo shows
  (projects, runs, kanban, files and the agent roster, from five seed files),
  what it does not show (consoles, hosts, devices, notes), every place in the
  code that read `WBMode`, the tag, and the ways to bring a demo back with
  their costs (see Considered options).
- It removes `wb-mode.js`, the `isDemo` and `seedAllowed` branches, the
  `"*"` target of `postMessage` that only the demo needed, and
  `assets/ui-demo/`.

**D9. The migration goes file by file, and both forms work during it.** While
classic scripts remain, `build.rs` copies a `.js` file unchanged. A file moves
to TypeScript as one change, with its test. A module tag sits before the
`vendor/alpine.min.js` tag, so it runs before Alpine starts and its
`alpine:init` listener still works. A classic script reads the namespace of a
module only inside a function that runs after the page has loaded, never while
the page is parsed. A module that a classic script still reads also sets its
`window.WB<Name>` namespace, until its last classic reader moves. D5 replaces
the CDN Alpine build when the last classic script moves.

**D10. No `.js` file is written by hand in `assets/ui/` after the migration.**
The only `.js` files in the source tree are under `vendor/`.

## Consequences

This decision amends ADR-0073 and ADR-0057:

- ADR-0073 D1 (classic scripts, no build step) is replaced by D1 to D3 here.
  ADR-0073 D3 changes: a component is exported from its module, and
  `window.WB<Name>` is no longer how a test reaches it. D2, D4 to D9 of
  ADR-0073 stay. The cuts that ADR-0073 still plans are made in TypeScript.
- ADR-0057 D3 (the gate needs no npm install) holds, and the gate adds the
  type check of D4. ADR-0057 D4 reads `wb-foo.ts` is tested by
  `ui-tests/wb-foo.test.mjs`.

The work is done in phases. A phase starts only when the phase before it
meets its exit criteria. If the pilot fails its criteria, this ADR is amended
or rejected, and no other file moves.

1. **Archive the demo** (D8).
2. **Pilot: the build and one pair.** `build.rs` with `oxc_transformer`
   (D3), `include_dir!("$OUT_DIR/ui")`, `tsconfig.json`, the gate command
   (D4), and `wb-hosts.js` with `wb-hosts-dialog.js` moved to TypeScript as
   modules, loaded by one module tag (D9). The pilot passes when all of these
   are true:
   - The behaviour does not change. `node --test` passes, and so do the
     browser checks `tests/browser/hosts/wb_hosts_497.py`,
     `tests/browser/hosts/wb_hosts_scroll.py` and
     `tests/browser/workbench/wb_modals_487.py`.
   - `cargo build` and `cargo install --path crates/ralphy-cli` work on
     Windows, Linux and macOS with no Node on the path.
   - `tsc --noEmit` passes in strict mode on the two files, and a name outside
     `uses` fails it (D6).
   - The two test files import the `.ts` modules, with no `new Function`.
   - The pilot pull request reports: the time `build.rs` adds to a clean and
     to an incremental `cargo build`, the count of crates that `oxc_*` adds to
     `Cargo.lock`, the asset pins before and after, and what jscpd reports for
     a file renamed from `.js` to `.ts`.
3. **The pure folds**, one or a few files for each pull request, from the
   files that nothing else reads at load time.
4. **The Alpine components and `app.js`.** Then `main.ts` and the Alpine ES
   module build (D5).
5. **`wb-console.js`**, together with its ADR-0073 D7 track.

New costs:

- The daemon gets new build dependencies. The `oxc_*` crates are 0.x and
  break their API often; a version bump is a reviewed change, not a bot merge.
- Each tool that reads `assets/ui/` by file name learns `.ts`: the asset pins
  (`include_str!` paths), `xtask ui-copy`, the check that user text cites no
  ADR, `.jscpd.json` (`format`), and `.oxlintrc.json`.
- The browser runs the built file. To find a line, a developer reads the
  `.js` in the browser, which keeps the source lines but not the types. There
  is no source map.
- The static demo runs only from the archive tag. A screenshot or a
  walkthrough of the current workbench needs a running daemon with a scratch
  store.

## Considered options

- **Keep JavaScript and add JSDoc types with `checkJs`.** No build step and no
  change to the tests. It gives the same checker, but the agent writes types in
  comments, and a module boundary is still a convention. Rejected: the code is
  written by agents, and `import`/`export` with types states the dependency and
  the contract in the code itself.
- **Commit the built JavaScript.** `cargo` would need no build step, but every
  pull request would show each change twice, and CI would need a check that the
  two copies match.
- **Run a Node tool from `build.rs`** (esbuild, `tsc`). Every developer, every
  CI job and every `cargo install` would need Node and the network on the first
  build.
- **One bundle** (esbuild, Rolldown). It would keep the `file://` demo and
  load one file. It needs a bundler in the Rust build, a large dependency, and a
  source map to read the code in the browser. With native modules the browser
  follows the imports, and the daemon already serves each file with an `ETag`
  and gzip.
- **Keep a demo in the shipped workbench.** The seeds have not changed since
  2026-09-07 (`989182d7`), and the demo already does not show consoles, hosts,
  devices or notes. Four ways to bring it back were weighed, and none is
  decided now. (a) A hosted demo: CI publishes the built tree with the seeds
  to GitHub Pages over https, where modules load; the demo mode comes from a
  build flag, not from the protocol. (b) `ralphy demo`: the binary serves the
  same tree on loopback with no store. (c) The real daemon with a scratch
  store and sample repositories, so no seed is needed and consoles work; runs
  and kanban then need sample forge data. (d) A one-file `demo.html` that a
  bundler joins into a classic script, so `file://` works again; this adds the
  bundler that D5 avoids.
- **TypeScript in classic scripts**, with no `import`/`export`. It is the
  smallest change, and the demo would stay. The dependencies between files
  would stay hidden in the order of the `<script>` tags.

## Compliance

- D1: checked by `first_party_scripts_move_to_typescript_and_never_back`
  (`crates/ralphy-daemon/src/tests.rs`) as a ratchet: it fails on a
  first-party `.js` file in `assets/ui/` that is not in `CLASSIC_SCRIPTS`, and
  on a name in that list that is no longer a `.js` file. `erasableSyntaxOnly`
  is checked by `tsc` (D4).
- D2: checked by the same test as a ratchet: a `.ts` file that assigns a
  `window.WB` name must be in `MODULE_WINDOW_NAMES`. A relative import that does
  not name a `.ts` file fails `cargo build`, and a type imported without
  `import type` fails `tsc` (`verbatimModuleSyntax`).
- D3: checked by `cargo build`: a `.ts` file that cannot be parsed fails it.
- D4: checked by the `UI type check (tsc)` step of the `ui-tests` job in
  `.github/workflows/ci.yml`.
- D5: not checked by code until phase 4. Phase 4 adds a test that each page
  has one module tag and calls `Alpine.start()` once.
- D6: checked by `tsc` (D4), and by `loadComponent` in
  `crates/ralphy-daemon/ui-tests/harness.mjs` for the markup.
- D7: not checked by code: reviewed in the PR. A module with hidden state shows
  up as a test that passes alone and fails in the suite.
- D8: checked by the D1 test: `wb-mode.js` and `assets/ui-demo/` are not
  allowed back in the source tree. The tag and the archive note are not
  checked by code: reviewed in the PR.
- D9: not checked by code: reviewed in the PR, and by the browser checks of the
  moved feature. A load order fault shows only in a browser.
- D10: checked by the D1 test, when its list is empty.

## Amendment (2026-10-06): the pilot, as measured

**D3 uses `swc_ts_fast_strip`, not `oxc_transformer`.** `oxc_codegen` prints
the module again from its syntax tree. Measured on `wb-hosts-dialog.js` with
types added: 327 source lines became 414, indentation became tabs, and an
unused import was dropped. So the browser would not show the source lines,
which Consequences states. `swc_ts_fast_strip` 59 (`StripOnly` mode, the
engine Node uses to run `.ts`) replaces each type with spaces: 327 lines stay
327, and every column stays. `build/ui.rs` then changes each relative import
path from `.ts` to `.js` with `swc_ecma_parser`, which the strip crate already
depends on. The two extensions have the same length, so the columns still
hold. D3 reads "with `swc_ts_fast_strip`" where it says "with
`oxc_transformer`". The `swc_*` crates publish a new major version often
(`swc_ts_fast_strip` 59, `swc_common` 26): a bump is a reviewed change, the
same cost that Consequences states for `oxc_*`.

**D2 adds `import type`.** Removing types cannot tell a type import from a
value import, so an untagged type import would stay in the served file and
fail in the browser. `tsconfig.json` sets `verbatimModuleSyntax`, so `tsc`
refuses it.

**D6 is `component(uses, data)` in `wb-alpine.ts`.** It types the
component's `this` as its own members, `Pick<Shell, …>` of `uses`, and the
Alpine magics. `shell.d.ts` types the `shell()` members a component lists.
The classic globals a module reads are typed in `globals.d.ts`.

**The pilot moved `wb-hosts.js` and `wb-hosts-dialog.js`**, with `build.rs`, `tsconfig.json`, the gate step and the D1
test. Measured on Windows on 2026-10-06:

| Exit criterion | Result |
|---|---|
| Behaviour unchanged | `node --test`: 865 passed. Browser checks: `wb_hosts_497.py` 12/12, `wb_hosts_scroll.py` 6/6, `wb_modals_487.py` 36/36, `wb_security_headers.py` 13/13 (the module loads under the CSP) |
| `cargo build` with no Node on the path | passes; `build.rs` runs no other program. CI builds and tests on Windows, Linux and macOS |
| `tsc --noEmit` strict passes, and a name outside `uses` fails it | passes; removing `loadRepos` from `uses` gives two `TS2339` errors |
| The tests import the `.ts` modules, with no `new Function` | yes: `wb-hosts.test.mjs`, `wb-hosts-dialog.test.mjs` (through `MODULE_COMPONENTS` in the harness) and `shared-replies.test.mjs` |
| Time `build.rs` adds | the first build of the new crates: about 3 minutes (debug). An incremental build after a `.ts` edit: 4.4 to 5.7 seconds, with the daemon crate rebuilt |
| Crates `swc_*` add to `Cargo.lock` | 69 (344 to 413) |
| Asset pins | 803 claims by 512 assertions before, 841 by 516 after: the new tests add 4 assertions, and no pin moved |
| jscpd on a renamed file | no new clone; the 4 `.ts` files have 0 duplicated lines. A file moved to TypeScript changes most of its lines anyway |

One more cost was found: `xtask ui-copy` read a helper's body only when `{`
came right after `)`, so a TypeScript return type (`): string {`) hid two
texts of `wb-hosts.ts`. It now skips a simple return type.

With this, the pilot meets every criterion, and this ADR is accepted.

## Amendment (2026-10-06): phase 4, as measured

Phase 4 moved the six Alpine components and `app.js` to TypeScript, then
added the entry modules. Four decisions changed in the work.

**D5 does not wait for the last classic script.** D9 said that D5 replaces
the CDN Alpine build when the last classic script moves. No classic script
reads `Alpine`, and none reads a module's name while the page is parsed. A
module tag runs after every classic script, with or without an entry module.
So D5 is done in phase 4, and the classic scripts that remain still work.

**Each page has an entry module.** `main.ts` is the entry of `index.html`;
`detached-main.ts` and `detached-fence-main.ts` are the entries of the two
torn-off window pages, which have no Alpine. `main.ts` imports the modules in
the order of the old module tags, sets `window.Alpine`, calls `wire(window,
document)` from `app.ts`, registers each component and the `x-icon`
directive, and calls `Alpine.start()` once. A component module no longer
registers itself on `alpine:init`: ADR-0073 D3 reads "registered by the
entry module". The `<body>` reads `x-data="shell"`.

**A relative import may name a vendored file.** `main.ts` imports
`./vendor/alpine.esm.min.js`, the `dist/module.esm.min.js` file of the same
npm package and version (3.14.1), recorded in the manifest (ADR-0072 D12).
`build/ui.rs` keeps an import of a `.js` file under `./vendor/` as it is, and
still fails the build on any other relative import that does not name a
`.ts` file. The types of the import come from `vendor/alpine.esm.min.d.ts`,
which the build does not serve.

**D6 types `Shell` from the real `shell()`.** `Shell` is
`ReturnType<typeof shell>`, exported by `app.ts`, and replaces the
hand-written `shell.d.ts`. `app.ts` was converted mechanically: a parameter
with no obvious type is `any`, and `strict` stays on. Narrowing the `any`s is
later work, one area at a time. Everything `app.js` did at load is in the
exported `wire(window, document)`, so a test runs it on its own stubs and
each call starts with new state (D7).

D5 is now checked by `each_page_starts_from_one_entry_module`
(`crates/ralphy-daemon/src/tests.rs`): each page has one module tag, the
entry it names; only `main.ts` calls `Alpine.start`, `Alpine.data` and
`Alpine.directive`; and `main.ts` registers exactly the `x-data` names of
`index.html`.
