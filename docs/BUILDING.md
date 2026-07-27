# Building Ralphy

Ralphy ships as a single self-contained executable — `ralphy.exe` on Windows, `ralphy`
on Linux. To build it yourself you need the
[Rust toolchain](https://www.rust-lang.org/tools/install).

```bash
git clone https://github.com/paulocorcino/ralphy
cd ralphy
cargo build --release
# binary at target/release/ralphy  (target\release\ralphy.exe on Windows)
```

Put the binary somewhere on your `PATH` so you can run `ralphy` from any repo. The
bundled skills (`reviewer`, `staged-plan`) are embedded into the binary at build time —
there's nothing else to install or copy alongside it.

## Prerequisites

`ralphy init` enforces this environment before it runs; the same set is what any
repo needs at runtime:

- **git** — ralphy shells to the `git` CLI (no libgit2) to branch, commit, tag
  the pre-run marker, and undo. On Windows install
  [Git for Windows](https://git-scm.com/download/win), which also provides the
  **git-bash** shell ralphy pins agent subprocesses to; on Linux/macOS use your
  package manager.
- **python** — backs the `reviewer` skill's `scripts/*.py` (`python` or
  `python3` on `PATH`).
- **gh** — the [GitHub CLI](https://cli.github.com/), **authenticated**
  (`gh auth login`); ralphy uses it for every forge operation.
- **an agent CLI** — at least one supported vendor CLI installed and logged in
  (e.g. `claude`, `codex`, `gemini`, `kimi`, `opencode`, Copilot, Cursor).

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

Two notes for anyone chasing a slow suite, both measured on Windows:

- The suite is bound by **process creation**, not by Rust. A bare `git --version`
  spawn costs ~68ms here against ~15-25ms on a box whose antivirus is not
  inspecting the build tree. Excluding the repo's `target/`, `~/.cargo`, and the
  `rustc`/`cargo`/`git`/`link` executables from real-time scanning is the single
  biggest lever, and it is the operator's call to make.
- Dependencies build optimized in dev (`[profile.dev.package."*"]` in the root
  `Cargo.toml`) because the daemon's PBKDF2 tests are otherwise the slowest in
  the workspace. That override does not touch CI, which tests in `--release`.

## CI & releases

Two GitHub Actions workflows live under [`.github/workflows/`](../.github/workflows/):

- **`ci.yml`** — runs on every push to `main` and every PR. A `lint` job checks
  formatting (`cargo fmt --check`) and lints (`cargo clippy -D warnings`) once on
  Linux, and a `test` matrix builds and runs the suite (via `cargo nextest run`,
  plus a `cargo test --doc` step for the doctests nextest skips) in release mode
  on **both `windows-latest` and `ubuntu-latest`** (the PTY tests drive `cmd.exe`
  on Windows and `sh` on Linux).
- **`release.yml`** — builds the shippable artifacts for every platform:
  - `ralphy-<version>-windows-x64.zip`
  - `ralphy-<version>-linux-x64.tar.gz` — a **static musl** binary with no glibc
    dependency, so it runs on any Linux distro (tar preserves the executable bit)
  - `ralphy-<version>-macos-x64.tar.gz` — Intel, floored at macOS 12 Monterey
  - `ralphy-<version>-macos-arm64.tar.gz` — Apple Silicon, floored at macOS 12 Monterey

  Each contains the binary (`ralphy.exe` / `ralphy`) plus `README.md`, `LICENSE`,
  and this `BUILDING.md`. Because the prompts and skills are embedded in the binary
  on every platform, those archives are everything a user needs.

- **`refresh-seed.yml`** — a scheduled (weekly) maintenance job that keeps the
  offline pricing floor current without hand-edits (ADR-0034 A3, issue #290). It
  runs the generator (below) and opens a **diffable PR only when the seed
  changes** — reviewed as data before it merges. No network ever runs at build
  time; the refresh is strictly out-of-band.

Ralphy's code is cross-platform (`portable-pty`, `HOME`/`~/.local/bin/claude`
fallbacks), and both the Windows and Linux binaries are built and exercised by the
CI suite on every push.

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

To cut a release, push a `v*` tag — the build matrix produces both archives (each
with a `.sha256` checksum) and a final job publishes a single GitHub Release with
both attached and auto-generated notes:

```bash
git tag v0.1.0
git push origin v0.1.0
```

You can also run the **Release** workflow manually (`workflow_dispatch`) to produce
the archives as downloadable run artifacts without publishing a Release.

## Layout

| Path | Role |
|------|------|
| `crates/ralphy-cli/` | The `ralphy` binary: flag parsing and the composition root. |
| `crates/ralphy-core/` | Queue lifecycle, git/GitHub integration, run reporting. |
| `crates/ralphy-agent-claude/` | The Claude Code adapter (plan + execute sessions). |
| `crates/ralphy-pty/` | PTY handling for the interactive execution session. |
| `crates/xtask/` | Out-of-band repo tooling (`refresh-seed`); not part of the shipped binary. |
| `assets/pricing/` | The offline price floor: machine-owned `models-dev-seed.json` + human-owned `slug-overlay.json`. |
| `assets/prompts/` | The plan/execute prompt charters. |
| `assets/plugin/` | The Claude Code plugin (the `reviewer` + `staged-plan` skills), embedded into the binary. |
| `docs/adr/` | Architecture decision records. |
| `legacy/` | The original PowerShell orchestrator, superseded by the Rust binary. |
