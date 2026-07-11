# Telegram notifier: a CLI-owned, read-only run monitor driven by a shared `RunState` fold

Ralphy gains an optional, always-on Telegram monitor: once a bot token and chat
are registered globally, every real `ralphy run` posts a single live **card** to
a Telegram chat and edits it in place across the run's whole lifecycle —
planning, per-issue execution, usage-limit sleeps, stops, and the final summary —
plus a short push message at the milestones that matter (start, sleep-in,
sleep-out, final outcome). It is **read-only**: the bot only reports; it accepts
no commands. As with the console UI (docs/adr/0006), this is a pure CLI concern —
the core and adapters never learn a notifier exists.

This is grounded in the same two facts ADR-0006 established by reading the runner
and the Claude adapter, plus three Telegram Bot API facts verified against its
documented behaviour. From the codebase: (1) the events that describe a run —
`queue built` (count, order), `issue started` (number, title — see D5),
`plan written` (open_steps), `green — issue closed`, `non-green — stopping run`,
the usage-limit wait heartbeats in `WallClock::wait_for_reset`, and the deadline
stops — are already structured `tracing` events with stable targets and typed
fields, a usable event bus as-is; and (2) the agent's own output is teed to files
(`plan.log`, `exec.log`) and never to Ralphy's terminal, so the longest phases
are silent — exactly what a remote monitor exists to illuminate. From Telegram:
(a) a bot can edit a sent message in place via `editMessageText`, so a live card
is one message, not a stream; (b) **editing a message raises no push
notification** — only a new `sendMessage` pings a phone — which forces the
edit-vs-ping split in D3; and (c) a bot cannot initiate contact, so a `chat_id`
must be discovered from an inbound message (D2). The workspace is fully
synchronous today (no `tokio` in the workspace deps); D4 keeps it that way.

The console presenter of ADR-0006 is **not yet implemented** (its slices #30–#32
are open, unmerged). This ADR therefore ships the first *built* consumer of the
event stream and, with it, the shared semantic model both consumers will render
from (D6). The two log-only event additions it depends on (D5) are the same ones
ADR-0006 D2 specified; whichever track lands first adds them, the other relies on
them — they are identical and idempotent, so neither duplicates the other.

## D1 — Always-on from global config; the notifier is a CLI-installed `tracing` Layer

Notifications are **on by default once configured**: a token and `chat_id` saved
in a global config file (D2) make every real run notify, with `--no-telegram` to
mute a single run. This matches the operator's intent ("uso global") and the
unattended nature of the tool — a monitor you must remember to enable is a
monitor you will run without.

The notifier subscribes to the existing event stream through a custom
`tracing_subscriber::Layer` installed in `main.rs`, exactly as ADR-0006 D1 chose
for the presenter and for the same reason: threading a typed `&dyn Reporter`
through `run_queue(...)` and the `Agent` trait would change the core's public
signature and push a notification concern across the core-agnostic boundary
ADR-0002 protects. The Layer stays entirely inside `ralphy-cli`. When no config
is present (or `--no-telegram` / a dry run, D7), the Layer and its worker are
**never installed**, so the feature has exactly zero cost when off.

## D2 — Token in a restricted global config file; `chat_id` via a guided `setup`

