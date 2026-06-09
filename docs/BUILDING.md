# Building Ralphy

Ralphy ships as a single Windows executable (`ralphy.exe`). To build it yourself you
need the [Rust toolchain](https://www.rust-lang.org/tools/install).

```powershell
git clone https://github.com/paulocorcino/ralphy
cd ralphy
cargo build --release
# binary at target\release\ralphy.exe
```

Put `ralphy.exe` somewhere on your `PATH` so you can run `ralphy` from any repo. The
bundled skills (`reviewer`, `staged-plan`) are embedded into the binary at build time —
there's nothing else to install or copy alongside it.

To run the test suite:

```powershell
cargo test
```

## CI & releases

Two GitHub Actions workflows live under [`.github/workflows/`](../.github/workflows/):

- **`ci.yml`** — runs on every push to `main` and every PR. A `lint` job checks
  formatting (`cargo fmt --check`) and lints (`cargo clippy -D warnings`) once on
  Linux, and a `test` matrix builds and runs the suite in release mode on **both
  `windows-latest` and `ubuntu-latest`** (the PTY tests drive `cmd.exe` on Windows
  and `sh` on Linux).
- **`release.yml`** — builds the shippable artifacts for both platforms:
  - `ralphy-<version>-windows-x64.zip`
  - `ralphy-<version>-linux-x64.tar.gz` (tar preserves the executable bit)

  Each contains the binary (`ralphy.exe` / `ralphy`) plus `README.md`, `LICENSE`,
  and this `BUILDING.md`. Because the prompts and skills are embedded in the binary
  on every platform, those archives are everything a user needs.

> **Linux is built, not yet officially supported.** Ralphy's code is
> cross-platform (`portable-pty`, `HOME`/`~/.local/bin/claude` fallbacks), so it
> compiles and the suite passes on Linux — but the README still scopes the tool to
> Windows, and the Linux binary's runtime behaviour is unverified end-to-end.

To cut a release, push a `v*` tag — the build matrix produces both archives (each
with a `.sha256` checksum) and a final job publishes a single GitHub Release with
both attached and auto-generated notes:

```powershell
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
