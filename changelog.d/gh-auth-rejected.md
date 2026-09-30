---
kind: fix
headline: 🔑 **GitHub login errors** — say which login failed and how to fix it
---
When GitHub rejects the login, the board and the run now say which login failed:
a token in `GH_TOKEN` or `GITHUB_TOKEN`, or the stored `gh` login. The error also
tells you how to fix it, and no longer shows the raw GitHub response.
