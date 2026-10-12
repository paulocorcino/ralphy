# The workbench pages are built from parts

Status: proposed
Kind: structural
Protects: testability, extensibility

Amends ADR-0057 D6 (`index.html` is not split) and ADR-0073 D9
(`index.html` stays one file).

## Context

The workbench has three pages, one for each `Shell` in
`crates/ralphy-daemon/src/assets.rs`: `index.html` (the desk, 3,273 lines),
`detached.html` (57 lines) and `detached-fence.html` (117 lines). Measured on
2026-10-12 with a script that finds each `x-data` element and its close tag:

- 16 Alpine components own 2,676 lines of `index.html`. The largest are
  `wbSpend` (407), `wbBoard` (295), `wbRuns` (282), `wbHostsDialog` (279),
  `wbSettingsDialog` (210) and `wbChanges` (200). The shell keeps about 600
  lines: the head, the rail, the side panel and the canvas tabs.
- Each component's markup reads 0 to 2 members of `shell()`. `shell()` has
  30 members (`cargo run -p xtask -- ui-groups`). The markup of a component
  can be read with only its own `.ts` module.
- The three pages repeat one block: the twelve links from
  `styles/01-base.css` to `styles/12-spend.css`, in the same order. The test
  `every_shell_links_the_whole_cascade` checks that each page has all of
  them.

An agent that changes the Hosts dialog today searches a 3,273-line file for
the 279 lines it needs. The size gate (ADR-0022 §7) does not measure HTML, so
nothing stops the file from growing.

ADR-0057 D6 kept `index.html` in one file because "there is no include
mechanism, and the daemon may not grow a template engine to get one". The
first premise is no longer true. Since ADR-0075 D3, `build.rs`
(`build/ui.rs`) writes the served tree from the source tree at compile time.
The same step can put parts into a page, and the daemon does not change:
it serves the built page as it does today.

What reads the markup today (measured on 2026-10-12):

- 36 tests in `crates/ralphy-daemon/src/tests.rs` read
  `include_str!("../assets/ui/index.html")`, with 170 claims
  (`cargo run -p xtask -- asset-pins`). About 12 of them count matches or cut
  the file at a position. These would give wrong results on one part.
- 22 files in `ui-tests/` read `index.html`. `componentMarkup`
  (`harness.mjs`) cuts a component out of it, from its `x-data` to the next
  banner comment.
- `ui-copy` reads every `*.html` file at the top of `assets/ui/` and does not
  go into subfolders (`sources` in `crates/xtask/src/ui_copy.rs`).
  `docs/ui-copy-rules.json` has 5 exceptions keyed by `"file": "index.html"`.
- `ui-groups` reads only the bindings in the scope of `x-data="shell"`. The
  shell stays in `index.html`.

## Decision

**D1. A page is built from parts at compile time.** A part is a file
`<name>.part.html` at the top of `assets/ui/`. A page names a part with one
line that holds only the comment `<!-- part: <name>.part.html -->`.
`build/ui.rs` replaces that line with the text of the part. The browser
receives one complete page, as before. The daemon has no template engine and
assembles nothing at run time.

**D2. A part is never served.** `build/ui.rs` does not copy a
`*.part.html` file into the served tree. No URL serves a part.

**D3. The build fails on a wrong part.** These fail the build: a marker that
names a missing file, a part that no page names, and a part that a page
names twice. A part does not name another part.

**D4. A component's markup is one part, next to its module.** The markup of
`x-data="wbFoo"`, defined in `wb-foo.ts`, is the part `wb-foo.part.html`. The
part holds the `x-data` element and nothing outside it. Markup that belongs to
no component (the shell, the rail, the side panel) stays in the page.

**D5. The three pages share one head part.** The links that all three pages
have, in the same order, are one part, `head.part.html`. A page keeps its own
links for anything only it loads, and for any link whose position differs.

