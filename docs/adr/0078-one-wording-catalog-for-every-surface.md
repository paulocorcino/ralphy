# Every surface reads the same wording from one catalog, owned by text domain

Status: proposed
Kind: structural
Protects: consistency of the workbench, operability

## Context

Ralphy shows the same facts on several surfaces: the CLI presenter, the
Telegram notifier, the event sink and the workbench. Each surface writes its
own text for them. Facts read on 2026-10-11:

- The words and icons for an issue status are written three times: in the
  CLI presenter (`crates/ralphy-cli/src/ui/render/line.rs:92`,
  `crates/ralphy-cli/src/ui.rs:72`), in the Telegram notifier
  (`crates/ralphy-cli/src/telegram/notifier/render.rs:17`) and in the
  workbench (`crates/ralphy-daemon/assets/ui/wb-runs.ts:106`). `IssueStatus`
  itself is in `crates/ralphy-cli/src/runstate/state.rs:12`.
- A daemon reply puts two different things in one `message` field
  (`crates/ralphy-daemon/src/routes/ws_command/oneshot.rs:444`): a code of
  the daemon (`exists`, `unknown repo`) or the whole output of a failed
  `ralphy` child. `wb-fail.ts` maps the codes to text in its `CAUSE` table
  and reads the CLI output as text: it removes `Error: `, folds
  `Caused by:`, and splits on ` — `. A change of wording in a `bail!` breaks
  the workbench, and no Rust test sees it.
- Some replies send the whole anyhow chain as text
  (`{"error": format!("{e:#}")}` in `routes/api_desk.rs` and
  `routes/api_audit.rs`).
- For the collected verbs (Query, Mutate), the daemon merges the child's
  stdout and stderr into one pipe (`crates/ralphy-daemon/src/dispatch/spawn.rs:249`).
- The CLI prints a top-level error in the anyhow debug form, because `main`
  returns `Result<()>` (`crates/ralphy-cli/src/main.rs:53`). A one-shot
  command has no log, so this chain is its only diagnostic record.
- `RunEvent::Notice` sends a free English sentence written in the core
  through the event sink (`crates/ralphy-cli/src/events/envelope.rs:455`),
  and the event sink is a published contract.
- The daemon does not import `ralphy-core` (ADR-0032 §10). A leaf crate that
  both sides name already solves this kind of problem: `ralphy-run-snapshot`
  (ADR-0047) and `ralphy-git-read` (ADR-0069).
- The workbench assets are embedded in the binary (`include_dir!`), and the
  daemon's `build.rs` already writes the served tree.

PRD #422 and ADR-0065 rejected a translation catalog with keys: it had no
second caller. English stays the only language. The second caller now exists,
and it is not a second language: it is a second and a third surface that show
the same fact.

Outside practice that this decision follows (read on 2026-10-11):

- Google AIP-193: an error is identified by a `reason` that is unique inside
  its `domain`, with a `metadata` map for the data of the occurrence. The
  client never parses the message, and the message is English.
- RFC 9457 (Problem Details): a fixed type and title, the details of the
  occurrence apart.
- The rustc error index: one register of codes, checked by a tool. A
  retired code is never used again.
