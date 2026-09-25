---
kind: internal
---
The peer relay now uses tokio-tungstenite 0.29, the same WebSocket library the
browser side already used, so the build carries one WebSocket stack instead of
two.
