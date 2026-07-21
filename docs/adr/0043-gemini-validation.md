# Gemini adapter — live validation note

Companion to [ADR-0043](./0043-gemini-adapter.md), recording what the live probes
of issue #253 **actually observed** on 2026-07-21, and — as importantly — what
they could not.

Host: Windows 11 (10.0.26200), `gemini` 0.51.0 installed by npm at
`%APPDATA%\npm\gemini.CMD`, node 22.22.2. Operator auth mode
`security.auth.selectedType = "gemini-api-key"`, credential in the Windows
credential store (`LegacyGeneric:target=gemini-cli-api-key/default-api-key`).

## The blocker that shaped this note

**No live model call completed on this host.** Every invocation that reaches the
provider fails, reproducibly:

```
Attempt 1 failed. Retrying with backoff... Error: exception TypeError: fetch failed sending request
    at async Models.generateContentStream (…/@google/gemini-cli/bundle/chunk-DHQ53XVO.js:259310)
```

…followed by an unbounded retry loop (killed at 75 s and again at 120 s).

Four controls were run before concluding, and they place the fault inside the
vendor CLI rather than in this adapter or the host:

| Control | Result |
| --- | --- |
| Same command against the operator's own `~/.gemini`, no `GEMINI_CLI_HOME` at all | same failure — **not** caused by Ralphy's isolation |
| `-m gemini-3-flash` pinned instead of the routed `auto` | same failure |
| `curl https://generativelanguage.googleapis.com/v1beta/models` | HTTP 403 — the host reaches the API |
| `node -e "fetch('https://generativelanguage.googleapis.com/v1beta/models')"` | HTTP 403 — Node's own HTTP client reaches it too |

Everything below is what remained verifiable under that constraint. The probes
requiring a model **response** are recorded as not executed, not as passed.

## What the vendor's own surface confirmed

- `gemini --help` documents `-p, --prompt` as *"Run in non-interactive (headless)
  mode with the given prompt. **Appended to input on stdin (if any)**"* — stdin
  is delivered FIRST. `build_gemini_command` relies on this ordering, and the
  round-trip below proves it on the wire rather than from the help text.
- The stream-json record shape, observed live:
  `{"type":"init","timestamp":…,"session_id":…,"model":…}` then
  `{"type":"message","timestamp":…,"role":"user","content":"…"}`. The `init` and
  `user` records are emitted **before** the provider call, which is why the
  charter round-trip is verifiable on a host that cannot complete one.

## Probe A — the login probe (executed, passed)

`probe_gemini_login()` run through the real production path (a throwaway
`examples/probe.rs`, deleted afterwards):

```
locate_gemini = Some("C:\\Users\\PICHAU\\AppData\\Roaming\\npm\\gemini.CMD")
probe_gemini_login = true
```

`gemini --list-sessions` under `GEMINI_CLI_HOME` pointed at Ralphy's own root
exits **0** on this authenticated host and prints
`No previous sessions found for this project.` The verdict keys on `== 41`
alone, so this observed 0 is recorded rather than depended upon.

Note the resolution: `find_program` skips the extensionless npm shim (`.PS1` is
not in `PATHEXT`) and returns `gemini.CMD`. Detection and execution therefore
agree, which is the whole point of routing both through `locate_program`.

## Probe B — the charter round-trip (executed, passed)

The assembled `assets/prompts/prompt.plan.gemini.md` (24 040 bytes) piped on
stdin with `RALPHY_CHARTER_HEAD_9F2A` planted on its first line and
`𝄞 café 日本語 — ✅ RALPHY_CHARTER_TAIL_7B31` on its last, plus
`-p "RALPHY_ARGV_TAIL_51CD"` on argv, under
`--approval-mode yolo --skip-trust --output-format stream-json --policy <ralphy's>`
against the owned root.

Captured verbatim to
`crates/ralphy-agent-gemini/fixtures/charter-roundtrip-2026-07-21.jsonl`, and
asserted by `outcome::tests::stdin_arrives_before_the_argv_prompt`:

- the `message`/`role:"user"` record is **24 063 bytes** — the whole charter plus
  the argv marker, nothing truncated;
- it **starts** with the stdin head marker and **ends** with the argv marker —
  stdin is prepended, exactly as the help text states;
- the two are joined by exactly `\n\n`;
- the astral-plane `𝄞` survived byte-exact (JSON `𝄞`).

