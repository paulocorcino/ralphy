# Runbook — Gemini live-validation capstone (#265)

A trail-to-completion for the HITL capstone of the `ralphy-agent-gemini` vendor
(PRD #252, issue **#265**). All twelve discovery slices (#253–#264) are closed;
this is the live, end-to-end reconfirmation a human signs off before Gemini ships.

This file is the **operational checklist**, not the evidence. On completion the
captured numbers, commands and log lines move into
`docs/evidence/265-gemini-capstone-live.md` (H2 phases mirroring
`251-cursor-capstone-live.md`), raw per-command logs go to
`docs/live/gemini-265-<probe>.log`, and the verdict-per-phase plus a back-link
land in [docs/adr/0043-gemini-validation.md](../adr/0043-gemini-validation.md).
The decisions each phase exercises are D1–D18 in
[docs/adr/0043-gemini-adapter.md](../adr/0043-gemini-adapter.md).

Grounded on Gemini CLI **0.51.0** (the validation note records the #253
`fetch failed` blocker as **healed** — `gemini -p hello` now returns on the host).

---

## Why this is HITL, and what only a human can close

- It **spends real, metered API requests**. Each `auto` (unpinned-model) turn also
  spends a second, paid `utility_router` call (D8) — budget for ~2× the visible
  request count unless `--plan-model`/`--exec-model` are pinned.
- The **usage-vs-billing reconciliation (AC2)** is a judgement call: the store
  under-reports by 20–35 % (router tax, D9) and `output_tokens` under-counts
  thinking tokens up to 25× (D9 trap 1). The discrepancy must be *explained*, not
  noted.
- Two decisions were **left deliberately unobserved** in discovery and get a human
  ruling in **Phase 7 (AC6)**: true quota exhaustion (D11, `Limit(None)`
  provisional) and browser-OAuth isolation (D4, verified only for `gemini-api-key`).

## Operator inputs required before Phase 0

1. **Spend authorization + ceiling** — an explicit go, and a request/token budget
   you accept burning. *(Pending.)*
2. **Simulation repository** — **resolved:** `<SIM_REPO> = C:/Dev/FinCal`
   (`paulocorcino/FinCal`), `<BASE> = master` — the authorized lab used by the
   Cursor #251 capstone and every #253–#264 slice. Two pre-existing states the
   Phase 0 baseline must record as *pre-#265*, not run debris: `master` sits **one
   local commit ahead** of `origin/master` (`f15623d5 chore: opt out of Cursor
   codebase indexing (D6)` — the Cursor opt-out, harmless to Gemini), and a D4
   owned root **already exists** at `C:/Dev/FinCal/.ralphy/gemini-home/` from the
   discovery slices. AC5 therefore means restoring the **pre-#265** state, not a
   pristine `master`. Ralphy mints its own `afk/run-*` branch off `<BASE>`; no
   manual run branch is created.
3. **Auth mode** — `gemini-api-key` in the OS credential store (the D4 default),
   and whether the **browser-OAuth isolation path** is exercised now or ruled
   deferred in Phase 7. *(Pending.)*

Record the resolved values at the top of the evidence doc (host OS + build,
`gemini --version`, account/tier, `security.auth.selectedType`, ralphy binary path
+ branch + commit, date) — the environment block every `docs/evidence/*` file
opens with.

Standard invocation used throughout (Windows shown; WSL/Linux identical but
`./target/release/ralphy` and forward-slash paths):

```bash
./target/release/ralphy.exe run --repo <SIM_REPO> --issues <n> --agent gemini \
  --base-branch <BASE> [--plan-agent gemini] [--dry-run] --verbose
```

---

## Phase 0 — Baseline capture (feeds AC5)

Establish the pre-run state that AC5 must restore byte-for-byte.

- **Operator root manifest, BEFORE.** SHA-256 every file under the operator's own
  `~/.gemini` (the validation note took this over 9 264 files and got an empty
  diff). Ralphy must never write here (D4).
  - Windows: `Get-ChildItem -Recurse -File $env:USERPROFILE\.gemini | Get-FileHash -Algorithm SHA256 | Sort-Object Path | Format-Table -Auto | Out-File before-gemini-root.txt`
  - Linux/WSL: `find ~/.gemini -type f -print0 | xargs -0 sha256sum | sort > before-gemini-root.txt`
- **Sim repo state, BEFORE.** Record `git -C <SIM_REPO> rev-parse HEAD`, the branch,
  and `git -C <SIM_REPO> status --porcelain`. If the tree is dirty, stash with a
  tagged message (`git stash push -u -m ralphy-265`) as the kimi/cursor smokes did.
- **Owned-root check.** Confirm `<SIM_REPO>/.ralphy/gemini-home/` does **not** yet
  exist (or note its pre-state); it is gitignored and Ralphy-owned.

**Pass:** both manifests and the sim-repo HEAD/branch recorded; no surprise
pre-existing owned root.

## Phase 1 — Autonomy revocations against a real environment (AC3)

The three **hard-stop** revocations of `--approval-mode yolo` (D5), exercised
live rather than against `revocation.rs` fixtures. Restore each staged control
immediately after.

- **UntrustedWorkspace (exit 55).** Ralphy always passes `--skip-trust`, so the
  revocation is confirmed at the vendor layer: `gemini -p hello` in `<SIM_REPO>`
  **without** `--skip-trust` must exit 55 and print
  `Gemini CLI is not running in a trusted directory…`. Confirms the
  `revocation::NEEDLES` string still matches 0.51.0. Capture stderr →
  `docs/live/gemini-265-untrusted.log`.
- **AutonomyDisabled (exit 52).** Stage the admin system-settings file
  (`read_admin_tier` reads `system_dir()`: Windows `%ProgramData%\gemini-cli\settings.json`,
  Linux `/etc/gemini-cli/settings.json`) with
  `{"security":{"disableYoloMode":true}}` (or `{"admin":{"secureModeEnabled":true}}`).
  A `ralphy run --agent gemini` must **bail in `prepare_root`** with the
  `AutonomyDisabled` stop (not a bare exit-52 "malformed root"). **Delete the
  staged file afterward.**
- **Policy sovereignty (D5 / D15), in a real turn.** Confirm the `--policy` deny of
  `invoke_agent` removes the tool from the model's schema (the model reports it is
  *"not defined or available"*), not a call-time refusal. Optionally stage a
  user-tier `allow` rule in the owned root's `policies/` and confirm the argv `deny`
  still wins (the priority-900-vs-argv conflict of #253 step 19, which the
  validation note recorded as **not executed** on the dead host — this closes
  Probe C).

