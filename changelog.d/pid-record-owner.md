---
kind: fix
---
A daemon that stops no longer erases the record of a newer daemon that has
just started. On a Mac host this record was lost during a restart, so the
next restart did not find the running daemon.
