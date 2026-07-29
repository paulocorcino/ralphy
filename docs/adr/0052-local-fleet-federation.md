# Local fleet federation: two daemons, one workbench

Status: accepted.

A Windows operator with projects inside WSL cannot see them. The daemon lists
the repos of the environment it runs in, and a WSL repo belongs to a different
environment — a different filesystem, a different toolchain, a different vendor
CLI inventory.

The instinctive fix is to let the Windows daemon reach across the boundary and
read `\\wsl$`. [ADR-0032](0032-daemon-mode-supervised-launcher.md) §3 already
rejected that — "one daemon per environment; WSL is just Linux", no `\\wsl$`
registry reads, no `wsl.exe` spawn recipes — and
[CONTEXT.md](../../CONTEXT.md) already says a session is spawned native to the
hosting daemon's OS, "picking a WSL repo means picking the WSL daemon, never a
cross-boundary spawn".

So the architecture was never the gap. The gap is that **nothing federates the
two daemons**. ADR-0032 answers this with the **control plane** (Phase 2,
outbound tunnel, enrollment), and the **fleet** vocabulary in CONTEXT.md already
names "a Windows host and its WSL distro" as two members of one machine — but
no line of that exists in code, and an operator should not need a hosted control
plane to see the repo sitting on the other side of their own laptop.

This ADR defines **local fleet federation**: the same fleet shape, between
daemons on one machine, with no control plane, no inbound port, and no human
step.

## Grounding

Every decision below rests on measurements taken on a Windows 11 host with
WSL2 Ubuntu-22.04 in default NAT mode. They are recorded because several of
them contradict the intuitive design.

| Measurement | Result |
|---|---|
| Windows → WSL listener bound to `127.0.0.1` | **HTTP 200** (WSL2 `localhostForwarding`) |
| WSL → Windows listener bound to `127.0.0.1` | fails (separate netns) |
| WSL → Windows listener bound to `0.0.0.0`, via gateway | HTTP 200 |
| `git rev-parse` on a repo over `\\wsl.localhost` | **fails** — `detected dubious ownership` |
| Recursive walk of a 103,596-file repo over `\\wsl.localhost` | **aborted at >120 s** |
| Single directory listing over `\\wsl.localhost` vs native | 58 ms vs 15 ms |
| `git status` native inside WSL, same repo | **22 ms** |
| `wsl.exe -e true` warm | 163 ms |
| Write to `/mnt/c/Users/<user>/.ralphy/` from WSL | works, 7 ms |
| `chmod 600` on `/mnt/c` | **silently ignored** — 9p drvfs without `metadata` |

## Decision

### 1. Nothing crosses the filesystem boundary

No daemon reads or writes another environment's repo. This restates ADR-0032
§3, but the measurements above turn it from a preference into a constraint:
git **refuses to operate** on a WSL repo reached over UNC, and the workbench's
tree walk does not complete. A cross-boundary daemon would not be a slower
workbench; it would be a broken one.

Two further breakages make the point structural rather than a matter of
tuning. With `rev-parse --show-toplevel` failing,
[`project_slug`](../../crates/ralphy-core/src/git.rs) falls back to hashing the
path string, so the same repo registers under two identities depending on which
side observed it. And the vendors' session stores are keyed by a mangled cwd
(Claude) or a lowercased Windows path (Gemini), so token attribution would
split too.

The **only** thing that crosses the filesystem boundary is the peer descriptor
of §3 — one small file, written once per daemon boot.

### 2. Transport: the local daemon dials the peer over loopback

The daemon that serves the browser is the **local** daemon; every other
enrolled daemon on the machine is a **peer**. The local daemon opens the
connection; the peer never dials in.

Both daemons stay bound to `127.0.0.1`. This is possible only because WSL2's
`localhostForwarding` relays a Windows-side `127.0.0.1:<port>` connection into a
WSL listener bound to `127.0.0.1` — measured, and the reason this ADR does not
need the outbound tunnel Phase 2 requires.

The consequence is the load-bearing one: **neither daemon ever leaves
loopback**, so both keep `AuthPolicy::Localhost`, no firewall rule is added, no
LAN interface is exposed, and the operator's credential experience is unchanged.