**Out of reach, feed to Phase 7 as residual:** admin-tier `deny` beating argv,
Enterprise Strict Mode, and **server-pushed** admin controls (`settings.admin`,
never on disk) — no managed host available. Note them; do not fake them.

**Pass:** 55 and 52 reproduced live with the real needle strings; `invoke_agent`
absent from schema in a real turn; residuals listed.

## Phase 2 — Real plan-then-execute to green (AC1)

The core gate. A real run against an issue **with no prior plan on this vendor**
(the validation note flags that `resume.rs::plan_is_finalized_for` keys resume on
the issue number, so re-probing a stale-plan issue skips the planning pass).

```bash
./target/release/ralphy.exe run --repo <SIM_REPO> --issues <n> --agent gemini \
  --base-branch <BASE> --verbose
```

Capture and assert:

- `.ralphy/plan.md` **written by the planner in yolo mode** (D12 — native plan mode
  rejected; Ralphy's charter writes the artifact), with feasibility verdict, ledger,
  `## Verify`, trailer.
- The executor consumes it, commits, the **verify gate passes**, and the issue
  **closes green** (`Done`, terminal `result.status:"success"`).
- Session id minted via `--session-id` equals the stream's `init.session_id` (D9).
- Model attribution + pricing sane; if unpinned, confirm the `utility_router`
  second call is present in `stats.models` (D8 cost story).

Optionally run a **split** (`--plan-agent claude`) to confirm the plan artifact is
vendor-neutral (US 4). Capture the run summary line + plan.md →
`docs/live/gemini-265-execute.log`.

**Pass:** green close on a real issue, plan.md authored by Gemini, verify gate
green.

## Phase 3 — Usage vs billing, discrepancy explained (AC2 — HITL)

- **Envelope arithmetic (D9), all three traps** on the Phase 2 run:
  1. billable output = `total_tokens − input_tokens` (NOT `output_tokens`; the
     latter drops thinking tokens billed at output rate — 25× undercount seen);
  2. `input` = `input_tokens − cached` (cached already sits inside input);
  3. `stats.models` is a map — `Usage::fold_usage` heaviest-model attribution.
- **Ralphy's view:** `ralphy usage --project <SIM_REPO> --by phase` and `--by model`
  (`--format json` for the raw ledger). Interactive floor: start an ephemeral daemon
  (`RALPHY_DAEMON_DIR=<tmp>` to bypass `daemon-require-login`) and read
  `GET /api/usage` — every Gemini `interactive` record must carry
  `lower_bound: true` and render `≥ n (lower bound)` (D10). `RALPHY_GEMINI_DIR`
  overrides the scan store if needed.
- **Vendor's own billing view:** pull the request + token counts from Google's
  console (AI Studio / Cloud usage for the API key). Tabulate against the envelope
  sums (the cursor capstone's Phase-3 dashboard-vs-envelope table is the model).
- **Explain the gap, don't note it (HITL):** the store/interactive floor will read
  **20–35 % under** the envelope because the router call's tokens never hit disk;
  the reported billable output will exceed `output_tokens` by the thinking residual.
  State each number and its cause.

**Pass:** a table comparing envelope, `ralphy usage`, and the vendor console, with
every discrepancy attributed to a named D9/D8 mechanism.

## Phase 4 — Budget kill, clean process tree, Windows AND Linux (AC4)

Gemini is a **five-level tree** (`cmd`/shim → node → node self-relaunch (16 GB heap)
→ `pwsh`/shell tool → command); `child.kill()` on the direct child strands four
processes, so `kill_tree` is mandatory (D18).

- Force a mid-run kill with a small cap:
  `... --agent gemini --max-minutes-per-issue 2` (or `--idle-minutes`). Expect
  `outcome=Timeout`, `saw_envelope=false`, and — because usage rides the envelope —
  a **0-token** report for the killed run.
- **Survivor sweep, Windows:** before the kill, note the child pids
  (`Get-CimInstance Win32_Process | Where CommandLine -match gemini`); after, confirm
  `Get-Process gemini,node,pwsh -ErrorAction SilentlyContinue` shows no run
  descendants (kill path = `taskkill /F /T /PID`). Keep a long-running shell tool
  (e.g. a 120 s `ping`) alive at kill time and confirm it dies, as the D18 probe did.
- **Survivor sweep, Linux/WSL:** repeat; kill path = `kill -KILL -<pgid>` against the
  process group set up by `own_process_group`. Confirm `pgrep -f gemini` /
  `ps --ppid` finds no survivors.

**Pass:** no survivors on **either** platform; killed run reports 0 tokens.

## Phase 5 — Cross-platform parity (WSL), reconfirms US 1–74 (AC4 cont.)

Repeat Phase 2 on a native WSL/Linux ralphy build
(`CARGO_TARGET_DIR=~/ralphy-target-wsl`), same Gemini 0.51.0. Specifically exercise
**D16 binary resolution**: `locate_program` must reject the `/mnt/c` Windows shim
(which dies `exec: node: not found`, exit 127) and resolve the real
`~/.nvm/versions/node/<ver>/bin/gemini`. Diff the mechanics (init record, envelope
arithmetic, pricing, skill discovery) against the Windows run; only genuine
feasibility differences are acceptable divergences (cf. cursor Phase 5).

**Pass:** byte-identical mechanics both platforms; `/mnt/c` shim rejected.

## Phase 6 — Restore & prove isolation (AC5)

- **Sim repo → exact pre-run state.** Return to `<BASE>` at the recorded HEAD;
  restore any Phase-0 stash. **Watch the branch-guard gotcha:** the validation note
  (Probe D) recorded that Ralphy's own guard refuses the agent's `git checkout` off
  the run branch, leaving a leftover `afk/run-*` branch with a finalized
  `.ralphy/plan.md`. Clean those up from the operator side and confirm
  `git status --porcelain` is empty and no stray run branches remain.
- **Operator root byte-identical.** Re-take the SHA-256 manifest and diff against
  Phase 0 — the diff must be **empty**. Only `<SIM_REPO>/.ralphy/gemini-home/`
  (installation_id, projects.json, settings.json, .project_root, session JSONL)
  should have changed; the operator's `~/.gemini` is untouched.

**Pass:** empty `git status`, no leftover run branches, empty manifest diff.

## Phase 7 — Human ruling on the two deferred decisions (AC6 — HITL)

Present to the maintainer for a recorded ruling — **observe-before-release** or
**stay deferred**:

- **True quota exhaustion (D11).** `Limit(None)` + ADR-0030 synthetic cadence is
  provisional; real exhaustion was never seen (costs a day's allowance) and the CLI
  absorbs transient 429s via `retryWithBackoff`. Ralphy adds no retry. Decide
  whether release requires forcing a real quota stop, or the provisional mapping
  ships.
- **Browser-OAuth isolation (D4).** Everything observed is under `gemini-api-key`,
  whose secret lives in the OS store. Under OAuth the credential is **file-based
  under the root**, and relocating `GEMINI_CLI_HOME` may orphan it. Decide whether an
  OAuth isolation run is a release gate or a documented open limit.
- Fold in Phase 1's admin-tier residuals (no managed host) if the human wants them
  in the same ruling.

**Pass:** a written ruling on each, recorded in the evidence doc and, if it changes
a decision, amended into `0043-gemini-validation.md`.

## Phase 8 — Capture in the repo (AC7)

- Write `docs/evidence/265-gemini-capstone-live.md` with the H2 phase structure of
  `251-cursor-capstone-live.md` (`## Phase 0 …` → `## Phase 7 …` +
  `## Follow-ups filed`), embedding the raw numbers/commands/log lines.
- Move per-command captures to `docs/live/gemini-265-<probe>.log`.
- Add the back-link line to `docs/adr/0043-gemini-validation.md` and update its
  verdict-per-phase (especially the note's open items: OAuth isolation, admin tier,
  quota exhaustion, and the usage-accounting gap now closed by #263).
- File follow-up issues for any bug found (the cursor capstone spun out
  #268–#271); list them under `## Follow-ups filed`.
- Any host residue (leftover branches, staged system-settings files, tmp probe
  dirs) removed — the [[evidence-discipline]] / host-residue rule.

**Pass:** evidence doc + raw logs committed on the current branch (no new branch,
no push, no PR unless explicitly asked); validation ADR back-linked; follow-ups
filed.

---

## Acceptance-criteria ledger (#265)

| AC | Criterion | Phase(s) | Kind |
|----|-----------|----------|------|
| 1 | Real plan-then-execute reaches green | 2 (parity 5) | mechanical |
| 2 | Reported usage vs vendor billing, discrepancy **explained** | 3 | HITL |
| 3 | Autonomy revocations against a real env, not fixtures | 1 | mechanical |
| 4 | Process tree clean after budget kill, Windows **and** Linux | 4, 5 | mechanical |
| 5 | Sim repo pre-run state + operator root byte-identical | 0, 6 | mechanical |
| 6 | Human rules on quota exhaustion + browser-auth (observe vs defer) | 7 | HITL |
| 7 | Live smoke captured the way other vendors' are | 8 | mechanical |

**Reconfirms US 75, plus end-to-end reconfirmation of US 1–74.**

## Guardrails carried from house rules

- Current branch only — **no new branch without authorization**; do not push or
  open a PR (CLAUDE.md).
- Every artifact in this trail is **English** (canonical written language).
- Screenshots are for browser-driven verification only, never terminal/CLI output.
