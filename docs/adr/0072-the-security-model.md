# The security model: trust zones, and the premises every change keeps

Status: accepted
Kind: structural
Protects: security, integrity of change

## Context

Ralphy runs AI agents with shell access on the operator's machine, serves a
workbench that opens shells in a browser, and installs its own updates. Each
of these is a way into the machine, and each was secured when it was built:

- ADR-0032 and its amendments: the daemon's bind, login, session, TOTP,
  step-up, response headers, comment trust, local-only keys.
- ADR-0036: the closed verb registry and path confinement.
- ADR-0049, ADR-0055, ADR-0064: what the daemon may serve and write.
- ADR-0052, ADR-0067: peers and the ssh tunnel.
- ADR-0056: the release watch and the update.
- ADR-0041, ADR-0042, ADR-0043: per-vendor guards and risk hatches.

No document states the whole model: who is trusted, where the boundaries
are, and which rule holds at each one. A change to one surface cannot see
what the others assume. `docs/ARCHITECTURE.md` §2 has the one general rule:
the strongest option is the recommended default, but it is opt-in, and
Ralphy never takes a capability away from the operator.

The audit of 2026-09-21 (two passes, findings F1–F16, code reading and a
live probe) covered the daemon in depth. Its fixes and its accepted risks
are in the ADR-0032 amendment of that date. An inventory on 2026-10-01
(code reading only) covered the two surfaces the audit did not reach in
depth: the agent session and the distribution chain. The gaps it found are
listed under Compliance, next to the premise each one breaks. The largest:

- Claude's plan session and its triage, consolidate, diagnose and draft
  tasks run with the permission gate off and no guard hook
  (`crates/ralphy-agent-claude/src/settings.rs:214`, `tasks.rs:45`), while
  they read issue text. Only the execute session has the guard.
- The guard exits 0 (allow) when it cannot read its input
  (`crates/ralphy-cli/src/guard.rs:349-356`), and it does not deny
  `gh pr create`.
- `queue.trust_all_comments`, which undoes the comment trust filter, can be
  set from the workbench (`crates/ralphy-daemon/assets/ui/wb-settings.js:180`).
- The updater checks a SHA-256 that comes from the same release as the
  archive (`crates/ralphy-cli/src/update/apply.rs:128`). The binaries are not
  signed. D12 accepts this, with the reason.

## Decision

### Trust zones

| Zone | Trust | Why |
|---|---|---|
| The operator | Full | Ralphy runs as the operator's OS account and acts for them. |
| Other processes on the operator's machine | Full under the default policy; none with `require-login` or `require-token` | A loopback bind trusts the local machine. This is correct only on a single-user host. |
| A network client of the daemon | None until it authenticates | It may be anyone who can reach the port or the tunnel. |
| A peer daemon | As much as the local daemon | It holds the peer's token and reaches it only through loopback or `ssh -L`. |
| The issue body | Trusted | The label is the gate, and only a user with triage rights can set it. |
| An issue comment | Untrusted, unless its author is an owner, member or collaborator | On a public repository anyone can comment. |
| The agent session | Acts as the operator; its output is unverified | A prompt can turn it. Its claims are checked by the runner, not believed. |
| An outside service (GitHub, models.dev, vendor APIs, Telegram, an event sink) | Its answer is untrusted data | Ralphy does not control it. |
| A Ralphy release | Trusted after its checksum is verified | It comes from this repository's release workflow. |

### Premises

**D1. The default is safe for one person on one machine, and the stronger
posture is one setting away.** The daemon binds loopback by default. A
setting that lowers the posture is allowed, because Ralphy never takes a
capability from the operator. A key that lets someone other than the
operator influence the agent, or sends data off the machine, is local-only:
the workbench shows it read-only, and `ralphy config set` changes it.
Local-only is not a boundary, because a workbench session can open a
console. It makes the change a deliberate gesture instead of one click. A
setting that lowers the posture costs a fresh factor when one is armed.
`queue.trust_all_comments` is local-only. `remote_control` is not: it opens
the run to the operator's own Claude account, which is inside the operator
zone. `events.url` is not: the token that goes with it is. The docs tell
the operator when to use the stronger posture: `require-login` on any
machine that is not single-user, and a TLS front for any network reach.

**D2. A network bind fails closed.** A non-loopback bind with no credential
does not start. Host and Origin are checked on every request. The daemon
never terminates TLS: a front (a tunnel or a proxy) encrypts. A secret that
would cross a plain-HTTP hop that is not loopback is refused, or the
operator is warned; it is never sent in silence.

