---
kind: fix
---
On a Mac host, restarting the daemon now goes through its launch agent. Before,
`ralphy host add` left the daemon outside launchd, and the agent tried to start
a second daemon every ten seconds.