The federated request is server-side. A browser page served by the local daemon
cannot call a peer directly: the origin check matches the daemon's own bound
**port** and the daemon emits no CORS header at all, so a cross-port fetch is
refused and would be discarded by the browser even if it were not. The local
daemon therefore proxies, and the browser only ever speaks to one origin.

This introduces the first HTTP client into `ralphy-daemon`, which today has
none. That is a real cost, accepted deliberately: it is one dependency, at one
seam, replacing the alternative of a second listener and a second origin.

### 3. Discovery and handshake: one descriptor file, one token per daemon

Nothing on disk records a daemon's live port today — not `daemon.toml`, not any
pid file — and autostart does not pass `--port` through. Federation needs that
fact to exist, and it needs a credential. Both are the same file.

On boot, a daemon that was started with a peer target writes a **peer
descriptor** into that peer's store (from WSL, `/mnt/c/Users/<user>/.ralphy/`;
measured writable at 7 ms), carrying:

- `daemon_id`, `name`, `avatar` — the ADR-0032 identity triple;
- `port` and the loopback address to dial;
- the environment label (distro name, OS);
- the **access token of the announcing daemon**;
- the **protocol version** it speaks.

The handshake is then trivial: the local daemon reads the descriptor and dials
with `Authorization: Bearer <token>`, which the daemon already accepts under
both `Bearer` and `Session` policies with a constant-time compare. No new
cryptography, no enrollment code, no human step.

**Each daemon keeps its own token.** ADR-0032 requires a per-daemon revocable
credential — "revoking one daemon never shuts the fleet" — so a single secret
shared by both is refused: revoking it would revoke both.

The protocol version is carried for a reason the topology forces: two daemons
on one machine are upgraded independently, and a Windows daemon newer than its
WSL peer must fail with a legible message rather than a decoding error. The
version is checked on connect and a mismatch marks the peer degraded, not
silently partial.

**What the token does and does not protect.** It protects against other users
and other machines. It does **not** protect against malicious code running as
the operator, which can simply read the descriptor — and on `/mnt/c` it cannot
even be mode-protected, because that mount is 9p drvfs without `metadata` and
`chmod 600` is silently ignored, leaving only the Windows profile ACL. This is
not a regression: it is the trust boundary the daemon already operates under,
where the `Localhost` policy authorizes everything on loopback with no
credential at all. It is recorded so no one describes this mechanism as
protection it does not provide.

### 4. Liveness: nudge, never supervise

A WSL daemon dies in ways a Windows one does not. The VM idle-terminates
(`vmIdleTimeout`), and `systemd --user` does not survive the last session
without `loginctl enable-linger`. A descriptor therefore outlives its listener:
the port is announced, the daemon is gone.

So a descriptor is a **claim, not a fact**. The local daemon probes the peer and
reports it as unreachable exactly the way the registry already reports an
unreachable repo — entries are never auto-deleted, only marked.

When a peer is unreachable, the local daemon may **nudge** it: a
fire-and-forget `wsl.exe -d <distro> -e …` that asks the distro's own systemd to
start the unit, measured at 163 ms warm. The daemon does not hold the `wsl.exe`
process, does not parent the daemon through it, and does not signal it.

This is the distinction that keeps §4 inside ADR-0032 rather than against it.
What ADR-0032 rejects is *supervising* work across the boundary — "opaque
process trees, signal quirks". What it already blesses is a wake nudge (the
documented `schtasks` task that wakes the distro at logon). Starting a peer is
the second kind: supervision belongs to systemd **inside** the distro, never to
a Windows parent.

`loginctl enable-linger` is therefore a documented prerequisite, not an
implementation detail — without it the unit cannot outlive the session that
started it, and no nudge can fix that.

### 5. Identity: the aggregate key is `daemon_id` + slug

The registry is a `BTreeMap<slug, RepoEntry>`, and a slug is not unique across a
machine. This is measured, not hypothetical: `paulocorcino/FinCal` is registered
on **both** sides of this very host — `C:/Dev/FinCal` on Windows and
`/home/corcino/FinCal-273` in WSL. A naive merge silently discards one.