**D3. The browser reaches only the daemon, and only through closed
vocabularies.** A capability is a verb in the registry, not a new route. A
child process gets an argv, never a shell command line. Every argument from
the browser has its shape checked before it reaches a CLI, and a value that
git or `gh` would read as a positional comes after `--` or
`--end-of-options`.

**D4. The daemon writes only inside a registered root.** A path is confined
before any write. `.git` and `.ralphy` are refused under every spelling the
OS treats as equal, except a target that a verb itself fixes. Every read
has a size cap.

**D5. Forge text is input, never instruction.** The issue body carries
authority. In a run, a comment reaches a prompt, the blocked-by gate or a
handoff only if its author is an owner, member or collaborator. The charters
say that a comment is data about what its author wants.

Triage is the one exception, on purpose: it reads every comment and every
attachment, because the reporter of a bug is often an outsider and the
evidence is theirs. So Ralphy gives the triage session the thread itself,
with each outsider's comment marked, and the agent does not fetch the thread
on its own (ADR-0017 amendment of 2026-10-01, A1). An outsider's comment can
still shape a consolidated spec, which is then posted under the operator's
identity and trusted by the run. Interactive triage shows that spec before
it is published. Under `--yes`, ADR-0017 A2 holds such a spec back for the
operator.

**D6. The agent runs as the operator and is not sandboxed. Ralphy says so,
and puts a guard where the vendor allows one.** The flags that let an agent
run unattended (`--dangerously-skip-permissions` and each vendor's
equivalent) stay: an unattended run has nobody to answer a permission
prompt. The guard is a layer on top of them, not a replacement. Every
headless session runs with Ralphy's guard hook when the vendor offers a
pre-tool hook, and otherwise with the vendor's own deny policy. The
adapter's ADR names which one, or states that the vendor has neither. For
Claude, this means every headless session (plan, execute, triage, diagnose,
draft, consolidate) carries the guard's deny-list; the verification-cost
gate runs only in execute, because it reads the plan of the current issue.
The guard denies the forge writes the charter forbids: `git push`, every
`gh pr` verb that writes (`create`, `edit`, `ready`, `reopen`, `review`,
`comment`, `merge`, `close`), and `gh release`, `repo`, `workflow`,
`secret`, `auth`. It allows the `gh pr` reads (`view`, `list`, `diff`,
`checks`). It does not deny `gh api`: skills use it for legitimate reads and
writes, and a pattern wide enough to stop a hostile call would stop those
too. That is an accepted risk: an agent turned by a prompt can still write
to the forge through `gh api`. The guard fails closed: input it cannot read
(empty, or not JSON) is a deny with a message that says so, and a test pins
the payload shape Claude sends today.

**D6a. A security fix does not break what works.** A new deny rule, filter
or limit is checked against what the current flows do before it ships. A
rule that would block a command a charter tells the agent to run is a bug in
the rule.

**D7. A secret Ralphy owns stays with Ralphy.** The daemon token, peer
tokens, the events token, the Telegram token, the TOTP seed and the password
hash are stored owner-only on every platform: a secret file is created
owner-only (mode `0600` at creation on Linux and macOS, then renamed into
place; a protected DACL on Windows), not made owner-only after it is
written, and the store directory is owner-only too (`0700`). They are
removed from the environment of every child, and never appear in a log, an event, an error,
or the UI. They are compared in constant time. The operator's own forge and
vendor credentials do reach the agent, because the agent needs them; the
docs state this.

**D8. Ralphy never publishes on its own.** Ralphy pushes only when the
operator runs `ralphy sync push`. It never opens a pull request. An agent's
claim of success is not evidence: the verify gate runs the checks itself.

**D9. Every outside call has an owner and a limit.** The owner is the one
named in ARCHITECTURE.md §6. The call uses HTTPS, has a timeout and a cap
on the response size. It sends no identifier of the operator, except to a
service the operator configured to receive one (an event sink, Telegram).
The answer has its shape checked before it is used.

**D10. The workbench runs no script it did not ship.** The CSP allows no
inline script except by hash and allows no framing. Every HTML sink takes
DOMPurify output or static markup. First-party code has no `eval` and no
`new Function`. Remote content (images) is opt-in. `connect-src` names the
daemon's own WebSocket origins, not every host. Notes render through the
ProseMirror schema, which builds the DOM itself; their control is the
link-scheme allowlist, not DOMPurify. `'unsafe-eval'` stays in `script-src`
for Alpine and Monaco: removing it would mean the CSP build of Alpine and a
rewrite of its directives.

