---
kind: feature
---
`ralphy daemon install` registers a launchd agent on macOS, so the daemon
starts at login there as it already does on Windows and Linux. On a Windows
without PowerShell 7 the registration now runs under Windows PowerShell
instead of pointing at an interpreter that is not there.
