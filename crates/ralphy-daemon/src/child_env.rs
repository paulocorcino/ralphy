//! The secrets no child of the daemon gets. Every spawn site in this crate
//! removes them from the child's environment at the spawn; the daemon's own
//! environment is not changed for them, so no thread races a write to it.
// ADR-0072 D7: a secret Ralphy owns is removed from the environment of every child.

/// The name of the events sink token. The CLI owns it
/// (`ralphy_cli::events::config::TOKEN_ENV`); a test there checks the two agree.
pub const EVENTS_TOKEN_ENV: &str = "RALPHY_EVENTS_TOKEN";

/// The environment variables that hold a secret of the daemon: its access
/// token, the Telegram bot token and the events sink token.
pub const SECRET_ENV: [&str; 3] = [
    crate::auth::TOKEN_ENV,
    ralphy_telegram::config::TOKEN_ENV,
    EVENTS_TOKEN_ENV,
];

/// Remove every [`SECRET_ENV`] variable from the environment `cmd` gives its
/// child. Call it on each `Command` before `spawn` or `output`.
pub fn remove_secrets(cmd: &mut std::process::Command) {
    for key in SECRET_ENV {
        cmd.env_remove(key);
    }
}
