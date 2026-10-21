use crate::audit::facts::{ClientFacts, Text};

use super::software::Hints;
use super::{field, Field, Form, Gpu, GpuVendor, Os, Source};

pub(super) fn form(
    hints: &Hints,
    os: Option<Os>,
    ua: &str,
    client: &ClientFacts,
    screen: Option<(u32, u32, f64)>,
) -> Option<Field<Form>> {
    match hints.form_factor.as_deref() {
        Some("Mobile") => return field(Form::Phone, Source::Hint),
        Some("Tablet") => return field(Form::Tablet, Source::Hint),
        Some("Desktop") => return field(Form::Desktop, Source::Hint),
        _ => {}
    }
    if hints.mobile == Some(true) {
        return field(Form::Phone, Source::Hint);
    }
    match os {
        Some(Os::Ios) => return field(Form::Phone, Source::Inferred),
        Some(Os::Ipados) => return field(Form::Tablet, Source::Inferred),
        Some(Os::Android) => {
            let f = if ua.contains("Mobile") {
                Form::Phone
            } else {
                Form::Tablet
            };
            return field(f, Source::Ua);
        }
        Some(Os::Windows | Os::Macos | Os::Linux | Os::Chromeos) => {
            return field(Form::Desktop, Source::Inferred)
        }
        Some(Os::Other) | None => {}
    }
    let coarse = client.media.pointer.as_ref().map(Text::as_str) == Some("coarse");
    let (short, _, _) = screen?;
    let f = match (coarse, short) {
        (true, ..600) => Form::Phone,
        (true, _) => Form::Tablet,
        (false, _) => Form::Desktop,
    };
    field(f, Source::Feature)
}

/// Short side, long side and pixel ratio. Safari keeps `screen` in portrait
/// on an iPhone and reports it turned on an iPad, so the sides are sorted.
pub(super) fn screen_size(client: &ClientFacts) -> Option<(u32, u32, f64)> {
    let s = client.screen.as_ref()?;
    let (w, h) = (s.width?, s.height?);
    Some((w.min(h), w.max(h), client.dpr.unwrap_or(1.0)))
}

pub(super) fn ratio(dpr: f64) -> String {
    let text = format!("{dpr:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The iPhones that share one screen size and pixel ratio. Apple gives a page
/// no model, so this is the closest a line can say.
pub(super) fn iphone_family((short, long, dpr): (u32, u32, f64)) -> Option<&'static str> {
    let dpr = dpr.round() as u32;
    Some(match (short, long, dpr) {
        (375, 667, 2) => "iPhone 6/7/8/SE",
        (414, 736, 3) => "iPhone 6/7/8 Plus",
        (375, 812, 3) => "iPhone X/XS/11 Pro/12 mini/13 mini",
        (414, 896, 2) => "iPhone XR/11",
        (414, 896, 3) => "iPhone XS Max/11 Pro Max",
        (390, 844, 3) => "iPhone 12/12 Pro/13/13 Pro/14",
        (428, 926, 3) => "iPhone 12 Pro Max/13 Pro Max/14 Plus",
        (393, 852, 3) => "iPhone 14 Pro/15/15 Pro/16",
        (430, 932, 3) => "iPhone 14 Pro Max/15 Plus/15 Pro Max/16 Plus",
        (402, 874, 3) => "iPhone 16 Pro/17/17 Pro",
        (440, 956, 3) => "iPhone 16 Pro Max/17 Pro Max",
        _ => return None,
    })
}

/// `ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002487) Direct3D11 vs_5_0 ps_5_0, D3D11)`
/// to the vendor and `NVIDIA GeForce RTX 3060`.
pub(super) fn gpu(raw: &str) -> Gpu {
    let inner = raw
        .strip_prefix("ANGLE (")
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or(raw);
    let parts: Vec<&str> = inner.split(", ").collect();
    let model = if parts.len() >= 2 { parts[1] } else { inner };
    let model = model
        .split(" (0x")
        .next()
        .unwrap_or(model)
        .split(" Direct3D")
        .next()
        .unwrap_or(model)
        .split(", ")
        .next()
        .unwrap_or(model)
        .trim();
    // The cut at ` (0x` can leave an opening bracket of the model's own name.
    let open = model
        .matches('(')
        .count()
        .saturating_sub(model.matches(')').count());
    let model = format!("{model}{}", ")".repeat(open));
    let lower = inner.to_ascii_lowercase();
    let vendor = if lower.contains("nvidia") {
        GpuVendor::Nvidia
    } else if lower.contains("amd") || lower.contains("radeon") {
        GpuVendor::Amd
    } else if lower.contains("intel") {
        GpuVendor::Intel
    } else if lower.contains("apple") {
        GpuVendor::Apple
    } else if lower.contains("qualcomm") || lower.contains("adreno") {
        GpuVendor::Qualcomm
    } else if lower.contains("mali") || lower.contains("arm") {
        GpuVendor::Arm
    } else if lower.contains("swiftshader") || lower.contains("llvmpipe") {
        GpuVendor::Software
    } else {
        GpuVendor::Other
    };
    Gpu { vendor, model }
}
