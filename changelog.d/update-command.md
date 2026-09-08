---
kind: feature
---
`ralphy update` takes the newest release for you: it downloads the archive for
your platform, refuses it unless it matches the published checksum, replaces the
binary in place, and restarts the daemon so the build you are running is the one
that was installed.
