# Local fleet federation: live validation

Status: accepted — validated live, with five amendments to
[ADR-0052](0052-local-fleet-federation.md) recorded below.

Companion to ADR-0052, produced by the HITL capstone of issue **#353** on
2026-07-31. Slices #349–#352 were verified in CI with two daemons on loopback on
one OS, deliberately, so no CI job depends on WSL existing. This is the run
against the real topology: a native Linux daemon inside WSL2 Ubuntu-22.04,
federated with the Windows daemon, driving actual work and a real push to a real
forge.

Operational checklist: [353-fleet-capstone-runbook.md](../evidence/353-fleet-capstone-runbook.md).
Captured evidence: [353-fleet-capstone-live.md](../evidence/353-fleet-capstone-live.md).

**Findings are sorted, not merged.** Design corrections amend ADR-0052 and are
listed first. Host and environment issues are recorded as such — they are not
defects and they do not amend anything. One claim could not be reached with this
host as configured and is scoped rather than assumed.

## Verdict per phase

| Phase | AC | Verdict |
|---|---|---|
| 0 — topology, loopback-only | AC3 | **pass**, after two design fixes |
| 1 — `init` inside the distro | AC4, AC5 | **pass** — gate blocked truthfully, repo registered anyway |
| 2 — roster and availability | AC5, AC8 | **pass**, after one design fix; routing correct throughout |
| 3 — sync and push on a peer repo | AC1, AC2 | **pass** — real push, ff-only intact, `verify.command` denied on both paths |
| 4 — real run and session reattach | AC6 | **pass** — native child, detach-only survival, scrollback replayed |
| 5 — peer death and nudge | AC7 | **pass**, with the dominant cause corrected |
| 6 — usage federation | AC8 | **pass** — faithful fold, `missing` populated |
| wrap-up — this note | AC9 | **pass** |

No stop condition fired. Neither daemon ever bound a non-loopback address,
nothing crossed the filesystem boundary, no agent session was hosted by the wrong
daemon, no unreachable peer was auto-removed, and no short total was served
silently.

## Design corrections — amendments to ADR-0052

### A1. The descriptor loses its distro under systemd (§3, §4)

`announced_descriptor` derived both the environment label and the `NudgeSpec`
from `WSL_DISTRO_NAME`, which WSL injects into a **login session**. `systemd
--user` does not inherit it, and nothing inside a distro can name itself:
`/proc/sys/kernel/osrelease` proves only that this *is* WSL, `/mnt/wsl` names the
sibling distros rather than this one, and the hostname is the Windows machine's.
So the only bring-up docs/daemon.md describes was the one that could not see it.

The cost was not the label. Without the `NudgeSpec` a sleeping peer advertises no
way to wake it, so §4's revival path — and AC7 entirely — was unreachable, and a
peer free console had no distro to name.

**Amendment:** §3's descriptor is written by `ralphy daemon install`, which runs
in the operator's own shell — the one context that has the name — and pins it
into the unit. §4's nudge is therefore available only to a peer installed that
way, which docs/daemon.md now states. Fixed in `4d8c14a`.

### A2. Both daemons default to the same port, and §2's own relay is why that bites (§2)

