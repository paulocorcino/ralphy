# Peers on other machines: the local fleet through an SSH tunnel

Status: **accepted** (2026-09-29). Decided in a design discussion, after a
check of each decision against the code and a web search for the credential
facts, then validated the same day by a spike with no code change, against a
Linux host and a macOS host (see "Spike results"). §13 lists what is still open
for the implementation. Amended 2026-09-29: the add flow can install Ralphy on
the host, after the operator allows it (see "Amendment").

Extended by ADR-0070 (proposed): an unreadable peer store is a failure, never "no peers" (D4); peers' state is read again on the ADR-0070 D2 events.

An operator has computers other than the one that serves the workbench: a VPS
reached over SSH, an old MacBook on the home network, maybe a second Windows
computer. They want the repos on those computers in the same workbench, the
same way the WSL repos appear today. They also want work on those computers to
continue when the local computer is off, and the workbench to take it up again
when the local computer comes back.

[ADR-0052](0052-local-fleet-federation.md) already does almost all of this for
WSL. Every operation runs on the daemon that owns the repo; the local daemon
only asks and proxies. Peer-owned PTY sessions survive a proxy that goes away
("proxy teardown is detach-only"), and a command socket that closes never stops
a run (`routes/ws_command.rs`). Autostart already exists for Windows, Linux and
macOS (`autostart.rs`), and the release builds `x86_64-apple-darwin`.

What ties ADR-0052 to one machine is small and in known places:

- The peer client dials loopback only (`peer/client.rs` `classify_address`),
  over plain HTTP.
- A peer announces itself by writing a descriptor into the local daemon's store
  through the file system (`/mnt/c`). Another computer cannot do that.
- The descriptor carries the port the peer bound. Through a tunnel, the port to
  dial on this machine can be a different one.
- The nudge and the keepalive are `wsl.exe`.
- A free console on a peer is spawned locally with `wsl.exe`; a peer with no
  distro gets a 502 (`routes/ws_session.rs`).

## Decision

### 1. "Local" names the path, not the machine

A daemon on another machine is a **peer**, the same kind as a WSL peer. The
local fleet is every daemon the local daemon reaches over its own loopback.
WSL's `localhostForwarding` relay is one way to reach it; a **peer tunnel** is
the other. The peer client, the protocol, the composite `(daemon_id, slug)`
key and every federated surface stay as they are. There is no "remote peer"
type in the code or in the UI.

### 2. The transport is an SSH local forward, held by the daemon

The local daemon holds one `ssh -N -L 127.0.0.1:<local port>:127.0.0.1:<peer
port>` process per peer on another machine. The peer's daemon then answers on
this machine's loopback, and the loopback gate of `peer/client.rs` passes with
no change. SSH encrypts the traffic and authenticates the host; both daemons
stay bound to `127.0.0.1`.

The tunnel uses the system `ssh` (OpenSSH, in Windows since 10 1809, and native
on Linux and macOS) and the operator's `~/.ssh/config`, so a jump host, a proxy
or an alias works with no Ralphy code. It always runs with `BatchMode=yes`,
`ExitOnForwardFailure=yes`, `ServerAliveInterval=15` and
`ServerAliveCountMax=3`. Without the last two, the OpenSSH defaults can leave a
tunnel that a VPN drop has broken open for hours, while its local port still
accepts connections that never answer.

The tunnel is held the way the **keepalive** is held (ADR-0052, 2026-09-21
amendment): started detached, never waited on, never signalled, and replaced
when it has exited. It is ensured at daemon start, by every nudge, and after a
probe fails. Holding a handle is not supervision: the daemon on the other
machine belongs to that machine's own service manager.

Because the daemon knows whether its `ssh` process is alive, the diagnosis
separates two states that would otherwise both read "unreachable": the tunnel
is closed (reconnecting), and the tunnel is open but the daemon on the host
does not answer.

The peer daemon keeps its default port. Only the local end of the tunnel needs
a port of its own, different from the local daemon's port (the self-dial gate
of `peer/client.rs` still applies). The local daemon chooses it and records it.

### 3. Authentication: a peer key, and a password used once

Each local machine has one **peer key**, an ed25519 key that the daemon
generates in `~/.ralphy/ssh/`, owner-only. Every tunnel from that machine uses
it, unless the operator's `~/.ssh/config` or agent already signs in to the
host.

When the operator chooses password sign-in in the add dialog, the daemon uses
the password once, in memory, to install the peer key on the host, and then
discards it. **Ralphy never stores a password or a key passphrase.** A tunnel
must reopen with no person present (after a reboot, after a VPN drop), and a
stored password would add a secret store with three back ends (Windows
Credential Manager, macOS Keychain, Secret Service), and those fail on a
headless Linux host. On macOS the Keychain also asks for permission again after
an update of an unsigned binary. A dedicated key is also the smaller secret:
it can be limited in `authorized_keys` to the port forward alone, and it is
revoked by deleting one line, with no change to the operator's password.

If a prompt the dialog cannot handle appears (a second factor, an unusual
prompt), the dialog opens a workbench console that runs `ssh` so the operator
answers it in a real terminal.

On first contact, the dialog shows the host key fingerprint and asks the
operator to confirm it; Ralphy then writes it to `known_hosts`. With
`BatchMode=yes` an unknown host is refused, so this step is required.

A key that has a passphrase and is not in an agent cannot reopen a tunnel
alone. The dialog says so and offers the peer key.

