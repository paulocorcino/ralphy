# Console UI: a CLI-owned presenter driven by the existing `tracing` event stream

Ralphy gains an animated, Claude-Code-style console (spinners, a queue progress
bar, semantic colour, emoji status, finished-issue lines that scroll up) **without
touching the core's API or adding a vendor-aware UI seam**. The presenter is a
pure CLI concern: it installs a custom `tracing_subscriber::Layer` that consumes
the `info!`/`warn!`/`error!` events the core and adapters *already* emit and
renders them with `indicatif`. The core keeps emitting plain events; it never
learns a UI exists.

This is grounded in two facts established by reading the runner and the Claude
adapter. First, the events that describe a run — `queue built` (count, order),
`plan written` (number, open_steps), `green — issue closed`, `non-green — stopping
run` (outcome), and the planning/execution events — are already structured
`tracing` events carrying stable targets (`ralphy`, `ralphy_core::runner`,
`ralphy_agent_claude`) and typed fields. They are a usable event bus as-is.
Second — and decisively — during both planning (`claude -p`) and execution (the
live PTY session), the agent's output is captured to files (`plan.log`,
`exec.log`); it is **never mirrored to Ralphy's own terminal**
(`ClaudeAgent::drive_session` tees the PTY to `exec.log`). Ralphy therefore owns
the visible terminal for the entire run, so an animated region never contends
with the child for the screen — and the longest phase today (a multi-minute
execution) is a silent black box, which is the strongest motivation for the work.

## D1 — The event source is a custom `tracing` Layer, not a typed reporter trait

The presenter subscribes to the existing event stream through a `Layer` installed
in `main.rs`. We rejected threading a typed `&dyn Reporter` through
`run_queue(...)` and the `Agent` trait: it would change the core's public
signature and every call site/test, and push a UI concern across the
core-agnostic boundary ADR-0002 protects. We also rejected the span-oriented
`tracing-indicatif` crate, which would require instrumenting per-issue work as
`#[instrument]` spans inside the core — again a core change — for less layout
control. A `Layer` keeps the entire UI inside `ralphy-cli`; the core stays a queue
engine that happens to log.

The cost is explicit: the presenter keys on event `target` + `message` + fields,
so a handful of message strings become a **consumed contract**. We accept this by
keying on structured fields wherever a datum is needed (more stable than rendered
text) and by marking each consumed event in the source with a short comment
(`// message consumed by the CLI presenter — keep stable`). The mapping from
`(target, message, fields)` to a UI action is a **pure function**, unit-tested in
isolation in the same style as the adapter's `classify_*` functions, so a drift
between an event and the UI that reads it fails a test rather than silently
breaking the display.

## D2 — Two log-only additions are the entire footprint outside the CLI

The Layer approach has one blind spot: the queue loop emits no "issue N started"
event before planning, and the adapter's planning event does not carry the issue
number (the adapter never receives it). Without a fix the presenter could only
learn the active issue *retroactively*, at `plan written` — exactly the phase we
want to animate would be anonymous. Rather than have the Layer infer the current
issue by advancing a pointer over the `queue built` order (which `blocked` /
`stop-before` / `skipped` issues desynchronise), we add **one** event at the top
of the per-issue work in `runner.rs`:

```rust
info!(number = issue.number, title = %issue.title, "issue started");
```

Symmetrically, so the active line can show the per-issue budget
(`12:43 / 45:00`), the adapter's existing execution event gains **one field**,
`budget_min`. Both changes are log-only: no trait signature changes, no new
dependency in the core, no crossing of the ADR-0002 boundary — the core and
adapter emit one more datum and remain ignorant of the UI. This is the deliberate
limit of the footprint; anything richer would have meant a reporter trait, which
D1 rejected.

## D3 — Pretty by default; `--verbose` and no-TTY degrade deterministically

By default the console shows **only** the presenter. The full structured log is
always written to the run's `ralphy.log` (no colour), so suppressing raw lines on
screen never loses the audit trail. `warn!`/`error!` still surface on screen as
styled lines routed through `indicatif`'s writer (so they never corrupt the live
bars) — chosen over enumerating every warning in the presenter, so a future
warning cannot silently vanish from the terminal.

- `--verbose` (also engaged by `RUST_LOG`/`RALPHY_LOG`) drops to the raw INFO
  `fmt` lines and **disables animation**, so debugging is unobstructed.
- When stderr is **not a TTY** (a pipe, CI, `--headless-exec` with no terminal)
  or `NO_COLOR` is set, the presenter auto-detects and prints **one clean line per
  key event** (local timestamp, no spinner, no colour). No ANSI ever reaches a
  redirected file.

`indicatif` (with `console`) is the renderer: it provides `MultiProgress`, a
self-ticking spinner, TTY detection, `NO_COLOR` handling, and enables virtual-
terminal sequences on Windows — the parts most likely to be buggy if hand-rolled.

## D4 — Layout, vocabulary, and the local-clock fix

The animated region is a queue progress bar plus **one** live line for the active
issue (phase icon · `#n` title · model · `elapsed / per-issue-budget`); finished
issues print as permanent lines that scroll up, each carrying an **absolute local
timestamp**, an outcome emoji, and a duration. The final summary replaces the
per-issue re-listing (those lines already scrolled) with a **totals panel** —
counts by outcome, commits on the branch, the stop reason if any, and the single
actionable next step (`➜ git merge afk/run-…`).

Status vocabulary is emoji with semantic colour and an automatic ASCII fallback
when the terminal/locale cannot render it: 🧠 planning · ⚙️ executing · ✅
done/closed · ⛔ blocked · ⏱️ timeout · 🪨 stuck · 🌙 limit(reset) · ⏭️ skipped ·
🤷 infeasible · 📋 queue · ⚠️ warn · 💥 error (green = done, red = failure, yellow =
wait/warn, cyan = active, dim = pending/skip).

Finally, the reported root complaint — log timestamps in UTC, not local — is
fixed at the source by replacing the `fmt` layer's default UTC timer with
`ChronoLocal` (enabling `tracing-subscriber`'s `chrono` feature; the crate already
depends on `chrono` and uses `chrono::Local` throughout). This applies to
`ralphy.log` and `--verbose`; the presenter composes its own local timestamps via
`chrono::Local` directly.

## Consequences

- The entire UI lives in a new `ralphy-cli/src/ui.rs` (the `Presenter`, the
  `Layer`, the pure `(target, message, fields) → action` mapping, the icon/colour
  table, the non-TTY renderer, and the final totals panel). `ralphy-core` and the
  adapters are unchanged except for the two log-only additions in D2.
- Two crate dependencies are added to `ralphy-cli`: `indicatif` and `console`. The
  `chrono` feature is enabled on the workspace `tracing-subscriber`.
- Error paths must clear the live bars before `anyhow` prints, so a `bail!` is not
  tangled with a spinner. The presenter owns this teardown.
- The consumed event messages (D1) are a contract: changing one without updating
  the presenter's mapping breaks the display, but the pure-function unit tests
  catch the drift at build time rather than at runtime.
- Observability is unchanged for machines: `ralphy.log` and the no-TTY line mode
  preserve everything CI or a log scraper reads today; only the interactive
  terminal experience changes.
