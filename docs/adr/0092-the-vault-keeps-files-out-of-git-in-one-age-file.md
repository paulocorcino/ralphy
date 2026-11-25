# The vault keeps files that stay out of git in one age file in the checkout

Status: proposed
Kind: structural
Protects: recoverability, security

## Context

Some files of a project never go into git: a `.env`, a local config, a key
under `~/.ssh`. Some secrets of a project have no file at all: an API key,
the login of a service the project uses. When the disk is lost or the
checkout is cloned again, they are lost. The operator wants Ralphy to keep
them, protected by a password, in a file that is committed with the repo,
and wants agents to use them from the command line. For the secrets of a
project, this takes the place of a password manager such as Bitwarden.
Personal logins, browser autofill, mobile access and sharing with other
people stay out of scope.

The daemon already watches checkouts with OS events
(`crates/ralphy-daemon/src/watch.rs`), so it can see each change of such a
file. The repo's `.ralphy/` folder is gitignored and is read by the agents
that run in the checkout, so it can hold neither the vault nor its key. The
CLI reaches the daemon with the daemon token, which is stored owner-only
(`crates/ralphy-daemon/src/auth/token.rs`). The agent runs as the operator
and is not sandboxed (ADR-0072 D6). ADR-0072 D4 (write paths), D7 (secrets
Ralphy owns) and D8 (Ralphy never publishes on its own) apply to every rule
below.

## Decision

**D1. What the password protects.** The vault protects the copy that leaves
the host: the forge, a clone, a backup, a lost disk. It does not protect
against a process of the operator's own OS account on the host, and it does
not try to: such a process can already read the files and the daemon's
store. Another OS account on the same host can reach nothing of the vault.
The user docs state both limits.

**D2. One vault per checkout, in one file.** The **vault** is
`.ralphy.vault` at the root of the checkout. Ralphy writes it in the working
tree and never commits or pushes it: the operator commits it. When the vault
is created, the workbench offers to add `.ralphy.vault binary` to the
checkout's `.gitattributes`; the operator confirms, and Ralphy writes the
line in the working tree. The workbench warns when the line is missing.

**D3. A vault entry is a file on the daemon's host.** Its path is relative
to the checkout when the file is inside it, and absolute when it is outside.
Restore writes to that path or to another path on the same host. No rule
moves an entry to another machine. A checkout on a **peer** has its vault
kept by the peer's daemon. Reaching a peer's vault through the workbench is
not decided yet; no route does it today.

**D4. A vault secret is a named record with no file.** It holds one or more
fields that the operator names. A field is text (shown) or hidden (masked
until the operator reveals or copies it). There are no templates and no
TOTP fields: a second factor kept behind the same password stops being a
second factor.

**D5. The format is binary age, in two layers.** The vault holds its own age
X25519 identity, encrypted to the password with the age scrypt recipient at
age's default work factor. The body (entries, secrets, versions, paths,
names and dates) is encrypted to that identity. Only a header with the
format version is plain text. The identity's recipient never leaves the
daemon, so nobody outside the host can write a body the daemon opens. A
password has at least 15 characters and no composition rule. There is no
recovery key: a lost password loses the content.

**D6. The open vault's key lives in the daemon's store.** When the operator
gives the password, the daemon keeps the vault identity owner-only in the
daemon's store (`~/.ralphy`), by checkout, under ADR-0072 D7. It never
stores the password, and never writes the identity to the checkout or to the
repo's `.ralphy/`. A vault is open on a host while its identity is in that
host's store; a daemon restart does not close it.

**D7. Remembering the password does not close the vault.** Every 14 days
the workbench asks for the password as a memory check, as an item of the
attention list. A wrong answer warns the operator and offers a password
change; the vault stays open in both cases. A password change re-encrypts
only the identity. Earlier commits of the vault stay readable with the old
password; Ralphy offers no re-key and no rotation advice, and the user docs
say so.

**D8. Limits.** One entry is at most 2 MB; a bigger file keeps its last good
version and is marked as over the limit. The whole vault is at most 20 MB.
Each entry and each field keeps its current value and 10 earlier versions.
When a new version does not fit, the oldest earlier versions of the whole
vault are dropped until it fits, and the modal says how many. A current
value is never dropped: when the current values alone pass 20 MB, the write
fails, the entry is marked, and the attention list shows that the vault is
full.

**D9. Capture.** The daemon writes a new version when the content hash of a
watched entry changes, after a short debounce. A deleted or renamed file
keeps its entry, marked as missing. Changes made while the vault is closed or
the daemon is stopped are captured when it opens, by hash. Removing an entry
never touches the file on disk.

**D10. Who reaches the vault.** The operator's browser session reaches every
vault action. The CLI on the daemon's own host, with the daemon token,
reaches `list`, `show` (text fields only), `run`, `set` and `add`. Every
vault route refuses the console token, a peer token and the events token.
Removing a secret, a field or an entry, and restoring an entry, are done
only in the workbench. There is no MCP tool for the vault. The CLI never asks
for the password: on a host where the vault is not open, every vault command
fails and says to open the vault in the workbench.

