---
kind: feature
---

The workbench loads faster over a tunnel and reloads for free: every asset
carries a content ETag so a reload is a round of 304s, and text assets go out
gzipped (11 MB down to 3 MB on first open).
