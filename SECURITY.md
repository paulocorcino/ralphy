# Security

## Report a vulnerability

Do not open a public issue for a vulnerability. Report it privately on
GitHub: **Security → Report a vulnerability** in this repository.

Include the version (`ralphy --version`), the platform, and the steps that
show the problem. You get an answer on the report. A confirmed issue is fixed
in a new release, and the release notes name the fix. Tell us if you want to
be credited.

## Supported versions

Ralphy is before 1.0. Only the latest release gets security fixes. Update
with `ralphy update` or with **Update now** in the workbench.

## What Ralphy is, from a security view

Ralphy runs AI coding agents on your machine, as your user. It reads issues
from GitHub, gives them to an agent, and checks the result. The optional
daemon serves a workbench in your browser, and that workbench can open a
shell on your machine. Read the rest of this page before you expose the
daemon beyond your own computer or run Ralphy on a public repository.

## What Ralphy protects

**The daemon.**

- It listens on `127.0.0.1` by default. A bind to any other address does not
  start without a token.
- Login is available with a TOTP code, and with an optional password.
  Sessions expire. Logout and every change to the security settings end
  every open session.
- Turning off a security setting of the daemon (such as "Require login")
  needs a fresh TOTP code once one is set up.
- Every request is checked for its Host and Origin, so another web page
  cannot drive the daemon from your browser.
- The workbench can only call a fixed list of actions. Child processes get
  their arguments directly, never through a shell. Files are written only
  inside a registered project, and never inside `.git` or `.ralphy`.
- The pages carry a strict Content Security Policy. Rendered markdown is
  sanitized. Remote images are off until you turn them on.

**The agent's input.**

- An issue runs only when it carries the queue label, and only a user with
  triage rights can add a label.
- In a run, only comments by the repository's owners, members and
  collaborators reach the agent. Comments by anyone else are dropped and
  named in the run log. Triage still reads the whole issue thread, every
  author included, but each comment by someone who is not an owner, member
  or collaborator is marked as such. `ralphy triage --yes` does not publish
  a spec built on such a comment: it leaves the issue for you to review.
- The agent's instructions say that a comment is information, never a
  command.

**What the agent may do.**

- Ralphy never pushes a branch or opens a pull request on its own. It pushes
  only when you run `ralphy sync push`.
- Every Claude Code session that Ralphy starts carries a guard that blocks
  pushes, writes to pull requests, merges, destructive git commands, and
  writes to secret files. Every OpenCode and GitHub Copilot session that
  Ralphy starts blocks pushes and writes to pull requests through the
  vendor's own deny rules. Other
  agents run with their own vendor's controls, which differ by vendor.
- A task counts as done only when Ralphy has run its checks itself.

**Ralphy's own secrets.** The daemon token, the two-factor seed, the peer
tokens, and the event sink and Telegram tokens are stored in files only your
user can read, on every platform. The daemon token, the event sink token and
the Telegram token are removed from the environment of the processes Ralphy
starts, and the event sink token is masked in `ralphy config get`.

**Third-party code.** Each browser library the workbench embeds has its
version and hashes recorded, and a daily scan checks those versions against
the known advisories.

**Releases.** Every release archive has a SHA-256 file and a build
provenance attestation. You can check an archive with
`gh attestation verify <archive> --repo paulocorcino/ralphy`. `ralphy update`
checks the SHA-256 before it replaces the binary.

## What you must do

- **On a machine other people can use, turn on "Require login"** (account
  menu → Security, after you set up two-factor login). Without it, every
  process on the machine can reach the daemon, and the daemon can open a
  shell as you.
- **To reach the daemon from another machine, put TLS in front of it** (a
  tunnel such as ngrok or dev tunnels, or an SSH tunnel), and turn on login.
  The daemon does not do TLS itself.
- **Do not turn on `queue.trust_all_comments` on a public repository.** It
  lets anyone with a GitHub account write into what the agent reads.
- **Treat a settings key that ends in `_i_understand_the_risk` as what it
  says.** Each one turns off a protection on purpose.

## What Ralphy does not protect against

- **The agent itself.** The agent runs as your user, with your credentials
  for GitHub and for its vendor. The guard reduces mistakes. It is not a
  sandbox. Run Ralphy in a VM or a container if you need one.
- **Prompt injection, completely.** The comment filter and the agent's
  instructions are layers. No filter can prove that a model ignores every
  hostile sentence in an issue.
- **Other users on the same machine**, while "Require login" is off.
- **Attacks from the internet on the daemon.** It is not built to face the
  internet directly.
- **A compromised GitHub account, vendor account, or vendor service.**
- **Encryption at rest.** Ralphy's files are plain files, protected by your
  OS account.
