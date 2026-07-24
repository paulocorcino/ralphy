# Triage attachment evidence: mechanical fetch with guardrails

Status: proposed.

ADR-0018 made promotion depend on an **evidence gate**: promote and consolidate
require the problem be *confirmable at source* and *localizable* (`file:line`),
not merely a crisp-looking spec. But the triage charter's only reading
instruction is `gh issue view <n> --comments` (ADR-0017 §2), which renders the
issue body and thread as markdown. GitHub **attachments** — the `.log`, `.json`,
`.txt` files, and the screenshots, a reporter drags into an issue — surface there
only as `github.com/user-attachments/...` links. Their *content* never reaches the
session.

For a class of bug reports this makes the evidence gate unsatisfiable by
construction. The motivating case is [#133](https://github.com/paulocorcino/ralphy/issues/133):
`ralphy init` fails with `diagnosis report ... did not match the schema` and the
entire evidence — `diagnose.log` and `diagnosis.json` — lives in two attachments.
The triage agent sees two URLs and nothing behind them, so it cannot confirm at
source or localize, and the honest verdict collapses to **bounce** ("paste the
content inline") on an issue whose evidence the reporter already supplied. The gap
is not that the bug is unreal; it is that the evidence is in a format triage
cannot reach.

The obvious fix — let the agent `curl` the links itself — is rejected. The triage
session already runs in a shell and *could*, but that makes fetching a
non-deterministic LLM judgment (sometimes fetched, sometimes not; untestable, and
nothing the evidence gate can rely on), and it puts the "is this URL safe to
fetch?" decision in the model's hands: an arbitrary URL pasted into an issue could
point at an internal host (SSRF) or a multi-gigabyte file that blows the context
window. Deciding what is safe to download is mechanical policy, not judgment — it
belongs in Rust, the same way `apply_triage` (not the agent) performs every
outward act (ADR-0017 §5).

## Decision

The CLI fetches attachment evidence **mechanically, with guardrails**, before the
triage session starts, and hands the session local files plus an inline manifest.
The agent decides only what is *relevant* to the verdict — never whether a fetch is
safe, and never performs the fetch.

### 1. A pure pre-fetch in `ralphy-core::github`

Link extraction, guardrail filtering, and the download live in
`ralphy-core::github` as a function pure over the extracted link list (no network
in its unit tests) — this is the security axis, and it is fixed in Rust and
testable, not left to the model. `ralphy triage`, after `select_triage_agent`
and before `triage_with_agent`, calls it and passes the resulting directory +
manifest into `TriageRequest` (additive field; no existing field changes — the
public core API stays stable).

The step reads each issue's body and comments once
(`gh issue view <n> --json body,comments`) to find the links. The triage agent
still runs its own `gh issue view --comments` for the prose — the issue is fetched
twice. This duplication is deliberately accepted: it is cheap at the labelled
subset scale (tens of issues, not hundreds) and is the same trade ADR-0017 §4
already made for per-issue comment fetches at queue-build. Rewriting the agent's
reading contract to dedupe the fetch would destabilize a path that works today for
an optimization that does not hurt.

### 2. Authenticated fetch, never anonymous

Attachments on **private** repos require authentication; an anonymous `curl`
follows the redirect to a login page and would save that HTML *as if it were*
`diagnosis.json` — worse than not fetching, because the format allowlist would pass
the `.json` name and the agent would read fabricated evidence. So:

- The download authenticates with the same credential triage already requires —
  `gh api` or `curl` carrying `Authorization: token $(gh auth token)`.
- **Login-HTML guard.** When the expected format is text/structured, a response
  whose content-type is HTML or whose body opens with `<!DOCTYPE html`/`<html` is
  rejected as `not fetched (auth)` rather than delivered. A private-repo attachment
  never masquerades as satisfied evidence.

### 3. Guardrails (all in Rust, deny-by-default)

1. **Host allowlist.** Only `github.com/user-attachments/...` — files the reporter
   attached through GitHub's own UI. Arbitrary URLs pasted into the prose are left
   as links, never fetched. This is what closes SSRF/exfiltration: the model never
   chooses a fetch target.
2. **Format allowlist.** Text the model reads with profit —
   `.log .txt .json .yaml .yml .toml .csv .md .diff .patch` — plus images
   (§4) — `.png .jpg .jpeg .webp .gif`. Everything else
   (`.exe .bat .cmd .ps1 .sh .msi .dll .zip` and any unknown extension) is
   rejected. Not primarily an anti-virus measure — a downloaded file is inert; we
   never execute, `chmod`, or run anything — it falls out of a simpler truth: an
   attachment the model cannot read does not help the judgment.
   **Amendment (2026-07-19, from [#216](https://github.com/paulocorcino/ralphy/issues/216)):
   classification is by extension *or* by content, never by extension alone.**
   GitHub has two attachment URL shapes: the named
   `user-attachments/files/<id>/<name.ext>`, and — for an image pasted or dragged
   into the body, which is the default path in today's UI —
   `user-attachments/assets/<uuid>`, carrying **no filename and no extension**.
   Extension-only classification denied the second shape unconditionally, so every
   inline screenshot came back `not fetched (denied format)`: the exact evidence
   §4 exists to deliver, denied before the download that (verified) would have
   succeeded. So an extensionless URL **under the already-allowlisted asset path**
   is a *candidate*: it is gated on adapter capability and downloaded as an image
   would be, then must prove itself by **magic bytes** (PNG, JPEG, GIF, WEBP). On a
   match it is written with the sniffed extension — adapters that deliver pixels by
   path need the suffix — and on no match it is `denied format`, exactly as before.
   This tightens the allowlist rather than loosening it: a `.png` *name* is still
   believed on the named path, but an asset must *be* an image. Extensions remain
   authoritative wherever one exists, and no new host, no new format, and no
   model judgment enters the fetch decision.
3. **Size cap, truncation by category.**
   - Free text (`.log .txt .md .diff .patch`): downloaded up to a cap (default
     1 MB); an over-cap file is kept **head + tail** with a
     `[... N bytes elided ...]` marker in the middle — a log's error is usually at
     the tail and its context at the head, so cutting the middle preserves both.
   - Structured (`.json .yaml .yml .toml .csv`): **never truncated** — half a JSON
     is noise, not evidence. Over-cap → `not fetched (too large)`.
   - Images: **never truncated** (a truncated PNG corrupts); own larger cap
     (default 5 MB). Over-cap → `not fetched (too large)`.
4. **Count cap.** At most N attachments (default 10) per issue, counted across body
   and thread after dedup (§5).
5. **Download only, never execute.** Guardrails 1–4 carry the safety; this is the
   belt.

Defaults are constants, not flags, until a real need forces configurability —
consistent with `triage-agent` being non-configurable (ADR-0017 §1).

### 4. Images are per-adapter capability

Triage is single-adapter — the attachment directory is written by the CLI and
consumed once, by the adapter running *this* session — so image support does not
break the cross-adapter neutrality the plan artifact needs (there is no
planner→executor handoff here). The selected adapter is known before the fetch
(`select_triage_agent` precedes it), so:

- The adapter declares `accepts_images` in its contract (Claude, Codex: yes;
  OpenCode until it gains vision: no) — a capability on the adapter, never a
  hardcoded `match` in the CLI, so a future vision adapter is a bool in its own
  crate.
- If the selected adapter cannot read images, they are **not fetched** —
  `screenshot.png → not fetched (<adapter> has no image input)` — no wasted
  download or context; the agent still sees an image exists and can bounce for the
  text.
- If it can, the image is downloaded under the guardrails and the **per-adapter
  invocation** wires the pixels to the model (Claude references the path in the
  prompt; Codex uses its image-input flag). This delivery is the one vendor-specific
  piece — exactly what ADR-0002 says lives in the adapter. The core fetch stays
  neutral: it downloads the file and lists it; *how* the pixel reaches the model is
  the adapter's.

### 5. Extraction scope and determinism

- Links are extracted from the **body and every comment** (the real spec, and its
  evidence, often emerged across the thread — ADR-0017's premise).
- **Dedup by URL:** the same attachment linked in the body and re-pasted in a
  comment downloads once.
- The count cap (§3.4) applies to the deduped total across body + thread.
- **Deterministic order:** body first, then comments chronologically; the cap drops
  the last. Reproducible output — required for the pure unit tests and for
  Windows/Linux parity.

### 6. The manifest is the contract; silence never

- **Delivered inline** in the prompt as a short `## Attachments (issue #N)` block:
  each attachment as `name → path (fetched)` or `name → not fetched (<reason>)`.
  The list is short (≤10/issue); paths and reasons in-context beat sending the
  model to open another file. The downloaded files sit on disk; only the list is
  inline.
- **Every negative outcome is visible** with its reason — `denied format`,
  `too large`, `auth`, `download failed: <code>`, `attachment cap reached`,
  `<adapter> has no image input`. The invariant is *silence never*: an attachment
  the agent could not inspect is evidence it does **not** have, and the charter
  treats a missing needed attachment as a **bounce**, never mistaking "saw no
  evidence" for "saw all the evidence". This is what keeps ADR-0018's gate honest.
- **Best-effort, never blocking.** A failed download (404, timeout, login-HTML)
  becomes a manifest line, never an aborted triage run — the event-sink posture
  (CONTEXT.md: "additive, best-effort, never blocking the run").

### 7. Lifecycle: an OS temp directory, deleted when triage returns

Attachments are downloaded into a per-run **OS temp directory**
(`<os-temp>/ralphy-triage-<runid>/<n>/`), never into the target repo's working
tree, and the directory is **deleted when triage finishes** — a `tempfile::TempDir`
whose `Drop` removes it on normal return and on panic alike (a hard kill leaks it,
but the OS reclaims its temp on its own). Two properties follow:

- **The repo tree is never touched.** Private-repo attachment content never lands
  in a versionable directory, so this carries no dependency on the target repo's
  `.gitignore` at all (unlike `.ralphy/triage-draft.json` and the logs, which stay
  in `.ralphy/` as before).
- **No stale state.** A fresh directory per run means a re-triage never reads an
  attachment from a prior run (the attachment may have changed, or the issue may
  have left the queue); attachment-side idempotence is free.

The session reads the files while it runs — the three adapters drive these
one-shot init/triage sessions with unrestricted filesystem access
(`--dangerously-skip-permissions` / `-s danger-full-access`), and `diagnose_repo`
already reads/writes outside the repo from a neutral cwd, so an absolute temp path
is proven-readable across Claude, Codex, and OpenCode. The files are needed only
during the agent session; the apply phase (preview → confirm) does not touch them.

## Consequences

- Bug reports whose evidence is attached (logs, diagnostic JSON, screenshots)
  become triageable at the evidence gate instead of bouncing on format. #133 is the
  first beneficiary.
- Triage grows one `gh issue view --json` fetch per issue plus bounded downloads;
  paid at the labelled-subset scale (tens), same order as ADR-0017's per-issue
  comment fetches.
- The safety axis (what is fetchable, and that a private-repo login page never
  masquerades as evidence) is fixed in Rust and unit-testable: host filter, format
  allowlist, by-category truncation, login-HTML guard, dedup, deterministic order,
  and reject-visibility are all pure over the extracted link list. The LLM keeps
  only the relevance judgment.
- Image delivery is the one vendor-specific addition, in the adapter crates behind
  an `accepts_images` capability; OpenCode degrades honestly until it gains vision.
- Attachments live in an OS temp directory deleted when triage returns, never in
  the repo tree — no dependency on the target's `.gitignore`, and private-repo
  content does not linger in a versionable directory.
- Cross-platform: download-to-file, OS-temp path handling, and the paths passed to
  the model must be Windows/Linux clean; the pure extractor/filter is tested
  without network per CONTEXT.md testing conventions.
- ADR-0018's evidence gate is unchanged in intent; this ADR only widens the set of
  evidence the agent can actually reach to satisfy it.