On a Windows host, the key of a user in the Administrators group goes to
`C:\ProgramData\ssh\administrators_authorized_keys`, not to
`~\.ssh\authorized_keys` (the default `sshd_config`). The remote shell there is
`cmd.exe`. The add flow handles both; the operator does not need to know them.

### 4. Adding a host is the pairing

The SSH sign-in of the add flow is the proof that the operator owns the host.
Over that same session, the local daemon reads the peer's descriptor (identity,
port, token, protocol version) and writes a local descriptor for it, with a
tunnel section: the SSH destination, the peer port and the local port. Nothing
is written into the local store from the other machine.

There is no pairing code. To run a command on the host, the operator needs
SSH access to it anyway, so a code proves nothing more, and exchanging it needs
an endpoint that accepts requests with no credential. **Enrollment** with a
one-time code stays the control plane's mechanism, for a host that has no SSH
path.

### 5. A peer behind a tunnel requires its token, even on loopback

Through a tunnel, the peer daemon sees each connection come from `127.0.0.1`
(it comes from `sshd`). Under `AuthPolicy::Localhost` it would authorize every
process on the host with no credential: another user, a container on the host
network, a web application with a request forgery bug. For a WSL distro on the
operator's own computer ADR-0052 accepted this; for a VPS it is not
acceptable.

This hole exists only inside the host. From the network, a daemon on its
default bind cannot be reached at all: it listens on `127.0.0.1`, and the only
open port on the host is the SSH server's. A daemon bound to a network address
refuses to start without a token (`AuthPolicy::for_bind`), but it speaks plain
HTTP, so on a shared network its token travels in clear text; that is why the
tunnel, not a network bind, is the transport.

The add flow therefore sets a marker on the host, beside `daemon-require-login`
(for example `daemon-require-token`), and with it a loopback bind uses
`AuthPolicy::Bearer`. With the marker and no token, the daemon fails closed.
The local daemon already sends the token from the descriptor. The token file is
owner-only, and on a real Linux or macOS file system that mode works, unlike on
`/mnt/c`.

The cost: a browser on that host itself gets a 401 unless the operator turns
on sign-in there too. For a headless VPS this costs nothing.

### 6. The name is the host daemon's name

The group header in the tree is `<name>: <OS>` (`vps-hetzner: Linux`), next to
`WSL: <distro>`. The name is the one the host daemon announces
(`ralphy daemon setup` on that host). The local workbench never gives a peer a
second name. If the host daemon has no name, the add flow asks for one and sets
it on the host, over SSH. A name that another daemon in the fleet already uses
is refused, and the operator renames it on the host.

### 7. A free console on such a peer runs on the peer

A peer with no WSL distro hosts its own free consoles, through the peer-owned
session path that agent sessions already use. The console then continues when
the local computer is off, and the operator attaches to it again later. A
local `ssh -t` console was rejected: it would die with the local computer, and
it would put a process between the terminal and the shell for resize and
signals, the problem ADR-0052 §7 names for `wsl.exe`. The WSL free console
keeps its local `wsl.exe` form.

### 8. A host starts Ralphy only where the agents can sign in

A daemon that runs but whose agent CLIs cannot sign in is worse than one that
says it waits for a sign-in. The facts come from vendor issue trackers and the
platform documentation; the macOS facts were then measured on a real host
(see "Spike results"), the Windows ones were not:

- **Linux:** the systemd user unit with lingering starts at boot. Vendor
  credentials are files. No sign-in is needed.
- **macOS:** Claude Code and `gh` keep their tokens in the login Keychain,
  which is locked outside a signed-in GUI session: over SSH, Claude fails with
  `errSecInteractionNotAllowed`, and a LaunchDaemon cannot read it even with
  `UserName`. The existing LaunchAgent in `gui/<uid>` is the right form; the
  host needs automatic sign-in. When FileVault is on, macOS does not offer
  automatic sign-in, and the daemon starts after the operator signs in. The
  only path with no sign-in is a per-vendor token (`claude setup-token`, about
  one year, with no refresh; `GH_TOKEN`), which Ralphy shows but does not
  manage. A MacBook also sleeps when idle or when the lid is closed, and a
  sleeping Mac is unreachable: the host checks tell the operator to prevent
  sleep (Energy settings or `pmset`). The release's macOS binaries are built
  with `MACOSX_DEPLOYMENT_TARGET=12.0`, so macOS 12 Monterey is the oldest
  supported host.
- **Windows:** Claude Code keeps its token in a file, but `gh` uses Credential
  Manager, which needs a sign-in with a password (a key-based SSH sign-in is
  S4U and has no DPAPI). The daemon's own start is what matters, not the SSH
  session. The host needs automatic sign-in (Sysinternals Autologon).

The host checks of the add flow detect each case and say what the operator
must do. A start-at-boot mode (LaunchDaemon, a scheduled task with a stored
password) is not in scope.

### 9. Remove and revoke

Removing a host deletes the peer key's line in the host's `authorized_keys`
over SSH, and forgets the local descriptor. The host daemon continues with its
repos, runs and sessions. When the host does not answer, the workbench says the
key line remains and shows it. Changing the host daemon's token disconnects
every computer connected to that host, so it is a separate, explicit action,
never a side effect of removal.

### 10. Limits

- **One hop.** `POST /api/peer/command` never re-routes to another peer
  (ADR-0052), so the WSL distros of a remote Windows host do not appear.
