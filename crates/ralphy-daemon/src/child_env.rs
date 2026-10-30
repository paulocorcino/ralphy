//! The secrets no child of the daemon gets. The boot path removes them from
//! the daemon's own environment once, before the runtime and its threads
//! exist ([`take_boot_secrets`]), so a child that a library crate starts gets
//! none of them. Every spawn site in this crate still removes them from its
//! child at the spawn.
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

/// Read the access token and the Telegram token of the environment, then
/// remove every [`SECRET_ENV`] variable from the daemon's own environment.
/// Returns the non-empty access token. Call it once, before any thread of the
/// daemon exists: a write to the environment races every other thread.
#[expect(
    clippy::disallowed_methods,
    reason = "it removes the secrets from the daemon's own environment once, on the boot path, \
              before the runtime starts a thread, for every child started after it"
)]
pub fn take_boot_secrets() -> Option<String> {
    let token = crate::auth::env_token();
    crate::telegram_state::capture_env_token();
    for key in SECRET_ENV {
        std::env::remove_var(key);
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "nextest runs each test in its own process, so a test may change its environment"
    )]
    fn boot_secrets_are_taken_and_removed() {
        // Fake values only.
        for key in SECRET_ENV {
            std::env::set_var(key, format!("fake-{key}"));
        }

        let token = take_boot_secrets();

        assert_eq!(token, Some(format!("fake-{}", crate::auth::TOKEN_ENV)));
        for key in SECRET_ENV {
            assert!(std::env::var(key).is_err(), "{key} is still set");
        }
    }
}
