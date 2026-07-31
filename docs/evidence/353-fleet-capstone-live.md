# Live evidence — local fleet capstone (#353)

Captured evidence for the HITL capstone of
[ADR-0052](../adr/0052-local-fleet-federation.md), following
[353-fleet-capstone-runbook.md](353-fleet-capstone-runbook.md). Phases below
mirror the runbook's; the verdict-per-phase and the design-vs-host sorting land
in `docs/adr/0052-local-fleet-validation.md` at the wrap-up.

## Resolved environment (captured 2026-07-31)

| Field | Value |
|-------|-------|
| Host OS | Windows 11 Pro 26200 |
| WSL distros | `Ubuntu-22.04` (default, WSL2) and `docker-desktop` (WSL2) — the **sibling distro** ADR-0052 names in its rejected alternative is present on this host |
| WSL networking mode | NAT (`.wslconfig` has no `networkingMode`), `localhostForwarding=true` — the §2 precondition holds |
| `vmIdleTimeout` | **30000** (30 s) — Phase 5 has a real, short idle window to exercise |
| `loginctl enable-linger` | `Linger=no` — **off**; the §4 prerequisite is unmet on this host as found |
| Distro user / HOME | `corcino` / `/home/corcino` |
| Windows `gh` | 2.89.0 — logged in as `paulocorcino` (keyring), scopes `gist, read:org, repo, workflow` |
| WSL `gh` | **2.4.0+dfsg1** (Ubuntu package, 2022) — logged in as `paulocorcino` via `/home/corcino/.config/gh/hosts.yml` |
| Windows git | 2.50.1.windows.1 |
| WSL git | 2.34.1 |
| WSL toolchain | cargo/rustc 1.96.0, python3 present, **`node` absent natively** |
| Windows daemon | `daemon_id = 01KX5QAGWETWBGA08VVFJAT6YT`, name `ralphy`, avatar 🐺 — **not running at capture time** |
| WSL daemon | **did not exist at capture time** — no `ralphy` on PATH, no `~/.cargo/bin/ralphy`, no `ralphy-daemon.service` unit |
| Peer store | `%USERPROFILE%\.ralphy\` — **no `peers/` directory**, i.e. no daemon had ever announced |
| Fleet code branch | `feat/ui-v1.1` @ `2fec0c2` — **`crates/ralphy-daemon/src/fleet.rs` does not exist on `main`**; #349–#352 are unmerged, 408 commits ahead |
| Distro checkout | `~/ralphy-ci`, `origin = /mnt/c/Dev/ralphy` (local clone), synced to `2fec0c2` |
| Peer repo under test | `~/FinCal-353` — `paulocorcino/FinCal`, base `master` @ `6092733`, clean (the authorized lab repo) |

### Bring-up notes (not defects)

- The distro had **no ralphy at all**: the capstone builds the Linux daemon from
  `~/ralphy-ci` with `CARGO_TARGET_DIR=$HOME/ralphy-target-wsl`. Both daemons
  must run the same commit — ADR-0052 says the two are upgraded together and the
  handshake makes skew legible — so the distro checkout was fast-forwarded to
  the Windows working commit before building.
- `~/ralphy-ci` carried an **uncommitted, superseded copy** of the `kill(1)` →
  `libc::kill` process-group fix. The branch already carries that fix
  (`crates/ralphy-proc-util/src/lib.rs:100`), so the working-tree copy was
  **stashed, not discarded** (`353-capstone: superseded kill(1) fix, pre-sync`)
  before the reset.
- ADR-0052 §5 grounds the composite-key decision on `paulocorcino/FinCal` being
  registered on both sides — `C:/Dev/FinCal` and `/home/corcino/FinCal-273`.
  **`FinCal-273` no longer exists in the distro**; the same-slug-both-sides
  condition was recreated as `~/FinCal-353`. This is grounding drift, not a
  design correction — the decision it supports is unaffected.

## Phase 0 — the topology stands up, and neither daemon leaves loopback (AC3)

### Distro listeners before bring-up

```
LISTEN 127.0.0.53%lo:53 · 10.255.255.254:53 · *:80 · *:8222 · *:1433 · *:4222 · *:8080
```

No ralphy listener; the non-loopback listeners above belong to unrelated
services in the distro and are recorded as the Phase 0 baseline so the bind
check reads against a known starting point.

### Bring-up, exactly as docs/daemon.md prescribes

`loginctl enable-linger corcino` → `Linger=yes`. `ralphy daemon install`, then
the documented one-time edit appending
`--peer-store /mnt/c/Users/PICHAU/.ralphy` to the unit's `ExecStart`,
`daemon-reload`, `systemctl --user enable --now`.

**The daemon refused to announce, legibly**, on first start:

```
WARN ralphy_daemon: --peer-store was given but this daemon is un-baptized —
run `ralphy daemon setup` to mint its identity, then restart; announcing nothing
```

This is the right behaviour and a good message, but it is a **step missing from
docs/daemon.md**: the documented sequence is install → edit → enable, and
`daemon setup` appears nowhere in it. A reader following the doc gets a running
daemon that announces nothing and no reason to look in the journal.

`ralphy daemon setup` is interactive-only (no flags). Driven with piped answers
it minted `daemon_id = 01KYVG62RH1SBHXQX23J1DTHB4`, name `wsl-ubuntu`, avatar
🦊 — and then **aborted with `Error: unexpected end of input during baptism`**
at the login-password prompt, *after* having already written `daemon.toml`,
`daemon-token` and `daemon-totp`. The identity was usable, so this was not
blocking, but a baptism that fails part-way leaves partial state on disk.

### The descriptor (AC3)

`%USERPROFILE%\.ralphy\peers\01KYVG62RH1SBHXQX23J1DTHB4.toml`, after restart:

```toml
daemon_id = "01KYVG62RH1SBHXQX23J1DTHB4"
name = "wsl-ubuntu"
avatar = "🦊"
address = "127.0.0.1"
port = 7257
environment = "Linux"
token = "<redacted>"
protocol_version = 3
```

- **Bind check, distro side:** `ss -ltnp` → `LISTEN 127.0.0.1:7257
  users:(("ralphy",pid=54674))`. Loopback only.
- **`/mnt/c` is writable from the distro** — confirmed, as §3 measured.
- **`chmod 600` is silently ignored** — `chmod 600` on the descriptor, then
  `stat -c %a` → **777**. ADR-0052 §3 confirmed verbatim.

### Finding — the descriptor loses the distro under systemd (design, not host)

Two fields are wrong above, from one cause. `environment` reads `"Linux"`
rather than `"WSL: Ubuntu-22.04"`, and the optional `nudge` block is **absent
entirely**.

[`announced_descriptor`](../../crates/ralphy-daemon/src/lib.rs#L220) derives
both from `wsl_distro`, which
[`detect_environment`](../../crates/ralphy-daemon/src/peer.rs#L204) reads from
the process's `WSL_DISTRO_NAME`. Measured on this host:

| Context | `WSL_DISTRO_NAME` |
|---|---|
| interactive `bash -lc` | `Ubuntu-22.04` |
| `systemctl --user show-environment` | **absent** |
| the running daemon's `/proc/<pid>/environ` | **absent** |

WSL's interop variables are injected into a login session, and the
`systemd --user` manager does not inherit them. So the *documented* bring-up —
a systemd user unit, which is the only form docs/daemon.md describes — is
exactly the one that cannot see the variable. Started by hand from a shell it
would work; started the way the docs say, it does not.

The consequence is not cosmetic, because `nudge` is what carries the distro
name across:

- **AC7's nudge cannot fire at all.** `nudge_argv` needs `spec.distro`, and the
  descriptor carries no `NudgeSpec`. §4's revival path is unreachable for a
  peer installed as documented.
- **The peer free console** (`wsl.exe -d <distro> --cd <path>`, the one locally
  hosted exception in the 2026-07-29 amendment) has no distro to name.
- The workbench groups the peer under a generic `Linux` label instead of the
  distro.

The unit test at `lib.rs:4577` asserts the WSL branch, but it feeds
`wsl_distro` directly — it proves `environment_label`, not the detection that
turns out to be the fragile half.

### Fix and re-verification

`ralphy daemon install` now captures `WSL_DISTRO_NAME` into the unit
(`Environment="WSL_DISTRO_NAME=Ubuntu-22.04"`), because the operator's shell is
the only context that has it. Reinstalled and restarted, the same daemon
announces:

```toml
environment = "WSL: Ubuntu-22.04"

