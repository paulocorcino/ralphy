---
kind: fix
---

A worktree that shares a directory such as `node_modules` from the primary tree is clean on Linux and macOS as it already was on Windows: the link is excluded locally, so the tree no longer reads dirty, can be removed, and a commit never picks the link up.
