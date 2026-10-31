//! Agent-session and console tests over a real loopback WebSocket, one test
//! binary. Every module reaches the daemon through the `console` channels of
//! the page socket (`/ws`). Every module points the session launcher at the
//! helper child through the same process-wide `RALPHY_DAEMON_AGENT_OVERRIDE`
//! value, and the CLI at `command_test_child` through `RALPHY_EXE_OVERRIDE`, and
//! sets no other env var, so they can share a process.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

mod console_command_ws;
mod console_reattach;
mod console_record;
mod console_ws;
mod session_persistence;
mod session_ws;
mod session_ws_checkout;
mod session_ws_cursor;
mod session_ws_gemini;
mod session_ws_refusals;

#[path = "../support/golden.rs"]
mod golden;

#[path = "../support/page.rs"]
pub mod page;

/// Point the session launcher at `session_test_child` instead of a real agent
/// or shell. The env var is set exactly ONCE: the tests of this binary can run
/// in parallel threads, and a repeated `set_var` across threads is a data race
/// even when every write carries the same value.
fn point_launcher_at_test_child() {
    static OVERRIDE: std::sync::Once = std::sync::Once::new();
    OVERRIDE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
        // A Gemini launch with no owned root runs `ralphy gemini prepare-root`
        // first; without this the daemon would run the TEST binary itself.
        std::env::set_var(
            "RALPHY_EXE_OVERRIDE",
            env!("CARGO_BIN_EXE_command_test_child"),
        );
    });
}
