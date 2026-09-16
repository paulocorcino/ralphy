# The ralphy daemon

Ralphy's resident daemon (docs/adr/0032): a foreground HTTP+WebSocket
listener serving the embedded workbench UI, `ralphy daemon` run in the
foreground until Ctrl+C.

## Setup and status

```
ralphy daemon setup    # baptize: pick a name, an avatar, mint an access token
ralphy daemon status   # identity, access token state, listener, autostart
```

## Restart

```
ralphy daemon restart   # end the running daemon, start this binary in its place
```

The daemon records its pid **and its invocation** in `<store>/daemon.pid` at
startup, so a restart brings back *the same* daemon: one started with
`--port 8080` comes back on 8080, not on the default. `ralphy update` calls this
for you after it replaces the binary — without it the resident daemon would keep
serving the image it was replaced from.

## Agent state

A console whose vendor has hooks tells the workbench what its agent is doing
(ADR-0059): a dot before the console's title, on the project row, on the
picker's worktree rows and on the Go-to list — green while it works, yellow
when it is waiting for you (with what it asks in the tooltip), grey once the
turn ended, hollow when a green went quiet for longer than 45 minutes. The
daemon launches the console with a settings file registering the vendor's
hooks under `<store>/sessions/<id>.settings.json` (your own settings and
hooks keep their say — the file is merged over them) and tails
`<store>/sessions/<id>.agent-status.jsonl`, which each hook appends to
through `ralphy hook status`; both files go with the session, and nothing is
read off the terminal. The yellow clears as soon as the tool you answered
returns. An interrupt (Esc, Ctrl-C) fires no hook, so a green dot after one
stays green until it ages hollow or you send the next prompt. Consoles of a
vendor without hooks show no dot.

## Release watch

The daemon asks GitHub what has been published, every six hours and once at
startup, and answers `GET /api/release` with where this build stands: `behind`
(with the whole gap, newest first), `level`, `ahead` (a development build, never
offered an update), or `unknown`.

It is one unauthenticated GET. Its entire outbound content is the page size and a
static `User-Agent: ralphy` that GitHub refuses the request without — **no body,
no credential, and nothing that distinguishes one installation from another**.
The result is TTL-cached to
`<store>/releases.json`, and a failed fetch is silent: the workbench shows what
was last known. Turn it off by creating the marker file, and on by removing it:

```
touch ~/.ralphy/daemon-release-watch-off
```

How loudly the workbench says it comes from the *kinds* the releases carry
(`### New`, `### Fixed`, `### Breaking`, `### Security` in the release body), not
from the version delta — while the project ships candidates there is no
minor-versus-patch signal to read. See
[ADR-0056](adr/0056-release-communication-and-the-update-watch.md).

## Worktrees

A worktree is a console's workspace
([ADR-0063](adr/0063-a-worktree-is-a-console-workspace.md)): a second working
tree of a project on its own branch that the workbench creates, opens consoles
in and removes, so two agents on one project never share a tree while a
scheduled `ralphy run` keeps the primary tree.

The branch chip's picker has a **Worktrees** section: a `primary` row, one row
per worktree (`<name> · <branch>`, a dot when dirty), a
`+ new worktree from <branch>` row that takes a name, and a remove action per
row. Picking a row selects the project's checkout — the Files tree, the
viewer, Find, Changes, the diff, and the branch chip all follow it, **New
console** opens the agent inside it for every agent, and a console keeps the
worktree it was born in: its title reads `<agent> · <name>`, and a restart —
its own button, or the desk relaunching it after the daemon came back — lands
it in the same worktree. When the project has a worktree, that title segment
is also a switcher: pick another tree (or `primary`) and the console restarts
there, after asking, since its scrollback goes with the session. If that worktree is gone by then, the box says so and
offers to relaunch in the primary; it never lands there unasked. The selection
is desk state (a reload and a second browser agree); picking `primary`
restores today exactly.

A worktree lives at the fixed location `<repo>/.ralphy/worktrees/<name>`,
inside the registered path and gitignored, never a second project. `<name>` is
directory and branch at once, cut from the branch you are on
(`--base <ref>` picks another). Worktrees made by hand elsewhere are not
listed and are never removed.

**gitignored files come along only when you say so.** A fresh worktree has
no `.env` and no `node_modules/`; two lists in `.ralphy/settings.json` name
what a new worktree gets from the primary tree, both relative to the
repository root and both warn-only — an entry that is missing, not
gitignored or of the wrong kind is reported under the create row and
skipped, and the worktree is created regardless:

```json
{ "worktree": { "copy": [".env", ".vscode/"], "share": ["node_modules", "target"] } }
```

`worktree.copy` lists gitignored paths to **copy** (a file, or a directory
recursively) — copied, never linked, so the agent's edits cannot leak back
into the primary. `worktree.share` lists gitignored **directories** to
**link** — a junction on Windows (no privilege needed), a symlink elsewhere —
which is the mode for `node_modules`: a copy would be slow, duplicate disk
and, nested under `.ralphy/worktrees/<name>`, cross `MAX_PATH` without
`core.longpaths`. A share that cannot be linked is skipped, never copied.
These keys are edited in the file; `ralphy config set` takes no arrays.

