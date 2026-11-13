# Runbook: Antigravity adapter live capstone (#784)

The checklist for the live capstone of the `ralphy-agent-antigravity` adapter.
It runs each of the ten items that
[ADR-0086](../../adr/0086-antigravity-adapter.md) lists under
**The live capstone of #784**. The results go to
[784-antigravity-capstone-live.md](./784-antigravity-capstone-live.md), the
raw logs to `docs/spike/live/antigravity-784-capstone-*.log`, and one line
per open item to the ADR's `## Measured in #784` section.

Rules for every step:

- The binary is the one built from this tree
  (`cargo build --locked -p ralphy-cli --bin ralphy`), run by its path. The
  installed `ralphy` and the operator's daemon are not touched.
- The model is `gemini-3.8-flash-low`, passed as `--plan-model` and
  `--exec-model`. No other model is called.
- The lab is `paulocorcino/FinCal`, in fresh temporary clones; the
  operator's own clone `C:/Dev/FinCal` is not touched.
- The operator's `~/.gemini` is not touched. `agy update`, `agy install`,
  `agy remote-control` and `agy mic-serve` are not run. Nobody logs the
  operator out except the operator.
- A process is stopped only by its PID, never by its name.
- Committed logs have no token, OAuth `state` or `code_challenge` value,
  email or account id.

Cost: a small FinCal issue on Flash is a few model calls (the proof turn is
about 8 000 input tokens). The whole capstone stays under about 20 Flash
runs. No run tries to reach a real limit (operator decision, 2026-11-12).

| n | item | host | command | cost | operator step |
| --- | --- | --- | --- | --- | --- |
| 1 | Windows logged-out signature (D7) | Windows | `agy models`; `agy -p ...`; `ralphy models --agent antigravity`; `ralphy run --agent antigravity` on a temp FinCal clone | 0 tokens (no model call while logged out) | runs `/logout` in an `agy` console, then signs back in |
| 2 | A real limit against the patterns of D6 | Windows | `agy --gemini_dir=<root> -p /quota --output-format json`, parsed by the adapter's quota parser | 0 tokens | none: the operator decided not to spend to reach a limit; the answer is `not reached` with a quota read-out |
| 3 | Signed-in run on Linux: usage, hooks, stop (D5, D9, D17) | WSL | `ralphy run --agent antigravity --issues <n2>` on a temp FinCal clone; a second run stopped by `kill -TERM <pid>` | one small issue plus one short stopped run | signs in once with `agy --gemini_dir=$HOME/.ralphy/antigravity-root` |
| 4 | Hooks fire from Ralphy's file, with the proof turn and a program path with a space (D9) | Windows, WSL | the run of items 3 and 5; a copy of `ralphy.exe` in a folder with a space, one guarded run | one small run | none |
| 5 | `swarm` disabled in the owned root (D13) | Windows | `agy --gemini_dir=<root> -p /skills` | 0 tokens (a slash command) | none |
| 6 | Two runs at once in one root (D8) | Windows | two `ralphy run --agent antigravity` started together on two temp clones | two small runs | none |
| 7 | A 72 KB prompt as one NDJSON line (D2) | Windows | one `agy` call with one 72 KB `{"event":"user",...}` line on stdin | one Flash call, about 20 000 input tokens | none |
| 8 | The updater ended by the tree kill (D11) | Windows | a run stopped while a `--bg-updater` child runs; then `agy --version` | none beyond a run already made | none |
| 9 | Repository upload A/B on two fresh repositories, with tool calls (ADR-0040 C9) | Windows | two runs with tool calls in two fresh repositories; agy's own log read for upload or sync lines | two short runs | none |
| 10 | The vendor's terms (ADR-0043 D17) | any | read the terms page; record the URL and what it says about data use and automation | 0 tokens | none |

## The FinCal issues

Two small throwaway issues, one per host, in the shape of the earlier
capstones (a one-file change with a bare `## Verify` command), opened on
`paulocorcino/FinCal` for this capstone and passed with `--issues <n>`.

## Clean up

- FinCal: the temp clones deleted; the `afk/run-*` branches a run pushes
  are its delivery (merge by hand) and are recorded, not deleted.
- The operator is signed in on both hosts at the end.