**D11. Every input the daemon accepts has an explicit cap.** This covers
request bodies, WebSocket messages, file sizes and item counts. A library
default is not a decision. The transport cap is derived from the largest
legitimate payload, not written as a free number: the base64 size of
`MAX_IMAGE_BYTES` plus 64 KiB. It applies to `/ws/command`, `/ws/session`
and the HTTP routes that carry a command, so a larger content cap raises
the transport cap with it. A general rate limit and per-request timeouts
are out of scope, by the non-goal on denial of service; the login throttle
stays.

**D12. The supply chain is pinned and watched.** Rust dependencies come
from crates.io only, and a RustSec advisory fails CI. Every action is pinned
by SHA, a workflow starts with `permissions: {}`, and a job gets a write
permission only when it needs one. Every vendored browser library has a
recorded version, source URL and SHA-256 in one manifest, a test recomputes
the hashes, and a daily scan checks those versions against the advisory
databases. A vendored library is updated by hand and reviewed, never by a
bot, because a bump can break the workbench. A release carries a SHA-256 per
archive and a build provenance attestation. The updater verifies the
archive's SHA-256 before it replaces the binary. That check detects a
corrupt or cut download; against an attacker it adds nothing to TLS,
because whoever can replace an archive in a release can replace its
`.sha256` too. The trust anchor of an update is this repository on GitHub,
and a compromised forge account is a non-goal. A signature by a CI secret,
or a check of the attestation inside the updater, has the same anchor, so
neither is added. A signature by an offline key is the option to reopen if
Ralphy is ever distributed outside GitHub. Code signing (Authenticode,
macOS notarization) is an install-experience question, not an integrity
one, and is not decided here.

**D13. `unsafe` Rust is for FFI only.** It lives in named modules, and every
block has a `SAFETY` comment.

**D14. A security fix is proven and announced.** It ships with a test that
was seen red, and with a `kind: security` changelog fragment. A finding that
is accepted without a fix is recorded, with the reasoning, in the ADR of its
surface.

### What Ralphy does not defend against

- **Another user on the same machine, under the default policy.** Use
  `require-login` or `require-token`.
- **The agent itself.** The guard reduces mistakes and the effect of a
  prompt injection. It is not a sandbox, and a determined agent can work
  around it.
- **Perfect prompt-injection prevention.** No filter can prove that a model
  ignores every hostile sentence. D5 and D6 are layers, not proof.
- **A compromised operator account, forge account, or vendor service.**
- **Denial of service from the internet.** The daemon is not built to face
  the internet directly. A tunnel with login is the supported way to reach
  it from outside.
- **Encryption at rest.** Ralphy's stores are plain files protected by the
  OS account.

## Consequences

- This ADR is the index of the security model. The detail of each surface
  stays in that surface's ADR. A new security decision for one surface goes
  in that ADR, and changes a premise here only when it changes the model.
- `SECURITY.md` is the summary for users and for people who report a
  vulnerability. It states only what is true today: a premise with an open
  gap below is written there with its gap, or not at all. A change that
  adds, removes or closes a gap in a protection that `SECURITY.md` names
  updates `SECURITY.md` in the same change.
- `docs/ARCHITECTURE.md` §9 maps each boundary to its control and its ADR.
- The gaps under Compliance become issues. While a gap is open, the premise
  it breaks is a target, not a fact. Each fix that closes a gap updates its
  Compliance line in the same change.

## Considered options

- **Write the premises in ARCHITECTURE.md only.** Rejected: ARCHITECTURE.md
  is a map that points to decisions. The premises are decisions with real
  trade-offs (D1, D6), so they need an ADR that can be amended.
- **Amend ADR-0032 again.** Rejected: ADR-0032 is the daemon. The agent
  session (D5, D6) and the distribution chain (D12) are not the daemon, and
  ADR-0032 already holds seven amendments.
- **Run every agent session in a sandbox (container or VM) by default.**
  Not decided here. The vendor CLIs need the operator's toolchain and
  credentials, and Windows has no container that is portable to the other
  two platforms. A sandbox as an opt-in is a separate decision.

## Compliance

- D1: not checked by code: no check knows which keys lower the posture.
  `queue.trust_all_comments` is in `LOCAL_ONLY_KEYS`
  (`crates/ralphy-daemon/src/dispatch/argv.rs`), so the workbench cannot
  set it (`config_argv_refuses_local_only_keys`), and its row is read-only
  (`every_settable_key_the_panel_offers_is_a_key_the_cli_accepts`,
  `ui-tests/wb-settings.test.mjs`).
