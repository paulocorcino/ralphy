# Peers on other machines: the local fleet through an SSH tunnel

Status: **accepted** (2026-09-29). Decided in a design discussion, after a
check of each decision against the code and a web search for the credential
facts, then validated the same day by a spike with no code change, against a
Linux host and a macOS host (see "Spike results"). §13 lists what is still open
for the implementation.

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
- **Upgrades stay manual on the other machine.** A protocol version mismatch
  is diagnosed as it is today; the automatic peer update after a workbench
  update covers WSL peers only.
- **Text in a console is capped.** A session keeps 256 KiB of output
  (`session/manager.rs`); after a long disconnection, the operator sees only
  the end.
- **Credentials are per host, permanently** (ADR-0052 Consequences), now for
  more hosts.
- **Installing Ralphy on the host is the operator's job.** The add flow checks
  the version and shows the install command; it never installs software on
  another computer.

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
   - Ralphy is installed, with a compatible peer protocol; if not, the install
     or upgrade command and a *Check again* button.
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
