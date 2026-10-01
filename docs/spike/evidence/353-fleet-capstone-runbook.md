# Runbook — local fleet live capstone (#353)

A trail-to-completion for the HITL capstone of **local fleet federation**
([ADR-0052](../adr/0052-local-fleet-federation.md), issue **#353**, parent PRD
#348). Slices #349–#352 shipped and are verified in CI with **two daemons on
loopback on one OS**, deliberately, so no CI job depends on WSL existing. That
leaves the real topology unproven. This is the run a human signs off before the
feature is called done.

This file is the **operational checklist**, not the evidence. On completion the
captured commands, numbers and log lines move into
`docs/evidence/353-fleet-capstone-live.md` (H2 phases mirroring
`272-copilot-capstone-live.md`), raw per-command logs go to
`docs/live/fleet-353-<probe>.log`, and the verdict-per-phase plus a back-link
land in a new companion note beside the ADR,
`docs/adr/0052-local-fleet-validation.md`, following the repo's convention for
validation notes (`0041-copilot-validation.md`, `0043-gemini-validation.md`).

**Findings are sorted, not merged.** A finding that contradicts the design
amends ADR-0052. A finding that is a host or environment issue is recorded as
such — not as a defect, and not as an ADR amendment. The wrap-up must state
which bucket each finding landed in.

---

## Resolved environment (capture before Phase 0)

| Field | Value |
|-------|-------|
| Host OS | *(capture)* |
| WSL distro + version | *(capture — `wsl -l -v`)* |
| WSL networking mode | *(capture — NAT expected; mirrored mode invalidates §2)* |
| `vmIdleTimeout` in `.wslconfig` | *(capture — Phase 5 depends on it)* |
| `loginctl enable-linger` state | *(capture — `loginctl show-user $USER -p Linger`)* |
| Windows daemon | version, `daemon_id`, bound port |
| WSL daemon | version, `daemon_id`, bound port, `--peer-store` value |
| Peer protocol version | 3 on both (ADR-0052 amendment 2026-07-29) |
| Windows `gh auth status` | *(capture — expected to diverge from WSL)* |
| WSL `gh auth status` | *(capture)* |
| Windows vendor CLIs | *(capture — `ralphy` roster availability)* |
| WSL vendor CLIs | *(capture — including any resolving to `/mnt/c`)* |
| Peer repo under test | *(capture — path inside the distro, slug, base branch)* |
| Forge remote | *(capture — push targets a real forge; Phase 3 spends real credentials)* |

Setup is the documented one — [docs/daemon.md § *Local fleet: adding a WSL
peer*](../daemon.md#local-fleet-adding-a-wsl-peer). Do **not** invent a
different bring-up: if the documented path does not work, that is a finding.

---

## Why this is HITL, and what only a human can close

- **Push needs real credentials against a real forge.** The gate on push is
  human-in-the-loop by design; there is no unattended form of this step.
- **A real WSL distro cannot be a CI dependency.** Every measurement in
  ADR-0052's *Grounding* table was taken by hand on this host, and the ones this
  capstone re-tests (localhost forwarding, `/mnt/c` writability, `chmod 600`
  silently ignored, `wsl.exe -e true` warm cost) are host facts, not code facts.
- **Idle termination is a wall-clock wait**, not a mock. Phase 5 kills the peer
  the way the VM does, then watches the degradation the design promises.
- **Reconciling usage against the vendors' own dashboards is a judgement call**
  — the same one the adapter capstones made, now with two HOMEs.

---

## Phase 0 — the topology stands up, and neither daemon leaves loopback (AC3)

1. Bring up the WSL daemon per docs/daemon.md: `loginctl enable-linger`,
   `--peer-store /mnt/c/Users/<user>/.ralphy`, `systemctl --user enable --now`.
2. Confirm the descriptor landed: `<store>/peers/<daemon_id>.toml`, carrying
   `daemon_id`, `name`, `avatar`, `port`, environment label, token, protocol
   version. Re-confirm the ADR's §3 note in the flesh: `chmod 600` on it is
   **silently ignored** on `/mnt/c`.
3. **Bind check, both sides.** `netstat -ano` on Windows and `ss -ltnp` in the
   distro: each listener is on `127.0.0.1`, neither on `0.0.0.0` and neither on
   the NAT gateway address. This is the load-bearing property of §2 — if either
   daemon binds non-loopback, the capstone stops here.
4. The Windows workbench lists the peer's repos with no restart (the store is
   read fresh per request).

**Pass:** peer visible, both listeners loopback-only, descriptor shaped as §3
says. → `docs/live/fleet-353-bringup.log`

## Phase 1 — `init` inside the distro tells the truth (AC4, AC5)

`ralphy init` is the existing environment gate; this phase adds **no new code**
and validates the distro on its own terms — git, `gh` auth, the forge remote,
vendor CLI presence and vendor login.

1. Run `init` **inside the distro**, on the peer repo. Record the gate's verdict
   verbatim.
2. Then exercise the case the gate creates on purpose (ADR-0052 *Consequences*):
   **`init` registers the repo before it evaluates the gate**, so arrange a repo
   that fails the gate (an unauthenticated `gh`, or a vendor CLI absent from the
   distro) and confirm it is *registered anyway*.

**Pass:** the gate reports that environment truthfully, and a gate-failed repo
is present in the peer daemon's registry. → `docs/live/fleet-353-init.log`

## Phase 2 — the workbench states the problem instead of offering the impossible (AC5, AC8-partial)

1. The gate-failed repo from Phase 1 is **visible** in the Windows workbench,
   with its problem stated — not silently listed as healthy, not hidden.
2. `GET /api/agents?repo=<daemon_id>/<slug>` routes to the owning daemon and its
   rows carry `available` plus a `reason` computed by *that* daemon's program
   locator. No agent unavailable in the distro is offered as launchable.
3. Confirm the ADR's §6 divergence claim on this host: a vendor CLI that **runs
   interactively from the distro but resolves to `/mnt/c/…`** must report
   unavailable, because `locate_program` refuses a Windows binary for a Linux
   daemon (ADR-0043 D16). If no such CLI exists on this host any more, say so —
   that is a *correction to the grounding*, not a defect.
4. Availability is a **signal, not a gate** (§6, rejected alternatives): confirm
   the row is marked, and that the spawn-time "CLI not found" error remains the
   backstop.

**Pass:** the gate-failed repo tells its story; availability is per-daemon and
correct; nothing unavailable is launchable. → `docs/live/fleet-353-roster.log`

## Phase 3 — sync and push on a peer repo (AC1, AC2)

Sync verbs are `EffectClass::Mutate`, so they proxy through
`POST /api/peer/command` and execute on the **owning** daemon
([ADR-0052 §7](../adr/0052-local-fleet-federation.md)). Nothing about the
safety properties may differ from local.

1. `sync.status` and `sync.fetch` on the peer repo.
2. **Fast-forward-only pull.** Confirm `sync.pull` is still ff-only through the
   proxy: arrange a diverged branch and confirm it refuses rather than merging.
3. **Push, with the human gate.** Confirm the HITL gate on push is present and
   unchanged through the proxy, then complete a real push against the real
   forge. Record the commit that landed.
4. **The remote-boundary denial is unchanged (AC2).** `config.set` /
   `config.unset` of `verify.command` must be refused through the proxy exactly
   as locally. The denial lives in argv composition
   ([`dispatch.rs`](../../crates/ralphy-daemon/src/dispatch.rs) —
   `EXEC_ADJACENT_KEYS`, `remotely_settable_key`), so both hops compose the same
   argv; the point of the live check is that **federation did not open a second
   path around it**. Try it both ways: the browser-facing socket with a
   composite repo ref, and a direct authenticated `POST /api/peer/command`.
   Confirm a neighbouring, non-exec-adjacent key still sets, so the refusal is
   the key and not the surface.

**Pass:** sync/push behave exactly as local, push gate intact, ff-only intact,
`verify.command` denied on both paths, a benign key still settable. →
`docs/live/fleet-353-sync-push.log`

## Phase 4 — a real run, and a session that drops and comes back (AC6)

1. Dispatch a **real run** end to end on the WSL repo **from the Windows
   workbench**. `run` is `EffectClass::Spawn`, so it proxies over the peer's
   authenticated `/ws/command` and relays `spawned`/`output`/`error`/`exited`.
   The child must run **native inside the distro** — confirm its cwd and
   toolchain are the distro's, never `wsl.exe` from Windows.
2. Open an **agent session** on that repo. Per the amendment it is peer-owned:
   the attachment starts with a `session-open` carrying the owning daemon ID and
   environment, before scrollback.
3. **Drop and reattach.** Close the browser tab, confirm proxy teardown is
   **detach-only** — the peer-owned child and its ring buffer survive — then
   reattach and confirm scrollback is intact and the numeric session ID is still
   paired with the composite repo ref.
4. Sanity: a **free console** on the peer repo is the one locally hosted
   exception (`wsl.exe -d <distro> --cd <path>`) — confirm it lists with the
   composite repo and peer environment.

**Pass:** run completes natively in the distro, session opens, survives a drop,
reattaches with scrollback. → `docs/live/fleet-353-run.log`,
`docs/live/fleet-353-session.log`

## Phase 5 — the peer dies the way WSL kills it (AC7)

Do **not** simulate this with `systemctl stop`. The design's claim is about a
descriptor that **outlives its listener** — announce the port, lose the daemon.
Induce the real thing: idle-terminate the distro (`wsl --shutdown` is the
deterministic stand-in; if `vmIdleTimeout` is configured, prefer waiting it
out and record the wall-clock).

Then confirm, in order:

1. The descriptor is **stale**: the file is still there, the listener is gone.
2. The peer is marked **unreachable**, *not removed* — §4's "entries are never
   auto-deleted, only marked", the same way the registry reports an unreachable
   repo.
3. Its **repos stay listed** in the sidebar.
4. A **nudge** brings it back: `POST /api/fleet/nudge?daemon_id=<id>` fires
   `wsl.exe -d <distro> -e systemctl --user start ralphy-daemon.service`
   fire-and-forget. The daemon must not hold, parent or signal that process.
5. **The linger prerequisite is real.** With lingering *off*, confirm no nudge
   can fix it — §4 says so explicitly, and it is the difference between a
   documented prerequisite and an implementation detail.

**Pass:** stale descriptor degrades as designed, repos survive, nudge revives,
linger proven load-bearing. → `docs/live/fleet-353-lifecycle.log`

## Phase 6 — usage federates or it lies (AC8)

The usage resolvers read the HOME of the process that runs them, so an
unfederated aggregate silently omits everything spent in WSL — and ADR-0052 is
explicit that partial numbers presented as totals are worse than absent ones.

1. `GET /api/peer/usage` returns **one daemon's contribution only**.
2. Browser-facing `GET /api/usage` folds contributions concurrently, stamps the
   source daemon **without overwriting an emitted identity**, and lists
   `{daemon_id, environment, why}` under `missing` for any peer that fails or
   serves malformed data. Force that path: point at a dead peer and confirm the
   `missing` entry rather than a silently short total.
3. **Reconcile against reality.** The distro's figures must match what that
   environment actually has and spent — place the Phase 4 run's tokens next to
   the vendor's own session store inside the distro, and next to its dashboard.
   State any mismatch plainly (Ralphy's `$` is the ADR-0034 metered-API
   counterfactual, not a bill).

**Pass:** per-daemon contribution isolated, fold correct, `missing` populated on
failure, distro figures reconcile. → `docs/live/fleet-353-usage.log`

## Phase 7 — the three grounding findings, confirmed or corrected

Issue #353 names three findings from the design measurements that must be
**checked against a real run, not assumed**. Each gets an explicit verdict:
*confirmed*, *corrected* (→ amends ADR-0052), or *host/environment issue*.

1. **Credentials are per environment and expire independently.** Already
   surfaced while drafting the PRD — the Windows `gh` was unauthenticated while
   the WSL one was fine. Re-confirm with both `gh auth status` outputs side by
   side, and with each vendor's session store per HOME.
2. **Vendor parity is environmental, and presence diverges from usability.**
   The Phase 2 finding, stated as a grounding verdict.
3. **Distro lifecycle.** Idle termination and user-service lingering decide
   whether a peer daemon survives at all; a stale descriptor with a dead
   listener must degrade the way §4 says. The Phase 5 findings, stated as a
   grounding verdict.

## Wrap-up — the companion note (AC9)

Write `docs/adr/0052-local-fleet-validation.md`: one H2 per phase, a verdict
each, the captured strings inline, a back-link from ADR-0052, and — the part
the issue asks for by name — **design corrections separated from host and
environment issues**. Raw logs stay under `docs/live/`; the narrative evidence
goes to `docs/evidence/353-fleet-capstone-live.md`.

---

## What fails the whole exercise outright

Any one of these stops the capstone and is a defect, not a note:

1. **Either daemon binds a non-loopback address.** §2's entire security
   argument — `AuthPolicy::Localhost` preserved, no firewall rule, no LAN
   exposure — rests on this.
2. **`verify.command` is settable through the proxy**, on either path.
3. **The push gate or ff-only pull weakens through federation.** A proxied
   request is still a remote request.
4. **Anything crosses the filesystem boundary** — a daemon reading or writing
   the other environment's repo, or an agent session hosted by the wrong
   daemon. §1 and §7 are constraints, not preferences.
5. **An unreachable peer is auto-removed, or its repos vanish**, rather than
   being marked.
6. **`GET /api/usage` reports a short total silently** instead of naming the
   peer under `missing`.

## Ledger

| AC | Phase | Verdict |
|---|---|---|
| Sync and push on a peer repo, gate + ff-only unchanged | 3 | |
| `verify.command` still denied through the proxy | 3.4 | |
| Native Linux daemon federates over loopback, neither non-loopback | 0 | |
| `init` inside the distro reports truthfully | 1 | |
| Gate-failed repo visible with its problem; nothing impossible offered | 2 | |
| Real run end to end; session opened, dropped, reattached | 4 | |
| Peer death by idle termination degrades as designed; nudge revives | 5 | |
| Vendor availability and usage match the distro's reality | 2, 6 | |
| Validation companion note recorded beside ADR-0052 | wrap-up | |
