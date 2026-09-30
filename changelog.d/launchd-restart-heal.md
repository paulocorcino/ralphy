---
kind: fix
---
On a Mac host, a restart now also repairs a daemon that an older Ralphy left
running outside its launch agent: it ends that daemon and lets the agent start
it again.
