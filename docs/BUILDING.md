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

To run the test suite:

```bash
cargo test
```

## CI & releases

Two GitHub Actions workflows live under [`.github/workflows/`](../.github/workflows/):

- **`ci.yml`** — runs on every push to `main` and every PR. A `lint` job checks
  formatting (`cargo fmt --check`) and lints (`cargo clippy -D warnings`) once on
  Linux, and a `test` matrix builds and runs the suite in release mode on **both
  `windows-latest` and `ubuntu-latest`** (the PTY tests drive `cmd.exe` on Windows
  and `sh` on Linux).
- **`release.yml`** — builds the shippable artifacts for every platform:
  - `ralphy-<version>-windows-x64.zip`
  - `ralphy-<version>-linux-x64.tar.gz` — a **static musl** binary with no glibc
    dependency, so it runs on any Linux distro (tar preserves the executable bit)
  - `ralphy-<version>-macos-x64.tar.gz` — Intel, floored at macOS 12 Monterey
  - `ralphy-<version>-macos-arm64.tar.gz` — Apple Silicon, floored at macOS 12 Monterey

  Each contains the binary (`ralphy.exe` / `ralphy`) plus `README.md`, `LICENSE`,
  and this `BUILDING.md`. Because the prompts and skills are embedded in the binary
  on every platform, those archives are everything a user needs.

Ralphy's code is cross-platform (`portable-pty`, `HOME`/`~/.local/bin/claude`
fallbacks), and both the Windows and Linux binaries are built and exercised by the
CI suite on every push.

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
| `assets/prompts/` | The plan/execute prompt charters. |
| `assets/plugin/` | The Claude Code plugin (the `reviewer` + `staged-plan` skills), embedded into the binary. |
| `docs/adr/` | Architecture decision records. |
| `legacy/` | The original PowerShell orchestrator, superseded by the Rust binary. |