- **Upgrades are not automatic on the other machine.** A protocol version
  mismatch is diagnosed as it is today, and the operator can install the local
  version from the add flow (Amendment 2026-09-29); the automatic peer update
  after a workbench update covers WSL peers only.
- **Text in a console is capped.** A session keeps 256 KiB of output
  (`session/manager.rs`); after a long disconnection, the operator sees only
  the end.
- **Credentials are per host, permanently** (ADR-0052 Consequences), now for
  more hosts.
- **Installing Ralphy on the host needs the operator's permission.** The add
  flow checks the version. It installs Ralphy only when the operator selects
  *Install Ralphy on the host*, or runs `ralphy host install` (Amendment
  2026-09-29).

### 11. The workbench: adding, showing and removing a host

**The entry point.** An **Add a host** button in the PROJECTS header, beside
the refresh button: the Lucide `server` icon with a small plus, composed by
hand as the "plugged" glyph already is (Lucide v0.460.0 has `server`,
`server-cog`, `server-off`, but no `server-plus`). It is in the header, not a
row at the end of the list, because the header is the place of the list's
actions, stays visible when the list is long, and exists when the fleet has one
daemon and no group headers are drawn.

**The group header.** A tunnel peer's group shows the `server` icon, then
`<name>: <OS>`. Its state uses the plug and unplug glyphs that exist today; no
new state glyph.

**The add dialog** has four steps:

1. **Connection.**
   - *Host*: a list of the `Host` entries of `~/.ssh/config`, resolved with
     `ssh -G <alias>`, or an address typed by the operator.
   - *User* and *port*: filled from `ssh -G` when the host is an alias.
   - *Sign in with*: the key or agent of the SSH config; a key file; or a
     password, used once to install the peer key.
   - There is no *name* field (§6), and no jump host or proxy field: those stay
     in `~/.ssh/config`, which the tunnel inherits.
2. **Host identity**, on first contact only: the host key fingerprint and a
   *Trust* / *Cancel* choice. Trust writes the key to `known_hosts`.
3. **Host checks**, a list that updates as each check runs over SSH, each with
   a mark, a one-click fix, or a command to copy:
   - Ralphy is installed, with a compatible peer protocol; if not, an
     *Install Ralphy on the host* button (Amendment 2026-09-29) and a *Check
     again* button.
   - The host daemon has a name; if not, a field to set it (§6).
   - The daemon starts with the system: `ralphy daemon install`, and on Linux,
     lingering. When enabling lingering needs `sudo`, the list shows the
     command instead of asking for an administrator password.
   - On macOS and Windows: whether the host starts Ralphy only after someone
     signs in, with the text of §8.
   - The daemon asks for its token on loopback (§5), turned on here.
   - The peer key is installed (§3).
4. **Connect**: the tunnel opens, the local daemon reads the token, probes, and
   the group appears in the tree.

When the SSH connection itself fails (refused or timed out on the SSH port),
the dialog cannot know the host's operating system yet. It shows a help panel
with one tab per system that says how to turn on the SSH server:

- **Windows**: in PowerShell as administrator,
  `Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0`,
  `Start-Service sshd`, `Set-Service -Name sshd -StartupType Automatic`; then
  check the firewall rule with `Get-NetFirewallRule -Name OpenSSH-Server-In-TCP`
  and create it when it is missing. The panel says to sign in with the account
  password, not the Windows Hello PIN, and that with a Microsoft account the
  user name is the local one (`whoami` shows it).
- **macOS**: *Remote Login* in System Settings, General, Sharing (macOS 13
  and later), or in System Preferences, Sharing (macOS 12).
- **Linux**: install and start `openssh-server`.

**Removing** is in the menu of the group: *Remove host*, with an unchecked
option *Also change this host's token. Every other computer connected to it is
disconnected.* (§9).

The Hosts dialog amendment (2026-09-30) replaces the entry point, the group
header and removing: see there.

### 12. What the operator sees when something stops

| Event | The workbench | The host |
|---|---|---|
| The VPN drops | The group turns grey: the tunnel to that host closed, and it is reconnecting. `ssh` sees the drop in about 45 s (§2). | Runs, agent sessions and consoles continue. |
| The VPN comes back | The group comes back when the tunnel reopens, with no action. | The operator attaches again; output past 256 KiB is lost from the screen, not from the work. |
| The local computer is off, then on | The local daemon starts at sign-in, reopens every tunnel, and the groups come back. | Nothing stopped. |
| The host restarts | The group is grey until its daemon is up. | Linux: up at boot. macOS and Windows: after a sign-in (§8). |
| The tunnel is open but the daemon does not answer | A different message: the tunnel is open and the daemon on the host does not answer. | The daemon is down; the operator starts it. |

### 13. Open points for the implementation

- **Restricting the peer key.** §3 says the key can be limited in
  `authorized_keys` to the port forward. But the add flow reads the token over
  SSH, and removal deletes the key line over SSH; both need a shell. Either the
  restriction is added as the last step of pairing and removal uses the
  operator's own sign-in, or the key stays unrestricted. To be decided with the
  add flow.
- **The local port** of each tunnel is chosen by the local daemon from the free
  loopback ports and stored in the descriptor's tunnel section; how it is
  chosen when that port is taken after a restart is not decided.
- **A detached `ssh.exe`** that must survive its parent was not measured
  (spike step 6).