[nudge]
distro = "Ubuntu-22.04"
unit = "ralphy-daemon.service"
```

`docs/daemon.md` gained the missing `ralphy daemon setup` step and a note on
why `install` must be run from the operator's own shell.

### Finding — both daemons default to the same port, and WSL's relay is why that bites (design)

With the WSL peer up, the Windows daemon **would not start**:

```
Error: binding the daemon listener on 127.0.0.1:7257
Caused by: os error 10048  (address already in use)
```

`127.0.0.1:7257` on the Windows side was held by **`wslrelay.exe`**. That is
`localhostForwarding` working exactly as ADR-0052 §2 requires — and §2's own
mechanism is what makes the shared `DEFAULT_PORT` unusable: the peer's listener
occupies the local daemon's default port on the very interface it binds.

Reversing the start order is worse, because it fails **silently**:

| Start order | Result |
|---|---|
| WSL first, Windows second | Windows daemon exits with `os error 10048` — loud, though the message names the address and not the cause |
| Windows first, WSL second | **Both run and both look healthy.** Windows owns `127.0.0.1:7257`; the relay loses; the local daemon dials `127.0.0.1:7257` to reach the peer and **reaches itself** |

In that second case the fleet view reported:

```
"state": "unauthorized",
"diagnosis": "peer WSL: Ubuntu-22.04 refused the credential — its token was
rotated; restart that daemon with --peer-store to re-announce"
```

The peer's token was never rotated. The local daemon presented the peer's token
to *its own* auth, which correctly rejected it, and the diagnosis then sent the
operator to fix a credential that was fine. Same shape as #292, where a missing
`git` misreported as `NoGithubRemote`.

Recorded as a design finding, not a host one: ADR-0032 §4 provides no `--port`
passthrough in autostart, so the operator has no documented way out on the
Windows side.

**Fixed both ways.** A self-dial gate refuses a descriptor announcing loopback
on the port this daemon bound, before a socket is opened — the check has to be
on the target rather than the answer, because auth rejects before the handshake
serves any body, so inspecting the returned `daemon_id` would have arrived too
late (an identity echo check is kept as a second line for a loop arriving some
other way). And `docs/daemon.md` now makes `--port` a required part of the WSL
peer bring-up, since the guard makes the failure legible but only a distinct
port makes the two federate.

Re-verified live by recreating the collision with the guard in place — Windows
daemon first on 7257, peer second on 7257:

```
"state": "refused",
"diagnosis": "peer WSL: Ubuntu-22.04 was not dialled: it announces
127.0.0.1:7257, the port this daemon is bound to — a connection there arrives
back here, so the two cannot federate; give one of them a distinct `--port`"
```

Moving the peer back to `--port 7357` restores `"state": "reachable"` and both
repo rows, with no restart of the Windows daemon.

With distinct ports, the handshake succeeds:

```
"state": "reachable", "diagnosis": "peer WSL: Ubuntu-22.04 answered the handshake",
"nudgeable": true
```

## Phase 1 — `init` inside the distro (AC4, AC5)

`ralphy init` run in `~/FinCal-353`, inside the distro. **The gate reported the
environment truthfully and blocked:**

```
Error: `gh label list --json name,color` failed: unknown command "label" for "gh"
```
exit 1

The distro's `gh` is the Ubuntu 22.04 archive package, **2.4.0+dfsg1 (2022)**,
which predates `gh label` entirely. The Windows host runs 2.89.0. The gate is
doing exactly its job: this environment is authenticated but **not usable** for
ralphy's flow, and only a run inside it could have found that. Recorded as a
**host/environment issue** — the fix is `gh` in the distro, not code.

**Register-before-gate confirmed (AC5).** Despite exit 1, `~/.ralphy/repos.toml`
in the distro carries:

```toml
[repos."Dev/FinCal"]
path = "/home/corcino/FinCal-273"

