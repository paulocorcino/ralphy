# The vault keeps files that stay out of git in one age file in the checkout

Status: proposed
Kind: structural
Protects: recoverability, security

## Context

Some files of a project never go into git: a `.env`, a local config, a key
under `~/.ssh`. When the disk is lost or the checkout is cloned again, they
are lost. The operator wants Ralphy to keep a copy of each one, protected by
a password, in a file that is committed with the repo.

The daemon already watches checkouts with OS events
(`crates/ralphy-daemon/src/watch.rs`), so it can see each change of such a
file. The repo's `.ralphy/` folder is gitignored and is read by the agents
that run in the checkout, so it can hold neither the vault nor its key.
ADR-0072 D4 (write paths), D7 (secrets Ralphy owns) and D8 (Ralphy never
publishes on its own) apply to every rule below.

## Decision

**D1. One vault per checkout, in one file.** The **vault** is
`.ralphy.vault` at the root of the checkout. Ralphy writes it in the working
tree and never commits or pushes it: the operator commits it.

**D2. A vault entry is a file on the daemon's host.** Its path is relative
to the checkout when the file is inside it, and absolute when it is outside.
Restore writes to that path or to another path on the same host. No rule
moves an entry to another machine. A checkout on a **peer** has its vault
kept by the peer's daemon.

**D3. A vault secret has no file.** It is a name and a value the operator
creates in the workbench. The operator reads and copies it. No console,
run, agent or child process receives a vault secret.

**D4. The format is age, in two layers.** The vault holds its own age X25519
identity, encrypted to the password with the age scrypt recipient. The body
(entries, secrets, versions, paths, names and dates) is encrypted to that
identity. Only a header with the format version is plain text. A password
change re-encrypts only the identity. There is no recovery key: a lost
password loses the content.

**D5. The open vault's key lives in the daemon's store.** When the operator
gives the password, the daemon keeps the vault identity owner-only in the
daemon's store (`~/.ralphy`), by checkout, under D7. It never stores the
password, and never writes the identity to the checkout or to the repo's
`.ralphy/`.

**D6. Remembering the password does not close the vault.** Every 14 days
the workbench asks for the password as a memory check, as an item of the
attention list. A wrong answer warns the operator and offers a password
change; the vault stays open in both cases.

**D7. Limits.** One entry is at most 2 MB; a bigger file keeps its last good
version and is marked as over the limit. The whole vault is at most 20 MB.
Each entry and secret keeps its current value and a few earlier versions;
older history is in the operator's commits.

**D8. Capture.** The daemon writes a new version when the content hash of a
watched entry changes, after a short debounce. A deleted or renamed file
keeps its entry, marked as missing. Changes made while the vault is closed or
the daemon is stopped are captured when it opens, by hash. Removing an entry
never touches the file on disk.

**D9. Only the operator's session reaches the vault.** Every vault route
accepts the operator's browser session and refuses the console token, a
peer token and the events token. There is no CLI command and no MCP tool for
the vault. Restore shows its target and asks for confirmation; it overwrites
an existing file only after a second confirmation.

**D10. No content leaves the vault in clear text except to the operator.**
No event, log line or error carries the content of an entry or the value of
a secret; they carry only a path or a name and a size.

**D11. A merge conflict picks one whole vault.** When `.ralphy.vault` holds
conflict markers or does not decrypt, the workbench asks the operator to
pick one side. There is no merge by entry.

## Consequences

- Each change of an entry rewrites the whole file, so every commit of the
  vault adds a new blob. D7 bounds this.
- A new outside dependency: the `age` crate. It is a vendored library under
  ADR-0072.
- The watcher now watches single files outside any checkout.
- The workbench gets a vault button in the FILES bar, a vault modal with a
  read-only Monaco view, and an "Add to vault" item in the tree menu. Each
  gets its entry in docs/DESIGN-SYSTEM.md first.

## Considered options

- **Our own format with Argon2id and XChaCha20-Poly1305.** Rejected: more
  code of our own to audit, for a format that must be read for as long as a
  repo keeps the file. age is specified and audited.
- **The key in the repo's `.ralphy/`.** Rejected: agents in the checkout
  read that folder, so any agent could open the vault.
- **Ralphy commits the vault on each change.** Rejected: it breaks
  ADR-0072 D8, and it would fill the history with one commit per save.
- **The reminder closes the vault.** Rejected: a closed vault stops
  capturing changes, which is the loss the vault exists to prevent.

## Compliance

- D1: not checked by code: manual, reviewed in the PR (no git spawn in the
  vault module).
- D3: not checked by code: reviewed in the PR; the child environment rules
  of ADR-0072 D7 do not see vault secrets.
- D4: checked by a format test that decrypts a committed fixture vault
  (to be added with the first slice).
- D5: not checked by code: reviewed in the PR.
- D9: not checked by code: a new route is reviewed in the PR (as ADR-0072
  D2).
- D10: not checked by code: reviewed in the PR.
