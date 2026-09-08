# The workbench's assets are gated by their behaviour, their tree and their tags — not by several hundred substrings of their text

Status: accepted (2026-09-08).

`crates/ralphy-daemon/assets/ui/` holds five files over 700 lines — `styles.css`
(6,476), `wb-console.js` (5,195), `app.js` (5,089), `index.html` (2,398),
`wb-viewer.js` (732) — and they grow. `wb-console.js` gained 871 lines in one
week and passed `app.js` doing it. Splitting them is the obvious move and
[ADR-0022](./0022-file-split-conventions.md) already says how a file in this
repo splits.

It says how a **Rust** file splits. It names `foo.rs` and `foo/` and `mod.rs`,
its gate is three `cargo` commands, and it does not contain the strings `.js`,
`.css` or `.html` anywhere. That is not an oversight to patch by widening its
vocabulary: the two problems differ in the one place that matters. `rustc`
gates a Rust split — move an item and forget to `pub use` it and the build
fails. Nothing gates an asset split. The browser is the only thing that
executes these files, and CI has no browser.

So the question this ADR answers is not "how do we split them". It is **what
has to exist first so that splitting them is a mechanical operation with a
verdict, instead of a bet.**

## What is actually there

Measured with `cargo run -p xtask -- asset-pins`, which exists so this number
stops being something a review counts by hand and gets wrong (a review in July
put it at 110; the plan that opened this work said 145):

| Shape | What a file split does to it |
|---|---|
| A — `contains("identifier")` | survives, if the text lands in the file the test names |
| F — `!contains(…)` | restate over the tree and it gets **stronger** |
| B — scoped slice (`find`/`split_once` + `.expect()`) | **panics** on a missing delimiter, not fails |
| D — count / uniqueness (`matches().count()`) | whole-file count silently degrades to per-fragment |
| C — normalized (`split_whitespace`, comment-stripped) | survives reformatting; the shape to imitate |
| E — `<script>` order over `index.html` | redundant once the tag cross-check exists |

**The counts are deliberately not written down here.** Run the command. At the
time of writing it reported 772 claims made by 470 assertions, with A the large
majority — but that figure moved three times inside the branch that introduced
it, twice because the code changed and once because the tool was corrected, and
an ADR that freezes it becomes another number a reader has to distrust. Quoting
the command instead of its output is the whole point of having built it.

Two numbers, because they answer different questions: a CLAIM is one pinned
fragment of asset text and is what a split must account for one at a time; an
ASSERTION is what reds and what a reviewer reads. The dominant idiom drives many
claims through one assertion.

Nobody decided to build this contract. It accreted, one pin per issue, each with
the same honest justification in its doc-comment: *neither `node --test` nor
Playwright runs in CI, so this substring is the only CI-visible gate over this
markup*. Every one of those pins was the right local call. The aggregate is a
contract over the **byte layout** of five files that are about to be rearranged.

## Three facts, each measured rather than assumed

**The JS suite was red for a week and CI could not tell.** `node --test
crates/ralphy-daemon/ui-tests` failed 2 of 327 from `6c52da8` (2026-09-01)
onward. Eight commits edited that directory and added 46 tests in that window.
No workflow in `.github/` ran `node` at all.

**A hand-maintained sweep list goes stale faster than it gets audited.** Three
tests swept the served assets for mock copy, seed data and dead translation
identifiers, each over a hardcoded list of paths. `wb-release.js` shipped in
#391 and was outside all three the day it was born; seven embedded `.js` files
were invisible to every sweep.

**The two regressions a split actually causes were caught by nothing.** Dropping
a `<script>` tag, and a whole-file uniqueness claim quietly becoming a
per-fragment one. Not one of the claims addresses either. Worse, the first
attempt at a gate for it here *passed* while a tag was deleted, because it asked
whether **some** shell referenced the asset — and a popup still did.

## Decisions

**D1. The contract moves to three layers, and each claim goes to the layer that
can state it best.**

- **Behaviour → `node --test`.** A claim about what a function computes belongs
  in `ui-tests/`, against the real source. This is the layer that can say a fold
  is *correct*; a substring can only say it is *present*.
- **Negatives and tree-wide facts → a Rust sweep over the embedded tree.**
  "No served asset says `mock`", "no embedded path is a seed file", "only
  `wb-view.js` names `localStorage`". Shape F is the one shape that gets
  stronger under a split, because a claim over the tree cannot be defeated by
  moving the offending text into a new file. These stay in Rust and they widen.
