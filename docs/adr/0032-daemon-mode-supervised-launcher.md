# Daemon mode: a supervised launcher, never a runtime

Status: proposed (design interview 2026-07-09; not yet implemented).

Ralphy gains a resident "department": a process that survives between runs,
starts with the OS, and gives the operator remote reach — triggering
operations from anywhere, driving interactive agent CLI sessions from a phone
browser, and eventually a web control plane over a fleet of machines. This is
the daemon ADR-0019 explicitly deferred ("a future daemon is just a periodic
invoker of the same emission paths") and ADR-0026's ethos constrains ("the
run, not the cron"). This ADR promotes the deferral to a decision and fixes
the surface before implementation slices, in the ADR-0026 manner. Vocabulary
(**Daemon**, **Workbench session**, **Control-plane tunnel**, daemon
identity) is defined in [CONTEXT.md](../../CONTEXT.md); this document records
the decisions.

## Decision

### 1. A launcher, never a runtime

`ralphy daemon` is a top-level subcommand of the same binary (the ADR-0026
argument verbatim: a department gets a noun, not a flag), with
`install`/`status`/`uninstall` for OS autostart, mirroring `schedule`. It is a
**supervised launcher**: a remote command makes it spawn an ordinary
run-scoped child process — the same `ralphy run --if-idle` invocation a cron
timer would fire. The run's execution loop, the adapters, and the core never
move into the daemon's process; a daemon crash never takes a run down with
it. "The run, not the cron" survives the daemon because the daemon *is* the
cron's replacement, not the run's host.

**Rejected: the hosting model** — runs executing inside the daemon process.
It would force the run lifecycle to become embeddable, promote PTY from
adapter capability to process infrastructure (against CONTEXT.md's execution
mode boundary), and couple every in-flight run's stability to the daemon's.

### 2. Workbench sessions: curated launcher, tmux-model persistence

The daemon hosts **workbench sessions** — human-driven interactive agent CLI
sessions (Claude/Codex/OpenCode) rendered in a browser via xterm.js. The
curated launcher (repo × agent, one click) is the product; a free console is
a separate, explicit session kind, not the default. The curated form is an
intent-and-surface boundary, not a hard security boundary (an interactive
agent with permissions *is* a remote shell in practice) — recorded so nobody
mistakes it for one.

Sessions belong to the daemon, not the connection: the PTY (via the existing
`ralphy-pty`, `portable-pty` 0.9) and a scrollback ring buffer survive a
dropped WebSocket, and a reconnecting browser **reattaches**. This is the
requirement that makes mobile/travel use real — a session must outlive every
4G tunnel — and it resolves the "waiting on a human" workflow: a session
opened at the desk in the morning is answered from the phone in the
afternoon, same session.

### 3. One daemon per environment; WSL is just Linux

WSL gets **its own daemon** — the plain Linux build, systemd unit, own
`~/.ralphy` — not a Windows daemon reaching across the boundary. No `\\wsl$`
registry reads (they fail when the distro hibernates), no `wsl.exe` spawn
recipes (opaque process trees, signal quirks). Each daemon does only what is
native to its OS; the control plane groups a machine's daemons by host. A
resident daemon inside the distro also keeps WSL from idle-terminating —
desired for remote access. The one seam: the distro must be woken at Windows
logon (a one-line scheduled task), documented, not architecture.

The daemon's **repo registry** is passive: every `init`/`run`/`triage`
upserts its repo into the daemon-readable global store, keyed by the ADR-0008
project identity (`owner/repo` slug) with `path` as a mutable attribute — a
moved repo self-heals on its next run. Entries are never auto-deleted, only
marked unreachable; removal is a human act. `ralphy daemon add` exists only
for registering a repo before its first run.

### 4. Exposure: local listener first, outbound tunnel second, inbound never

The daemon **never opens an internet-facing port and never implements TLS**.
Two transports over one transport-agnostic protocol (channel-multiplexed
terminal streams + control commands + presence heartbeat):

- **Phase 1 — self-sufficient local daemon.** An axum HTTP+WebSocket listener
  bound to `127.0.0.1` by default, serving an embedded minimal UI (static
  HTML + xterm.js, embedded like `assets/prompts`). Non-localhost bind
  (`--bind` / `daemon.toml`, e.g. a Tailscale interface IP) is explicit
  opt-in and **requires a bearer token** (generated at install); localhost is
  exempt. Personal remote access works in this phase via an overlay VPN with
  zero extra code.

  **People vs. machines (issue #179).** Over a network bind the two client
  kinds authenticate differently:
  - **Machine clients** (the phone `run` trigger, CloudEvents consumers, curl)
    keep the **static bearer token, unchanged** — one `Authorization: Bearer`
    header per request.
  - **Browsers** get an interactive **login screen**. The core factor is
    **TOTP** (RFC 6238, Authy/Google Authenticator); an operator-set
    **password is an optional second factor** (defense-in-depth, the weakest /
    highest-friction link — TOTP first, password opt-in). A valid login mints a
    **signed, stateless, short-TTL session cookie** (`ralphy_session`): value
    `1.<exp>.<HMAC-SHA1(token, "1|<exp>")>`, signing key = the daemon access
    token, verified by recompute + constant-time compare + `exp > now` — **no
    server-side session store** (re-minting the token invalidates live cookies,
    accepted). TTL is a fixed 12h. The cookie is `HttpOnly; SameSite=Strict;
    Path=/` but **not `Secure`**: the daemon never does TLS (this §4) and rides
    Tailscale/localhost for transport confidentiality.

  **Posture: recommended default, operator's choice.** TOTP (+ optional
  password) is the recommended hardening for a network bind and is documented
  as best practice, but it is **opt-in, never forced**: a network bind resolves
  to the session policy only once a TOTP seed is enrolled (via `ralphy daemon
  setup`, mint-once like the token); with no seed a network bind stays
  bearer-only — the operator is never denied the bearer-only trade-off they
  accept. Localhost stays frictionless (no login, no token).
- **Phase 2 — control-plane tunnel.** The daemon dials **out** (WSS, 443) to
  the control-plane relay and the same protocol rides the tunnel (the GitHub
  Actions runner / Cloudflare Tunnel pattern). The relay is a stateless
  bridge; session state stays in the daemon. The relay host sits in the trust
  path and is therefore critical infrastructure.

**Rejected: direct internet exposure** (inherits the whole attack surface for
one user) and **building the relay first** (two systems in the dark; Phase 1
matures the protocol with a real user before it becomes a contract).

### 5. The tunnel carries only what is interactive

Run telemetry does **not** move into the tunnel. Runs — daemon-spawned or
not — keep their own CloudEvents sink (ADR-0019, unchanged), so a daemonless
cron run stays observable and the events contract never forks. The tunnel
carries terminals, commands, and daemon presence: the things that need a
resident process and a round trip. One additive stitch: a daemon-spawned run
carries `emitter.daemon_id` in its CloudEvents (injected via the child's
environment), letting the platform correlate run ↔ fleet daemon without
host/PID guessing; cron/manual runs simply lack the field.

**Live logs, Part A only (issue #180).** A run the daemon spawns streams its
merged stdout+stderr over the existing `/ws/command` socket as transient
`status:"output"` frames, so the UI shows the live log of a run it fired — no
new on-disk log/history store, CloudEvents stay the sole run-history source.
This is bounded to runs the daemon spawned: cross-cutting/external run
observability (Part B) is **deferred entirely to the events platform**, with
no daemon-side CloudEvents mirror. The stream never weakens teardown — a
detached drain reads the child's pipe to EOF regardless of client presence, and
no path kills the child (the child ignores `SIGPIPE`, so a daemon crash yields
a non-fatal broken-pipe write, not a kill).

### 6. Remote command vocabulary v1: only what cron could already fire

The daemon accepts: `run` (spawns `ralphy run --if-idle`), `triage` (spawns
`ralphy triage --if-idle --yes`), `queue` (spawns `ralphy issues --push`),
plus daemon-native verbs (list/open/reattach/close workbench sessions,
status, registry). These are exactly ADR-0026's blessed invocations — the
overlap guard and the triage trust act come along for free, and the daemon
gains **no powers a scheduled timer lacks**, so authorization stays binary
(may talk to this daemon ⇒ may use the whole vocabulary).

**Deliberate exclusion: no remote kill.** Killing a run mid-flight leaves the
tree and branch in arbitrary state; Ralphy's stop mechanisms remain
`stop-before` and non-green stops. If remote kill ever justifies itself, it
is its own decision with a strong confirmation, never a v1 fat-finger.

The vocabulary also carries a read-only **forge query** family (design
interview 2026-07-09, second pass): request/response verbs — issues in any
state, an issue's full thread, labels, branches — answered by the daemon with
the operator's local forge auth, so the control plane keeps holding no forge
token (the ADR-0019 stance, extended from push to pull). Verbs are named in
Ralphy's vocabulary, parameterized and paginated, each backed by a **fixed
read-only invocation** with the repo always resolved from the repo registry;
GitHub is the only implementation, behind a forge-neutral contract.
Authorization stays binary because nothing in the vocabulary writes.

**Rejected: a raw forge passthrough** (arbitrary GraphQL / `gh api` relayed
by the daemon). Two independent reasons, either fatal: (1) the operator's
forge auth can *write* — and GraphQL carries queries and mutations through
one endpoint, so proving a relayed query harmless means building an
inspection guardrail that fails open, versus an allowlist that fails closed;
(2) raw queries are written in the forge's dialect, so the passthrough
maximally couples the control plane to GitHub — the forge-portability
requirement itself forces a Ralphy-owned vocabulary. Deliberately *not*
built now: a second forge implementation, a multi-impl trait, or a
constrained ad-hoc query verb — each is its own future decision with an
actual buyer.

### 7. Telegram commands live on the control plane

Telegram's `getUpdates`/webhook allows **one consumer per bot token**, so a
fleet of daemons cannot each answer the bot. The control plane owns the bot
(webhook — it has public HTTPS), authenticates by `chat_id`, resolves the
target daemon by **name**, and dispatches over the tunnel in the §6
vocabulary: Telegram is just another UI of the control plane. Consequence:
Telegram *commands* do not exist until Phase 2 — a daemon-side interim bot
would be throwaway and is not built. The per-run Telegram **notifier**
(ADR-0007, `sendMessage` push) is untouched and keeps working with no daemon
and no control plane.

### 8. Daemon identity and enrollment

Three layers, three audiences: `daemon_id` (ULID minted once at install,
persisted in `~/.ralphy/daemon.toml` — the machine key; credentials and
history reference it, humans never see it), a **name** (operator-given at
install, hostname-suggested, fleet-unique — enforced by the control plane at
enrollment — the handle humans *and models* address: "run X on *anvil*";
names colliding with command-vocabulary terms ("forge", "queue", "run"…) are
refused at enrollment, since the name exists precisely to be unambiguous to
models; renameable without touching the ULID), and an **emoji avatar** (picked by
number from a list at install; cosmetic). Models and humans speak the name;
machines speak the ULID — name→id resolves once at the control plane, so a
rename never breaks anything in flight.

Fleet admission is **enrollment**: the control plane issues a one-time,
short-lived code; `ralphy daemon enroll <url> <code>` exchanges it for a
long-lived **per-daemon revocable credential**. Revocation is per daemon (a
stolen laptop ≠ shutting the fleet). The credential lives in the global
store, mode 0600, **never under `.ralphy/`** (the coding agent's scratch
dir — the ADR-0019 exfiltration argument verbatim), and is stripped from the
environment of every child the daemon spawns, like `RALPHY_EVENTS_TOKEN`.

### 9. One platform, two data paths

The control plane (Phase 2, does not exist yet) is **one web application**:
CloudEvents consumer (ADR-0019/0020), tunnel relay, Telegram webhook, fleet
UI with xterm.js. The two data paths stay separate in protocol (§5) and
converge only at the destination — fleet presence (tunnel) beside run
telemetry (events) beside queue snapshots (`--push`) on one screen, one auth,
one deploy. A separate relay service was rejected: the fleet view would have
to join two services' data for one operator's benefit.

### 10. `crates/ralphy-daemon`: the async runtime is confined

The daemon is a new library crate, `crates/ralphy-daemon`, wired by
`ralphy-cli` as the subcommand. It brings the workspace's **first** async
stack — tokio (narrow features, not `full`), axum (HTTP + WebSocket upgrade
in one listener; tungstenite underneath) — and that stack is confined there:
`ralphy-core`, the adapters, and `ralphy-adapter-support` stay sync. The
crate depends on `ralphy-pty` for sessions (blocking PTY I/O bridged to tokio
via reader threads + channels) and reaches runs only by spawning `ralphy`
processes — it never imports the core. Cross-platform per CLAUDE.md: the
listener, session spawn, and autostart registration (a per-user HKCU `Run`
value on Windows / a systemd `--user` unit on Linux) must work on both
Windows and Linux, tested per the CONTEXT.md helper-bin convention.

Windows autostart is a per-user HKCU `…\CurrentVersion\Run` value (no
elevation, hidden console via `pwsh -WindowStyle Hidden`), chosen over a
machine-level `/SC ONLOGON` Task Scheduler task because the daemon is a
per-user loopback resident, not a machine service — systemd `--user` on
Linux/WSL already has this property (#177).

## Consequences

- The surface is fixed before slices: subcommand shape, session model,
  registry mechanics, bind/auth rules, command vocabulary, identity scheme,
  and crate placement are decided here, not re-decided per slice.
- **Phase 1 is independently shippable and useful** (local bench + overlay
  VPN remote); Phase 2 (control plane, tunnel, enrollment, Telegram
  commands) builds against a protocol that already has a working client.
- ADR-0019 §Rejected's deferred daemon is hereby resolved: the daemon is the
  periodic-invoker shape that decision reserved space for, and the events
  contract does not move.
- ADR-0007 and ADR-0026 are untouched: the notifier keeps pushing per-run;
  `schedule` keeps registering OS timers for operators who want timers —
  the daemon replaces neither, it adds a remote trigger with the same blessed
  invocations.
- `docs/events.md` gains one additive field (`emitter.daemon_id`, present
  only on daemon-spawned runs) under its existing additive-evolution rules.
- New vocabulary lands in CONTEXT.md (**Daemon**, **Workbench session**,
  **Control-plane tunnel**), and **Emitter identity**'s "no persistent
  instance id" caveat is amended: the persistent key is the daemon's, run
  events stay keyed by `runid`.