**D11. An agent uses a hidden field only through `run`.**
`ralphy vault run --secret <name>… -- <command>` runs the command with the
fields of the named secrets in its environment. At least one `--secret` is
required. The variable name is `<SECRET>_<FIELD>` in upper case, with every
character that is not a letter or a digit replaced by `_`, unless the
operator set a name on the field. Two fields with one name fail the command
before it runs. A variable already in the environment is replaced, with a
warning on stderr that names it. The value of each hidden field is replaced
by `***` in the command's stdout and stderr. The masking stops an accident,
not an agent that encodes the value or writes it to a file. No command
prints a hidden value.

**D12. The CLI uses the vault of its checkout.** `run`, `show`, `set` and
`add` act on the vault of the checkout that holds the current directory.
There is no flag to name another checkout. `set` reads the value from stdin
only, never from an argument.

**D13. Every use is recorded.** Each `run` and `set` records the secret, the
program and its arguments (never the environment), the time and the origin
(a terminal, or the console and run when Ralphy knows them). The record
lives in the daemon's store, not in the vault, and the modal shows it.

**D14. No content leaves the vault in clear text except to the operator and
to a `run` command.** No event, log line, error or record of use carries the
value of an entry or of a hidden field; they carry only a path or a name and
a size.

**D15. Restore.** Restore shows its target and asks for confirmation; it
overwrites an existing file only after a second confirmation. It writes the
file owner-only (`0600`, or a protected DACL on Windows, by the routine of
ADR-0072 D7), and sets the execute bit only when the captured file had it.
A binary entry shows its size, hash and dates, with restore and download,
and no compare.

**D16. A merge conflict picks one whole vault.** When git reports
`.ralphy.vault` as unmerged, or it does not decrypt, the workbench asks the
operator to pick one side. There is no merge by entry. After the pick, the
daemon compares each entry with the file on this host by hash and captures a
new version where they differ. It keeps the other side owner-only in its
store, and the modal lists what only that side holds, with a copy into the
vault one item at a time, until the operator discards it. When the picked
side has another identity, the vault counts as closed on this host until the
operator gives the password; the old identity is then deleted.

## Consequences

- Each change of an entry or a secret rewrites the whole file, so every
  commit of the vault adds a new blob. D8 bounds this.
- The vault file is open to an offline password search, without limit and
  forever, including its old commits. Only scrypt and the password length
  (D5) stand against it; a hosted password manager can limit attempts, the
  vault cannot.
- A new outside dependency: the `age` crate. It is a vendored library under
  ADR-0072.
- The watcher now watches single files outside any checkout.
- The CLI gets a `vault` command group (D10–D12).
- The workbench gets a vault button in the FILES bar, a vault modal with a
  read-only Monaco view, and an "Add to vault" item in the tree menu. Each
  gets its entry in docs/DESIGN-SYSTEM.md first.

## Considered options

- **Our own format with Argon2id and XChaCha20-Poly1305.** Rejected: more
  code of our own to audit, for a format that must be read for as long as a
  repo keeps the file. age is specified and audited.
- **Armored (text) age.** Rejected: a merge writes conflict markers into the
  base64 and the file stops opening, and the file is a third bigger.
- **The key in the repo's `.ralphy/`.** Rejected: agents in the checkout
  read that folder by default, and it would travel with copies of the
  checkout.
- **Ralphy commits the vault on each change.** Rejected: it breaks
  ADR-0072 D8, and it would fill the history with one commit per save.
- **The reminder closes the vault.** Rejected: a closed vault stops
  capturing changes, which is the loss the vault exists to prevent.
- **`vault get` prints a hidden value for the agent.** Rejected: a value in
  the agent's context goes to the vendor and to the transcript.
- **A per-secret switch "agents may use".** Rejected: a process of the
  operator's account already reaches the host's data (D1), so the switch
  would add a step and protect nothing.
- **Approval of each use in the attention list.** Rejected: it stops
  unattended runs.
- **A re-key and a list of exposed secrets after a leaked password.**
  Rejected: the old commits stay readable whatever Ralphy does, and only the
  operator knows what to rotate at each service.
- **A password strength meter.** Rejected: a vendored library (zxcvbn) for
  little gain over the 15-character minimum.
- **An MCP tool.** Rejected: its result goes straight into the agent's
  context, and the CLI already reaches every vendor with a shell.

## Compliance

- D2: not checked by code: manual, reviewed in the PR (no git spawn in the
  vault module).
- D5: checked by a format test that decrypts a committed fixture vault, and
  a test that refuses a 14-character password (to be added with the first
  slice).
- D6: not checked by code: reviewed in the PR.
- D8: checked by a test that fills a vault past 20 MB and finds every
  current value (to be added with the slice of entries).
- D10: checked by a test that calls each vault route with the console,
  peer and events tokens and gets a refusal (to be added with the first
  slice); a new route is reviewed in the PR (as ADR-0072 D2).
- D11, D14: checked by a test that runs a helper child which prints its
  environment through `run`, and finds no hidden value in its output, in
  the logs or in the events (to be added with the slice of secrets).
- D15: checked by a test that restores an entry and reads its mode or DACL
  (to be added with the slice of restore).
- D16: not checked by code: reviewed in the PR.
