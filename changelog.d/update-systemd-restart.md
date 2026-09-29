---
kind: fix
---
`ralphy update` and `ralphy daemon restart` now restart a daemon that runs as a
systemd service through systemd. Before, they stopped it in a way that systemd
reads as a crash.
