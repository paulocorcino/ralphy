//! The channels of `/ws` (one socket per page), read off a real loopback
//! socket: each kind of channel has its own module, and `support` holds the
//! helpers they share.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

mod auth;
mod console;
mod control;
mod flood;
mod peer;
mod peer_channel;
mod spawn;
mod support;
mod tree;
