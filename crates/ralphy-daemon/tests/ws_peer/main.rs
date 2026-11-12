//! Bind auth, presence and the peer transport over real loopback sockets: the
//! `/ws` handshake, the peer handshake and connection pool, and fleet usage.
//! No module sets an env var, so they share one test binary.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

mod auth_ws;
mod fleet_usage;
mod peer_handshake;
mod peer_pool;
mod ws_presence;
mod ws_pushes;
