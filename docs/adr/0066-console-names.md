# Console names: a short name for each console, given by Ralphy and changed by the operator

Status: **accepted** (2026-09-27). Decided in the discussion of issue #478,
after a check of each decision against the code.

A console has no name. Its title is built as
`agent · checkout · repo · environment` (`wb-console.js`
`sessionPresentation`). Two Claude consoles on the same repo therefore have
the same title in the title bar, in the Go-to menu and in the "Open in a
column" list. With **columns** (ADR-0051, 2026-09-26 amendment) this became a
daily problem: the operator cannot tell which column is which. Fences have
names (ADR-0051 §6); consoles do not.

The code has three places that build a console's label today, and they do not
agree:

- `sessionPresentation` + `renderTitle` build the title bar.
- `index.html` builds the Go-to row inline (`agent · repo`).
- `WBColumns.rowLabel` builds the column list row, and prints a shared repo
  once in the group head.

Claude has a second, unrelated name. When the repo setting
`claude.console_name` is on, the daemon starts Claude with
`--name wb-<repo>-<4 hex>` (`session/spec.rs` `console_name`). That name is the
address that other Claude sessions use to send a message to this one. It has
no ADR before this one.

## Decision

### 1. A console name is a field of the window record

The **desk layout**'s window record gains an optional string
**`consoleName`** (`DeskRecord::console_name`). It is the name a person reads.
It is not an identity: the record `id` stays the key, and the daemon's session
id stays a volatile attribute (ADR-0050). `None` is not serialised, so an older
desk and an older shell keep their exact record shape (the #411 template).

The name belongs to the console, not to the session. A restart, a restart in
another worktree and a relaunch after a daemon restart all keep the same
record `id` (`relaunchIn`, `moveTo`, `reconcileDesk`), so they keep the name.
A live session that no record claims (`adopt`) gets a new record and so a new
default name.

### 2. Ralphy gives every console a default name

A new console is named `<prefix> #<n>`:

- `<prefix>` is the **last segment** of the repo slug (`owner/fincal` gives
  `fincal`), the same segment `console_name` already uses. A console with no
  repo (`~`) has the prefix `home`.
- `<n>` is the **lowest free number** among the console names on the desk
  that have the form `<prefix> #<n>`. With `#1`, `#3` and `#6` on the desk,
  the next console is `#2`.

The count is by prefix, across every kind of console (agent, plain shell,
shell with a start command) and across every environment. So `acme/api` and
`other/api` give `api #1` and `api #2`, and the same repo cloned on Windows
and on a WSL peer never gives two equal names. No counter is stored: the
number is computed from the desk the shell already holds.

The shell assigns the name when it creates the record. A record loaded without
a name (a desk written before this ADR) gets one on its first load. Two clients
that create a console at the same moment can both pick the same number. That
is accepted: it is rare, it only affects the text, and the operator can rename.

### 3. The operator can rename a console

Double-click on the name in the title bar opens it for editing. Enter saves.
Escape, or a press anywhere else, cancels. A single click does nothing, so a
click to raise, focus or drag never starts a rename. This is the fence rename
(`wb-console.js`, the `fence-name` input), copied with its measured fixes. Only
the name is editable; the label and the worktree button stay fixed.

- Leading and trailing spaces are removed. An empty name gets the default
  name again (§2), so a console never has no name.
- At most 40 characters. The input sets `maxlength`, and the daemon cuts a
  longer value when it stores the desk, because the input can be bypassed.
- Two consoles may have the same name. It is the operator's choice.
- Any character is allowed in the name. Only the Claude address (§6) is
  folded.

**A rename reaches other clients through the mirror.** The desk fold is per
record, and the newest `ts` wins (ADR-0050, amendment of 2026-09-20). Without
more, client B's next drag uploads its old name with a newer `ts` and removes
client A's rename. So `consoleName` is applied from the mirror onto a live
window, and its title is drawn again, in the same way as `locked`
(`applyLocksFromMirror`). Rects are still never applied from the mirror.

**In a detached fence the name is read-only.** The detached window writes to a
sink that stores nothing (`WBDeskSink.none()`), and when the fence returns, the
main window restores the snapshot taken at detach time. A rename there would be
lost.

### 4. One label, used everywhere

The label is `<consoleName> (<label>)`, where `<label>` is what the title shows
first today: the agent, the start command of a shell, or `console`.

```
fincal #1 (claude) · primary ▾
fincal #2 (console)
backend (codex) · wt-foo ▾
home #1 (console)
```

One function builds it, and every surface that names a console calls that
function: the title bar, the Go-to row and the column list row. The column
list no longer prints a shared repo in a group head, because every row now
names its repo.

`_deskAgent` is not the label. It is a matching key (`reconcileDesk`,
`wb-agents.js`, `consoleCommand`) and stays the agent or command it is today.

### 5. The environment leaves the title

The title no longer shows the environment (`Windows`, `WSL: Ubuntu-…`). The
operator knows it from the project, and §2 keeps two consoles of the same repo
on two environments apart by their number. The environment moves into the
title's tooltip, next to the full repo ref. No code reads the title text:
every match uses the ref, the session id, the agent and the checkout, so this
changes only what a person sees.

### 6. The Claude address comes from the console name

When `claude.console_name` is on, the daemon starts Claude with
`--name wb-<fold(consoleName)>` instead of `wb-<repo>-<4 hex>`. The shell sends
the console name as a `name` parameter on the launch request
(`/ws/session`), and the daemon relays it to a peer (`relay.rs`).

- **The fold is the only protection against the Windows command processor,
  and it must stay that way.** `portable-pty` builds the Windows command line
  with ArgvQuote rules. They add quotes for a space, a tab or `"`, and they do
  not escape `& | < > ^ %`. When `claude` is an npm `.cmd` shim, Windows runs
  that line through `cmd.exe`, which reads it again. So the raw name never
  enters argv. The daemon that owns the session folds it to `[a-z0-9-]` (each
  run of other characters becomes one `-`) before it builds the arguments. A
  test pins that `a&b %PATH% "x"` gives `wb-a-b-path-x`.
- The fold is the one `console_name` already has. It does not remove accents:
  `ação` gives `wb-a-o`. Removing accents needs a Unicode crate that the
  workspace does not have, and the fold that guards argv should stay small.
- A name that folds to nothing (only symbols or emoji) and a launch with no
  name (an older shell, or an older peer that ignores the parameter) keep the
  current `wb-<repo>-<4 hex>`.
- The name is applied **only at launch**. A rename does not restart the
  session. The next restart uses the new name. Until then, the tooltip shows
  the address Claude really has.
- Two consoles can fold to the same address (`Fincal #1` and `fincal-1`).
  The Claude roster already handles two rows with one name.
- The opt-in stays as it is, and off by default: changing the address changes
  what other sessions know this one by.

### 7. The terminal title is deferred, not rejected

A program in the terminal can set a title (OSC 0/2). Claude Code sets it to
the topic of the conversation, and xterm reports it with `onTitleChange`. It
answers a different question — *what is this console doing now* — and could be
shown later where there is no operator name. It is not part of this ADR
because it needs a measurement first: which of the seven vendors set it, and
whether a reload keeps it.

## Rejected alternatives

- **A name only when the operator gives one.** With many consoles the operator
  does not name them, and the problem stays. The default name solves it with
  no action.
- **A number that never repeats (`#7` after `#6`, even when `#2` is free).**
  It needs a counter per prefix in the desk, kept in step between clients and
  across restarts. The lowest free number needs no stored state.
- **One sequence per kind of console.** It gives `fincal #1 (claude)` and
  `fincal #1 (console)` at the same time, so "go to #1" has two answers.
- **The environment shown only when two titles would be equal.** It is
  conditional logic in the title for a case that §2 already removes.
- **Rename from the Go-to menu or a context menu.** Two ways to do one act.
- **A rename that restarts Claude with the new `--name`.** It would end a
  conversation to change a label.

## Consequences

- `DeskRecord` gains one optional field. The shell copies it in every place
  that builds a record field by field: `persistWin`, `deskOf`, `buildChrome`
  and `WBWindowState.FIELDS`. A copy that is missed drops the name.
- A rename can still be lost in the short time before the other client's next
  `GET`. This is the same last-write-wins that ADR-0051 §8 and the lock
  already accept.
- After a repo is re-keyed (ADR-0036), a default name keeps the old last
  segment. It is a name for a person, and the operator can rename it.
- The help text of `claude.console_name` (settings panel, `ralphy config`, the
  Claude settings struct) no longer describes a hex name.
- A separate issue covers what §6 found: the comment in `ralphy-proc-util`
  says `.cmd` arguments are escaped safely. That is true for
  `std::process::Command` and not for the PTY spawn path.