A share is a link to the primary's directory, and what is done **through**
it is done to the primary: `ralphy worktree remove` unlinks every share
before git touches the tree (git itself would follow a junction into the
primary's `node_modules` and delete it — measured), but a `rm -rf
node_modules` typed inside the worktree deletes the primary's. Treat a shared
directory as the primary's, because it is.

```
ralphy worktree list [--format json] [--repo <path>]
ralphy worktree add <name> [--base <ref>] [--repo <path>]
ralphy worktree remove <name> [--repo <path>]
```

Removal is gated; each gate answers in these words (the picker shows the same
line):

- `worktree '<name>' has a live console: close it first` — a console is open
  in it; the daemon's own gate, before anything runs.
- `refusing to worktree remove: a run holds this repo's lock (pid …, since
  …) — wait for it to finish or stop it` — a run holds the primary tree's
  `run.lock`; `add` is refused the same way.
- `worktree '<name>' is locked: unlock it first` — you locked it with
  `worktree lock`.
- `worktree '<name>' has uncommitted changes: commit or discard them first`
- `removed worktree '<name>'; branch '<name>' kept: it has commits not on
  <base>` — the directory is gone; the branch is deleted only when git agrees
  it is fully merged (`branch -d`, never `-D`) — otherwise it stays and the
  message says why.

### Windows: long paths

A worktree adds `.ralphy/worktrees/<name>/` to every path inside it, so a deep
`node_modules/` that fit in the primary tree can cross `MAX_PATH` (260
characters) there; git fails with `Filename too long` and it is not a Ralphy
bug.

```
git config --global core.longpaths true
```

## Autostart

`ralphy daemon install` registers the daemon to start at logon, using the
native OS mechanism for the running platform — ralphy never becomes the
scheduler, it only writes and removes one registration:

- **Windows**: a per-user registry value `ralphy-daemon` in
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, running
  `pwsh -WindowStyle Hidden` → `ralphy daemon` (no visible console window) and
  appending its output to `<home>/.ralphy/daemon.log`. No elevation required.
- **Linux / WSL**: a systemd **user** unit at
  `~/.config/systemd/user/ralphy-daemon.service`, `WantedBy=default.target`
  (starts at user login), enabled via `systemctl --user enable`.

Both registrations run the daemon with its DEFAULTS (loopback bind, the
default port) — no `--bind`/`--port` passthrough in this slice; edit the task
or unit by hand for a non-default listener.

```
ralphy daemon install     # register autostart
ralphy daemon status      # …prints an `autostart: registered` / `not registered` line
ralphy daemon uninstall   # remove autostart (idempotent — a second call is a no-op)
```

### WSL wake-at-logon nudge

WSL is just Linux to ralphy (ADR-0032 §3): the WSL daemon is a plain Linux
build with its own `~/.ralphy`, installed the same way as any other Linux
host, from *inside* the distro. The one Windows-side seam is that the distro
itself must be woken at Windows logon for its systemd user unit to ever run —
WSL does not start a distro on its own just because a scheduled task exists
inside it.

This is a **manual, documented step**, not something `ralphy daemon install`
automates: register a Windows-side Task Scheduler entry that runs at logon
and wakes the distro:

```powershell
schtasks /Create /TN wsl-wake-ralphy /SC ONLOGON `
  /TR "wsl -d <distro> true" /F
```

(or the equivalent `Register-ScheduledTask` PowerShell form). `wsl -d
<distro> true` starts the distro if it is not already running and exits
immediately — enough to let its own `ralphy-daemon.service` (installed from
inside the distro via `ralphy daemon install`) come up under systemd.

## Local fleet: adding a WSL peer

Two daemons — one on Windows, one inside a WSL distro — can show each other's
repos in one sidebar (ADR-0052). Each daemon still binds loopback only, and
each announces its **own** access token: there is no shared secret, so stopping
one peer or rotating its token leaves the others working.

Set this up once, from *inside* the distro:

1. **Let the user session survive logout.** A systemd *user* unit dies with the
   last login shell unless lingering is on. No nudge can substitute for this —
   without it the daemon stops the moment you close the WSL terminal:

   ```
   loginctl enable-linger $USER
   ```

2. **Baptize the daemon.** A daemon announces its identity, so it must have one
   first:

   ```
   ralphy daemon setup
   ```

   Skip this and the daemon still starts and still serves — it just announces
   nothing, saying so in the journal (`--peer-store was given but this daemon is
   un-baptized`). There is no way to federate an un-named daemon: the
   `daemon_id` it mints is half of every federated repo ref.