The token and `chat_id` live in a global config file resolved with the
`directories` crate (e.g. `~/.config/ralphy/config.toml`, `%APPDATA%\ralphy\` on
Windows), written with owner-only permissions. The environment variable
`RALPHY_TELEGRAM_TOKEN` overrides the stored token. We rejected an OS keyring
(Credential Manager / Keychain / Secret Service): it is stronger on a desktop but
unreliable on the headless Linux servers this tool is built to run on
unattended, and it adds a heavy dependency with two code paths to test.
"Protected" here means filesystem permissions, deliberately, not an OS vault — a
portable floor that works everywhere Ralphy runs.

A bot cannot message a chat it has never heard from, so a `chat_id` must be
captured from an inbound message. `ralphy telegram setup` owns this: it stores
the token, prompts the operator to send `/start` to the bot, polls `getUpdates`,
auto-detects the originating `chat_id`, and saves it. We rejected auto-capturing
any chat that messages the bot (it leaks notifications to anyone who finds the
bot) and manual `chat_id` entry (error-prone copy/paste). The command group is
`telegram setup | test | status | disable`: `test` sends a ping to confirm
token+chat, `status` shows the configured chat and a masked token, `disable`
removes the config.

## D3 — One edited card per run, plus a push at milestones

Each run owns **one** message: it `sendMessage`s a card at start and
`editMessageText`s that same message through the lifecycle to a terminal state.
Two concurrent runs are two independent cards. We rejected a single global
message aggregating all active runs — the operator's first instinct — because
independent OS processes would then have to coordinate on one `message_id`
through a shared state file, a file lock, and dead-process cleanup (a crashed run
orphans its line forever). A per-run card needs none of that and degrades
cleanly: a run that dies simply stops editing its own card. The shared-box design
is not foreclosed (it would layer shared state on top), merely not opened.

Because an edit raises no push (fact (b) above), a silent card alone would notify
the operator exactly once. So the card is the silent live view, and a short
**new** `sendMessage` is sent at the milestones that warrant a buzz: run start,
entering a usage-limit sleep, resuming from it, and the final outcome. This is
the literal reading of the request — read-only, aware of the whole lifecycle,
"ok, ciente" on every sleep — without flooding a phone on a long queue.

**Amendment (sleep notice is disposable).** Start/final pushes were later dropped
(the card carries those silently), and the sleep/resume pushes were reworked: a
limit that keeps re-parking (synthetic reset, ADR-0030) would otherwise post a
fresh buzz every cycle and bury the chat in an alternating
`waiting for reset` / `resuming` ladder. The engine now treats the sleep push as a
single disposable notice — it stores the sent `message_id`, deletes the prior
notice before posting a new one (so at most one is ever live), and on resume
deletes the notice outright with no `resuming` message, since the live card
already reflects the resume. `deleteMessage` was added to the transport for this.

**Amendment (progress ping).** An `editMessageText` never raises a Telegram
notification, so a run's progress — folded silently into the card — would buzz the
phone only once, at the initial send. To restore a buzz on genuine progress, a
real card edit posts a short `🔔` message (which does notify) and a later tick
deletes it after a 2s TTL, keeping the chat clean. Bursts of edits coalesce into
the one live ping, and the ping is suppressed while parked in a usage-limit sleep
(the disposable sleep notice already buzzes, and the 60s countdown re-render must
not ping every minute). The resume card edit is genuine progress, so it fires this
ping too — a self-deleting "resumed" buzz that complements the notice cleanup.

## D4 — Synchronous `ureq` on a worker thread fed by an mpsc channel

The Layer must never block the thread that emits a log on a network call, and the
binary is synchronous today. The Layer therefore only translates each event into
a lightweight message and pushes it onto a `std::sync::mpsc` channel; a single
background worker thread owns the `message_id`, the throttle, the refresh timer,
and performs the actual HTTP with blocking **`ureq`**. We rejected `reqwest`
(blocking) for the far larger transitive dependency tree it pulls into a lean
CLI, and `tokio`+async for standing up a runtime to serve one feature in an
otherwise synchronous binary.

The worker also drives a **throttled ~60s refresh** during the long silent phases
(execution and usage-limit sleep) so the card's elapsed/countdown stays alive
without approaching Telegram's per-chat edit ceiling (~1/s sustained — the 60s
cadence sits orders of magnitude under it, even with several runs). On run
teardown `main.rs` signals the worker to render the terminal state, send the
final milestone push, and flush; it joins with a bounded timeout so a wedged
network never holds the process open.

## D5 — Two log-only event additions (the unpaid ADR-0006 D2 debt)

The Layer's blind spot is the same one ADR-0006 D2 identified: the queue loop
emits no per-issue "started" event before planning, and the adapter's planning
event carries no issue number — so the active issue and its title would only be
knowable retroactively at `plan written`. ADR-0006 D2 specified the fix; its
slices (#30 for `issue started`, #31 for `budget_min`) are open but unmerged.
The fix is:

```rust
info!(number = issue.number, title = %issue.title, "issue started");
```

at the top of the per-issue work in `runner.rs`, and the existing execution event
gains a `budget_min` field so the card can show `12:43 / 45:00`. Both are
log-only: no trait signature changes, no new core dependency, no crossing of the
ADR-0002 boundary — the core emits one more datum and stays ignorant of any
consumer. Whichever track (this one or ADR-0006's) reaches `runner.rs` first adds
them; the additions are identical, so the second finds them present rather than
duplicating them.

## D6 — A pure `events → RunState` fold, shared with the future presenter

ADR-0006 D1 flagged that keying a consumer on raw `(target, message, fields)`
strings turns those messages into a consumed contract. With a second consumer
(the presenter) coming, two independent string-keyed mappings would drift apart.
So the event-to-semantics step is factored out: a **pure function** folds the
event stream into a transport-agnostic `RunState` (the run's title, the issues
and their per-issue status, the current phase, any active sleep with its reset,
the deadline, and the final summary). The Telegram worker renders a card from
`RunState`; when the ADR-0006 presenter is built it renders a terminal UI from
the *same* model. The fold is unit-tested in isolation, in the style of the
adapters' `classify_*` functions, so a drift between an event and the model that
reads it fails a test rather than silently breaking a display. `RunState` is
per-run and in-process — matching the per-run card of D3.

Rendering keeps the card within Telegram's 4096-char message limit: a large queue
collapses to counters plus the active issue and the most recently finished ones,
rather than one line per issue unbounded.

## D7 — Dry runs do not notify; failures are loud at startup, best-effort at runtime

A `--dry-run` only plans, makes no commits, and is typically local iteration; it
does **not** notify by default, so plan-probing never spams the chat. Real runs
always notify.

The failure contract separates two kinds. A **config** error detectable at
startup — token or chat missing or malformed, or a `getMe` that fails — surfaces
once as a visible `warn!` ("Telegram on but getMe failed — continuing without
notifications") and the run proceeds normally; the run's real job is working
issues, never reporting on them, so a broken notifier must not abort it.
**Runtime** errors — a network blip, a 5xx, a rate-limit — are best-effort: a
light retry, logged to `ralphy.log`, never propagated, never blocking. The
channel from Layer to worker is bounded and drops oldest under back-pressure so a
stalled network can never slow the run.

## Consequences

- A new module `ralphy-cli/src/telegram.rs` (the `Layer`, the worker thread, the
  `ureq` Bot-API calls, config load/save, and the `telegram` subcommands) and a
  transport-agnostic `RunState` fold (D6) — placed so the ADR-0006 presenter can
  consume it without depending on Telegram. `ralphy-core` and the adapters are
  unchanged except for the two log-only additions in D5.
- New `ralphy-cli` dependencies: `ureq` and `directories`. No async runtime.
- The consumed event messages/fields (D5, D6) are a contract; the pure-function
  fold's unit tests catch drift at build time, not runtime.
- The future "listening mode" (remote actions) the operator mentioned is out of
  scope; the stored token is reusable and the transport sits behind a thin
  module boundary, so a second use needs no redesign here.

Status: accepted — implemented as `crates/ralphy-cli/src/telegram/`
(`notifier`, `client`, `config`), covering D1–D7 including the D6 `RunState`
fold and the D5 log-only events.
