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

use super::facts::{Brand, ClientFacts, List, Text};
use super::ServerFacts;

/// The version of these rules. A line keeps it, so an old line can be
/// normalized again when the rules change.
pub const NORMALIZER: u32 = 1;

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
    let model_hint = hints
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .and_then(|m| field(m, Source::Hint))
        .or_else(|| {
            (os_value == Some(Os::Ios))
                .then_some(())
                .and(screen_size)
                .and_then(iphone_family)
                .and_then(|m| field(m.to_string(), Source::Inferred))
        });
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
    let screen = screen_size.map(|(short, long, dpr)| format!("{short}x{long}@{}", ratio(dpr)));

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

/// The client hints, from the headers when the browser sent them, else from
/// the page's `userAgentData`.
struct Hints {
    platform: Option<String>,
    platform_version: Option<String>,
    model: Option<String>,
    mobile: Option<bool>,
    form_factor: Option<String>,
    brands: Vec<(String, String)>,
}

impl Hints {
    fn read(client: &ClientFacts, server: &ServerFacts) -> Hints {
        let header = |name: &str| {
            server
                .client_hints
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
        };
        let data = client.ua_data.as_ref();
        let text = |t: Option<&Text>| t.map(|t| t.as_str().to_string());
        let brands = header("full-version-list")
            .or_else(|| header("brands"))
            .map(|v| brands_from_header(&v))
            .filter(|b| !b.is_empty())
            .or_else(|| {
                data.map(|d| {
                    let list: &List<Brand> = if d.full_version_list.0.is_empty() {
                        &d.brands
                    } else {
                        &d.full_version_list
                    };
                    list.0
                        .iter()
                        .map(|b| (b.brand.as_str().to_string(), b.version.as_str().to_string()))
                        .collect()
                })
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|(name, _)| !is_grease(name))
            .collect();
        Hints {
            platform: header("platform").or_else(|| text(data.and_then(|d| d.platform.as_ref()))),
            platform_version: header("platform-version")
                .or_else(|| text(data.and_then(|d| d.platform_version.as_ref()))),
            model: header("model").or_else(|| text(data.and_then(|d| d.model.as_ref()))),
            mobile: header("mobile")
                .map(|v| v == "?1")
                .or_else(|| data.and_then(|d| d.mobile)),
            form_factor: header("form-factors")
                .and_then(|v| {
                    v.split(',')
                        .next()
                        .map(|f| f.trim().trim_matches('"').to_string())
                })
                .or_else(|| data.and_then(|d| text(d.form_factors.0.first()))),
            brands,
        }
    }
}

/// `"Microsoft Edge";v="153.0.4234.48", "Chromium";v="153"` to name and version.
fn brands_from_header(value: &str) -> Vec<(String, String)> {
    value
        .split(',')
        .filter_map(|part| {
            let (name, version) = part.split_once(";v=")?;
            Some((
                name.trim().trim_matches('"').to_string(),
                version.trim().trim_matches('"').to_string(),
            ))
        })
        .collect()
}

/// The made-up brand Chromium adds so that sites do not match on the list
/// (`Not/A)Brand`, `Not_A Brand`).
fn is_grease(name: &str) -> bool {
    name.starts_with("Not") && name.contains("Brand")
}

fn os_from_hint(platform: &str) -> Option<Os> {
    Some(match platform {
        "Windows" => Os::Windows,
        "macOS" => Os::Macos,
        "Android" => Os::Android,
        "iOS" => Os::Ios,
        "Chrome OS" | "ChromeOS" => Os::Chromeos,
        "Linux" => Os::Linux,
        "" => return None,
        _ => Os::Other,
    })
}

fn os_from_ua(ua: &str) -> Option<Os> {
    Some(if ua.contains("iPhone") || ua.contains("iPod") {
        Os::Ios
    } else if ua.contains("iPad") {
        Os::Ipados
    } else if ua.contains("Android") {
        Os::Android
    } else if ua.contains("Windows") {
        Os::Windows
    } else if ua.contains("CrOS") {
        Os::Chromeos
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        Os::Macos
    } else if ua.contains("Linux") {
        Os::Linux
    } else {
        return None;
    })
}

/// Whether a hint and the user agent name the same system. Android's user
/// agent says Linux too.
fn same_os(hint: Os, ua: Os) -> bool {
    hint == ua || (hint == Os::Android && ua == Os::Linux)
}

fn browser(
    brands: &[(String, String)],
    ua: &str,
    os: Option<Os>,
) -> (Option<Field<Browser>>, Option<String>) {
    let named = |name: &str| match name {
        "Microsoft Edge" => Some(Browser::Edge),
        "Google Chrome" => Some(Browser::Chrome),
        "Opera" => Some(Browser::Opera),
        "Samsung Internet" => Some(Browser::Samsung),
        _ => None,
    };
    if let Some((b, v)) = brands
        .iter()
        .find_map(|(n, v)| named(n).map(|b| (b, v)))
        .or_else(|| {
            brands
                .iter()
                .find(|(n, _)| n == "Chromium")
                .map(|(_, v)| (Browser::Chromium, v))
        })
    {
        return (field(b, Source::Hint), Some(major_minor(v)));
    }
    const TOKENS: &[(&str, Browser)] = &[
        ("EdgiOS/", Browser::Edge),
        ("EdgA/", Browser::Edge),
        ("Edg/", Browser::Edge),
        ("OPiOS/", Browser::Opera),
        ("OPR/", Browser::Opera),
        ("SamsungBrowser/", Browser::Samsung),
        ("CriOS/", Browser::Chrome),
        ("FxiOS/", Browser::Firefox),
        ("Firefox/", Browser::Firefox),
    ];
    if ua.contains("; wv)") {
        return (field(Browser::Webview, Source::Ua), None);
    }
    if ["FBAN", "FBAV", "Instagram", "Line/"]
        .iter()
        .any(|t| ua.contains(t))
    {
        return (field(Browser::InApp, Source::Ua), None);
    }
    if let Some((token, b)) = TOKENS.iter().find(|(t, _)| ua.contains(t)) {
        return (field(*b, Source::Ua), ua_version(ua, token));
    }
    if ua.contains("Chrome/") {
        return (
            field(Browser::Chrome, Source::Ua),
            ua_version(ua, "Chrome/"),
        );
    }
    if ua.contains("Safari/") && ua.contains("Version/") {
        return (
            field(Browser::Safari, Source::Ua),
            ua_version(ua, "Version/"),
        );
    }
    if matches!(os, Some(Os::Ios | Os::Ipados)) && ua.contains("AppleWebKit") {
        return (field(Browser::Webview, Source::Ua), None);
    }
    if ua.is_empty() {
        (None, None)
    } else {
        (field(Browser::Other, Source::Ua), None)
    }
}

fn engine(
    client: &ClientFacts,
    os: Option<Os>,
    browser: Option<Browser>,
    has_hints: bool,
) -> Option<Field<Engine>> {
    if matches!(os, Some(Os::Ios | Os::Ipados)) {
        return field(Engine::Webkit, Source::Inferred);
    }
    let s = &client.engine_signals;
    if s.webkit_touch_callout == Some(true)
        || s.safari_object == Some(true)
        || s.gesture_event == Some(true)
    {
        return field(Engine::Webkit, Source::Feature);
    }
    if s.moz_appearance == Some(true) {
        return field(Engine::Gecko, Source::Feature);
    }
    if has_hints {
        return field(Engine::Blink, Source::Feature);
    }
    match browser? {
        Browser::Safari => field(Engine::Webkit, Source::Inferred),
        Browser::Firefox => field(Engine::Gecko, Source::Inferred),
        Browser::Chrome
        | Browser::Chromium
        | Browser::Edge
        | Browser::Opera
        | Browser::Samsung
        | Browser::Webview => field(Engine::Blink, Source::Inferred),
        Browser::InApp | Browser::Other => None,
    }
}

fn os_version(os: Os, hints: &Hints, ua: &str, browser: Option<Browser>) -> Option<Field<String>> {
    let hinted = hints.platform_version.as_deref().filter(|v| !v.is_empty());
    match os {
        Os::Windows => {
            // The hint's major is 13 or more on Windows 11, 1 to 10 on
            // Windows 10. The user agent says `Windows NT 10.0` on both.
            let major: u32 = hinted?.split('.').next()?.parse().ok()?;
            let name = match major {
                13.. => "11",
                1..=12 => "10",
                0 => return None,
            };
            field(name.to_string(), Source::Hint)
        }
        Os::Android => hinted
            .and_then(|v| field(major_minor(v), Source::Hint))
            .or_else(|| {
                // Chrome freezes `Android 10; K`.
                let v = ua.split("Android ").nth(1)?.split([';', ')']).next()?;
                (v != "10" || !ua.contains("; K)"))
                    .then(|| major_minor(v))
                    .and_then(|v| field(v, Source::Ua))
            }),
        Os::Ios | Os::Ipados => {
            // Safari freezes the system version in `iPhone OS 18_7`; its own
            // `Version/` follows the system.
            if browser == Some(Browser::Safari) {
                return ua_version(ua, "Version/").and_then(|v| field(v, Source::Ua));
            }
            let v = ua
                .split(" OS ")
                .nth(1)?
                .split(' ')
                .next()?
                .replace('_', ".");
            field(major_minor(&v), Source::Ua)
        }
        // The user agent says `10_15_7` on every Mac.
        Os::Macos | Os::Linux | Os::Chromeos | Os::Other => {
            hinted.and_then(|v| field(major_minor(v), Source::Hint))
        }
    }
}

fn form(
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
fn screen_size(client: &ClientFacts) -> Option<(u32, u32, f64)> {
    let s = client.screen.as_ref()?;
    let (w, h) = (s.width?, s.height?);
    Some((w.min(h), w.max(h), client.dpr.unwrap_or(1.0)))
}

fn ratio(dpr: f64) -> String {
    let text = format!("{dpr:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The iPhones that share one screen size and pixel ratio. Apple gives a page
/// no model, so this is the closest a line can say.
fn iphone_family((short, long, dpr): (u32, u32, f64)) -> Option<&'static str> {
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
fn gpu(raw: &str) -> Gpu {
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
    Gpu {
        vendor,
        model: model.to_string(),
    }
}

/// The version after `token`, as `major.minor`.
fn ua_version(ua: &str, token: &str) -> Option<String> {
    let v = ua.split(token).nth(1)?.split([' ', ';', ')']).next()?;
    (!v.is_empty()).then(|| major_minor(v))
}

/// `12.0.0` to `12`, `26.6.1` to `26.6`.
fn major_minor(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or("");
    match parts.next() {
        Some(minor) if minor != "0" && !minor.is_empty() => format!("{major}.{minor}"),
        _ => major.to_string(),
    }
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
