//! The image gate: whether the selected adapter takes images, checked before any download.

use super::{AttachmentOutcome, FormatClass};

/// Whether the selected adapter can consume image input, named for its manifest
/// line. A `false` adapter never fetches images — the pixels would have nowhere
/// to go (ADR-0025 §4).
#[derive(Debug, Clone, Copy)]
pub struct ImageCapability<'a> {
    pub accepts: bool,
    pub adapter_name: &'a str,
}

/// Gate an image attachment on the selected adapter's capability, BEFORE any
/// download: an image bound for a no-vision adapter is `not fetched (<adapter>
/// has no image input)` — never a wasted fetch. Returns `None` (proceed) for a
/// non-image class or a vision-capable adapter. Pure so the gate is unit-tested
/// without network.
pub fn gated_image_outcome(
    class: FormatClass,
    images: &ImageCapability<'_>,
) -> Option<AttachmentOutcome> {
    if class == FormatClass::Image && !images.accepts {
        Some(AttachmentOutcome::NotFetched {
            reason: format!("{} has no image input", images.adapter_name),
        })
    } else {
        None
    }
}