- The rustc Fluent work: rustc adopted Fluent for translatable diagnostics
  with no translation in progress. In October 2024 its tracking issue
  (rust-lang/rust#132181) recorded "significant friction for compiler
  contributors", downgraded the lints from `deny` to `allow`, and paused the
  work. A translation runtime with one language is a cost with no return.
- Fluent's own guidance (Mozilla): a message id is specific to its context,
  and the shared words of a product are terms. This is the split between a
  value wording and the other two kinds of wording below.

## Decision

**D1. Every surface reads the wording of a fact from the wording catalog.**
The CLI presenter, the console, the notifiers, the event sink and the
workbench show the same wording for the same fact. A surface keeps only the
arrangement: the order, the separators, the colors, the width, and the limit
of its channel (`TELEGRAM_LIMIT`). The catalog is English only. It is not a
translation catalog.

**D2. The catalog has three kinds of wording.** A *value wording* is the
text, the icon and the plain-text fallback of one value of a closed set (an
`IssueStatus`, an outcome). An *event wording* is a sentence with
`{placeholders}` that tells an event. A *failure wording* is the text of a
failure code (D6).

**D3. An event wording enters the catalog only when more than one surface
tells the event, or when the core sends it as text.** A sentence that only
one surface shows, as part of its own layout, stays in that surface.

**D4. The catalog is cut into text domains, and a text domain is the owner
of a fact, never a surface.** There is no `cli` or `workbench` text domain.
The set is a closed enum: `run`, `adapter`, `forge`, `working-tree`,
`settings`, `usage`, `daemon`, `files`, `desk`, `fleet`, `release`. A new
text domain is a change to this ADR.

**D5. The `adapter` text domain is vendor-neutral.** Its failure codes say
what happened (`quota-exhausted`, `cli-missing`), never which vendor. Each
`ralphy-agent-*` crate maps its vendor's output to these codes. A vendor's
own text goes in the `cause` detail. This keeps ADR-0002 and ADR-0023.

**D6. A failure is a text domain, a failure code, the details, and the
English text already filled in.** A failure code is unique inside its text
domain. A published failure code never takes a new meaning, and a retired one
is never used again. The details hold the data of this occurrence. A message
that git, gh or a vendor CLI printed goes in `cause`, unchanged (ADR-0065
§11). No reader ever takes data out of the text.

**D7. A failure wording is a cause, an optional remedy and a note.**

```json
"nothing-staged": {
  "cause": "nothing is staged",
  "remedy": "Stage a file first.",
  "note": "commit asked with an empty index"
}
```

`cause` is lowercase with no final period, so that it fits after
`Could not <act>: ` in the workbench and after `error: ` in the terminal.
`remedy` is a whole sentence. The act belongs to the surface: a failure does
not know which button was pressed. `note` is context for the writer, because
JSON has no comments.

**D8. The anyhow chain is the developer's diagnostic, not the operator's
text.** Only an error that reaches the operator gets a failure code: through
a verb, a notifier, or the terminal of a command the operator runs. The
failure is at the root of the chain, and the edge finds it with
`downcast_ref`. Every other `bail!` stays an anyhow error.

**D9. The source of the catalog is one JSON file per text domain, in
`assets/wording/<text-domain>.json`.** The crate that owns the facts of a
text domain loads its file with `include_str!` and owns the Rust enum of its
values and failure codes. The daemon's `build.rs` copies the files into the
served tree, and the workbench imports them as JSON. There is no code
generation, and no catalog is fetched at run time.

**D10. The format lives in a new leaf crate, `ralphy-wording`, that depends
only on `serde` and `serde_json`.** It holds the closed `TextDomain` enum, the
`Failure` type and its wire form, the form of a value wording, a trait that
each enum of values or failure codes implements, and the one function that
fills `{placeholders}`. It holds no content of any text domain. `ralphy-core`,
`ralphy-adapter-support`, `ralphy-cli` and `ralphy-daemon` may depend on it.
It depends on no other `ralphy-*` crate. The daemon still does not import
`ralphy-core`.

**D11. A collected `ralphy` child reports a failure as one JSON line on
stderr, when the daemon passes `--failure-format json`.** For the collected
verbs, the daemon reads stdout and stderr as two pipes, one thread for each.
The streamed path (consoles, `spawn`) keeps one merged pipe. The CLI turns an
error into a failure in one place, the handler of `main`. An error with no
failure in its chain becomes the generic `daemon` failure `ralphy-failed`,
with the chain in `cause`. The daemon writes the chain to its log. The browser
never receives an anyhow chain.

**D12. A daemon reply carries a failure in its own `failure` field:
`{domain, code, details, text}`.** The workbench decides on `code` and shows
`text`. A code it does not know still shows a correct text, so a tab that is
older than its daemon keeps working (ADR-0070 D6). Text that only the browser
knows (`transport`, a failed `localStorage`) stays in TypeScript, under
ADR-0065.

**D13. In the terminal, a failure prints its cause first, then its remedy,
then the anyhow chain as today.**

```
error: nothing is staged
Stage a file first.
  cause: <the git message, unchanged>

Caused by:
    0: …
```

The chain stays visible by default, because a one-shot command has no other
record of it. Argument errors from `clap` keep their own form.

**D14. The event sink sends codes and data, and keeps the text beside
them.** `RunEvent::Notice` becomes an enum of the `run` text domain. Its
CloudEvents payload gains `domain`, `code` and `details` next to `message`,
and `message` stays, filled from the catalog. The change is additive, so no
consumer breaks.

## Consequences

- ADR-0065 is amended: its §6 code table and its parsing of CLI output are
  replaced by the failure, and the rules of its §2, §6, §7, §8, §10 and the
  "no ADR in user text" guard apply to every text of the catalog, on every
  surface. The rejected "translation catalog with keys" stays rejected.
- A writer edits two places for a new failure: the enum and the JSON file.
  A test that compares the two in both directions names the missing entry.
  This is the cost the rustc Fluent work paid. It is accepted here because
  the gate runs that test anyway, and because a later translation then needs
  only new files, not a move of every text out of the code.
- The guarantee that every value has a wording moves from the compiler to
  that test. The same test checks that the `{placeholders}` of a wording are
  the fields of its variant.
- The `ui-copy` lint reads the catalog files as one more source, by fields,
  without its heuristics.
- The overloaded `message` field of a daemon reply is retired when the
  ratchets below reach zero. That change raises the peer protocol to 5
  (`PEER_PROTOCOL_VERSION`, `crates/ralphy-daemon/src/peer.rs:33`).
- The work is delivered in slices: first the value wordings of the `run`
  text domain on the three surfaces, then one failure from end to end
  (`commit` → `nothing-staged`), then one text domain at a time.
- A second language, if it is ever decided, starts from these files: the
  `{name}` placeholder form is the one Fluent and MessageFormat 2 also use.

## Considered options

- **The text as a `match` in Rust, with a TypeScript module generated from
  it.** The compiler checks every variant, and the text stays next to the
  code. Rejected: a later translation must move every text out of the code,
  and the lint must read Rust. That is the debt this ADR avoids.
- **Fluent or MessageFormat 2 at run time.** Rejected: the rustc experience
  above, with one language and no translation to pay for the friction. MF2
  is stable since CLDR 47 (March 2025), but the browser has no native
  `Intl.MessageFormat` yet.
- **JSONL, TOML or `.ftl` as the source format.** JSONL fits data that grows
  by append and is read as a stream; a hand-edited catalog with long texts
  needs neither. The browser does not import TOML, so it needs code
  generation. `.ftl` needs the Fluent parser on both sides.
- **The composition root injects the catalog into the daemon, and the
  browser fetches it.** Rejected: the text for "the daemon did not answer"
  must exist when the daemon does not answer, and TypeScript gets no types
  from a fetched catalog.
- **The format inside `ralphy-core`, with an amendment to ADR-0032 §10.**
  Rejected: the daemon would pull the whole core (git, settings, ledger) to
  read one enum, and the sync/async boundary would open again.
- **The failure as the last line of the merged output, or a mode set by an
  environment variable.** The first parses text again. The second is not
  visible in the argv, and every variable passed to a child needs a review by
  ADR-0072.
- **Hide the anyhow chain behind `--verbose`, as `gh` does.** Rejected while a
  one-shot command has no log of its own.
- **`miette`.** It brings its own terminal style for diagnostics, and the
  presenter of ADR-0006 already owns that.

## Compliance

- D1: checked by the ratchet on hand-written status tables (planned:
  `value_wordings_have_one_source` in `crates/xtask/tests/ratchets.rs`),
  baseline 3 (`line.rs`/`ui.rs`, `render.rs`, `wb-runs.ts`).
- D2, D3: not checked by code: whether a sentence is told by more than one
  surface is a judgment, reviewed in the PR.
- D4: checked by the `TextDomain` enum and a test that every file in
  `assets/wording/` names one of its values (planned, in `ralphy-wording`).
- D5: not checked by code: a vendor name in an `adapter` failure code is
  reviewed in the PR.
- D6: checked by a test that compares each enum of failure codes with its
  file in both directions, and that every published code is still present or
  listed as retired (planned, in `ralphy-wording`).
- D7: checked by `cargo run -p xtask -- ui-copy --check` once it reads the
  catalog (planned).
- D8: not checked by code: which error reaches the operator is reviewed in
  the PR.
- D9, D10: checked by `core_and_adapters_keep_their_dependency_edges`
  (`crates/xtask/tests/crate_dependencies.rs`), extended to keep
  `ralphy-wording` a leaf crate (planned).
- D11, D12: checked as ratchets (planned, in `crates/xtask/tests/ratchets.rs`):
  the rows of the `CAUSE` table in `wb-fail.ts`; the verbs whose failure still
  reaches the browser as CLI output; and the daemon replies that still send
  `format!("{e:#}")` or a free `message`. Each ratchet only goes down, to
  zero.
- D13: not checked by code: the terminal form is pinned by behavior tests of
  the CLI.
- D14: checked by `every_decoder_arm_has_a_pin`
  (`crates/ralphy-cli/src/runstate/capture/tests.rs`) for the event
  vocabulary; the additive payload is a behavior test.
