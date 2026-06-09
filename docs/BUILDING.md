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

- **`ci.yml`** — runs on every push to `main` and every PR. On `windows-latest`
  it checks formatting (`cargo fmt --check`), lints (`cargo clippy -D warnings`),
  builds, and runs the test suite in release mode.
- **`release.yml`** — builds the shippable artifact: a single Windows zip,
  `ralphy-<version>-windows-x64.zip`, containing `ralphy.exe`, `README.md`,
  `LICENSE`, and this `BUILDING.md`. Because the prompts and skills are embedded
  in the binary, that zip is everything a user needs.

To cut a release, push a `v*` tag — the workflow builds, packages the zip (plus a
`.sha256` checksum), and publishes a GitHub Release with the zip attached and
auto-generated notes:

```powershell
git tag v0.1.0
git push origin v0.1.0
```

You can also run the **Release** workflow manually (`workflow_dispatch`) to produce
the zip as a downloadable run artifact without publishing a Release.

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