- **Which `ssh` the daemon runs on Windows.** A Windows computer with Git for
  Windows has two: `C:\Windows\System32\OpenSSH\ssh.exe` and Git's
  `/usr/bin/ssh`. They behave differently: the spike's key was accepted by
  Git's `ssh` and refused by the Windows one because of its ACL. Which one
  `ralphy_proc_util::locate_program` finds depends on `PATH`. The daemon should
  choose one on purpose; the Windows one is the one the operator's own
  PowerShell uses, and the one the peer key's ACL must satisfy anyway.
- **Turning on SSH on macOS 12 from a terminal.**
  `sudo launchctl load -w /System/Library/LaunchDaemons/ssh.plist` failed on
  the Monterey host with a message that says to try `launchctl bootstrap`. The
  help panel must not suggest it; *Remote Login* in System Preferences is the
  path that worked.
- **A wrong address looks like SSH is off.** On the macOS test, the first
  address the operator tried belonged to another device; every port answered
  "connection refused". The help panel should say that "refused" can also mean
  a wrong address, and show how to read the host's address
  (`ipconfig getifaddr en0` on macOS).

## Rejected alternatives

- **An SSH client inside Ralphy (`russh`).** A second SSH implementation that
  ignores `~/.ssh/config`, the agent, jump hosts and `known_hosts`.
- **A tunnel the operator runs (autossh, a service).** No code, but the tunnel
  does not come back after a VPN drop or a reboot unless the operator sets up a
  tool per host, and on Windows autossh is not native.
- **Passwords in the OS secret store.** See §3.
- **A pairing code.** See §4. It stays the control plane's enrollment.
- **A network bind with a bearer token.** It is plain HTTP, so on a shared
  network the token travels in clear text, and it exposes a listener to the
  whole network.
- **A local `ssh -t` free console.** See §7.
- **A LaunchDaemon on macOS.** It cannot read the login Keychain (§8).

## Consequences

- The WSL case is unchanged. The new code is in one place: the tunnel handle,
  the descriptor's tunnel section, the add and remove flows, one auth marker,
  and one routing change for the free console.
- `ralphy-daemon` starts a second kind of held process (`ssh`), with the same
  rules as the keepalive.
- The control plane of ADR-0032 Phase 2 is needed only for a host that has no
  SSH path (behind a NAT the operator does not control).
- The operator's security posture on the local machine is unchanged. On the
  host it is stronger than ADR-0052's: the peer needs its token on loopback.

## Spike: what to measure before any code

Each step uses only what exists today. The prediction is written first, so a
wrong prediction is visible.

1. **VPS peer by hand.** On the VPS: `ralphy daemon setup`, then
   `ralphy daemon --peer-store /tmp/ralphy-out` (default port 7257). Copy
   `/tmp/ralphy-out/peers/<id>.toml` to the local `~/.ralphy/peers/` and change
   `port` to 7401. Locally: `ssh -N -L 7401:127.0.0.1:7257 -o
   ServerAliveInterval=15 -o ServerAliveCountMax=3 -o ExitOnForwardFailure=yes
   <vps>`.
   *Prediction:* the group appears as `Linux`; the tree, Changes, a run and an
   agent session work; a free console gets a 502.
2. **Detach and attach.** Start an agent session on the VPS, kill the local
   `ssh`, wait, start it again, attach.
   *Prediction:* the session is alive, and the output shown is at most the
   last 256 KiB.
3. **Latency and connections.** Measure the `/api/fleet` probe (2 s timeout)
   and the tree poll through the tunnel, and count local TIME_WAIT sockets
   during a tree poll.
   *Prediction:* the probe stays well under 2 s; the tree poll does not
   exhaust ports (the WSL fix applies).
4. **The loopback hole.** On the VPS, as another user,
   `curl -s 127.0.0.1:7257/api/repos`.
   *Prediction:* 200 with no token. This confirms §5 is needed.
5. **The macOS Keychain.** On the Mac, with no GUI sign-in, over SSH:
   `claude -p "say ok"` and `gh auth status`.
   *Prediction:* both fail for the Keychain reason. This confirms §8.
6. **Windows OpenSSH flags.** Check that the local `ssh.exe` accepts the flags
   of §2 and that `-N -L` survives the parent process exit when started
   detached.

## Spike results (2026-09-29)

Host: Ubuntu 20.04.6, OpenSSH 8.2p1, x86_64, signed in as `root` with a key
made for the spike. Local: Windows 11, `OpenSSH_for_Windows_9.5p2`. Ralphy
`v0.1.0-rc.30` on both sides; the Linux release is a static musl build, so it
ran on Ubuntu 20.04's old glibc.

| Step | Prediction | Result |
|---|---|---|
| 1. VPS peer by hand | group appears; free console 502 | **As predicted.** `/api/fleet` shows `svrapp` `reachable`, and its repo under the composite key. A free console through the local daemon is `502 Linux cannot host a free console: peer advertises no WSL distro`. The same console asked directly of the peer is `101`: §7 is a routing change only. |
| 2. Detach and attach | session alive | **As predicted.** A console ran a one-line-per-second loop on the peer. The tunnel was killed and reopened; a reattach to the same session id replayed ticks 1 to 35 with no gap. |
| 3. Latency and connections | probe well under 2 s | **As predicted.** `/api/fleet` with two peers: 0.11–0.15 s. A request through the tunnel alone: about 0.1 s. After 60 `/api/fleet` calls, TIME_WAIT sockets on the tunnel port did not grow (10 before, 10 after). |
| 4. The loopback hole | 200 with no token | **As predicted, and worse because of root.** As the host user `administrador`, `curl 127.0.0.1:7257/api/repos` with no token returned `200` and root's repo list. The token file itself was `Permission denied` for another user, but the `Localhost` policy does not ask for it. Through the tunnel from Windows, a request with no token also returned `200`. §5 is required. |
| 5. The macOS Keychain | fails over SSH | **As predicted, and the LaunchAgent case confirmed.** See the macOS run below. |
| 6. Windows OpenSSH flags | accepted | **Accepted**, after one fix below. Survival of a detached `ssh` after its parent exits was not measured. |

