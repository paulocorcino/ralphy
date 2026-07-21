//! The Cursor Agent CLI adapter. See docs/adr/0042.

mod guards;
mod settings;

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