[repos."paulocorcino/FinCal"]
path = "/home/corcino/FinCal-353"
```

The repo is registered even though the gate blocked — the ADR's *Consequences*
claim, live. Phase 2 must show the workbench telling that story.

Two incidental confirmations in that same file:

- **`Dev/FinCal` is a stale entry** whose path no longer exists — a ready-made
  unreachable repo for the Phase 2 display check.
- Its slug shape is the **split identity** §1 warns about: the same project
  registered once as `Dev/FinCal` and once as `paulocorcino/FinCal`.

**The §5 collision is live again.** `paulocorcino/FinCal` is now registered on
*both* daemons — `C:/Dev/FinCal` on Windows, `/home/corcino/FinCal-353` in the
distro. The condition the composite `(daemon_id, slug)` key exists for is
reproduced, and a naive merge would discard one.

With the current `gh` installed (2.89.0, into `~/.local/bin`), `init` re-run in
the same repo completes: labels reconciled against GitHub, exit 0. The gate's
verdict flipped for the reason the gate exists.

### Finding — the federated list discarded the peer's own verdict (design)

The peer serves the truth on `/api/repos`:

```json
[{"slug":"Dev/FinCal","path":"/home/corcino/FinCal-273","reachable":false,"branch":null,…},
 {"slug":"paulocorcino/FinCal","path":"/home/corcino/FinCal-353","reachable":true,"branch":"ralphy/init",…}]
