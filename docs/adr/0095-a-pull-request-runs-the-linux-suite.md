# A pull request runs the Linux test suite, and the Windows suite stays local

Status: accepted
Kind: process
Protects: integrity of change, testability

## Context

Since 2026-11-03 (`8492fdd5`, `efa1cf20`) a pull request builds no product
crate. `ci.yml` runs lint, clippy, `cargo doc`, the ui-tests, the ratchets,
the size, complexity and duplication checks, the changelog check and the
security scans, all on Linux. The test suite runs only in `release.yml`, on
Linux, macOS and Windows, before the packages are built. The reasons were
the time a pull request waited (the macOS job took 17 minutes) and a Windows
runner that hung (#441).

So `AGENTS.md` makes the local gate (`cargo run -q -p xtask -- gate`) the
only test run before a merge. The local gate runs the local form of every CI
step, to the end, one after the other. Measured on the operator's Windows
machine on 2026-11-26, a full run takes 12 to 15 minutes. About 2.5 to 3.5
of those minutes are `nextest`. The rest repeats checks that the pull
request runs again on Linux anyway. The operator reported waiting about
20 minutes for each one-line fix, because the full gate ran after every fix.

The last release run (`45479184083`, 2026-11-24) measured the suite jobs:
`ubuntu-latest` 9 minutes, `macos-latest` 18 minutes, `windows-latest`
21 minutes. The repository is public, so standard GitHub-hosted runners cost
no money. The cost of a CI job is the time a pull request waits for it.

A Linux break today shows up only when a release is tagged. Code that
compiles and passes on Windows can still fail on Linux (`cfg(target_os)`,
paths, PTY, file watching; see `docs/TESTING-TRAPS.md`).

## Decision

**D1.** `ci.yml` has a test job on `ubuntu-latest`. It runs
`cargo nextest run --workspace --locked` with the `ci` profile and
`--no-fail-fast`, then `cargo test --doc --workspace --locked`, with a build
cache. It runs on every pull request and every push to `main`. The ruleset
of `main` requires it, like the other jobs of `ci.yml`.

**D2.** The Windows suite stays local. Before a pull request that changes
Rust code, its author runs `cargo nextest run --workspace --locked` and
`cargo test --doc --workspace --locked` on Windows, and both pass. No pull
request job runs the Windows suite.

**D3.** The full local gate is no longer required before a pull request.
While working, the author runs `cargo run -q -p xtask -- gate --fast`, and
reruns a failed step alone with the command on the first line of its log in
`target/gate/`. Before the pull request, the author runs D2 and one
`gate --fast`. The pull request runs every lint step, so a lint failure that
`--fast` missed shows up there.

**D4.** macOS stays release-only. Its suite takes 18 minutes, longer than the
rest of the pull request.

**D5.** `release.yml` does not change. All three suites still run before any
package is built.

## Consequences

- A pull request waits about 9 minutes for the Linux suite instead of a few
  minutes. Its other jobs run at the same time, so the total wait is the
  longest job.
- A Linux break is found at the pull request, not at the release.
- The local loop is shorter: `gate --fast` plus the Windows suite, about
  5 minutes instead of 12 to 15.
- `AGENTS.md` (the hard rule "Run CI's gate before you call a change
  done"), `docs/BUILDING.md` (*CI & releases*) and `docs/TESTING.md` change
  in the same change as the code.
- Later, and not decided here: `xtask gate` can run the steps that read no
  build output (taplo, yamllint, markdownlint, html-validate, stylelint and
  others) at the same time as each other.

## Considered options

- **Run all three suites on each pull request.** Rejected: the Windows job
  takes 21 minutes and has hung before (#441), and the macOS job takes
  18 minutes. A pull request would wait for the slowest one.
- **Keep the full local gate as the only test run.** Rejected: it repeats
  about 10 minutes of checks that the pull request runs anyway, and it leaves
  Linux untested until the release.
- **Run the Windows suite on the pull request instead of Linux.** Rejected:
  it is the slowest job, and the operator already runs it on Windows.

## Compliance

- D1: checked by `crates/xtask/tests/ratchets/gate_mirrors_ci.rs`
  (`the_gate_mirrors_every_ci_step`): the CI test step must map to the
  `nextest` and `doctest` steps of the gate.
- D2: not checked by code: the local run is the author's; reviewed in the
  pull request.
- D3: not checked by code: it is how the author works.
- D4, D5: not checked by code: reviewed in the pull request that changes a
  workflow.