The federated view is therefore keyed by `(daemon_id, slug)`. The per-daemon
store keeps its own shape — a daemon's `repos.toml` is unchanged, and each
daemon remains authoritative for its own repos. Only the aggregate is composite.

This propagates: `desk.toml` records reference a repo by slug
([ADR-0050](0050-desk-layout-is-daemon-state.md)), and a desk that spans two
environments must disambiguate the same way.

### 6. Capability: the roster reports presence, per daemon

The roster is capability-blind by design — it reports what the daemon *can
launch*, "never whether the vendor CLI is installed or authenticated on the
host". Federation makes that blindness misleading rather than merely
incomplete: the vendor inventory genuinely differs per environment, so a global
agent list offers agents that cannot run where the repo lives.

The roster gains an **availability** field, computed by each daemon for its own
environment via `ralphy_proc_util::locate_program` — already the single source
of truth for "is this program available, and where", and already a dependency of
`ralphy-daemon`. No new crate, no new probe, no adapter-trait method. The
workbench's agent row already carries `disabled` and a `title`, so the UI
insertion point exists.

Because each daemon computes its own, federated availability is correct with no
cross-boundary probing at all.

**Presence, not login.** Probing a vendor login requires the vendor crates,
which the daemon must not link (ADR-0032 §10), and
[ADR-0013](0013-run-auth-preflight.md) already settled that the run preflight is
presence-only. Login validation stays where it lives: `ralphy init`, run inside
the environment it is validating.

Availability is deliberately a *signal*, not a gate: the existing "CLI not
found" errors remain the backstop. The signal exists because presence and
usability diverge here in a way an operator cannot see. On this host `opencode`
runs from WSL (1.18.4) — but resolves to `/mnt/c/…`, a Windows binary reached
through interop, which `locate_program` refuses on purpose (ADR-0043 D16), since
a Linux daemon must not spawn a Windows binary against a Linux cwd. Without the
signal, the operator sees "installed" and the daemon says "not found".

### 7. What crosses, and what never does

| Surface | Where it runs |
|---|---|
| File tree, file read, image bytes, file write | the **owning** daemon |
| git, Changes, stage/commit/discard/sync/push | the **owning** daemon |
| Run and queue dispatch, run snapshots, watch | the **owning** daemon |
| Agent workbench sessions | the **owning** daemon, always |
| Aggregated repo list, desk, UI assets | the **local** daemon |
| Free console | the **local** daemon (see below) |

A **free console** may be hosted by the local daemon with `wsl.exe -d <distro>
--cd <path>` as the PTY program. The scrollback ring and reattach then stay on
one side, which is a real reduction in proxy work, and a free console is by
definition an unmanaged shell — nothing about a run depends on it.

An **agent session** may not. It needs the environment's toolchain and the
repo's `.ralphy` artifacts, CONTEXT.md requires it to be native to the hosting
daemon's OS, its events would carry the wrong daemon's identity, and resize and
Ctrl+C through `wsl.exe` are precisely the signal quirks ADR-0032 names.

`verify.command` remains denied at the remote boundary. Federation does not
widen it — a proxied request is still a remote request, and it becomes `argv[0]`
of a child spawned in the repo root either way.

## Rejected alternatives

- **Read WSL repos over `\\wsl$` / `\\wsl.localhost` from the Windows daemon.**
  Rejected by ADR-0032 §3 and confirmed by measurement: git refuses with
  `dubious ownership`, the tree walk does not finish in two minutes, and the
  project identity splits. This is not a performance trade-off; the workbench
  does not function.
- **The WSL daemon dials out to the Windows daemon** (the ADR-0032 control-plane
  shape, applied locally). Rejected on measured cost, not on principle. WSL
  cannot reach the Windows `127.0.0.1`, so the Windows daemon must bind
  non-loopback — which flips it out of `AuthPolicy::Localhost` and makes the
  operator authenticate to their own desktop, exposes the listener to the whole
  LAN on `0.0.0.0`, or pins it to a NAT gateway address that changes across WSL
  restarts. It also exposes the listener to every **sibling distro** (a
  `docker-desktop` distro shares that NAT). The inbound direction costs none of
  this. Phase 2's dial-out remains right for a *remote* control plane, where
  there is no shared loopback to exploit.
