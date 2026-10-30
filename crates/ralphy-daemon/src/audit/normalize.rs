//! Raw device facts to one closed vocabulary (ADR-0074 D6). Each field names
//! the source that answered it. When two sources disagree, the field goes in
//! `conflicts`; the normalizer never picks one silently.
//!
//! The rules follow what seven real devices reported on 2026-10-04 (the
//! fixtures in `tests/fixtures/devices/`): client hints come first, then the
//! page's `userAgentData`, then the user agent string, which several browsers
//! freeze.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::facts::{ClientFacts, Text};
use super::ServerFacts;

mod hardware;
mod software;

use hardware::{form, gpu, iphone_family, ratio, screen_size};
use software::{browser, engine, os_from_hint, os_from_ua, os_version, same_os, Hints};

/// The version of these rules. A line keeps it, so an old line can be
/// normalized again when the rules change.
pub const NORMALIZER: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Windows,
    Macos,
    Ios,
    Ipados,
    Android,
    Linux,
    Chromeos,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Webkit,
    Blink,
    Gecko,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Browser {
    Safari,
    Chrome,
    Chromium,
    Edge,
    Firefox,
    Opera,
    Samsung,
    /// A page inside an app's embedded browser view.
    Webview,
    /// The browser inside a social app (Facebook, Instagram, Line).
    InApp,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    Phone,
    Tablet,
    Desktop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Qualcomm,
    Arm,
    Software,
    Other,
}

/// Where a normalized value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A client hint header, or the page's `userAgentData`.
    Hint,
    /// The user agent string.
    Ua,
    /// A feature the page tested.
    Feature,
    /// Worked out from other normalized values.
    Inferred,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Field<T> {
    pub value: T,
    pub source: Source,
}

fn field<T>(value: T, source: Source) -> Option<Field<T>> {
    Some(Field { value, source })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gpu {
    pub vendor: GpuVendor,
    pub model: String,
}

/// What a line records about a device, in Ralphy's words.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Normalized {
    pub normalizer: u32,
    pub os: Option<Field<Os>>,
    pub os_version: Option<Field<String>>,
    pub browser: Option<Field<Browser>>,
    pub browser_version: Option<String>,
    pub engine: Option<Field<Engine>>,
    pub form: Option<Field<Form>>,
    pub model_hint: Option<Field<String>>,
    pub gpu: Option<Gpu>,
    /// `<short side>x<long side>@<pixel ratio>`, so a turned device reads the
    /// same.
    pub screen: Option<String>,
    pub time_zone: Option<String>,
    pub standalone: Option<bool>,
    pub automation: Option<bool>,
    pub conflicts: Vec<&'static str>,
    /// A hash of the facts that do not change while the device stays the same
    /// (no versions): a new value is a `device_profile_changed`.
    pub profile: String,
}

pub fn normalize(client: &ClientFacts, server: &ServerFacts) -> Normalized {
    let hints = Hints::read(client, server);
    let ua = client
        .ua
        .as_ref()
        .map(Text::as_str)
        .or(server.user_agent.as_deref())
        .unwrap_or("");
    let mut conflicts = Vec::new();

    let os = os_field(&hints, ua, client, &mut conflicts);
    let os_value = os.as_ref().map(|f| f.value);

    let (browser, browser_version) = browser(&hints.brands, ua, os_value);
    let browser_value = browser.as_ref().map(|f| f.value);
    let engine = engine(client, os_value, browser_value, !hints.brands.is_empty());
    if matches!(os_value, Some(Os::Ios | Os::Ipados)) && !hints.brands.is_empty() {
        // Every browser on iOS is WebKit, and WebKit reports no client hints.
        conflicts.push("engine");
    }
    let os_version = os_value.and_then(|os| os_version(os, &hints, ua, browser_value));
    let screen_size = screen_size(client);
    let form = form(&hints, os_value, ua, client, screen_size);
    if form.as_ref().is_some_and(|f| f.value == Form::Phone)
        && screen_size.is_some_and(|(short, _, _)| short >= 600)
    {
        conflicts.push("form");
    }
    let model_hint = model_hint(&hints, os_value, screen_size);
    let gpu = client.gpu.as_ref().and_then(|g| {
        let raw = g.unmasked_renderer.as_ref().or(g.renderer.as_ref())?;
        Some(gpu(raw.as_str()))
    });
    let time_zone = client
        .intl
        .as_ref()
        .and_then(|i| i.time_zone.as_ref())
        .map(|t| t.as_str().to_string());
    let standalone = client
        .media
        .display_mode
        .as_ref()
        .map(|m| m.as_str() == "standalone")
        .or(client.engine_signals.ios_standalone);
    let screen = shown_screen(client, form.as_ref(), screen_size);

    let profile = profile(&[
        &format!("{:?}", os_value),
        &format!("{:?}", browser_value),
        &format!("{:?}", engine.as_ref().map(|f| f.value)),
        &format!("{:?}", form.as_ref().map(|f| f.value)),
        screen.as_deref().unwrap_or(""),
        &format!("{:?}", client.cores),
        &format!("{:?}", client.memory_gb),
        &format!("{:?}", client.touch_points),
        &format!("{:?}", gpu),
        time_zone.as_deref().unwrap_or(""),
        &client
            .languages
            .0
            .iter()
            .map(Text::as_str)
            .collect::<Vec<_>>()
            .join(","),
    ]);

    Normalized {
        normalizer: NORMALIZER,
        os,
        os_version,
        browser,
        browser_version,
        engine,
        form,
        model_hint,
        gpu,
        screen,
        time_zone,
        standalone,
        automation: client.automation,
        conflicts,
        profile,
    }
}

