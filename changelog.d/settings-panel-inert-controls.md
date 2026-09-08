---
kind: fix
---
Settings no longer offers controls it cannot operate. The Schedule section and
the queue's "Eligible labels" field were never wired to anything — editing
either came back "config change refused" — and are gone; use `ralphy schedule`
for the timer. The fallback verify command is now shown read-only, because its
value names a program a later run executes and the daemon refuses to set it
from a browser; set it with `ralphy config set verify.command` in the repo.