- **Wiring → the tag cross-check.** Every `<script>`/`<link>` in the three
  shells resolves to an embedded asset, and every asset is tagged. Per shell,
  never as a union: `index.html`'s required set is DERIVED from the tree and
  cannot go stale, and the two popups load subsets so theirs are stated floors.
  This subsumes shape E entirely.
- **Identifier pins (shape A) are dropped as they are superseded, with the
  reason recorded.** Not deleted in a sweep, and not kept out of caution: each
  one is either replaced by a behavioural test, absorbed by a tree sweep, or
  written down as knowingly abandoned. Silence is not a decision.

**D2. ADR-0036 §3 is not the objection, and invoking it here would be wrong.**

Issue #369 asks whether these pins make the daemon "a front-end linter" and
whether that crosses the boundary. It does not. §3 governs the daemon's
**runtime** — it may observe the working tree and must never interpret or mutate
the repo — and it says nothing about where a test lives. A `#[cfg(test)]` module
is compiled out of the shipped binary; the daemon does not lint anything at run
time.

The real objection is different and worth stating plainly, because it is the one
that survives: **these assertions test no Rust.** They are a text contract that
happens to be executed by `cargo test`, they are fragile to formatting, and
their failure messages describe a missing substring rather than a broken
behaviour. That is the case against them. Not a boundary violation.

**D3. The gate is four commands, and `node --test` is one of them.**

ADR-0022 §4 names three `cargo` commands. For a change that touches
`crates/ralphy-daemon/assets/ui/` or `crates/ralphy-daemon/ui-tests/`, the gate
is those three plus `node --test crates/ralphy-daemon/ui-tests`, and CI runs it
in its own job — ubuntu only, no install step, for the same reason the lint job
runs once: the suite is platform-independent and declares no dependencies.

**D4. `assets/ui/wb-foo.js` is tested by `ui-tests/wb-foo.test.mjs`.**

The convention already holds in practice; this makes it the rule, and it is the
JS reading of ADR-0022 §3 ("tests migrate with the code"). Two constraints come
with it, both learned the hard way:

- The suite lives **outside** `assets/ui/`, because `include_dir!` embeds that
  directory wholesale and a test file there would ship inside the binary.
- Every `*.test.mjs` must be imported by `ui-tests/index.mjs`. `node --test` on
  a bare directory resolves `package.json#main` and does **not** recurse, so an
  unimported test file is not a failing test — it is a file the runner never
  opens. The JS side cannot detect this (it cannot notice a file it never
  loads), so the gate is a Rust test that reads the directory.

**D5. A CSS selector may repeat across sections; it may not set one property
twice.**

`styles.css` re-declares selectors by section and that is additive and fine.
What is not fine is the same selector setting the same property in two blocks:
source order picks a winner silently and the loser sits in the file reading like
an intention. `.run-verb:disabled` carried `opacity: 0.45` at line 1041 and
`opacity: 0.4` at 5451, and the 0.45 never rendered once.

This is also the precondition for partials: rules can be regrouped only while
none of them is deciding an outcome by coming last. Two exclusions, both
deliberate — inside `@media` an override is the mechanism, and inside one block
re-declaring is the documented fallback idiom (`height: 100vh` then `100dvh`).

**D6. `index.html` is not split.** It is declarative markup with no logic, there
is no include mechanism, and the daemon may not grow a template engine to get
one.

**D7. No ESLint, Prettier or Stylelint in this work.** Formatter-sensitivity is
a real cause of pin fragility and a formatter is the obvious answer, but it
would rewrite every asset at once and fail the entire contract in a single
commit — with no way to tell a real regression from a reflow. It is a separate
decision, and it wants D1 finished first so there is less to break.

## Consequences

The identifier claims do not disappear on the day this ADR lands. They are
retired as the layers that replace them are built, and `xtask asset-pins` is how
progress is measured rather than asserted — run it before and after, and the
shape distribution is the diff.

A new asset is now swept, tagged and barrel-checked the day it is embedded, with
no list for anyone to remember. That is the property `wb-release.js` did not
have, and its absence took a week to notice.

## References

- [ADR-0022](./0022-file-split-conventions.md) — the Rust file-split conventions
  this one is the asset-side sibling of, and deliberately does not amend.
- [ADR-0036](./0036-workbench-daemon-integration-protocol.md) §3 — the runtime
  boundary D2 declines to invoke.
- Issue #367 (the file-split series) and #369 (which raised the question D1 and
  D2 answer).
