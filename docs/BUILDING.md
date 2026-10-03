# Building Ralphy

Ralphy ships as a single self-contained executable — `ralphy.exe` on Windows, `ralphy`
on Linux and macOS. To build it yourself you need the
[Rust toolchain](https://www.rust-lang.org/tools/install).

```bash
git clone https://github.com/paulocorcino/ralphy
cd ralphy
cargo build --release
# binary at target/release/ralphy  (target\release\ralphy.exe on Windows)
```

Run `target/release/ralphy install` to put it on your `PATH`, so you can run `ralphy`
from any repo: it links the build into a folder in your home and adds that folder to
your `PATH` (see the README). The
bundled skills (`reviewer`, `setup-pocock`, `staged-plan`) are embedded into the binary at build time —
there's nothing else to install or copy alongside it.

## Prerequisites

`ralphy init` enforces this environment before it runs; the same set is what any
repo needs at runtime:

- **git** — ralphy shells to the `git` CLI (no libgit2) to branch, commit, tag
  the pre-run marker, and undo. On Windows install
  [Git for Windows](https://git-scm.com/download/win), which also provides
  **git-bash**: on Windows, ralphy sets the Cursor agent's `SHELL` to it. On
  Linux/macOS use your package manager.
- **python** — backs the `reviewer` skill's `scripts/*.py` (`python` or
  `python3` on `PATH`).
- **gh** — the [GitHub CLI](https://cli.github.com/), **authenticated**
  (`gh auth login`); ralphy uses it for every forge operation.
- **a GitHub remote** — the repo's `origin` must point at GitHub.
- **an agent CLI** — at least one supported vendor CLI installed and logged in.
  `ralphy run --help` lists the supported agents under `--agent`.

To run the test suite:

```bash
cargo nextest run --workspace   # the gate CI runs
cargo test --workspace          # same tests, ~50% slower
```

[cargo-nextest](https://nexte.st/) (`cargo install cargo-nextest --locked`) is
what CI runs and what you want locally. `cargo test` executes one test binary at
a time; this workspace has ~50 of them, so the cores sit idle while the long
ones — the queue-lifecycle tests, which shell out to `git` for every scenario —
run alone. Nextest schedules every test across a single pool. Measured on a
12-core Windows box: **190s → ~130s**.

Nextest does not run doctests, so CI keeps a separate `cargo test --doc` step.

Two notes for a slow suite, both measured on Windows:

- The suite is bound by **process creation**, not by Rust. A bare `git --version`
  spawn costs ~68ms here against ~15-25ms on a box whose antivirus is not
  inspecting the build tree. Excluding the repo's `target/`, `~/.cargo`, and the
  `rustc`/`cargo`/`git`/`link` executables from real-time scanning gives the
  largest gain. The operator decides whether to do it.
- Dependencies build optimized in dev (`[profile.dev.package."*"]` in the root
  `Cargo.toml`) because the daemon's PBKDF2 tests are otherwise the slowest in
  the workspace. That override does not touch CI, which tests with the `ci`
  profile: `release` optimization without the `release` link settings.

## CI & releases

Six GitHub Actions workflows live under [`.github/workflows/`](../.github/workflows/):

- **`ci.yml`** — runs on every push to `main` and every PR. It is the source of
  the gate; AGENTS.md lists the same commands in their local form. A `lint` job
  checks formatting (`cargo fmt --check`), lints (`cargo clippy --all-targets
  -D warnings`) and the workbench text (`xtask ui-copy --check`) once on Linux.
  A `ui-tests` job runs `node --test crates/ralphy-daemon/ui-tests` and oxlint
  once on Linux. A `test` matrix builds and runs the suite (via `cargo nextest
  run`, plus a `cargo test --doc` step for the doctests nextest skips) with the
  `ci` profile on **`windows-latest`, `ubuntu-latest` and `macos-latest`**,
  with `--no-fail-fast`. On Windows the tests run through
  `.github/scripts/nextest-windows.ps1`, which bounds the run at 14 minutes
  and kills what a hung test left alive (#441). On macOS the job also
  registers, probes and removes the launchd autostart agent: the one place
  `launchctl` runs for real. A separate job, `changelog`, runs **on pull
  requests only**: it fails when the diff touches the shipped surface
  (`crates/*/src/`, the workbench UI assets, `assets/`) without a `changelog.d/` fragment, and it checks
  that the fragments present parse. A human overrides it with the `no-changelog`
  label. The job `oversized`, also on pull requests only, fails when a number
  in the `OVERSIZED` table (`crates/xtask/tests/ratchets.rs`) grows or a new
  entry appears; a human allows it with the `oversized-ok` label.
- **`release.yml`** — builds the shippable artifacts for every platform:
  - `ralphy-<version>-windows-x64.zip`
  - `ralphy-<version>-linux-x64.tar.gz` — a **static musl** binary with no glibc
    dependency, so it runs on any Linux distro (tar preserves the executable bit)
  - `ralphy-<version>-macos-x64.tar.gz` — Intel, floored at macOS 12 Monterey
  - `ralphy-<version>-macos-arm64.tar.gz` — Apple Silicon, floored at macOS 12 Monterey

  Each contains the binary (`ralphy.exe` / `ralphy`) plus `README.md`, `LICENSE`,
  and this `BUILDING.md`. Because the prompts and skills are embedded in the binary
  on every platform, those archives are everything a user needs.

  A tag with a hyphen (`v0.1.0-rc.20`) is published as a GitHub pre-release.
  The release builds without a cargo cache, so a cache written by another run
  cannot reach a published binary. The publish job signs a build provenance for
  each archive. Check it with
  `gh attestation verify <archive> --repo paulocorcino/ralphy`.

- **`security.yml`** — runs on every push to `main`, every PR, and once a day,
  because a new advisory can make an unchanged tree unsafe. `cargo deny check`
  applies [`deny.toml`](../deny.toml): RustSec advisories, licenses, and
  crates.io as the only source. gitleaks looks for committed secrets
  ([`.gitleaks.toml`](../.gitleaks.toml); a reviewed false positive goes in
  `.gitleaksignore`). zizmor checks the workflows themselves. On a PR, the
  dependency review fails when a new dependency has a known vulnerability. Run
  `cargo deny --locked check` before you add or update a crate: CI passes
  `--locked`, and without it a stale `Cargo.lock` passes locally.
- **`codeql.yml`** — CodeQL static analysis of the Rust code, the workbench
  JavaScript and the workflows, on every push to `main`, every PR, and once a
  week. The results are in the repository's Security tab and in review comments
  on the PR.
- **`capabilities.yml`** — on every PR, lists what the change adds that gives
  the code a new power: network access or a URL host the repository did not
  name before, a subprocess, `unsafe`, a read of a secret, encoded or minified
  text, dynamic JavaScript code, a new crate, a build script, a changed
  workflow, or changed agent instructions. Each finding is an annotation on the
  diff. The detector runs from the base branch's code, so a PR cannot change
  the rules that check it. A PR from outside the maintainers fails while it has
  findings; a maintainer reads the lines and adds the `capability-reviewed`
  label. The label approves only the commits present when it was added: after
  a new push, remove the label and add it again. For a maintainer's own PR the
  findings are reported and do not fail the check. Run it locally with
  `cargo run -p xtask -- capabilities --base origin/main`.
- **`refresh-seed.yml`** — a scheduled (weekly) maintenance job that keeps the
  offline pricing floor current without hand-edits (ADR-0034 A3, issue #290). It
  runs the generator (below) and opens a **diffable PR only when the seed
  changes**, with the `no-changelog` label. A human reviews it as data before it
  merges. No build step fetches prices; the refresh runs only here.

Every action is pinned to a commit SHA, with the version in a comment, and
[`dependabot.yml`](../.github/dependabot.yml) proposes the updates.
[`CODEOWNERS`](../.github/CODEOWNERS) names the owner of the workflows, the
`Cargo.toml` files, `build.rs`, the prompts, the plugin skills, and the vendored
UI code. It blocks a merge only when branch protection on `main` requires Code
Owner review, and `main` has no branch protection today.

## Pricing seed refresh (`xtask`)

The offline price floor lives in `assets/pricing/`:

- **`models-dev-seed.json`** — machine-owned. Covers the providers Ralphy drives:
  **anthropic, openai, google, moonshotai** (matching the resolver's
  provider-prefix synthesis). Regenerate it with:

  ```bash
  cargo run -p xtask -- refresh-seed
  ```

  This fetches the live models.dev catalog and, for each id already in the seed,
  updates its price where upstream publishes one — preserving (never dropping or
  adding) the id set, so vendor spellings the catalog does not carry (Copilot's
  dotted ids, the CLI's Gemini forms, `kimi-for-coding`) survive. Output is sorted
  for a reviewable diff; a no-op run leaves the file byte-identical. The generator
  owns this file wholesale and never touches `slug-overlay.json`.
- **`slug-overlay.json`** — human-owned. The vendor-internal rates no catalog
  publishes. The refresh never touches it (ADR-0034 A3: one owner per file).

A deliberate floor above upstream (e.g. `claude-opus-4-8`, ADR-0008 D8) is a
review call on the refresh PR — restore it there rather than let the refresh
regress it, and move the `floor.rs` golden values with any accepted change.

## Taking a release

`ralphy update` resolves the newest release on its channel (`rc` by default while
the project ships candidates, `--channel stable` otherwise), downloads the archive
for the host, **refuses it unless it matches the published `.sha256`**, and puts it
where the running binary is — rename-then-place, because Windows will not let a
running image be deleted but will let it be renamed. `ralphy update --check`
reports and changes nothing.

Replacing the file is not the end of it: a resident daemon keeps executing the
image it started with, so the update restarts it (`ralphy daemon restart`, which
reads the `daemon.pid` the daemon records at startup). A machine with no daemon
running gets none started.

The download path is exercised against what is actually published by an ignored
test that replaces nothing:

```bash
cargo nextest run -p ralphy-cli -E 'test(takes_a_published_release)' --run-ignored all
```

## Changelog fragments (`xtask`)

Every pull request that changes what a user can see or do leaves one file behind:

```markdown
<!-- changelog.d/389.md -->
---
kind: feature
---
Paste a screenshot straight into a console.
```

`kind` is a closed set (`breaking`, `security`, `feature`, `fix`, `internal`) and
it is the only severity the release machinery has — it decides the heading, the
loudness of the workbench badge, and whether the release is announced at all
([ADR-0056](adr/0056-release-communication-and-the-update-watch.md)). The rules for
writing one are in [`changelog.d/README.md`](../changelog.d/README.md).

- **`changelog.d/*.md`** — human-owned. Written in the pull request, by whoever
  wrote the change. A CI job on pull requests fails when a change touches the
  shipped surface without one; a human applies the `no-changelog` label to
  override.
- **`CHANGELOG.md`** and **`changelog.json`** — machine-owned. Folded at release
  time, never hand-edited (ADR-0034 A3: one owner per file). `changelog.json` is
  the durable structured record; the markdown is rendered from it.

```bash
cargo run -p xtask -- changelog --check                 # do the fragments parse?
cargo run -p xtask -- changelog --pending               # what would the next release say?
cargo run -p xtask -- changelog --release v0.1.0-rc.20  # fold, and consume the fragments
```

The fold also writes `target/changelog/notes.md` (the release body) and
`target/changelog/announce` (`yes`/`no`) for you to read. The release workflow
does not use your copy: `target/` is not committed, so its publish job writes
both files again with `changelog --notes <tag>` from the committed
`changelog.json`.

Version numbers move together:

```bash
cargo run -p xtask -- bump 0.1.0-rc.20   # every crate manifest, in step
cargo check --workspace                  # moves Cargo.lock with them
```

Tag candidates as `v0.1.0-rc.N` — with the dot. The comparator normalizes both
spellings, but the dotted one is what orders correctly without help.

To cut a release: fold the fragments, bump the versions, commit, then push the tag.

```bash
cargo run -p xtask -- changelog --release v0.1.0-rc.20
cargo run -p xtask -- bump 0.1.0-rc.20
cargo check --workspace
git add -A                                   # NOT `commit -am`: see below
git commit -m "chore(release): 0.1.0-rc.20"
git push
git tag v0.1.0-rc.20
git push origin v0.1.0-rc.20
```

**`git add -A`, not `git commit -am`.** `-a` stages modifications and deletions of
*tracked* files only. The fold deletes the fragments (tracked, so staged) and
writes `CHANGELOG.md` and `changelog.json` — which are untracked the first time,
and therefore excluded. Committing that way pushes a tag whose fragments are gone
and whose record was never committed, and the release then publishes the
"no changelog entry was recorded" body.

The build matrix produces every archive (each with a `.sha256` checksum) and a
final job publishes one GitHub Release with them attached. Its body is
`--notes-file`, rendered from the committed `changelog.json` by the same xtask —
not `--generate-notes`, which folds commit subjects that name the change rather
than the capability. `changelog.json` is attached as an asset so the workbench
can read it. A release carrying a `feature`, `breaking` or `security` fragment also
opens a Discussions announcement in the `Announcements` category; a fix-only
release does not. When the announcement fails, the release is still published,
without it.

You can also run the **Release** workflow manually (`workflow_dispatch`) to produce
the archives as downloadable run artifacts without publishing a Release.

## Layout

The map of the crates, with the component each one holds, is §4 of
[ARCHITECTURE.md](./ARCHITECTURE.md). The other paths:

| Path | Role |
|------|------|
| `crates/xtask/` | Out-of-band repo tooling (`refresh-seed`, `changelog`, `bump`, `ui-copy`, `asset-pins`, `capabilities`); not part of the shipped binary. |
| `changelog.d/` | Human-owned changelog fragments, one per pull request; consumed by the `changelog` xtask. |
| `assets/pricing/` | The offline price floor: machine-owned `models-dev-seed.json` + human-owned `slug-overlay.json`. |
| `assets/prompts/` | The plan/execute prompt charters. |
| `assets/plugin/` | The Claude Code plugin (the `reviewer`, `setup-pocock` and `staged-plan` skills), embedded into the binary. |
| `docs/adr/` | Architecture decision records. |