What the spike did **not** measure, so no one reads more into the table than it
holds:

- The tree, Changes, a run and an agent session of a repo on the tunnel peer.
  The Linux host had no agent CLI and no `gh`; the spike measured the repo
  list, the peer handshake and a console. These surfaces use the same peer
  protocol that ADR-0052's capstone validated over the WSL relay, and the
  tunnel changes only the socket under it.
- The tree poll through the tunnel. Step 3 counted TIME_WAIT sockets during
  `/api/fleet` calls, not during a tree poll.
- `gh` on macOS: it was not installed on the Mac. Only Claude Code was
  measured against the Keychain.
- Step 5 as written asked for a Mac with no GUI sign-in. It ran with a signed-in
  GUI session instead, which is the stronger case: SSH still could not read the
  Keychain while the GUI session was open.
- A Windows host.

**The macOS run** (same day). Host: MacBook Pro, Intel `x86_64`, macOS 12.7.6
Monterey, FileVault off, a user signed in to the GUI; Claude Code 2.1.149 in
`/usr/local/bin`, no `gh`. The release's `macos-x64` binary ran (`minos 12.0`).
`ralphy daemon install` registered the LaunchAgent in `gui/501`, and the local
workbench showed the peer as `corcino-mac` `macOS` `reachable` through a
tunnel on local port 7402.

| Where `claude -p` ran | Login Keychain | Claude's answer |
|---|---|---|
| An SSH session (GUI session open at the same time) | `User interaction is not allowed` | `Not logged in · Please run /login`: it could not read its credential |
| A console hosted by the LaunchAgent daemon, through the tunnel | readable (`no-timeout`) | `OAuth access token has expired`: it read its credential, which had expired on that Mac |

This is the fact §8 rests on: a daemon started as a LaunchAgent in the GUI
session gives its children the login Keychain, and an SSH session does not.
Claude Code on macOS keeps no `~/.claude/.credentials.json`; the Keychain is its
only store. Two more facts from this host: SSH's non-login `PATH`
(`/usr/bin:/bin:/usr/sbin:/sbin`) does not contain `/usr/local/bin`, so the add
flow must run host commands through a login shell (`zsh -lc`) or by absolute
path; and `pmset` reported `sleep 1 (sleep prevented by sharingd)`, so sharing
held the Mac awake during the test, which is not a guarantee when nothing is
shared.

Findings the predictions did not cover:

- **Windows OpenSSH refuses a key whose ACL is too open.** The key file
  inherited an entry for another local group from the profile folder, and
  `ssh.exe` ignored the key (`bad permissions`). Git for Windows' `ssh` does
  not check this. `set_owner_only` is a no-op on Windows (`auth/token.rs`,
  `registry.rs`, `identity.rs`), so the peer key needs a real owner-only ACL
  when the daemon writes it (the spike used `icacls /inheritance:r
  /grant:r <user>:F`).
- **The unreachable diagnosis is wrong for a tunnel peer.** With the tunnel
  down, the workbench says "Start it. If it is a WSL distro, wake it." §2's
  tunnel-aware diagnosis replaces it.
- **`ralphy daemon setup` is interactive**, and it sets the name, the avatar,
  a TOTP seed and an optional password in one run. It also prints the TOTP
  secret. The add flow of §6 needs a non-interactive form that sets the name
  and avatar only.
- **`ralphy init` registers the repo even when the gate fails**, as ADR-0052
  says; on a host with no agent and no `gh` the repo still appears.
- **The `ralphy daemon install` help is out of date.** It says "with Task
  Scheduler or a systemd user unit", but Windows uses the `Run` key and macOS a
  launchd agent (`autostart.rs`). The add flow's host checks will show this
  text to operators, so it needs the correct list.
- **A daemon run as `root`** turns the loopback hole into a path from any host
  user to a root shell. With §5 this is closed; the add flow should still
  advise a normal user.

## Amendment (2026-09-29): the add flow installs Ralphy on the host, when the operator allows it

§10 said "it never installs software on another computer": the host checks
showed a link to the releases page and `./ralphy install`. On a new VPS this is
the first step, and the operator must leave the dialog, find the right archive,
copy it to the host and install it. The local computer already has what the
host needs: its own binary, or the release archive for the host's target. This
amendment lets the add flow send it. Decided in a design discussion on
2026-09-29, after a check of each decision against the code.

**D1. The bytes come from the local computer.** When the host has the same
target as the local computer, the local computer sends its own executable.
When the target is different, it downloads the release archive for the host's
target, of **the local computer's version**, never the latest one: peer
compatibility is an exact match of the peer protocol (`PEER_PROTOCOL_VERSION`),
so only the same version is sure to connect. A development build (ahead of its
tag) has no release archive. For another target it sends the latest published
release (amended 2026-10-02): its peer protocol may differ from the
development build's, and the operator of a development build takes that risk.
The describe after the install reports a mismatch. The host needs no internet
access.