3. **Install the unit, give it a peer store, and give it its own port.** Install
   as usual, then edit `~/.config/systemd/user/ralphy-daemon.service` and append
   both flags to its `ExecStart` — the store pointing at the *Windows* profile's
   global store:

   ```
   ralphy daemon install
   # then, in ~/.config/systemd/user/ralphy-daemon.service:
   #   ExecStart=/home/<user>/.cargo/bin/ralphy daemon --port 7357 --peer-store /mnt/c/Users/<user>/.ralphy
   systemctl --user daemon-reload
   ```

   `ralphy daemon install` does not pass either flag through — they are
   deliberate, per-host declarations, so it is an edit you make once.

   **`--port` is not optional here.** Both daemons default to the same port, and
   `localhostForwarding` — the relay that makes this whole setup work — publishes
   the distro's listener on *that same port* on the Windows loopback. Leave them
   equal and whichever starts second loses: if that is the Windows daemon it
   fails to bind and says so, and if it is the relay the loss is silent and the
   Windows daemon ends up dialling itself. It will refuse and tell you, but no
   repos federate until one side moves. Any free port will do.

   Run `install` **from your own shell inside the distro**, not from a script
   with a stripped environment: it captures `WSL_DISTRO_NAME` into the unit
   (`Environment="WSL_DISTRO_NAME=<distro>"`). `systemd --user` does not inherit
   WSL's login-session variables and nothing inside a distro can name itself, so
   this pin is how the daemon knows which distro it is — and therefore how a
   sleeping peer can be woken at all (step 5).

   For the same reason it captures your shell's `PATH`
   (`Environment="PATH=<your PATH>"`): `systemd --user` never sources
   `~/.profile`, so without the pin the daemon's children — `gh` for the board,
   `git`, every agent CLI — resolve against the distro's stock `PATH`, and a
   `~/.local/bin/gh` loses to whatever `/usr/bin/gh` the distro shipped with.
   If you wrote the unit by hand, add that line yourself.

   **After rebuilding or reinstalling the binary, restart the unit**
   (`systemctl --user restart ralphy-daemon.service`). The daemon spawns its
   own executable for every query (`changes list`, the board fold, …); a
   running daemon whose file was replaced underneath it sees
   `/proc/self/exe → … (deleted)` and every such query fails with
   `query read failed` until it is restarted.

4. **Start it, and start it at boot.**

   ```
   systemctl --user enable --now ralphy-daemon.service
   ```

5. **Wake the distro at Windows logon** — see *WSL wake-at-logon nudge* above.
   WSL does not start a distro just because something inside it is enabled.

   A sleeping peer can also be woken on demand. Its environment group reads
   `asleep` when WSL has stopped the distro — the ordinary case, since WSL
   terminates an idle one and the daemon goes with it — and `unreachable` when
   the distro is up but its daemon is not. Clicking that chip wakes it, and so
   does opening one of its projects; both call
   `POST /api/fleet/nudge?daemon_id=<id>`, which runs
   `wsl.exe -d <distro> -e systemctl --user start ralphy-daemon.service` and then
   waits, up to 30 s, for the peer to answer its handshake. The reply's `ready`
   is the field to act on: `nudged` only ever meant that `wsl.exe` was spawned.

   Step 1 remains the prerequisite no nudge can substitute for.

The Windows daemon needs no flag: it reads `%USERPROFILE%\.ralphy\peers\` fresh
on every request, so a newly announced peer appears on the next page load — no
restart needed.

### What the descriptor's token does and does not protect

The descriptor at `<store>/peers/<daemon_id>.toml` carries that daemon's access
token in the clear. It protects against **other users and other machines** —
nothing on the network can reach a loopback listener, and nothing another user
can read grants access. It does **not** protect against code running as you:
anything with your profile can read the file, and on `/mnt/c` it cannot even be
mode-protected — 9p drvfs without `metadata` silently ignores `chmod 600`,
leaving only the Windows profile ACL (ADR-0052 §3). Treat a peer store the same
way you treat `~/.ralphy/daemon-token` itself.

### When a peer's file tree flickers or empties

The symptom is specific: the FILES tree paints, then blanks; a reload paints it
again; a file created on the peer never shows up. The tree is fine and so is the
peer — what has run out is the **dialling host's ephemeral ports**.

Check it from the host that dials (the Windows side, for a WSL peer):

```
netstat -ano -p tcp | grep -c ":<peer-port>.*TIME_WAIT"
netsh int ipv4 show dynamicport tcp
```

A count approaching the dynamic range (16384 by default) means every new
`connect` is about to fail with `os error 10048`, "address already in use". The
peer answers in milliseconds throughout, which is what makes this look like a
tree bug rather than a transport one. The pool in `peer/client.rs` and the poll
pacing in `spawn_peer_tree_poller` exist to make this unreachable; a recurrence
means something is polling in a loop again.

To see which, the daemon logs every poll with the reason it came back — a
healthy one reads `reason=timeout` after its full 25 s window, or `reason=dirty`:

```
# in the WSL unit: systemctl --user edit ralphy-daemon.service
[Service]
Environment="RUST_LOG=ralphy_daemon=debug"
```

then `journalctl --user -u ralphy-daemon.service | grep 'peer tree poll'` and
group by `reason`. Anything answering in milliseconds — `no-receiver`,
`no-sub`, a burst of `dirty` — is the caller spinning. Take the reading with the
workbench open on the peer's project and the tree expanded, and with nothing
else probing the peer: a `curl` loop of your own exhausts the same pool and will
be blamed on the daemon.
