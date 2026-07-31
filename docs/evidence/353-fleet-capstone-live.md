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

## Phase 2 — vendor availability (AC5, AC8) — evidence captured early

The §6 divergence claim is **confirmed on this host**, and more strongly than
the ADR states it. Vendor CLI resolution inside `Ubuntu-22.04`:

| CLI | Resolves to | Reading |
|---|---|---|
| `claude` | `/home/corcino/.local/bin/claude` | native — genuinely available |
| `cursor-agent` | `/home/corcino/.local/bin/cursor-agent` | native — genuinely available |
| `opencode` | **`/mnt/c/Users/PICHAU/AppData/Roaming/npm/opencode`** | Windows binary via interop — `locate_program` must refuse it (ADR-0043 D16) |
| `gemini` | **`/mnt/c/Users/PICHAU/AppData/Roaming/npm/gemini`** | same |
| `kimi` | `command -v` says MISSING, but the binary **exists** at `/home/corcino/.kimi-code/bin/kimi` | off-PATH native install |
| `codex` | MISSING | genuinely absent |
| `copilot` | MISSING | genuinely absent |

Two things follow. First, the ADR's example is still live: `opencode` runs
interactively from the distro while resolving to `/mnt/c/…`, exactly the
presence-vs-usability split §6 exists to surface — and `gemini` does the same,
which the ADR did not name. Second, `npm root -g` inside the distro answers
`C:\Users\PICHAU\AppData\Roaming\npm\node_modules`: there is **no native
node/npm** here at all, so every npm-installed vendor CLI in this distro is a
Windows one. The unavailability is not incidental.

Windows-side resolution, for contrast — all seven present natively:
`claude.exe`, `codex.exe`, `kimi.exe`, `opencode.ps1`, `gemini.ps1`,
`copilot.EXE`, `cursor-agent.ps1`.

## Phase 7.1 — credentials are per environment (grounding finding 1)

**Confirmed as a standing condition; the specific instance in the PRD has since
flipped.** At PRD drafting the Windows `gh` was unauthenticated while the WSL
one was fine. Today both authenticate as `paulocorcino`, from two entirely
separate credential stores — Windows keyring versus
`/home/corcino/.config/gh/hosts.yml` — and at two versions four years apart
(2.89.0 vs 2.4.0+dfsg1). The drift the consequence predicts is visible in the
*versions* even while both happen to be logged in. This is a host condition,
not a design correction.
