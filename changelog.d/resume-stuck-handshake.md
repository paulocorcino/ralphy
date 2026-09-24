---
kind: fix
---
A console no longer hangs on "connection lost — reconnecting…" after a phone
comes back from a call or another app: a reconnect that cannot finish its
handshake is dropped after a few seconds and retried, and coming back to the
page replaces one that is stuck.