**D6. Rust tests read the built page.** A test about the page reads
`$OUT_DIR/ui/<page>.html` (`include_str!(concat!(env!("OUT_DIR"), …))`),
not the source. A claim counts over the whole page, as it does today.

**D7. Node tests read the part.** `componentMarkup` reads
`<name>.part.html` and does not cut a page. The Node tests do not assemble
pages. The only code that puts parts into a page is `build/ui.rs`.

**D8. Moving markup into a part changes no byte of the built page.** A
commit that only moves markup into a part leaves the built page equal to the
source page before the commit. The only exception is the whitespace of the
marker line.

**D9. The size gate measures HTML.** `xtask oversized` measures `.html` with
a limit of 500 lines of code. A line inside `<!-- … -->` does not count. The
rule of ADR-0022 §7 applies: a new file over the limit fails, and an old
large file may not grow.

## Consequences

The work is done in phases. A phase starts only when the phase before it
meets its exit criteria.

1. **Pilot: the Hosts dialog and the head.** `wb-hosts-dialog.part.html`,
   `head.part.html`, the build step of D1–D3, the Rust tests of D6, the
   harness of D7, and `ui-copy` keyed to the new file. The pilot passes when
   the gate is green, `node --test` is green, the browser checks in
   `tests/browser/hosts/` pass, and the built `index.html` is equal to the
   source before the pilot (D8).
2. **One commit for each component**, with no change in behavior, in the
   order of the table in Context, largest first.

What becomes harder:

- A browser error in the markup gives a line in the built page, not in the
  part. Alpine errors name the expression, not the line, so this costs little.
- A new component has two files to create, the module and its part.
- A reader of a page sees a marker line where the markup was. The marker
  names the file, so the next step is one file open.

ADR-0057 D6 and ADR-0073 D9 stay true for anything that is not a part: the
markup of a page stays declarative, and a component changes only its own
markup.

## Considered options

- **Load the parts at run time with Alpine** (`fetch` plus `x-html`, or a
  custom directive). Each part costs one request more, and over a dev tunnel
  each request is a round trip. The page shows before its parts arrive, and
  the order of the parts against the Alpine stores and the shared modal stack
  becomes a new source of bugs. It is also the run-time template engine that
  ADR-0057 D6 forbids.
- **Write the markup in TypeScript** (template strings that render HTML). The
  type check and oxlint do not read inside a string. The markup would be set
  through `innerHTML`. `ui-copy`, `ui-groups` and the harness would read code
  in place of markup.
- **Merge the three pages into one.** A torn-off window would load the whole
  desk (Monaco, xterm, Crepe, the stores, 17 components) to show one file or
  one fence. The page would need code that chooses a shell at run time. The
  three pages share only their head, which D5 makes one part.
- **Put the parts in a subfolder.** `ui-copy` does not read subfolders, so
  the text in a part would leave the text check, and nothing would fail.
  Changing `ui-copy` to read subfolders is possible. A name suffix next to the
  module is simpler and puts the markup beside its code.

## Compliance

- D1: checked by `build/ui.rs`: the build writes the page from the source
  and its markers. The pilot adds a unit test of the marker rule.
- D2: checked by `crates/ralphy-daemon/src/tests.rs` (a test the pilot adds:
  no path in the embedded tree ends with `.part.html`).
- D3: checked by `build/ui.rs`: each case stops the build with an error.
  The pilot adds a unit test for each case.
- D4: not checked by code: reviewed in the PR.
- D5: not checked by code: reviewed in the PR.
  `every_shell_links_the_whole_cascade` (`crates/ralphy-daemon/src/tests.rs`)
  still checks, on the built pages, that each page links every stylesheet.
- D6: not checked by code: reviewed in the PR. A test that reads the source
  page still passes while the page has its markup.
- D7: not checked by code: reviewed in the PR.
- D8: not checked by code: the PR of each move shows the diff of the built
  page before and after, and it must be empty.
- D9: checked by `cargo run -q -p xtask -- oversized --base main` in CI.