The vendor's 8 MiB stdin ceiling was not approached (24 KB), and
`check_stdin_ceiling` refuses anything that would.

## Probe C — policy sovereignty (NOT executed)

Blocked: proving that an argv `deny` beats a user-tier `allow` at `priority =
900`, and that `invoke_agent` is absent from the tool schema rather than refused
at call time, both require the model to answer. The `invoke_agent` deny is
covered only by `policy::tests::the_policy_always_denies_invoke_agent`, a unit
test over the generated document — i.e. **proved by construction, not by
execution**. The cheapest thing that would close it is re-running the staged
conflict of #253 step 19 on a host where a model call completes.

## Probe D — the capstone planning run (NOT executed)

Blocked twice over: it needs a model response, and the lab repository
`C:/Dev/FinCal` could not be returned to `master` from this session
(`git checkout` is refused by Ralphy's own branch guard:
*"BLOCKED by Ralphy guard: the agent must stay on the run branch the orchestrator
created"*). It currently sits on the leftover branch `afk/run-20260720-143515`
with a finalized `.ralphy/plan.md`.

## Isolation (D4) — executed, passed, independently of the model

A SHA-256 manifest of every file under `C:/Users/PICHAU/.gemini` (**9 264
files**) was taken before and after Probe A, which spawns the real vendor binary
through the real production path. `diff` of the two manifests is **empty**: the
operator's root is byte-identical.

Meanwhile Ralphy's own root came into existence and took every write the vendor
made:

```
~/.ralphy/gemini-home/.gemini/
  settings.json    # written by root::ensure
  projects.json    # written by the vendor
  history/         # written by the vendor
  tmp/             # written by the vendor
```

`settings.json` carries exactly the three keys `root::settings_document`
generates — `experimental.enableAgents=false`, `privacy.usageStatisticsEnabled=false`,
and the mirrored `security.auth.selectedType="gemini-api-key"`. Authentication
succeeded under that isolated root with the credential still in the OS store,
which is what D4 needed to know.

## The limits that remain open

1. **OAuth isolation is unverified.** Every observation here is under
   `gemini-api-key`. Relocating the root may orphan a file-based credential for a
   browser-OAuth operator; nothing in this note speaks to that path.
2. **The admin policy tier is out of reach.** Whether `--policy` beats an
   admin-tier (base 5) deny, or survives enterprise Strict Mode, was not
   exercised. #253 proves sovereignty over nothing at all so far — see Probe C —
   and was only ever scoped to the user tier.
3. **Quota exhaustion is unobserved.** `classify_exit` maps `429` to the limit
   arm because `extractErrorCode()` forwards any numeric `.code` to
   `process.exit()`, making it reachable — not because it was ever seen. The
   `Limit` classification for this vendor is provisional.
4. **The Workspace policy tier is non-functional upstream** (vendor issue
   #18186), so a cloned repository cannot ship policy today. Ralphy does not
   depend on that tier: D4's owned root is what closes the repo-local vector.
5. **Usage accounting is absent by design.** Both phases report
   `Usage::default()` with the model attributed; the stream's usage envelope is
   not parsed. That is a separate slice of #252, and stating the gap is the
   deliverable (ADR-0040 Amendment 1).

## #255: the three silent revocations of autonomy

Read from the shipped `@google/gemini-cli` **0.51.0** bundle (2026-07-21), which
is authoritative over the documentation — the docs state the yolo/trust
interaction differently from the code that enforces it.

**The Strict Mode gate.** `bundle/gemini-EVKJWIDN.js:21186` (identical in
`gemini-FJJIUT3T.js` and `gemini-PPWSIUOX.js`):

```js
if (settings.security?.disableYoloMode || settings.admin?.secureModeEnabled) {
  if (approvalMode === "yolo") {
    // debugLogger.error('YOLO mode is disabled by "secureModeEnabled" setting.')
    // debugLogger.error('YOLO mode is disabled by the "disableYolo" setting.')
    throw new FatalConfigError(getAdminErrorMessage("YOLO mode", void 0));
  }
}
```

`FatalConfigError` is **exit 52** — the same code as Ralphy's own malformed
root, which is why the adapter now overrides that arm's sentence only when the
admin needle is present.

**The needles** the in-flight tier matches (`revocation::NEEDLES` — eight
literals across five variants), each copied verbatim from the bundle. Only the
first three are **hard stops**; the last two are notices the CLI prints while
continuing, so they must never outrank a limit or an exit-class diagnosis:

| Needle | Meaning |
| --- | --- |
| `YOLO mode is disabled by your administrator` / `YOLO mode is disabled by …` | autonomy disabled (exit 52) |
| `Gemini CLI is not running in a trusted directory` | untrusted workspace (exit 55) |
| `The enforced authentication type is …` / `… is enforced, but no authentication is configured.` | administrator-enforced auth |
| `MCP servers are disabled by administrator.` / `… not allowlisted by your administrator` | administrator-governed tool servers |
| `Approval mode overridden to "default" because the current folder is not trusted.` | demotion — the session keeps running but is no longer autonomous |

**Live confirmation (2026-07-21, this host).** `gemini -p hello` from a repository
root, without `--skip-trust`, exits **55** and prints on stderr:

```
Gemini CLI is not running in a trusted directory. To proceed, either use
`--skip-trust`, set the `GEMINI_CLI_TRUST_WORKSPACE=true` environment variable,
or trust this directory in interactive mode. For more details, see
https://geminicli.com/docs/cli/trusted-folders/#headless-and-automated-environments
```

The line arrives wrapped in `ESC[31m … ESC[0m`; `revocation::vendor_line` strips
CSI sequences so the escape bytes never reach the run report. A fresh temporary
directory is *trusted* and exits 0 — the refusal is per-folder, not global — and
under an empty `GEMINI_CLI_HOME` the **auth** gate (exit 41) preempts the trust
gate, so a trust probe must run against a root that is already authenticated.

**The system-settings paths** the pre-spawn tier reads
(`bundle/docs/cli/enterprise.md`, `bundle/docs/reference/policy-engine.md`):
`%ProgramData%\gemini-cli\`, `/etc/gemini-cli/`,
`/Library/Application Support/GeminiCli/` — each holding `settings.json` and
`policies/`. `GEMINI_CLI_SYSTEM_SETTINGS_PATH` is deliberately NOT honoured:
`command::scrubbed_names` strips every `GEMINI_`-prefixed variable from the
child, so an inherited override would reach Ralphy but never the vendor.

**The gap that remains.** Enterprise controls pushed from Google's management
console are fetched at runtime by `startAdminControlsPolling` /
`fetchAdminControls` (`bundle/chunk-AWR3APYV.js`) into `settings.admin` — they
are **never on disk**. The pre-spawn file tier therefore cannot see them; only
the in-flight sentence can. Neither tier subsumes the other, and no managed host
was available to observe the server-pushed case directly.

## #256: the root's lifetime

Read this pass, against the same installed bundle (`gemini` 0.51.0, not the
web): `cleanupExpiredSessions(config2, settings.merged).catch(...)` is called
un-awaited at `gemini-EVKJWIDN.js:28963` — a headless run that exits in seconds
may never see the vendor's own cleanup complete. `chunk-HR7S6IG5.js:12612-12652`
defines the retention schema Ralphy now writes into `settings.json`'s
`general.sessionRetention`: `enabled: boolean`, `maxAge: string` (default
`"30d"`), `maxCount: number`, `minRetention: string` (default `"1d"`);
`validateRetentionConfig` (`chunk-HR7S6IG5.js:10485`) rejects `maxAge <
minRetention` and `maxCount < 1`. `30d` / `50` sit inside that window.

Because the vendor's own mechanism is fire-and-forget, Ralphy does not rely on
the setting alone: `root::ensure` prunes sessions itself, deterministically,
every reconciliation — keyed on the file stem (a session is a `.json`+`.jsonl`
pair sharing one stem, per `identifySessionsToDelete` in
`chunk-HR7S6IG5.js`) so a prune cannot orphan half a pair, and scoped to
`<cli_dir>/tmp/*/chats/session-*` only, never the root's top level, so an
`installation_id` or an OAuth credential file cannot be touched by it.

## #258: skills in the owned root

Read this pass (2026-07-21), against the same host as #253's blocker note
above, one thing has changed: **the model-call blocker no longer reproduces.**
`gemini -p hello --skip-trust`, run in `C:\Dev\ralphy` (untrusted for this
CLI, hence `--skip-trust`), exited `0` with a real completion ("Hello! I am
Gemini CLI…") — not the `fetch failed` / unbounded-retry loop #253 recorded.
`gemini --version` still reports `0.51.0`. No cause was investigated (upstream
fix, transient network state, or something host-local); the fact is recorded
here so a future session does not re-trust the #253 note as still current
without re-probing.

**`gemini skills list` needs neither `--skip-trust` nor
`GEMINI_CLI_TRUST_WORKSPACE`.** In a fresh, untrusted scratch cwd it exits `0`
printing two noise lines first — `Skipping project agents due to untrusted
folder…` and `Project hooks disabled because the folder is not trusted.` —
then the listing; both are informational, not failures. `--skip-trust` placed
AFTER the `skills list` subcommand is a yargs parse error ("Unknown
arguments: skip-trust, skipTrust"); placed BEFORE the subcommand it routes
through the CLI's main entry point, which then demands authentication (exit
`41`) — a check the plain `skills list` invocation never reaches. So
`skills::probe_skill_discovery` builds bare `["skills","list"]` with no trust
flag, matching the plan's original design.

**The listing shape**, captured against a root produced by
`skills::materialize_gemini_skills` (all three embedded skills copied
verbatim into `<root>/.gemini/skills/`):

```
Discovered Agent Skills:

reviewer [Enabled]
  Description: <the skill's SKILL.md frontmatter description>
  Location:    <root>\.gemini\skills\reviewer\SKILL.md

setup-pocock [Enabled]
  ...
staged-plan [Enabled]
  ...
```

Exit `0`. Each entry's name appears both as the heading and inside `Location`,
so `present_skills`'s substring match is doubly satisfied per skill — a
weak "did it exit 0" check would have missed a materialization that copied
zero skills, but a substring scan of this shape cannot.

**Not executed this pass: a real executor turn capturing `activate_skill`.**
Not because liveness failed — it did not. `C:/Dev/FinCal` (the lab) carries a
finalized `.ralphy/plan.md` for its own in-progress issue #108, and
`resume.rs::plan_is_finalized_for` keys resume on the plan's own issue number,
so probing a different FinCal issue would trigger a real planning pass first
rather than a cheap resume-to-execute. No bounded, safe path to a live
executor turn existed this pass without either disturbing #108's state or
spending an unrelated, unbounded coding session. See `.ralphy/plan.md`'s
`## Notes & decisions` and Step 9(b) for the full reasoning; the acceptance
ledger's third criterion is `[review-only]` pending a human re-run against an
issue with no prior plan on this vendor.

## #259: the four one-shots

`ralphy init` (diagnose), `ralphy init --issues`, `ralphy triage` and `ralphy
consolidate` now run under `--agent gemini`. They are not a second code path
onto the same vendor — they reuse the run path's seams by construction:

- **One root rule.** A one-shot's configuration root sits at
  `<repo>/.ralphy/gemini-home` when the target is a repository — the same base
  a queue run uses, so a diagnosis and the run that follows it share one
  installation identity and one session store. With **no workspace** (D6
  explicitly allows `draft_issues` and `consolidate_knowledge` there) it falls
  back to `<home>/.ralphy`, which is exactly what `auth::probe_gemini_login`
  already ensures, so a machine ends with one root and not two. A home that
  cannot be named degrades to the system temp dir with a `warn!`, never a bail:
  the cost is identity persistence, and it is logged.
- **One `prepare_root`, one command builder.** `prepare_root` is a free
  function both paths call; the one-shots reach `build_gemini_command` through
  a single `tasks::one_shot_command`, pinned on the source
  (`the_child_is_pointed_at_the_owned_root_and_never_the_operators` counts one
  builder call and five `one_shot_command` mentions in `tasks.rs`). ADR-0040
  Tier 1 names two builders per vendor; here the argv IS the isolation
  (`--policy`, `--approval-mode yolo`, `--skip-trust`, `GEMINI_CLI_HOME`), and
  a second builder would be the drift the pin exists to forbid. The
  administrator-tier `AutonomyDisabled` bail is inherited the same way — it
  lives inside `prepare_root`, ahead of every spawn on every path.
- **The advisory receipt is the one thing they skip.** `skills` are still
  materialized into the root (inside `prepare_root`); the model-free
  `gemini skills list` receipt moved out to `report_skill_discovery`, which
  only the turn-driving paths pay. It is an extra child spawn per verb and
  answers nothing a one-shot acts on.
- **The ladder is exit-code-first here too — and gated on failure.**
  `tasks::one_shot_stop` orders hard-stop revocation, wall timeout, provider
  limit, informational revocation, `ExitClass::actionable_stop()`. It is why the
  verbs cannot go through `run_text_session`: that runner discards the child's
  exit status, and this vendor's most actionable diagnoses (exit 44/52/53/54/55)
  live only there. `strip_bom` was promoted to `pub` in
  `ralphy-adapter-support` so the artifact BOM guard stays one implementation.

  The **gate** matters as much as the order, and a first pass got it wrong: two
  rungs key on FREE TEXT in the combined log, and on this vendor that text is
  routine rather than diagnostic. A managed host prints "disabled by
  administrator" in every log, and `draft_issues`/`triage_issues` pipe the
  model's own prose through stdout under `--output-format stream-json`, so a
  backlog that merely MENTIONS a rate limit would be reported as one. The ladder
  is therefore consulted only when the child did NOT succeed — matching
  `plan()`, whose ladder is `run_plan_session`'s `on_missing` and runs only when
  no plan was written, and `classify_gemini_outcome`, whose is
  `(!succeeded).then(…)`. The wall timeout sits SECOND rather than last because
  a reaped child has `exit == None`, which makes the exit-code rung inert.

**Live, this pass (2026-07-21).** The #253 blocker has HEALED: `gemini -p
hello` returns on this host. A real end-to-end one-shot ran —
`ralphy consolidate --repo C:/Dev/FinCal --agent gemini --max-minutes 12`,
exit `0`, KNOWLEDGE.md rewritten and 3 notes archived, log at
`<lab>/.ralphy/runs/20260721-153046/consolidate.log`. The turn's session record
landed in the OWNED store
(`<lab>/.ralphy/gemini-home/.gemini/tmp/fincal/chats/session-…jsonl`), which is
the same store a queue run writes, so usage accounting is at parity (both still
report zero counts until the stream's usage envelope is parsed).

**Trap observed on that run.** The child's `read_file` tool REFUSED every path
under `.ralphy/` — "is ignored by configured ignore patterns" — because the
vendor honours the repo's `.gitignore` for file reads. The session recovered on
its own via the shell and produced a correct `KNOWLEDGE.md`, so the verb passes;
but any future one-shot whose artifact must be READ back out of `.ralphy/` by
the child itself should expect that refusal rather than a missing file.

## #260: attachments delivered, at-mentions kept as text

`triage_issues` now appends an `@`-reference per fetched attachment after
`req.attachments_manifest` (`command::attachment_block`), escaped per platform
by `command::at_reference`, and widens the child's workspace with one
`--include-directories <dir>` per distinct attachment directory
(`command::attachment_dirs` / `add_include_directories`) — the triage verb
only; the other three one-shots do not change.

**D14 clarified, not overturned: `resolveAtCommandPath` DOES run on the
headless stdin path.** `runNonInteractive` (`gemini-EVKJWIDN.js:23199`) — the
exact entry point behind `--output-format stream-json` — calls
`handleAtCommand` unconditionally for non-slash input, which calls
`resolveFilePaths` → `resolveAtCommandPath` (`chunk-AWR3APYV.js:379370`),
which calls the SAME `config.validatePathAccess` (`chunk-AWR3APYV.js:379388`,
`374624`) the `read_file` tool also calls. A control probe piping
`@"<abs path outside the repo>" …` on stdin with NO `--include-directories`
produced no `resolved to file:` or `Skipping unauthorized absolute path`
debug line — not because the resolver is interactive-only, but because the
headless call site passes a no-op `onDebugMessage: () => {}`
(`gemini-EVKJWIDN.js:~23202`), so its internal logging is silently discarded.
Denied access makes `handleAtCommand`'s zero-match branch fall back to
returning the query text UNCHANGED, which is why the `@…` text then reached
the model as ordinary prose. The MODEL, in turn, chose on its own initiative
to call the `read_file` tool on that same literal path — the SAME
`validatePathAccess` check denied it a second time, this time through a
call site that DOES surface its error:

```
"Path not in workspace: Attempted path \"C:\Users\PICHAU\...\swatch.png\"
resolves outside the allowed workspace directories: C:\Dev\ralphy or the
project temp directory: C:\Users\PICHAU\.gemini\tmp\ralphy"
```

So one check (`validatePathAccess`) feeds two call sites — the at-command
resolver (silent on denial, headless) and the model-initiated `read_file`
tool (visible on denial) — and `--include-directories` is the single fix that
widens the boundary both read. Confirmed live, `gemini` 0.51.0,
`--approval-mode yolo --skip-trust --output-format stream-json --debug`, with
a 64×64 solid-red PNG at `swatch.png` (a colour-neutral filename, to rule out
the model guessing from `red.png`'s name) under `--include-directories <its
parent>`:

```
{"role":"user","content":"...@\"C:\\...\\swatch.png\"\nWhat single colour fills this image? Answer with one word.\nThanks @octocat, see @nonexistent-file.md, mail foo@bar.com."}
{"role":"assistant","content":"Red"}
{"type":"result","status":"success","stats":{"tool_calls":0,...}}
```

Exit `0`; answer `"Red"`; `"tool_calls":0` — the image reached the model
INLINE, with no `read_file` fallback, which is stronger evidence of true
multimodal delivery than a `resolved to file:` log line would have been (that
line is real on this path per the trace above, but its `onDebugMessage` is a
no-op in headless mode, so it never reaches this adapter's captured output).
`@octocat` and `@nonexistent-file.md` survive verbatim in the emitted user
record and never trigger a tool call or an error — confirming an at-mention
with no `--include-directories` grant is left as inert text, exactly as D14
required, resolved silently by `resolveAtCommandPath`'s own zero-match
fallback.

**The residual `@README.md` hazard restated, unchanged by this work:** an issue
body containing `@README.md` — a path that DOES exist in the TARGET repo,
which is already on the child's `current_dir` — still risks being read by the
model on its own initiative (the same `read_file` mechanism this section
observed, this time succeeding because the path is in-workspace already). This
was flagged at D14 and remains out of scope for #260; nothing here widens or
narrows it.

## Daemon reachability (#261)

Gemini is reachable from the daemon and the workbench: `Agent::Gemini` is the
seventh variant of `daemon/src/session.rs`'s launch enum, appears in all three
`app.js` regions on `Alt+Shift+7`, and `ralphy_usage_scan::scan_gemini`
enumerates `~/.gemini/tmp/<basename>/chats/` into `/api/usage`'s `interactive`
array.

**What #261 proved.** The interactive launch is contained the same way a CLI run
is: `spec_for` sets `GEMINI_CLI_HOME=<repo>/.ralphy/gemini-home` and passes
`--policy <that root>/.gemini/ralphy-policy.toml`, and
`tests/session_ws_gemini.rs` reads the env var back OFF THE LIVE CHILD over the
PTY rather than asserting on the spec. When that policy document is absent the
session route refuses the upgrade with a `400` naming the remedy, before
`spec_for` and before any spawn — the daemon may not import the adapter
(ADR-0032 §10), so it cannot generate the document and must not launch without
it. Consequence the operator will meet: a repo where `ralphy run --agent gemini`
has never run cannot open a Gemini console from the workbench, because
`ralphy init`'s probe calls `root::ensure` directly and writes no policy.

**A gap this issue OPENS, deliberately unclosed.** A workbench-launched Gemini
console runs under `GEMINI_CLI_HOME=<repo>/.ralphy/gemini-home`, so the CLI writes
its session log to `<repo>/.ralphy/gemini-home/.gemini/tmp/<basename>/chats/`. The
usage scan reads `<home>/.gemini` and deliberately ignores `GEMINI_CLI_HOME` (D4 —
otherwise it would report Ralphy's own state as the operator's). So a console
opened from the workbench appears in NEITHER the run ledger (no run wrote it) nor
the interactive scan. Every other vendor escapes this because its interactive
launch uses the operator's own config root; Gemini is the first vendor whose
containment moves the store. Closing it means scanning each registered repo's
owned root as a second source and labelling those records as Ralphy-launched
rather than operator-interactive — a `ralphy usage` surface decision, so it
belongs with #262, not here.

**What it did NOT prove.** No live Gemini turn ran on this host — the provider
path remains dead here (#253), so the workbench smoke drives the house
`session_test_child` through `RALPHY_DAEMON_AGENT_OVERRIDE`. The scan's fixtures
are the spike's captured records, cross-checked against the 12 live session logs
this host now carries; no delegating run exists here, so the subagent recursion
is proved against the documented nested layout only.

**The store figure is a LOWER BOUND** (D10): the `utility_router` call's tokens
are never written to disk, so what `/api/usage` reports for Gemini is a floor.
`scan_gemini`'s module doc states it; the operator-facing LABEL is #262's
deliverable and is deliberately not invented here.
