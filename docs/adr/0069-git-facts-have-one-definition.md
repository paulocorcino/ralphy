# Read-only git facts have one definition, shared by core and the daemon

Status: proposed
Kind: structural
Protects: integrity of change, responsiveness

## Context

ADR-0068 D5 says a platform has one owner. git is a platform. Today the same
git facts are computed in several places, and the answers differ
(`docs/architecture-diagnosis-2026-09-30.md` §4, §5):

- **Current branch: three readers.** `crates/ralphy-core/src/sync.rs:71`
  (`symbolic-ref`; a short sha when detached),
  `crates/ralphy-core/src/git.rs:108` (`rev-parse`; `"HEAD"` when detached),
  and `crates/ralphy-daemon/src/registry.rs:70` (reads `.git/HEAD` bytes;
  `None` when detached or in a linked worktree). The workbench sidebar and the
  branch chip use different ones.
- **Dirty working tree: two definitions.** Core's change set leaves out
  `.ralphy/` (`crates/ralphy-core/src/changes.rs:169`). The daemon runs its
  own `git status --porcelain` (`registry.rs:82`) and counts `.ralphy/`.
  CONTEXT.md calls the change set the single definition.
- **The `.git` pointer file: four parsers** that accept different input:
  `crates/ralphy-daemon/src/tree/ignored.rs:149`,
  `crates/ralphy-daemon/src/checkout.rs:127`,
  `crates/ralphy-usage-scan/src/attribution.rs:62`, and
  `crates/ralphy-core/src/checkouts/carry.rs:151` (through git).
- **`git config user.email`: eight copies**, seven in `ralphy-usage-scan` and
  one in `crates/ralphy-core/src/git.rs:182`.

The daemon runs these reads on every `GET /api/repos`
(`crates/ralphy-daemon/src/routes/api_read.rs:89-103`), and through
`ralphy-usage-scan` when it serves usage. ADR-0036 §3 says the daemon "may
not interpret" the repo, and its HEAD-watch amendment says the daemon "never
reads `HEAD` and never runs git". The code breaks both, and no check noticed.

The risk ADR-0036 §3 guards against is a daemon that **changes** the repo
while a run holds it. A read of git plumbing changes nothing. The harm that
was measured is different: several definitions of one fact that disagree.

## Decision

**D1. One crate holds the read-only git facts.** A new leaf crate,
`ralphy-git-read`, is the only implementation of: the current head (the
`Head` type now in `sync.rs`: `Branch { name }` or `Detached { sha }`),
whether the working tree is dirty (by the change-set rule, so `.ralphy/`
never counts), the `origin` URL, the resolution of a `.git` pointer file to
the git directory, and the repo's `user.email`. It depends on no other
Ralphy crate.

**D2. The crate only reads.** It never runs a git command that changes the
repo, the index or the config. Every write to a repo stays a `ralphy`
invocation, under the run lock.

**D3. Core, the daemon and `ralphy-usage-scan` use the crate** for these
facts, and keep no copy of their own.

**D4. The daemon runs no git command of its own.** It reads git facts only
through `ralphy-git-read`. Anything beyond D1's list is still a `ralphy`
invocation, as ADR-0036 §3 says.

## Consequences

- This amends ADR-0036 §3: "may not interpret the repo" now allows the
  read-only facts of D1, through this crate only. The HEAD-watch amendment's
  "never reads `HEAD` and never runs git" is replaced by D4.
- This replaces the eight separate `user.email` reads, including the seven
  in `ralphy-usage-scan` (ADR-0033). ADR-0033 kept that crate a leaf; it stays
  a leaf, because `ralphy-git-read` is a leaf too.
- The sidebar and the branch chip show the same answer for a detached HEAD.
- `GET /api/repos` still costs git work per repo per request. This ADR fixes
  who defines the facts, not how often they are read.

## Considered options

- **The daemon never runs git; it asks `ralphy … --json`** (ADR-0036 §3 as
  written). Rejected: it adds a `ralphy` process for every page load, for
  reads that cannot corrupt anything.
- **Allow the daemon its own git module by amendment.** Rejected: it keeps
  two definitions of the same facts, which is the defect that was measured.

## Compliance

- D1, D3: not checked by code: a second implementation of a fact cannot be
  found reliably by a pattern. Reviewed in the PR. D4's check catches the most
  likely case.
- D2: to be checked by a test inside `ralphy-git-read` that every git argv it
  builds starts with a read-only subcommand from a fixed list. Not built yet.
- D4: to be checked by the ADR-0068 D5 spawn ratchet:
  `crates/ralphy-daemon/src` and `crates/ralphy-usage-scan/src` have zero
  `Command::new("git")` sites after the move (baseline today: 1 and 10). Not
  built yet.