`localhostForwarding`, the mechanism §2 depends on, publishes the distro's
listener on the *Windows* loopback at the same port. Both daemons default to
`DEFAULT_PORT`, so they collide. WSL-first fails loudly (`os error 10048`);
**Windows-first fails silently** — both appear healthy, the relay loses, and the
local daemon dials `127.0.0.1:<its own port>` and reaches itself. It then
presented the peer's token to its own auth, was correctly rejected, and reported
`its token was rotated`, sending the operator to fix a credential that was never
broken (the #292 shape).

Checking the `daemon_id` the handshake returns does not catch this: auth rejects
before serving any body. The check must be on the target, not the answer.

**Amendment:** §2 records that the relay both enables federation and occupies the
port. A descriptor announcing loopback on the port this daemon bound is refused
before a socket is opened, beside the existing non-loopback gate, and an identity
echo is kept as a second line. `--port` becomes a required part of the WSL peer
bring-up: the guard makes the failure legible, but only a distinct port makes the
two federate. Fixed in `982f6fd`.

### A3. The federated list discarded the peer's own verdict (§1, §5)

`store_from_repos_json` folded a peer's `/api/repos` answer down to
`slug -> path`; `aggregate` then hardcoded `branch: None` and derived `reachable`
from whether the **peer daemon** answered. A peer repo whose directory had been
deleted was listed as healthy — the AC5 failure mode, on a repo whose owning
daemon had already diagnosed it correctly.

The code contradicted its own documentation: `FederatedRepo::reachable` is
documented as "for a peer row it is the peer's own answer", and the sibling
`repo_from_repos_json` retains it under "the local daemon must not stat a peer
path". No test caught it because the fleet test's `/api/repos` stub omitted
`reachable` entirely.

**Amendment:** §1's "nothing crosses the filesystem boundary" implies its
converse — facts about a peer's filesystem may only come *from* that peer. A peer
row now carries the owner's verdict, and is reachable only when the peer is live
**and** the peer says its path is. Fixed in `db9774e`.

### A4. The roster offered agents that could not execute (§6)

§6 exists because "presence and usability diverge here in a way an operator
cannot see", and its example is a **false negative**: `opencode` runs
interactively from the distro while the daemon refuses its `/mnt/c` path. That
one is harmless — the operator can see the CLI working.

The live measurement found the inverse. `codex`, `copilot` and `gemini` are
installed natively under nvm's global bin and correctly reported available, but
each is a shim whose shebang is `#!/usr/bin/env node`, and `node` is a sibling in
that same off-`PATH` directory. All three died on
`env: 'node': No such file or directory`. The roster offered an agent that could
not start, which is the signal **concealing** the divergence it exists to
surface, and it breaks `locate_program`'s stated invariant that "detection and
execution can never disagree".

**Amendment:** §6's divergence is bidirectional, and the dangerous direction is
the false positive. A child is now given the one directory ralphy resolved its
program from, at the two shared spawn seams. Narrowing detection instead was
rejected: it would trade a false promise for a false denial and lose a capability
the environment really has. Fixed in `1de8588`.

### A5. `vmIdleTimeout` dominates lingering (§4)

§4 presents idle termination and `systemd --user` lingering as comparable risks
that together "decide whether a peer daemon survives at all". On this host they
are not comparable. With `vmIdleTimeout=30000`, WSL terminates the distro about
thirty seconds after the last activity and the daemon goes with it. Fourteen
samples with lingering **on** show peer state never diverging from distro state.

The consequence is the steady state, not an edge case: the federated workbench is
`unreachable` most of the time, every visit after half a minute of quiet costs a
nudge and a cold start, and each `/api/fleet` pays the per-peer probe timeout
first. This was felt repeatedly while running the later phases — a `502` from the
proxy and a `missing` entry in the usage fold both appeared unprompted.

**Amendment:** §4 names `vmIdleTimeout` as the more aggressive of the two and
notes that neither the daemon nor the nudge can see or change it. Lingering
remains a prerequisite; it is not the dominant one. The §4 machinery is correct,
the framing was not.

**Resolved without touching the host.** Raising `vmIdleTimeout` was rejected as a
remedy: it is the operator's own setting, and a keepalive from the local daemon
would both defeat it and collide with §4's "nudge, never supervise". What was
attacked instead is the *cost* of a cold start, in three parts:

1. `PeerStatus::Asleep` (`a2c4bc3`) — a stopped distro and a dead daemon were one
   `unreachable` with one prescription. `wsl.exe --list --running` separates them
   and starts nothing, which is what makes it usable as an observation; a host
   that cannot be asked degrades to the diagnosis that assumes nothing.
2. The nudge waits (`cd1c84a`) — `{"nudged":true}` meant "spawned `wsl.exe`",
   which is true a beat after the request and useless. It now polls the handshake
   and reports `ready`. Waiting is not supervising: nothing parents, holds or
   signals what it started.
3. The workbench wakes (`beed2af`) — `/api/fleet/nudge` had no caller at all.
   Opening a row on a sleeping peer wakes it, and the state chip is the explicit
   control. In the workbench the operator's own action is the whole consent
   (ADR-0046); in the daemon it would have been supervision by accident.

The reframing this rests on: with a wake that is quick and invisible, "does the
daemon survive the end of a session?" matters far less than "does it come back?"
— so the lingering prerequisite is reframed rather than attacked, and the scoped
limitation below stands as recorded.

## Confirmed as designed

- **§1 and §7 held throughout.** Every run child was native to the distro
  (PPID the peer daemon, Linux binary, Linux cwd, no `wsl.exe`), and no daemon
  read or wrote the other environment's repo.
- **§2's loopback property.** Both daemons bound `127.0.0.1` only, on every
  check, so `AuthPolicy::Localhost` was preserved and no firewall rule or LAN
  exposure was introduced.
- **§3's descriptor and its honesty about protection.** `/mnt/c` is writable
  from the distro, and `chmod 600` on the descriptor is **silently ignored** —
  `stat` reports `777`. The ADR says so; it is true.
- **§4's mechanism.** Stale descriptor, peer marked and not removed, repos
  retained, nudge revives, and what comes back is parented by the distro's own
  `systemd --user` rather than by `wsl.exe`.
- **§5's composite key.** `paulocorcino/FinCal` was registered on both daemons
  simultaneously and produced two distinct rows.
- **§6's routing.** `GET /api/agents?repo=<ref>` returned the peer's own body
  byte for byte, differing from the Windows daemon's.
- **§7's remote boundary.** `verify.command` was refused on the browser socket
  *and* on a direct `POST /api/peer/command`, while a neighbouring key set on
  both — and the peer's `settings.json` showed `"verify": {}` afterwards, so
  nothing was written. Federation neither widened the boundary nor opened a route
  around it.
- **The push path.** Sync and push behave as they do locally: a commit authored
  in WSL reached `github.com/paulocorcino/FinCal` from the Windows workbench
  through the proxy with the distro's own credentials, and `sync.pull` on a
  deliberately diverged branch refused with the core's own fast-forward prose.
- **The session amendment.** `session-open` carries the owning daemon and
  environment before any scrollback; aborting the browser socket without a close
  left the peer-owned child alive; reattach replayed the identical scrollback
  under the same numeric id paired with the composite ref.
- **Usage federation.** The fold adds exactly the peer's own rows, stamps each
  with its source, and names a peer it could not reach under `missing` rather
  than serving a short total.

## Host and environment issues — not defects

- **`gh` in the distro was 2.4.0+dfsg1 (Ubuntu 22.04 archive, 2022)**, predating
  `gh label`; `ralphy init` blocked on it, correctly and only because it was run
  inside that environment. Resolved by installing 2.89.0. This is the standing
  per-environment credential-and-toolchain drift §*Consequences* predicts, and
  the version gap is visible even when both sides happen to be logged in.
- **`~/.codex/config.toml` in the distro carries `service_tier = "default"`**,
  which that codex build rejects. Pre-existing; surfaced only once the A4 fix let
  codex get far enough to parse its own configuration.
- **`kimi` is installed at `~/.kimi-code/bin`**, a vendor-specific directory the
  program locator does not know, so the roster reports it absent while usage
  shows it was used. Both measurements are right about different things.
- **The grounding drift in §5.** `/home/corcino/FinCal-273`, cited in the ADR as
  the same-slug-both-sides case, no longer exists; the condition was recreated as
  `~/FinCal-353`. The decision it supports is unaffected.
- **`ralphy daemon setup` was missing from the bring-up docs**, and an
  un-baptized daemon starts, serves, and announces nothing — saying so only in
  the journal. Documentation gap, now closed.
- **`daemon setup` leaves partial state on EOF.** Driven non-interactively it
  wrote `daemon.toml`, `daemon-token` and `daemon-totp` and then failed with
  `unexpected end of input during baptism`. The identity was usable, so this did
  not block; recorded because a baptism that fails part-way still leaves files.

## Scoped limitation — the lingering claim could not be isolated

§4's lingering claim could not be tested independently on this host. With
`vmIdleTimeout` at 30 s the distro dies before lingering matters, and any device
that keeps the distro alive long enough to test it — a long-running `wsl.exe -e
sleep` — **is itself a session**, which supplies exactly what lingering would
have. Keeping the distro up and keeping a session open are the same act from
outside.

Isolating it requires changing the host's `.wslconfig` to raise or remove
`vmIdleTimeout`, a host reconfiguration outside this capstone's remit. Recorded
for a maintainer ruling, following the #272 pattern for a claim unreachable with
the environment as configured. What *is* established is that §4's machinery works
end to end and that idle termination, not lingering, is the dominant risk here.
