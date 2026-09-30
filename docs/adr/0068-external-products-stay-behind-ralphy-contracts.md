# External products stay behind Ralphy's contracts; platforms have one owner

Status: proposed
Kind: structural
Protects: extensibility, integrity of change

## Context

Ralphy depends on outside things of three different kinds, and until now no
decision said how tightly each one may be bound into the code. The
architecture diagnosis of 2026-09-30
(`docs/architecture-diagnosis-2026-09-30.md` §3) measured the result for the
forge:

- `IssueTracker` (`crates/ralphy-core/src/tracker.rs:15`) is shaped like
  GitHub (a `u64` issue number, labels as the queue, checkbox ticking), and
  its only implementation lives inside core.
- `ralphy-cli` skips the trait and calls 24 `ralphy_core::github::*` items
  directly, from 9 files.
- Ten prompt files, plus `assets/prompts/plan/template.md`, tell the agent to
  run `gh issue view` (20 lines).
- `IssueTracker::is_closed` defaults to `Ok(true)` and `create_issue` to
  `Ok(0)`. A second tracker that forgot them would pass every blocked-by gate
  and invent issue number 0.
- ADR-0032 §6 says GitHub is "the only implementation, behind a
  forge-neutral contract". That holds for the names of the forge-query verbs,
  which are Ralphy's words (CONTEXT.md, **Forge query**). It does not hold
  inside core and the CLI, where no neutral contract exists.

The same kind of coupling exists for agent vendors, which ADR-0002 otherwise
keeps out of core: `crates/ralphy-core/src/runner/clock.rs:146-200` parses
the reset-time text of Codex and of Z.ai/GLM.

The CONTEXT.md entry **Forge** already says "GitHub today, and GitHub only";
the word exists so contracts are named neutrally, not as a promise of a
second forge. No second forge is planned. So the question is not whether to
build a port now, but how to stop the coupling from growing.

## Decision

**D1. Three classes of external dependency.**

| Class | What it is | Examples |
|---|---|---|
| Replaceable product | A vendor's service that has competitors | GitHub as the forge, agent CLIs, models.dev, Telegram, GitHub Releases as Ralphy's distribution channel |
| Platform | An open standard or protocol with no vendor | git, ssh, HTTP, CloudEvents, the PTY, the OS schedulers |
| Library | Code linked into Ralphy | crates, Monaco, xterm, Alpine |

Libraries follow the normal dependency rules and are outside this ADR.

**D2. A product's meaning lives only in its adapter.** Across the boundary
with a product, only connascence of name and type in Ralphy's own words is
allowed: an issue id, "close the issue", "the queue". Connascence of meaning
or algorithm that belongs to the product stays inside the product's adapter:
"the queue is a label", "the id is a `u64`", "tick a checkbox in the body",
"run `gh`", a vendor's date format.

**D3. New code obeys D2 at once; existing coupling is contained, not
refactored.** The existing GitHub coupling in core, in `ralphy-cli` and in the
prompts is recorded and must not grow. It is removed when that code is changed
for another reason, or when a second product of the same kind becomes real.

**D4. A tracker method that guards integrity has no default.**
`IssueTracker::is_closed` and `IssueTracker::create_issue` become required
methods. A default that lets the operation continue when a method is missing
is not allowed on the forge contract.

**D5. Each platform has one named owner.** A platform is used directly, with
no adapter, but only by the component named as its owner in
`docs/ARCHITECTURE.md`. The owner of git is decided in ADR-0069.

## Consequences

- An agent that adds a forge feature cannot add a new
  `ralphy_core::github::` call in `ralphy-cli`. It must extend an existing
  call site or go through `IssueTracker`. This friction is on purpose. When it
  blocks real work, the answer is a decision, not a bypass.
- Below the forge-query verbs, ADR-0032 §6's "forge-neutral contract"
  becomes true only for new code. Existing code is contained, not refactored.
- The reset-time parsing in `clock.rs` breaks D2. It moves to the adapters
  (a typed reset instant in `Outcome::Limit`) the next time that code changes.

## Considered options

- **Extract a `ralphy-forge-github` crate now, with a neutral forge port in
  core.** Rejected: it touches about 2,200 production lines of core, 24 CLI
  call sites and 20 prompt lines, for a second forge that is not planned.
  AGENTS.md requires a real second caller before a new abstraction.
- **Keep GitHub as an open, uncontained dependency.** Rejected: nothing would
  stop each new feature from adding more GitHub meaning to core and the CLI.

## Compliance

- D1: not checked by code: a classification, applied in review and listed in
  `docs/ARCHITECTURE.md`.
- D2, D3 (forge in Rust): to be checked by an `xtask` ratchet: baseline 24
  distinct `ralphy_core::github::` items used from `crates/ralphy-cli/src`;
  the check fails if the count goes up. Not built yet.
- D2, D3 (forge in prompts): to be checked by the same ratchet: baseline 20
  `gh issue view` lines under `assets/prompts/`. Not built yet.
- D2 (agent vendors in core): not checked by code: a vendor date format in
  core cannot be found reliably by a pattern. Known violation:
  `crates/ralphy-core/src/runner/clock.rs`.
- D4: checked by the compiler once the defaults are removed: a tracker that
  does not implement the method does not build. Not done yet.
- D5: to be checked by an `xtask` ratchet over process spawns: each
  `Command::new("git" | "gh" | "ssh")` site must be in the owner's allowlist;
  baseline today's sites. Not built yet.