- **A stdio transport: the Windows daemon spawns `wsl.exe … --peer-stdio` and
  speaks the protocol over the child's pipes.** Rejected: it needs no port and
  no token, but it makes the Windows daemon the parent and supervisor of the
  peer — the exact opaque-process-tree shape ADR-0032 rejects — and the peer
  dies with its Windows parent.
- **A new wire format (protobuf or similar) for daemon-to-daemon traffic.**
  Rejected: [ADR-0036](0036-workbench-daemon-integration-protocol.md) already
  defines a protocol whose frames carry exactly what this needs, including RAW
  BYTE output. A second encoding at a second seam buys nothing and adds codegen.
- **One shared secret for both daemons.** Rejected: ADR-0032 requires a
  per-daemon revocable credential.
- **Waiting for the Phase 2 control plane.** Rejected: it requires hosted
  infrastructure and enrollment to solve a problem contained entirely within one
  laptop. This ADR is a strict subset of that shape and should converge with it.
- **Making agent availability a hard gate in the workbench.** Rejected: a probe
  is a snapshot, an operator can install a CLI without restarting a daemon, and
  the spawn-time error already exists. A wrong gate blocks work; a wrong signal
  is merely stale.

## Consequences

- The operator sees every project on the machine in one workbench, with each
  operation executing natively where the project lives.
- **Credentials are duplicated per environment, permanently.** `gh`, and every
  vendor's session store, are per-HOME; they expire and drift independently.
  This has no technical fix and is the standing cost of two environments.
- **Usage must federate or it lies.** The usage resolvers read the HOME of the
  process that runs them, so an unfederated aggregate silently omits everything
  spent in WSL. Partial numbers presented as totals are worse than absent ones;
  usage federation is in scope, not a follow-up.
- **Vendor parity is environmental and now visible.** The roster surfaces the
  difference rather than resolving it — installing a CLI in a distro stays the
  operator's job.
- The two daemons must be upgraded together, and the handshake makes that
  legible instead of mysterious.
- `ralphy init` gains its full weight inside WSL: it is the existing gate that
  validates git, python, `gh` auth, the remote, and vendor presence and login
  for that environment. Note that it registers the repo *before* evaluating the
  gate, so a repo appears in the daemon even when the gate blocks — with §6 the
  UI tells that story correctly instead of offering an agent that cannot run.
- The security posture is unchanged, not improved: same-user code can read a
  peer descriptor, and on `/mnt/c` it cannot be mode-protected at all.
- The first HTTP client enters `ralphy-daemon`.

## Amendment (2026-07-29): the federated repo surfaces

A repo ref is the routed key: `<daemon_id>/<slug>` names a peer repo, while a
bare slug names a local repo.

`POST /api/peer/command` carries the existing `protocol::Command` JSON. There is
no second wire format.

File watches use `POST /api/peer/tree/poll` and `POST /api/peer/tree/close`.
Long polling keeps the existing HTTP transport; a WebSocket client would add a
second transport dependency.

`POST /api/peer/command` resolves repos against its local registry only. It
never re-routes a command to another peer.

## Amendment (2026-07-29): peer-owned PTY sessions

An agent session for `<daemon_id>/<slug>` is opened through an authenticated
peer WebSocket to `/ws/session`; the local daemon upgrades the browser only
after that peer handshake succeeds. It then relays WebSocket frames unchanged.

The owning daemon's numeric session ID remains authoritative. Reattach and close
pair that ID with the composite repo ref, so equal numeric IDs on two daemons
remain distinct. `GET /api/sessions` federates peer rows and `POST
/api/sessions/close` routes by that same pair.

Every attachment starts with a `session-open` command carrying the owning
daemon ID and environment, before scrollback or live output. Deliberate
`session-end` commands retain that identity.

Proxy teardown is detach-only. Browser close, peer close, transport error, or
local-daemon shutdown drops the two bridge sockets but never invokes peer
session close; the peer-owned child and ring buffer therefore survive a browser
reconnect and a replacement local proxy.