- D2: not checked by code: a new route or a new bind path is reviewed in the
  PR. Accepted: `/api/session` tells a caller before login whether a
  password is set (audit F15); the login screen needs it.
- D3: partly checked: a new `git`, `gh` or `ssh` spawn site fails
  `spawn_sites_match_the_baseline` (`crates/xtask/tests/ratchets.rs`), and a
  new subprocess anywhere is flagged by `xtask capabilities`. "The browser
  reaches only the daemon" is held by the CSP's `connect-src 'self'` (see
  D10).
- D4: not checked by code: a new write path is reviewed in the PR.
- D5: checked for triage. Ralphy reads each thread with
  `authorAssociation` and gives the session a JSON block in which each
  outsider's comment is marked (`parse_issue_thread_marks_outsiders`,
  `render_triage_threads_marks_and_escapes`); the charter does not fetch
  the thread (`the_triage_charter_never_fetches_the_comment_thread`); under
  `--yes` a consolidation that draws on an outsider's comment, an unknown
  comment or an unread thread is held for the operator
  (`yes_holds_a_consolidation_that_drew_on_an_outsider`). Accepted: the
  agent reports `drew_on` itself, so an injected prompt could leave an id
  out (see "What Ralphy does not defend against"). Accepted: triage
  attachments come from every comment
  (`crates/ralphy-core/src/github/attachments.rs`), within the host, format
  and size limits of ADR-0025.
- D6: checked for Claude. Every settings file the Claude adapter writes
  (execute, plan, and the triage, consolidate, diagnose and draft tasks)
  carries the guard's `PreToolUse` hook, and only execute adds
  `--cost-gate` (`every_settings_file_carries_the_guard`). The guard denies
  the `gh pr` write verbs and allows the read verbs and `gh api`
  (`bash_commands_are_refused_or_allowed_by_the_deny_list`); input it cannot
  read is a deny (`unreadable_input_is_a_deny`), and a real Claude Code
  payload is judged by the rules (`a_claude_code_payload_is_judged_by_the_rules`).
  No charter tells the agent to run a denied command
  (`charters_never_tell_the_agent_to_run_a_denied_command`). Opencode:
  checked. Every session carries a `permission.bash` map that denies
  `git push` and the `gh pr` write verbs
  (`every_session_denies_the_forge_writes_and_only_them`,
  `crates/ralphy-agent-opencode/src/command.rs`; the list is
  `DENIED_FORGE_WRITES` in `ralphy-adapter-support`). On a live run (opencode
  1.18.32, FinCal, 2026-10-01) opencode refused `gh pr create --help` and the
  issue still ran to the end (ADR-0005 D5 amendment). Codex: the installed
  CLI (0.159.2) has a mechanism, a `forbidden` exec-policy rule, measured to
  refuse `git push` under `-s danger-full-access`; it is not wired yet,
  because the rule file must sit in the target repo's `.codex/rules`.
  Copilot: `--deny-tool` is documented to win over `--allow-all-tools`, but
  no live run was possible (no active Copilot subscription); not wired
  until a live run, as ADR-0041 D7 amendment records. Kimi: accepted, no
  mechanism is known. The
  vendor rules match the start of the command text, so a global flag
  (`git -C x push`) or a wrapper (`bash -c`) gets past them: a layer, not
  proof. Accepted too: hook input is not authenticated (a
  process with `RALPHY_FLAG_FILE` set can write the stop flag) and the guard
  trusts the payload's `cwd`; both come from the operator zone.
- D7: checked. A new read of a secret-named environment variable is
  flagged by `xtask capabilities`. The run captures the events and Telegram
  tokens, then removes both from its environment before it spawns a child
  (`strip_secret_tokens_from_env_removes_both`); `triage`, `init` and
  `consolidate`, which read neither token, remove both before they start
  (`agent_commands_outside_run_strip_the_secret_tokens`). One owner-only writer,
  `crates/ralphy-daemon/src/owner_only.rs`, serves every store of the
  daemon and the CLI's events and Telegram stores: the file is created
  `0600` before a byte is written on Linux and macOS
  (`write_owner_only_creates_the_file_0600`, which reads the mode at
  creation), gets a protected DACL while still empty on Windows
  (`write_owner_only_is_protected`), and the store directory is `0700` on
  unix (`the_saved_store_is_owner_only`, `the_saved_config_is_owner_only`).
  Accepted: the peer store directory is shared across a WSL/Windows mount
  and keeps a plain create; the SSH key is written by `ssh-keygen` and
  restricted after. Accepted: the operator's forge and vendor credentials
  reach the agent.
