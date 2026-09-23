---
kind: fix
---
After a daemon restart, an old console window can no longer reattach to a different
session, and the restarted daemon keeps writing to `daemon.log`.
