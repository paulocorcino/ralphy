# The tray starts the daemon and opens the workbench

Status: proposed
Kind: structural
Protects: operability

## Context

A user who does not use a terminal has no way into Ralphy: the daemon starts
from `ralphy daemon` or from `ralphy daemon install`, and the workbench opens
from an address that the user must type. We want a desktop icon that starts
the daemon, shows in the system tray that it runs, and opens the workbench in
a window. It must work on Windows, Linux and macOS.

Facts that limit the design:

- The release `ralphy.exe` is 50 MB (2026-11-27, `target/release`). The tray
  must add as little as possible.
- On macOS a tray icon must run on the main thread of its process (AppKit).
  The daemon's main thread runs its runtime and listener.
- The `tray-icon` crate links GTK and libappindicator on Linux when the binary
  loads, so a binary that uses it does not start on a headless server. The
  `ksni` crate talks StatusNotifierItem over D-Bus in pure Rust and links no C
  library.
- `daemon restart` and `scripts/update-ralphy.ps1` restart the daemon. A tray
  inside the daemon process would go away on each restart.
- The owner of "the daemon runs" is the probe of `ralphy daemon status`
  (`crates/ralphy-cli/src/daemon/describe.rs`). The owner of "stop the
  daemon" is `restart::stop` (`crates/ralphy-cli/src/daemon/restart.rs`), which
  proves the pid is the daemon before it kills it, and the console check in
  `crates/ralphy-cli/src/daemon/confirm.rs`.
- The browser window track (2026-10-20) already chose the installed
  Chrome/Edge over a bundled engine.

## Decision

**D1.** The tray is the `ralphy tray` subcommand of the one `ralphy` binary.
It runs in its own process, never inside the daemon. Its code lives in a new
crate, `ralphy-tray`; `ralphy-cli` only wires the subcommand.

**D2.** The tray uses `tray-icon` on Windows and macOS and `ksni` on Linux.
No dependency of `ralphy-tray` links a GUI library at load time on Linux.

**D3.** When no daemon answers, `ralphy tray` starts one as a detached
process, the same way `daemon restart` starts it. One tray runs per user,
held by a lock file in the global store. A second `ralphy tray` asks the
first one to open the window and exits.

**D4.** The tray learns the daemon state from the `daemon status` probe every
5 seconds. It never computes the state another way. It shows three states:
on (color icon; Open, Quit), starting (grey; Quit), off (grey; Start, Quit).

**D5.** The tray never restarts a daemon on its own. A daemon that stops is
shown as off, with a Start item.

**D6.** Quit stops the daemon and closes the tray. It stops the daemon
through `ralphy daemon stop`, a new subcommand over `restart::stop`. When
consoles are open, Quit asks first in a native dialog (Windows `MessageBoxW`,
macOS `NSAlert`, Linux `zenity` or `kdialog`). On Linux with no dialog
program, Quit is disabled while consoles are open. With no open console, Quit
does not ask.

**D7.** Open launches Edge, then Chrome, then Chromium, with `--app=<url>`
and `--user-data-dir=<global store>/app-window`. When none is found, it opens
the default browser. The URL comes from the `daemon status` owner. The tray
ships no web engine. The first start of the tray opens the window on its own.

**D8.** The tray shows no count, badge or notification. Every fact about runs
and consoles stays in the workbench.

**D9.** `ralphy tray` refuses to start, with exit code 1 and a message that
names `ralphy daemon`, when there is no desktop session (no `DISPLAY` or
`WAYLAND_DISPLAY` on Linux, an SSH session) and inside WSL. On Linux with a
desktop session but no StatusNotifierWatcher on D-Bus, it starts the daemon
and opens the window with no icon, and logs that `ralphy daemon stop` closes
it.

**D10.** `ralphy tray install` creates a desktop and menu entry that runs
`ralphy tray`: a `.lnk` on Windows, a `.desktop` file on Linux, and a small
`Ralphy.app` bundle with `LSUIElement` in `~/Applications` on macOS. With
`--login`, the tray starts at login and replaces the login entry of
`ralphy daemon install`, so only one of the two starts at login.
`ralphy tray uninstall` removes what install created.

**D11.** The tray adds at most 2 MB to the release binary. A spike that has
only the icon, Open and Quit on the three systems measures it with
`cargo bloat` before any production code. Over 2 MB, the libraries are
chosen again before this ADR is accepted.

## Consequences

- A second "start the daemon at login" path exists. D10 keeps the two
  exclusive: each install removes the other's entry.
- The dedicated browser profile uses about 30 to 60 MB on disk.
- Open twice opens a second window. Focusing the existing window needs
  native code for each OS and is not part of this decision.
- macOS needs an app bundle, the one artifact of this decision that is not
  the binary.

## Considered options

- **A native webview (`wry`).** It adds a few MB, and on Linux it links
  `libwebkit2gtk` at load time, so the binary would not start on a headless
  server or in WSL.
- **A separate `ralphy-tray` binary.** It keeps the main binary unchanged, but
  it adds a second artifact to the release, the installer and autostart on
  three systems. D2 already keeps the one binary safe on headless Linux.
- **The tray inside the daemon process.** It breaks the macOS main-thread
  rule and goes away on each restart.
- **Quit closes only the tray.** A user who started Ralphy from the icon
  could not tell whether it still runs.

## Compliance

- D1: not checked by code: manual, reviewed in the PR (a check that
  `ralphy-daemon` does not depend on `ralphy-tray` can follow with the crate).
- D2: not checked by code: needs a Linux build with no GTK installed; the
  release workflow's Linux job is where it shows.
- D4: not checked by code: manual, reviewed in the PR.
- D5: not checked by code: manual, reviewed in the PR.
- D6: not checked by code: manual, reviewed in the PR.
- D8: not checked by code: manual, reviewed in the PR.
- D11: not checked by code: measured by the spike; a size ratchet can follow.
