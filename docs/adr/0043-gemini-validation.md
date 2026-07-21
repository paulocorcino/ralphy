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
