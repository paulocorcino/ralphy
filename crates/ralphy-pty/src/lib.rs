//! `ralphy-pty` — shared PTY crate (stub).
//!
//! A `portable-pty`-backed implementation lands in a later slice (see
//! docs/backlog/0002). It will be a *shared* crate consumed by adapters that
//! drive an interactive CLI (and, later, the on-screen terminal) — never by
//! `ralphy-core`, which stays PTY-free by design (docs/adr/0002).
