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
