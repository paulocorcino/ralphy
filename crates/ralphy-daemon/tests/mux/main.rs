//! The channels of `/ws` (one socket per page), read off a real loopback
//! socket: each kind of channel has its own module, and `support` holds the
//! helpers they share.

mod console;
mod control;
mod flood;
mod peer;
mod spawn;
mod support;
mod tree;
