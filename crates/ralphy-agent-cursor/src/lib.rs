//! The Cursor Agent CLI adapter. See docs/adr/0042.

mod auth;
mod command;
mod guards;
mod settings;

/// Whether the operator is logged into Cursor, from the vendor's own structured
/// answer (ADR-0042 D8) — what `ralphy init`'s gate reports.
pub use auth::{cursor_status_verdict, probe_cursor_login, CURSOR_AUTH_ERROR_MSG};

/// Locating the vendor's binary, which is on `PATH` on neither platform
/// (ADR-0042 D14) — `ralphy init`'s presence gate goes through this.
pub use command::locate_cursor;

/// Persisted settings for `--agent cursor` (ADR-0042 D6). See [`CursorSettings`].
pub use settings::CursorSettings;

/// `false` (ADR-0042 D15): no attachment channel appears anywhere in Cursor's
/// headless surface, so a triage attachment fetched per ADR-0025 §4 has no
/// delivery path on this vendor.
pub const ACCEPTS_IMAGES: bool = false;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_images_is_false() {
        assert!(!ACCEPTS_IMAGES, "ADR-0042 D15");
    }
}