- D8: not checked by code for the agent. Ralphy's own push has one call site
  (`crates/ralphy-core/src/sync.rs:390`), held by
  `spawn_sites_match_the_baseline`.
- D9: checked. A new URL host is flagged by `xtask capabilities`. The
  models.dev read is capped at `MAX_MODELS_DEV_BYTES`
  (`crates/ralphy-pricing/src/fetch.rs`); a larger body fails the fetch and
  leaves the cache unchanged (`an_oversized_body_fails_and_leaves_the_cache`).
  Ingest skips a row with a negative or non-finite price like a malformed
  row (`negative_price_rows_are_skipped`).
- D10: checked by `every_response_carries_the_security_headers` and
  `no_shell_carries_an_inline_event_handler`
  (`crates/ralphy-daemon/src/tests.rs`); a new `eval` or `new Function` is
  flagged by `xtask capabilities`. `'unsafe-eval'` stays in `script-src`
  for Alpine and Monaco (accepted). The context menu (`app.js`
  `renderMenu`) and the console key bar (`wb-console.js` `key()`) build
  their icons as elements and their text with `textContent`
  (`renderMenu sets a label as text, never as markup` in
  `ui-tests/app.test.mjs`, `no_menu_or_key_sink_takes_a_template_string`).
  Accepted: the `wb-viewer.js` templates interpolate only helper literals
  and the `ENCODINGS` constant. `connect-src` is `'self'` alone
  (`every_response_carries_the_security_headers`). Every workbench socket is
  built from `location.host`, so the daemon's own origin is the whole set,
  and no list of hosts or ports is computed: a computed list could not name
  a dev tunnel, which rewrites `Host`. Measured on 2026-10-01 with
  `tests/browser/security/wb_csp_connect.py` in Playwright Chromium, Firefox
  and WebKit, on loopback, through ngrok (a declared host) and through dev
  tunnels: the workbench socket opens (`'self'` covers a same-origin `ws:`
  and `wss:` in all three engines), and a socket to another host, or to the
  same host on another port, is refused. Accepted: notes
  rely on the link-scheme allowlist (`wb-notes.js:1569-1604`).
- D11: checked. One cap, `MAX_COMMAND_BYTES` in
  `crates/ralphy-daemon/src/tree.rs` (the base64 size of a 4 MiB image plus
  64 KiB, `max_command_bytes_is_derived_from_the_image_cap`), applies to
  `/ws/command`, every `/ws/session` upgrade (the peer relay included) and
  `/api/peer/command`. A larger command closes the socket
  (`a_message_over_the_cap_closes_the_socket`) or is answered `413`
  (`peer_command_refuses_a_body_over_the_cap`); a 4 MiB paste still passes
  (`a_4_mib_image_paste_still_replies`, `peer_command_takes_a_4_mib_image`).
  Measured before the fix: `/api/peer/command` answered `413` to a forwarded
  4 MiB `image.write` under axum's 2 MB default. Accepted: `/ws` and
  `/ws/tree` keep the tungstenite default; the browser sends them only
  small control frames.
- D12: checked by `.github/workflows/security.yml` (cargo-deny, gitleaks,
  zizmor, dependency review, osv-scanner) and `.github/workflows/codeql.yml`.
  `crates/ralphy-daemon/assets/ui/vendor/manifest.json` records the
  library, version, source and SHA-256 of every vendored file;
  `vendored_files_match_the_manifest` recomputes the hashes and fails on a
  file the manifest does not name. The `vendored-libraries` job turns the
  manifest into an npm lockfile (`xtask vendor-lock`) and runs
  `osv-scanner` over it on every trigger, the daily schedule included.
  Dependabot watches npm for `vendor-build/crepe`. `refresh-seed.yml` starts
  with `permissions: {}`, grants write on its job only, and has a timeout.
  The bootstrap-icons and devicon stylesheets differ from upstream in one
  place, the `@font-face` font list; the manifest says so. Accepted (review
  of 2026-10-01): the updater trusts a checksum from the same release as the
  archive.
- D13: checked by the compiler. The root `Cargo.toml` sets
  `unsafe_code = "deny"` in `[workspace.lints.rust]` and every member
  inherits it; each FFI function or module that has `unsafe` carries an
  `allow` whose `reason` names the FFI, and each `unsafe` block a
  `// SAFETY:` comment. A new `unsafe` is also flagged by
  `xtask capabilities`.
- D14: not checked by code: manual, reviewed in the PR.
