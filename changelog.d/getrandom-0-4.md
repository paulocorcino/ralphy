---
kind: internal
---
The daemon now takes its random bytes for tokens, password salts, TOTP seeds and
console names from getrandom 0.4, the version its other dependencies already use.