**D2. The binary goes to `~/.ralphy/bin`.** On Windows it is
`%USERPROFILE%\.ralphy\bin\ralphy.exe`. This needs no root and no
administrator. The flow does not change `PATH` and edits no file of the
operator (no shell profile, no registry). Every command the add, check and
remove flows run on the host uses `~/.ralphy/bin/ralphy` when it exists, and
`ralphy` from `PATH` otherwise. The path is fixed, so the descriptor does not
record it. At the end, the flow says how to add the folder to `PATH` for manual
use.

**D3. An old Ralphy is not replaced; it is set aside.** When the host has a
Ralphy with another peer protocol, the new binary goes to `~/.ralphy/bin` as in
D2. The old binary stays where it is and is not touched: it can be a symlink
into a development checkout, or a file that `cargo install` owns. The flow says
that `ralphy` on `PATH` is still the old one. **The flow never installs an
older version.** When the host reports a peer protocol higher than the local
one, it sends nothing and says to update Ralphy on this computer.

**D4. The bytes go through the standard input of the SSH session.** No `scp`
and no `sftp`: they need a second connection, and some servers turn off the
`sftp` subsystem. The local computer sends the executable only, taken out of
the archive, so the host needs no `tar` and no `unzip`. On Linux and macOS the
host writes it with `cat`; on Windows with PowerShell, which copies the
standard input byte for byte. The `findstr` form that writes
`authorized_keys` cannot carry a binary. The host writes `ralphy.part`,
computes its SHA-256 (`sha256sum`, `shasum -a 256`, or `Get-FileHash`), and the
local computer compares it with the hash it computed. Only then does the host
make the file executable and rename it. An existing `~/.ralphy/bin/ralphy` is
first renamed to `.old`, because Windows cannot overwrite a running
executable.

**D5. `ralphy host install <destination>` is the permission.** The CLI never
asks a question, because the workbench runs it as a verb. So the permission is
the command itself. `ralphy host add` never installs; when Ralphy is missing or
old, the check suggests `ralphy host install`. In the dialog, the check of
Ralphy gets an *Install Ralphy on the host* button, with what it will do: the
version, the target, where the binary comes from (this computer, or a release
download), and the folder on the host. After the install, the dialog runs
*Check again* by itself. The new verb joins the `host` verb family and its
argv checks, and prints its progress as JSON lines like the others.

**D6. The probe reads the architecture.** It reads `uname -m` on Linux and
macOS, and `PROCESSOR_ARCHITECTURE` on Windows, and maps the result to the
release targets: `linux-x64`, `macos-x64`, `macos-arm64`, `windows-x64`. A
target with no release archive is refused with a clear sentence, for example
that there is no Ralphy build for Linux arm64 and it must be built on the host
with `cargo`. Windows on ARM64 is refused too: it can run x64 binaries through
emulation, but nobody has measured Ralphy's consoles there.

**D7. A downloaded archive is checked like `ralphy update` checks it.** The
flow downloads the `.sha256` file of the same release and compares. It does not
require the GitHub attestation, because that needs `gh` on the local computer.
The archive is kept in the local store, by version and target, so adding three
Linux hosts downloads it once. Its hash is checked again each time it is used.

**D8. `host install` moves a running daemon to the new binary; `host add`
does the rest.** The new binary reads the same store (`~/.ralphy`) as the old
one. If the old daemon continued to run, `describe` would report it as
running, `add` would not restart it, and the tunnel would reach the old
protocol. So `host install`, after it writes the binary, asks the new binary
`daemon describe` and then:

1. when autostart is registered, runs `~/.ralphy/bin/ralphy daemon install`,
   because autostart records the executable that registers it
   (`autostart.rs`);
2. when a daemon is running, runs `~/.ralphy/bin/ralphy daemon restart`,
   which stops it and starts the new binary in its place.

The daemon does not end stopped, because that cannot hold on every system.
There is no `ralphy daemon stop`. On macOS, `daemon install` unloads and loads
the launch agent, whose `RunAtLoad` starts the daemon, and whose `KeepAlive`
starts it again after a kill. On Linux, `daemon install` writes the systemd
unit, and systemd reads a changed unit only after a reload. Whether `enable`
reloads when the unit is already enabled is not measured, so `daemon install`
runs `systemctl --user daemon-reload` before `enable`. Without a reload, the
`systemctl --user restart` that `daemon restart` runs for a unit would still
start the old `ExecStart`.

*Check again* then shows what is still missing (the name, the token marker),
and *Connect* (`host add`) sets them and restarts or starts the daemon, as it
does today. `add` stays the only flow that configures the host daemon.

**D9. `host remove` never uninstalls.** Other computers can use the same host
daemon, and the computer that installed Ralphy does not own it. Removing a host
still undoes only what belongs to this computer: its key line and its
descriptor (§9).

**Not measured yet.** The implementation must check these before it relies on
them:

- The `macos-arm64` binary keeps its ad-hoc code signature when it is copied
  through standard input, and it runs with no quarantine attribute.
- PowerShell on a Windows host, started by OpenSSH, writes the standard input
  to a file byte for byte, so the SHA-256 on the host matches.
- A binary of about 30 to 40 MB, sent in one buffer through `HostShell::run`,
  arrives in an acceptable time on a slow link, and the dialog shows that the
  transfer is in progress.

## Amendment (2026-09-30): how the password travels (issue #498)

