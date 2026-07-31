# The ralphy daemon

Ralphy's resident daemon (docs/adr/0032): a foreground HTTP+WebSocket
listener serving the embedded workbench UI, `ralphy daemon` run in the
foreground until Ctrl+C.

## Setup and status

```
ralphy daemon setup    # baptize: pick a name, an avatar, mint an access token
ralphy daemon status   # identity, access token state, listener, autostart
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

3. **Install the unit and give it a peer store.** Install as usual, then edit
   `~/.config/systemd/user/ralphy-daemon.service` and append `--peer-store` to
   its `ExecStart`, pointing at the *Windows* profile's global store:

   ```
   ralphy daemon install
   # then, in ~/.config/systemd/user/ralphy-daemon.service:
   #   ExecStart=/home/<user>/.cargo/bin/ralphy daemon --peer-store /mnt/c/Users/<user>/.ralphy
   systemctl --user daemon-reload
   ```

   `ralphy daemon install` does not pass `--peer-store` through — the flag is a
   deliberate, per-host declaration, so it is an edit you make once.

   Run `install` **from your own shell inside the distro**, not from a script
   with a stripped environment: it captures `WSL_DISTRO_NAME` into the unit
   (`Environment="WSL_DISTRO_NAME=<distro>"`). `systemd --user` does not inherit
   WSL's login-session variables and nothing inside a distro can name itself, so
   this pin is how the daemon knows which distro it is — and therefore how a
   sleeping peer can be woken at all (step 5).

4. **Start it, and start it at boot.**

   ```
   systemctl --user enable --now ralphy-daemon.service
   ```

5. **Wake the distro at Windows logon** — see *WSL wake-at-logon nudge* above.
   WSL does not start a distro just because something inside it is enabled.

   A sleeping peer (its environment group reads `unreachable`) can also be woken
   on demand through `POST /api/fleet/nudge?daemon_id=<id>`, which runs
   `wsl.exe -d <distro> -e systemctl --user start ralphy-daemon.service` and does
   not wait on it. This is an API call today — the workbench has no nudge button
   yet — and step 1 remains the prerequisite it cannot substitute for.

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
