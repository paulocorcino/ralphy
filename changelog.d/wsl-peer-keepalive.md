---
kind: fix
---
A WSL peer stays up: the Windows daemon now holds its distro awake — at start
and on every nudge — so a console on a WSL project no longer closes half a
minute after the peer wakes.
