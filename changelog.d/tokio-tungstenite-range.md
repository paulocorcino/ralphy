---
kind: internal
---
Dependabot no longer proposes tokio-tungstenite 0.30 or later until axum moves
to it, and cargo-deny fails the build if two tungstenite versions appear, so the
daemon keeps one WebSocket library.
