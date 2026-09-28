//! Agent-session and console tests over a real loopback WebSocket, one test
//! binary. Every module points the session launcher at the helper child
//! through the same process-wide `RALPHY_DAEMON_AGENT_OVERRIDE` value and sets
//! no other env var, so they can share a process.

mod console_command_ws;
mod console_reattach;
mod console_ws;
mod session_persistence;
mod session_ws;
mod session_ws_checkout;
mod session_ws_cursor;
mod session_ws_gemini;

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
    });
}
