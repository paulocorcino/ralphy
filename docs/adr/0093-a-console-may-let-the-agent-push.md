# In a console, the operator may let the agent switch branches, push and write pull requests

Status: proposed
Kind: feature
Protects: operability; constraint security

Amends [ADR-0046](./0046-orchestrator-side-push.md) §1,
[ADR-0086](./0086-antigravity-adapter.md) D9 and
[ADR-0072](./0072-the-security-model.md) D6. Issue #1020.

## Context

The guard (`ralphy hook guard`, `crates/ralphy-cli/src/guard.rs`) denies
one list of commands in every session it runs in. Four of its rules exist
because a run has an orchestrator that owns the branch and the push
(ADR-0046 §1): `git push`, `git checkout|switch`, `git worktree`, and
`gh pr` with a verb that writes.

An interactive console has no orchestrator. The operator drives the agent
and asks it for the work. Today only an Antigravity console runs the guard.
There is one owned root per OS user, and runs and consoles share its
`config/hooks.json` (ADR-0086 D8, D9). Claude and Codex consoles carry only
the status hooks (`crates/ralphy-daemon/src/session/spec.rs`), so they
already allow these commands.

In an `agy` console, the agent stops when the operator asks for a branch, a
push or a pull request. Each workaround is worse than the rule. Editing the
hooks file removes the guard from runs too, and the next prepare of the root
writes the file again. A global `deny` in `agy` wins over a repository
hooks file. Starting `agy` outside the workbench loses the console.

## Decision

**D1. The guard has two groups of rules.** The *workflow rules* are
`git push`, `git checkout|switch`, `git worktree`, and `gh pr` with a verb
that writes. Every other rule is a *safety rule*: history rewrite,
`reset --hard`, `clean`, a recursive delete outside the worktree, a write
to Ralphy's tool directory, a download piped into a shell, disk commands,
`gh release|repo|workflow|secret|auth` and `cargo publish`. A new rule is a
safety rule unless this ADR is amended.

**D2. The setting is local-only, per repo, and off by default.** The key is
`antigravity.console_agent_workflow_i_understand_the_risk`, because only an
Antigravity console runs the guard. A key in a vendor-neutral section waits
for a second vendor. The key sends data off the machine (a push), so by
ADR-0072 D1 it is in `LOCAL_ONLY_KEYS`
(`crates/ralphy-daemon/src/dispatch/argv/config.rs`): only
`ralphy config set` on this machine changes it. The daemon reads it when it
spawns a console. A run never reads it.

**D3. Ralphy tells the guard the kind of session; the agent cannot.** When
the setting is on, the daemon adds `RALPHY_GUARD_CONSOLE=1` to the
environment of a console it spawns. The guard skips the workflow rules only
when this variable is `1`. The hook is a child of the vendor CLI and
inherits the environment the CLI was spawned with. A tool call changes the
environment of its own children only, so it cannot change what the hook
reads. This is a claim to measure before the status becomes accepted: in a
console with the setting off, `export RALPHY_GUARD_CONSOLE=1; git push` is
still denied.

**D4. A run never inherits the console's permission.** Every `ralphy`
command except `hook guard` removes `RALPHY_GUARD_CONSOLE` from its own
environment when it starts, before any thread. An agent in a console that
starts `ralphy run` therefore starts a run with every rule, whatever the
vendor of the run.

**D5. The console says so.** When the setting is on, the console shows one
line when it starts: "The agent may switch branches, push and write pull
requests in this console." With the setting off, it shows nothing new. The
daemon writes this line, as dim text, into the console's kept output before
the agent starts, so every attach shows it first. Only an Antigravity console
gets it.

**D6. No.** This ADR does not turn the guard off, does not change a run, and
does not move a safety rule. It does not add the guard to Claude or Codex
consoles.

## Consequences

- ADR-0046 §1 stays true for runs. Its rule now reads: the agent of a *run*
  cannot push.
- ADR-0086 D9 keeps one hooks file. The file does not change; the session
  kind comes from the environment, not from the hook command.
- ADR-0072 D6 gains one setting that lowers the posture of a console. It is
  local-only, by D1 of that ADR.
- The key lives in `.ralphy/settings.json`, which the agent can write, as it
  can write any file of the operator (ADR-0072 D6). An agent that turns the
  key on changes the next console the operator opens, not its own session.
  This is the same exposure as every other local-only key in that file.

## Considered options

- **The session kind in the hook command** (`hook guard --session console`).
  This works where Ralphy writes one settings file per session (Claude). It
  cannot work for `agy`, whose one hooks file serves runs and consoles.
- **A daemon record of console session ids, checked by the guard.** This is
  stronger than the environment, but the guard would read daemon state on
  every tool call. The environment is enough while D3's measure holds.
- **A setting in the Security dialog.** One click would let the agent push.
  ADR-0072 D1 asks for a deliberate gesture for a key that sends data off
  the machine.

## Compliance

- D1: checked by `crates/ralphy-cli/src/guard/rules.rs`
  (`the_workflow_rules_are_the_four_the_adr_names`).
- D2: checked by `crates/ralphy-daemon/src/dispatch/argv/tests/git.rs`, which
  refuses every `LOCAL_ONLY_KEYS` entry at the daemon.
- D3: not checked by code: needs a live vendor CLI; measured once and
  written here.
- D4: checked by `crates/ralphy-cli/src/main.rs`
  (`only_the_guard_hook_keeps_the_console_mark`).
- D5: checked by `crates/ralphy-daemon/src/session/spec/tests/console_workflow.rs`
  (`a_console_with_the_setting_on_carries_the_notice`) and
  `crates/ralphy-daemon/src/session/manager/tests/start_notice.rs`
  (`the_notice_is_the_first_thing_an_attach_replays`).
