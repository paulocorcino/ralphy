---
kind: feature
---
The workbench knows when a newer build exists. The daemon checks the published
releases every six hours, and `GET /api/release` reports the whole gap between
what you are running and what is out — with what each release changed. It sends
nothing, caches to disk, is silent when the network is absent, and switches off
with a marker file in the daemon store.