```

`/api/fleet` folded that to:

| key | branch | reachable |
|---|---|---|
| `…/Dev/FinCal` | *(empty)* | **true** |
| `…/paulocorcino/FinCal` | *(empty)* | true |

`store_from_repos_json` kept only `slug -> path`; `aggregate` then hardcoded
`branch: None` and set `reachable` from whether the **peer daemon** answered.
So a repo whose directory had been deleted read as healthy — the AC5 failure
mode exactly, on a repo whose problem the owning daemon had already diagnosed.

Notably the code contradicted its own documentation: `FederatedRepo::reachable`
is documented as "for a peer row it is the peer's own answer", and the sibling
`repo_from_repos_json` retains it under the comment "the local daemon must not
stat a peer path". Fixed by carrying the owner's verdict through in a
`PeerRepoRow`, with a row reachable only when the peer is live **and** the peer
says its path is. Re-verified live:

| key | branch | reachable | peer_state |
|---|---|---|---|
| `…/Dev/FinCal` | | **false** | reachable |
| `…/paulocorcino/FinCal` | `ralphy/init` | true | reachable |

The two verdicts are now distinct: the peer is up, and one of its repos is gone.

No test caught this because the fleet test's `/api/repos` stub omitted
`reachable` altogether; it now serves the shape a real peer serves.

## Phase 3 — sync and push on a peer repo (AC1, AC2)

Driven through the **browser's own path**: a binary frame on `/ws/command`
(tag `0x02` + the `protocol::Command` JSON), with a composite repo ref
`01KYVG62RH1SBHXQX23J1DTHB4/paulocorcino/FinCal`. The sync verbs are
`EffectClass::Mutate`, so the local daemon proxies them to the peer's
`/api/peer/command` and the work executes inside the distro.

Peer repo: `~/FinCal-353` on branch `capstone/fleet-353`.

### Sync and push (AC1)

| Verb | Result |
|---|---|
| `sync.status` | `{"head":{"kind":"branch","name":"capstone/fleet-353"},…}` — the peer's real branch |
| `sync.fetch` | `{"status":"ok"}` |
| `sync.push` | `{"status":"ok"}` |

**The push is real.** `git ls-remote --heads origin capstone/fleet-353` on the
peer answers `23df5954… refs/heads/capstone/fleet-353`: a commit authored inside
WSL, published to `github.com/paulocorcino/FinCal` from the Windows workbench,
through the proxy, using the distro's own credentials. Upstream tracking was
created by that push.

### Fast-forward-only pull, through the proxy (AC1)

Divergence arranged deliberately — one commit pushed to the branch from a second
clone, one commit made in the peer repo — then, after `sync.fetch`:

```json
"tracking":{"ahead":1,"behind":1,"upstream":"origin/capstone/fleet-353"}
```

`sync.pull`:

```json
{"status":"error","message":"Error: cannot fast-forward: the branch and its
upstream have diverged (1 ahead, 1 behind) — merge or rebase in a terminal"}
```

The refusal is the core's own prose, unchanged by the proxy hop.

### The human gate on push — what it actually is

Worth stating precisely rather than claiming a gate fired. There is **no
daemon-side confirmation** for push: per
[`app.js`](../../crates/ralphy-daemon/assets/ui/app.js#L816) the consent model is
"the OPERATOR's own click is the whole consent — there is no opt-in flag on this
path (ADR-0046 amendment)". So the gate lives in the workbench, and an
authenticated client acting for the operator (this capstone's script, or the
operator's own browser) pushes on the same terms.

Federation therefore neither adds nor removes anything on this path: the proxied
command carries no extra flag, the peer composes the same argv, and every refusal
the core models still arrives as `{status:"error"}` with the core's message.
That is the property AC1 asks for — *unchanged* — and it holds for the reason
that there is nothing federation could have changed.

### `verify.command` is still denied through the proxy (AC2)

Tried both ways, plus a neighbouring key each time so the refusal is proven to
be about the key and not the surface:

| Path | Key | Result |
|---|---|---|
| `/ws/command`, composite ref | `verify.command` | `{"status":"error","message":"invalid mutation options"}` |
| `/ws/command`, composite ref | `queue.assignee` | `{"status":"ok"}` |
| direct `POST /api/peer/command` | `verify.command` | `{"status":"error","message":"invalid mutation options"}` |
| direct `POST /api/peer/command` | `queue.assignee` | `{"status":"ok"}` |

And the peer's own `.ralphy/settings.json` afterwards:

```json
{ "verify": {}, "queue": { "assignee": "paulocorcino" } }
```

`verify` is empty: the denial is not merely a refused reply, nothing was written.
The benign key landed on both paths, so the surface works and the key is what is
refused. Federation did not widen the remote boundary, and did not open a second
route around it.

## Phase 4 — a real run, and a session that drops and comes back (AC6)

### The run executes natively in the distro

`run` is `EffectClass::Spawn`, so the local daemon proxies it over the peer's
authenticated `/ws/command` and relays the frames. Dispatched from Windows with
`{agent: "claude", branchMode: "new"}` against the composite ref:

```json
{"verb":"run","payload":{"pid":57402,"status":"spawned"}}
{"chunk":"FinCal-353 · capstone/fleet-353 · https://github.com/paulocorcino/FinCal\n"}
{"chunk":"[queue] queue built: 0 issue(s)\n"}
{"chunk":"No open issues for labels [ready-for-agent, AFK] assigned to paulocorcino…\n"}
{"verb":"run","payload":{"code":0,"status":"exited"}}
```

End to end, `spawned` through `exited`, with the peer's own repo header. The
empty queue was **this capstone's own doing**: the `queue.assignee` set during
the Phase 3 AC2 test was still in force, and the issues are unassigned — which
is incidental proof that the proxied `config.set` really took effect. Removing
it through the same proxy (`config.unset`, `{"status":"ok"}`, and the peer's
`settings.json` back to `{"verify":{},"queue":{}}`) and re-dispatching:

```json
{"verb":"run","payload":{"pid":57447,"status":"spawned"}}
{"chunk":"[queue] queue built: 5 issue(s)\n"}
{"chunk":"[plan] #108 Transferência: par vinculado neutro entre Contas — planning\n"}
```

Confirmed native, from the distro's own process table:

```
57447 56840  ralphy run --if-idle --agent claude --branch-mode new
57985 57447  /home/corcino/.local/bin/claude --model opus -p --dangerously-skip-permissions …
              --settings /home/corcino/FinCal-353/.ralphy/runs/20260731-051652/ralphy.settings.json
```

PPID `56840` is the peer daemon. The child is a **Linux** `claude` at
`~/.local/bin`, working in a Linux cwd, writing run artifacts to a Linux path —
no `wsl.exe` anywhere, which is what §7 requires. The run cut its own branch,
`afk/run-20260731-051652`.

**The run survived the command socket being dropped.** The dispatching client
hung up mid-plan and the run kept going, which is the same detach-only property
the session amendment states — the peer owns the child, not the proxy.

### The session opens, drops and reattaches

Opening `/ws/session?repo=<composite>&agent=claude` — the first frame, before
any scrollback, is exactly what the 2026-07-29 amendment specifies:

```json
{"verb":"session-open","payload":{"daemon_id":"01KYVG62RH1SBHXQX23J1DTHB4",
 "environment":"WSL: Ubuntu-22.04","session":1}}
```

followed by 869 bytes of terminal output showing Claude Code's trust prompt for
`/home/corcino/FinCal-353` — the distro's path, in the distro's Claude.

The browser socket was then **aborted without a close command**. The peer-owned
child survived (`57918`, parented by the daemon, not by the proxy). Reattaching
with `?repo=<composite>&id=1` replayed the **same 869 bytes** and re-announced
the same ownership, so the ring buffer outlived the proxy exactly as the
amendment promises. The owning daemon's numeric id stayed `1`, paired with the
composite ref.

`GET /api/sessions` federates the row with both identities intact:

```json
[{"id":1,"repo":"01KYVG62RH1SBHXQX23J1DTHB4/paulocorcino/FinCal","agent":"claude",
  "kind":"agent","daemon_id":"01KYVG62RH1SBHXQX23J1DTHB4","environment":"WSL: Ubuntu-22.04"}]
```

## Phase 5 — the peer dies the way WSL kills it (AC7)

### Idle termination is real on this host, and it happened on its own

`vmIdleTimeout=30000` — 30 seconds. This was not simulated: after the nudge
below revived the peer, it **idle-terminated by itself** between two API calls,
with nothing terminating it. The peer flipping back to `unreachable` without any
action is the genuine event the AC asks about.

`wsl --terminate Ubuntu-22.04` was used as the deterministic trigger for the
scripted checks. Note that `wsl.exe` in any form *restarts* the distro, so a
probe of the down state must not go through it — the first reading after a
terminate was still `reachable` because the shutdown had not finished.

### A descriptor outlives its listener, and degrades as designed

With `wsl --list --running` showing only `docker-desktop`, and nothing holding
port 7357:

```
descriptor still on disk: True
```

```json
"state": "unreachable",
"nudgeable": true,
"diagnosis": "peer WSL: Ubuntu-22.04 did not answer (connecting to
 127.0.0.1:7357 timed out: deadline has elapsed) — start it, or nudge it if it
 is a WSL distro"