§3 says the password is used once, in memory, and never stored. This amendment
decides the path it takes, because `ssh` runs with `BatchMode=yes`, which also
turns off every way to give it a password.

**P1. The path.** The dialog sends the password in the `host.check`,
`host.add` or `host.install` payload. The daemon adds `--password-stdin` to the
argv and writes the password to the child's standard input, which it then
closes. The CLI reads it into memory that is erased when dropped (`zeroize`).
When the host refuses every key, the CLI runs `ssh` without `BatchMode`, with
`SSH_ASKPASS` set to the `ralphy` binary itself and
`SSH_ASKPASS_REQUIRE=force`. That child `ralphy` gets the password from the CLI
over a loopback socket that answers only a one-time random nonce. The
environment carries only the port and the nonce. The password is never in an
argv, an environment variable, a file or a log.

**P2. What the password does.** It signs in only to add this computer's peer
key: `uname -s` (else `cmd /c ver`), on Windows the probe for the
Administrators group, then one command that appends the key line unless the
key is already there. Every later command, and the tunnel, uses the peer key.
On a Windows administrator the shared keys file gets an ACL for Administrators
and SYSTEM only, because sshd ignores it otherwise. `host check` with a
password therefore changes the host in one way: it adds the key.

**P3. One attempt.** The password sessions run with `PubkeyAuthentication=no`,
`NumberOfPasswordPrompts=1` and `StrictHostKeyChecking=yes`. The socket answers
the first prompt that asks for a password, and never answers the same prompt
twice, so a wrong password costs the account one failed attempt. A different
prompt (a second factor, a password change) is not answered; the flow fails
with the kind `prompt` and shows the prompt. The console fallback of §3 is not
built yet.

**P4. Where a password may come from.** The daemon accepts a password only on
a request that arrived over https (`X-Forwarded-Proto: https`) or from this
computer: a loopback `Host` and no forwarding header. Otherwise the reply is an
error and nothing runs. The dialog hides the field in the same case.

**Measured (2026-09-30).** `OpenSSH_for_Windows_9.5p2`, started with no console
(`DETACHED_PROCESS`, as the daemon starts its children), calls `SSH_ASKPASS`
with `SSH_ASKPASS_REQUIRE=force`, with or without `DISPLAY`. The prompt it
passed was `root@10.1.1.4's password: `, and it made one attempt.

## Amendment (2026-09-30): the Hosts dialog

This amendment changes three parts of §11. The rest of §11 stands.

**H1. One dialog for hosts.** The header button opens a dialog named
**Hosts**, with two tabs: *Your hosts (n)* and *Add a host*. It opens on
*Your hosts* when at least one host is paired over SSH, and on *Add a host*
otherwise. With no host, the tab bar is not drawn. The button keeps its icon.
A WSL daemon is not a host here: it is paired by the WSL setup, not by SSH.

**H2. The group header shows the operating system.** Every group header shows
an icon for the system of its daemon: a penguin for Linux and WSL, an apple for
macOS, the Windows logo for Windows, and a monitor for any other system. The
icon comes from the `environment` label the group already has. The `server`
icon of a tunnel peer is gone. The state glyphs do not change.

**H3. Removing and editing are rows of *Your hosts*.** The group menu is
removed. Each row of *Your hosts* shows the system icon, the name, the SSH
destination and the state, and has two actions:

- **Remove** asks for confirmation in the row, with the unchecked option of §9
  to also change the host's token.
- **Edit** opens *Add a host* with the connection of the host filled in: the
  destination and the key file. The password field is empty. The flow is the
  add flow: the checks run, and *Save* runs `host add` again. The descriptor is
  keyed by the daemon id, so it is written again in place, with the same local
  port; the name check leaves out the host's own name. A password is needed
  only when the host no longer accepts the peer key.

To fill the form, `/api/fleet` returns `destination` and `identity_file` for a
tunnel peer. Both are what the descriptor holds; no secret leaves the daemon.

**H4. A changed connection reopens the tunnel.** The daemon keeps each open
tunnel with the spec it was opened with. When the descriptor's spec differs,
the daemon stops that `ssh` and opens a new one, so an edit takes effect
without a daemon restart.

## Amendment (2026-10-02): several operators on one host (proposed)

A daemon serves one operator (ADR-0032). A Linux or macOS host can have
several accounts, and each account can be an operator with its own daemon. The
install folder, the store, the token and the autostart are already per account
(D2, §3, §8). The TCP port is not: every daemon binds `127.0.0.1:7257`, and the
autostart cannot pass `--port` (ADR-0032 §4).

**What happened (measured 2026-10-02, two accounts `ralphy1` and `ralphy2` on
the Ubuntu 20.04 test host 10.1.1.4).** `host add` for `ralphy1` worked. For
`ralphy2`:

1. Its daemon never started, because the daemon of `ralphy1` held port 7257.
2. `ralphy daemon describe` of `ralphy2` still said `running: true`, because
   it only tests that something answers on the port.
3. The add flow wrote a descriptor with peer port 7257, and the tunnel reached
   the daemon of `ralphy1`.
4. That daemon refused the token of `ralphy2` (§5), so no data leaked. But the
   add flow ended with exit 0, and the message said to restart the daemon with
   `--peer-store`, which was not the cause.

Without §5 (a daemon on the default `Localhost` policy), step 4 would have
shown the repos of `ralphy1` to the operator of `ralphy2`.

