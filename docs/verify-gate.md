# Verifying before close

For a tool that closes issues unattended overnight, "green = the agent said so" is the
central trust gap: an agent can declare *done* without the work actually being verifiable.
Ralphy closes that gap with a **runner-enforced verify gate** ([ADR-0011](adr/0011-verify-gate-before-close.md)).
After the agent reports done — but **before** the issue is closed — the runner itself
re-runs a set of commands the plan declared, over the committed code, and **only closes if
they pass**.

## The `## Verify` section

The planner emits a `## Verify` section in `.ralphy/plan.md`, one command per line:

```markdown
## Verify

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- **Technology-agnostic** — the gate runs whatever commands the plan names and checks
  exit codes. It knows nothing about Rust/Node/Python; the same machinery verifies
  `cargo test`, `pytest`, `npm test`, or `make check`.
- **Direct argv, no shell** — each line runs as `argv` directly (no `&&`, pipes, or
  globs), which makes `## Verify` portable Windows↔Linux for free. The runner chains the
  commands, runs them sequentially, and stops at the first non-zero exit. A command that
  truly needs a shell writes `sh -c "…"` explicitly.
- **Bounded** — the gate runs inside the per-issue time budget; a hung verification fails
  the gate rather than going green by silence.

## Pass, fail, and the comment

**Pass** → the issue closes on the existing green path. **Fail** → the issue stays open,
the run stops, and the branch is handed back with the work intact. Either way, Ralphy
posts a comment recording **each command, its exit code, and (on failure) a tail of the
output** — what you read in the morning to see why an issue did or didn't close.

## Resolution precedence

1. `## Verify` in the plan (per-issue, planner-emitted) — strongest.
2. `verify.command` in `.ralphy/settings.json` (per-repo default) — used when a plan has
   no `## Verify` section. Set it with `ralphy config set verify.command "cargo test"`.
3. Nothing resolves → the issue closes on the agent's self-report with a **loud warning**
   in the log (the absence of a gate is always a visible decision, never a silent hole).

`## Verify: none` on its own line is the **only** explicit opt-out — for an issue with
nothing machine-verifiable — and it skips the per-repo fallback.