/// The system, from the hints first and the user agent after. A hint that
/// disagrees with the user agent adds the `os` conflict.
fn os_field(
    hints: &Hints,
    ua: &str,
    client: &ClientFacts,
    conflicts: &mut Vec<&'static str>,
) -> Option<Field<Os>> {
    let ua_os = os_from_ua(ua);
    let mut os = match (hints.platform.as_deref().and_then(os_from_hint), ua_os) {
        (Some(h), u) => {
            if u.is_some_and(|u| !same_os(h, u)) {
                conflicts.push("os");
            }
            field(h, Source::Hint)
        }
        (None, Some(u)) => field(u, Source::Ua),
        (None, None) => None,
    };
    // Safari on iPad reports a Mac; a Mac has no touch points.
    if let Some(f) = &mut os {
        if f.value == Os::Macos && client.touch_points.unwrap_or(0) > 1 {
            *f = Field {
                value: Os::Ipados,
                source: Source::Inferred,
            };
        }
    }
    os
}

fn model_hint(
    hints: &Hints,
    os: Option<Os>,
    screen_size: Option<(u32, u32, f64)>,
) -> Option<Field<String>> {
    hints
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .and_then(|m| field(m, Source::Hint))
        .or_else(|| {
            (os == Some(Os::Ios))
                .then_some(())
                .and(screen_size)
                .and_then(iphone_family)
                .and_then(|m| field(m.to_string(), Source::Inferred))
        })
}

/// `<width>x<height>@<pixel ratio>` as the device shows it.
fn shown_screen(
    client: &ClientFacts,
    form: Option<&Field<Form>>,
    screen_size: Option<(u32, u32, f64)>,
) -> Option<String> {
    // A phone or tablet turns, so its sides are sorted; a computer's screen
    // keeps the orientation it reports (a 2560x1080 monitor is not 1080x2560).
    let shown = match form.map(|f| f.value) {
        Some(Form::Desktop) => client
            .screen
            .as_ref()
            .and_then(|s| Some((s.width?, s.height?, client.dpr.unwrap_or(1.0)))),
        _ => screen_size,
    };
    shown.map(|(w, h, dpr)| format!("{w}x{h}@{}", ratio(dpr)))
}

/// The fields of `next` that differ from `prev`, among those the profile
/// reads. `other` when only a fact outside these fields changed (cores,
/// memory, touch points, languages).
pub fn changed_fields(prev: &Normalized, next: &Normalized) -> Vec<&'static str> {
    fn same<T: PartialEq>(a: &Option<Field<T>>, b: &Option<Field<T>>) -> bool {
        a.as_ref().map(|f| &f.value) == b.as_ref().map(|f| &f.value)
    }
    let mut changed = Vec::new();
    for (name, differs) in [
        ("os", !same(&prev.os, &next.os)),
        ("browser", !same(&prev.browser, &next.browser)),
        ("engine", !same(&prev.engine, &next.engine)),
        ("form", !same(&prev.form, &next.form)),
        ("gpu", prev.gpu != next.gpu),
        ("screen", prev.screen != next.screen),
        ("time_zone", prev.time_zone != next.time_zone),
    ] {
        if differs {
            changed.push(name);
        }
    }
    if changed.is_empty() && prev.profile != next.profile {
        changed.push("other");
    }
    changed
}

fn profile(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for p in parts {
        hash.update(p.as_bytes());
        hash.update([0u8]);
    }
    hash.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