**M1. On Linux and macOS, every daemon also listens on a Unix socket in its
store: `~/.ralphy/daemon.sock`, mode `0600`.** The same router and the same
auth policy serve both listeners, so the token of §5 is still required on the
socket. The operating system gives each account its own socket, so two daemons
never collide, and the kernel refuses a connection from another account before
the daemon sees it. On start, a socket file that no daemon answers is deleted
and bound again. A socket that answers means that a daemon of this account
already runs, and the start fails, as it does today for a port in use.

This changes the exposure of ADR-0032 §4 only for the account that owns the
store: the socket admits no one that the store does not already admit.

**M2. A tunnel to a Unix host forwards to the socket.** The tunnel becomes
`ssh -N -L 127.0.0.1:<local port>:<socket path>`. The descriptor's tunnel
section records `peer_socket` (an absolute path) in place of `peer_port`. The
local end, the loopback gate and the peer client do not change. Because `sshd`
opens the socket as the signed-in account, the account that the operator signs
in with is the daemon that the tunnel reaches. §2's "the peer daemon keeps its
default port" no longer applies to Unix hosts.

**M3. `describe` reports the socket only when the socket answers.** Its JSON
gets `socket`: the absolute path when a connection to it succeeds, else absent.
On a Unix host, `running` means that the socket answers. A binary that was
updated while an older daemon still runs therefore reports no socket, and the
add flow uses the port, as today. The add flow already reads `describe` again
after it restarts the daemon (`pair.rs`), so a new host gets the socket on the
first `host add`. A descriptor written before this amendment keeps the port
until the next `host add` or *Edit* (H3). The field is optional, so the peer
protocol version does not change.

**M4. On a host paired by `host add`, the TCP port is optional.** When the
store has the `daemon-require-token` marker (§5) and the port is in use, the
daemon logs a warning and serves only the socket. Without the marker, a port in
use stays a fatal error, because on the operator's own computer the browser
needs that port. On the host, the cost is that a browser on the host reaches
the daemon of another account and gets a 401. A headless host has no browser.

**M5. The handshake checks the identity.** `probe` compares the `daemon_id`
of `/api/peer/hello` with the descriptor's `daemon_id`. When they differ, the
peer status is a refusal that names both ids, and never `Unauthorized`. This
covers a Windows host (M6), a descriptor still on the port (M3), and any other
path to the wrong daemon.

**M6. A Windows host keeps the port.** The Unix socket of `tokio` exists only
on Unix, and nobody has measured OpenSSH for Windows as a server that forwards
to a socket. On a Windows host, a second account's daemon still fails to bind.
With M5 the operator sees a clear refusal. A port per account on Windows waits
for an operator who needs it.

**Measured (2026-10-02).** Client `OpenSSH_for_Windows_9.5p2`, server
`OpenSSH_8.2p1 Ubuntu-4ubuntu0.9`, default `sshd_config`. A small server bound
`~/.ralphy/spike.sock` with mode `0600` in each account. The homes were mode
`0755`.

| Test | Result |
|---|---|
| Tunnel as `ralphy1` to the socket of `ralphy1` | answered `I am ralphy1` |
| Tunnel as `ralphy2` to the socket of `ralphy2`, at the same time | answered `I am ralphy2` |
| Tunnel as `ralphy2` to the socket of `ralphy1` | refused; `ssh` printed `channel 1: open failed: connect failed: open failed` and stayed up |
| `ralphy2` connects to the socket of `ralphy1` on the host | `PermissionError: [Errno 13] Permission denied` |
| Length of `/home/ralphy2/.ralphy/daemon.sock` | 33 bytes; the limit is 108 on Linux and 104 on macOS |

The failed forward does not end `ssh`: `ExitOnForwardFailure` covers only the
setup of the forward, and a socket is opened for each connection. The tunnel
therefore looks open, and only the probe through it fails.

**Open points.**

- **`AllowStreamLocalForwarding no` was not measured** (it needs root on the
  host). A host with that setting accepts the tunnel and refuses each
  connection. The add flow must probe through the socket tunnel before it
  writes the descriptor, and write the port form when the probe fails. Measure
  the error text first.
- **A path over the limit.** A home with a long path cannot hold the socket.
  The daemon then serves only the port, and `describe` reports no socket.
- **macOS** was not measured. It has the same sockets and the same OpenSSH.

## Amendment (2026-10-02): the header shows the OS release

This amendment changes §6 and H2.

**R1. The environment label names the release.** A daemon on Linux reads
`NAME` and `VERSION_ID` from `/etc/os-release` (`Ubuntu 24.04`, `Debian 12`).
A daemon on macOS reads `sw_vers -productVersion` and keeps the major version
(`macOS 15`). Windows stays `Windows`. A WSL daemon stays `WSL: <distro>`: the
workbench maps a `\\wsl.localhost\<distro>\…` path to its peer by that name.
When the release cannot be read, the label is the OS name, as before.

**R2. The header of a host is `<name> · <release>`.** The name is in capital
letters; the release keeps its own spelling (`VPS-HETZNER · Ubuntu 24.04`).

**R3. The icon comes from an `os` field, not from the label.** The peer
descriptor, `/api/fleet` peers and the fleet rows carry the daemon's OS family
(`windows`, `linux`, `macos`). A descriptor written before the field has none,
and the icon then comes from the label, as in H2.

A host's descriptor is written by `host add` and by Edit. A host paired before
this change keeps its old label until the host runs the new version and the
operator edits it.