```

| key | reachable | peer_state |
|---|---|---|
| `…/Dev/FinCal` | false | unreachable |
| `…/paulocorcino/FinCal` | false | unreachable |

Everything §4 promises: the file is a claim, the listener is the fact; the peer
is **marked, not removed**; its repos **stay listed**; the diagnosis names the
environment, the address and the remedy. A later reading, served from the
last-known cache, still carried the branch the peer had reported
(`afk/run-20260731-051652`) while correctly marking the row unreachable — the
two verdicts stay distinct.

`nudgeable: true` is only true because of the distro-pin fix earlier in this
capstone. Without it the descriptor carries no `NudgeSpec` and this AC has no
path at all.

### The nudge revives it, and systemd owns what comes back

```
POST /api/fleet/nudge?daemon_id=01KYVG62RH1SBHXQX23J1DTHB4  →  {"nudged":true}
```

The peer returned to `reachable` with both repos listed. Inside the distro:

```
daemon pid=270 ppid=260
/lib/systemd/systemd --user
```

The revived daemon's parent is the distro's own `systemd --user`, **not**
`wsl.exe`. Supervision stayed inside the distro, which is the distinction that
keeps §4 on the right side of ADR-0032: a wake nudge, never a Windows parent.

### Finding — on this host `vmIdleTimeout` dominates, not lingering (design)

ADR-0052 §4 frames the two lifecycle risks together: idle termination and
`systemd --user` lingering "decide whether a peer daemon survives at all". On
this host they are not comparable, and the ADR's emphasis is on the wrong one.

The first reading looked like the lingering story: with `disable-linger` and a
nudge, the peer was `reachable` at t+5 s and gone by t+10 s. But the same poll
**with lingering enabled** died just as fast:

```
t+8s  linger=yes: reachable
t+16s linger=yes: reachable
t+24s linger=yes: reachable
t+32s linger=yes: unreachable   ← and stays unreachable
```

Correlating peer state against whether the distro itself is running settles it.
Fourteen samples at 8 s, with lingering **on** throughout, and the two tracking
each other perfectly — the recovery at t+40 s is a later command of this
capstone restarting the distro, not the daemon recovering on its own:

```
t+8s    distro=up    peer=reachable
t+16s   distro=down  peer=unreachable
t+24s   distro=down  peer=unreachable
t+32s   distro=down  peer=unreachable
t+40s   distro=up    peer=reachable      ← distro restarted by an unrelated command
t+48s … t+112s  distro=up  peer=reachable
```

Peer state never diverges from distro state in any sample.

**The daemon is not dying — the distro is.** With `vmIdleTimeout=30000`, WSL
terminates `Ubuntu-22.04` about thirty seconds after the last activity, and the
peer goes with it. Lingering governs whether the daemon survives the end of a
*session inside a running distro*; it has no bearing on whether the distro
exists. Attributing the earlier flicker to lingering would have been wrong, and
is corrected here.

The consequence for the design is real, not cosmetic. On a host configured this
way the federated workbench is `unreachable` **most of the time**: every visit
after half a minute of quiet costs a nudge and a cold distro start, and each
`/api/fleet` pays the 2 s per-peer probe timeout first. The §4 machinery all
works — marked-not-removed, repos retained, nudge revives — but the steady state
it produces on this host is a peer that is usually down.

That is worth naming in the ADR and in docs/daemon.md next to
`loginctl enable-linger`, which currently reads as *the* prerequisite: a short
`vmIdleTimeout` is the more aggressive of the two, and neither the daemon nor
the nudge can see or change it.

### Scoped limitation — the lingering claim could not be isolated on this host

A third attempt held the distro up with a long-running `wsl.exe -e sleep`, so
`vmIdleTimeout` could not fire, then disabled lingering and nudged. The daemon
**stayed up** throughout:

```
t+6s   distro=up  peer=reachable
t+12s  distro=up  peer=reachable
t+18s  distro=up  peer=reachable
```

That is not evidence that lingering is unnecessary — it is the experiment
confounding itself. The holder process *is* a session, and a live session keeps
the user manager alive whether or not lingering is on; keeping the distro up and
keeping a session open are the same act from outside.

So on a host with `vmIdleTimeout=30000` the two variables cannot be separated
from the Windows side: too short a window and the distro dies before lingering
matters, and any device that widens the window supplies what lingering would
have. Isolating it needs the host's `.wslconfig` changed — raising or removing
`vmIdleTimeout` — which is a host reconfiguration outside this capstone's remit.
Recorded as a scoped limitation for a maintainer ruling, following the #272
pattern for a claim that cannot be reached with the environment as configured.

What *is* established: §4's mechanism is correct end to end (stale descriptor,
marked-not-removed, repos retained, nudge revives, systemd owns what comes back),
and the dominant lifecycle risk on this host is the distro's own idle
termination.

## Phase 2 — vendor availability (AC5, AC8) — evidence captured early

> **Corrected after measuring properly.** An earlier revision of this note read
> `command -v` in a login shell and recorded `codex`, `copilot` and `kimi` as
> absent and `gemini` as a `/mnt/c` refusal. That was wrong, and wrong in the way
> this repo already knows `which` to be unreliable: a negative proves nothing.
> Three of those CLIs are installed natively under nvm's global bin, which is off
> `PATH` in a non-login shell. The daemon's own answer was right and my probe was
> not. What follows is the corrected reading.

**Routing is correct.** `GET /api/agents?repo=<daemon_id>/<slug>` returns
*exactly* the peer's own `/api/agents` body, byte for byte, and both differ from
the Windows daemon's — which reports all seven available. So §6's
"each daemon computes its own" holds.

Vendor CLI resolution inside `Ubuntu-22.04`, as the daemon resolves it:

| CLI | Resolves to | `available` | Correct? |
|---|---|---|---|
| `claude` | `~/.local/bin/claude` | true | yes — runs in a bare env (`2.1.154`) |
| `cursor-agent` | `~/.local/bin/cursor-agent` | true | yes — runs in a bare env (`2026.07.20`) |
| `codex` | `~/.nvm/versions/node/v24.13.0/bin/codex` | true | present, **but see below** |
| `copilot` | `~/.nvm/…/bin/copilot` | true | present, **but see below** |
| `gemini` | `~/.nvm/…/bin/gemini` | true | present, **but see below** — the `/mnt/c` hit is correctly skipped and the native one found instead |
| `opencode` | only `/mnt/c/Users/PICHAU/AppData/Roaming/npm/opencode` exists | false, `"not installed here"` | yes — the interop path is refused (ADR-0043 D16) and there is no native install |
| `kimi` | not found | false, `"not installed here"` | the binary **does** exist at `~/.kimi-code/bin/kimi`, but that vendor-specific directory is not a known fallback |

The ADR's own example survives intact: `opencode` runs interactively from the
distro while resolving to `/mnt/c/…`, and the daemon refuses it — the
presence-vs-usability split §6 exists to surface.

### Finding — the roster reports three CLIs available that cannot execute (design)

`codex`, `copilot` and `gemini` are found through the **nvm fallback**
(ADR-0043 D16), which exists precisely because a version-managed Node install
keeps its global bin off `PATH`. The programs are really there. But each is a
symlink into `node_modules` whose shebang is `#!/usr/bin/env node`, and `node`
is off `PATH` too — so from the daemon's environment they die before doing
anything:

