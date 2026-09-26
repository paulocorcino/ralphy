---
kind: fix
---
Deleting a worktree on Windows now takes about a second. Before, it could take
half a minute, because every file in the worktree was opened and scanned.
