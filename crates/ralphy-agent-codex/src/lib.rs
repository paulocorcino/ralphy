//! The Codex CLI adapter: drives `codex exec` behind the core [`ralphy_core::Agent`]
//! contract. Everything Codex-specific — the binary, the model and reasoning-effort
//! flags, the headless invocation, and the signal→`Outcome` mapping — is confined
//! here. See docs/adr/0004.