```
/usr/bin/env: ‘node’: No such file or directory
```

Put nvm's bin on `PATH` and all three run: `codex-cli 0.116.0`,
`GitHub Copilot CLI 1.0.73`, `gemini 0.52.0`. The interpreter is **a sibling in
the same directory** as the resolved program.

This inverts the divergence §6 was written for. The ADR's case is "the operator
sees installed and the daemon says not found" — a *false negative*, harmless
because the operator can see the CLI working. This is a **false positive**: the
daemon offers an agent as launchable and the spawn fails on an interpreter the
locator never checked for. §6 keeps availability a signal rather than a gate, so
nothing is *blocked* wrongly — but the signal now conceals a divergence instead
of surfacing it, which is the opposite of its stated purpose.

**Fixed.** The child now gets the directory its program was resolved from on its
`PATH` — exactly one directory, the one ralphy itself chose, granting the child
no reach the parent did not already exercise in picking that program. A `PATH`
the caller already scrubbed stays the base, so a vendor's own containment still
wins. Applied at the two shared spawn seams rather than in each vendor crate:
`drive_headless`, beside the existing `own_process_group`/`no_window` mutations,
and `Session::spawn` for PTY sessions.

Re-verified live against the peer, on the two agents that could not start
before:

- **codex** now runs, and fails on *its own configuration* instead of on the
  interpreter — `unknown variant 'default', expected 'fast' or 'flex' in
  service_tier`, a pre-existing `~/.codex/config.toml` problem in that distro
  and a host issue, not ralphy's. The error moving from
  `env: 'node': No such file or directory` to the vendor's own config parser is
  the proof the interpreter was found.
- **copilot** opens fully: `Copilot v1.0.73`, TUI drawn, trust prompt naming
  `/home/corcino/FinCal-353`.

Windows-side resolution, for contrast — all seven available: `claude.exe`,
`codex.exe`, `kimi.exe`, `opencode.ps1`, `gemini.ps1`, `copilot.EXE`,
`cursor-agent.ps1`.

## Phase 7.1 — credentials are per environment (grounding finding 1)

**Confirmed as a standing condition; the specific instance in the PRD has since
flipped.** At PRD drafting the Windows `gh` was unauthenticated while the WSL
one was fine. Today both authenticate as `paulocorcino`, from two entirely
separate credential stores — Windows keyring versus
`/home/corcino/.config/gh/hosts.yml` — and at two versions four years apart
(2.89.0 vs 2.4.0+dfsg1). The drift the consequence predicts is visible in the
*versions* even while both happen to be logged in. This is a host condition,
not a design correction.
