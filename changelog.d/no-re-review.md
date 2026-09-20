---
kind: fix
---
An execute session no longer pays for a second self-review after fixing the
first one's HIGH findings: the fix is proved by the scoped test and the green
gate, not by re-running the reviewer over the same diff.
