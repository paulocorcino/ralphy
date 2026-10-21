use crate::audit::facts::{Brand, ClientFacts, List, Text};
use crate::audit::ServerFacts;

use super::{field, Browser, Engine, Field, Os, Source};

/// The client hints, from the headers when the browser sent them, else from
/// the page's `userAgentData`.
pub(super) struct Hints {
    pub(super) platform: Option<String>,
    pub(super) platform_version: Option<String>,
    pub(super) model: Option<String>,
    pub(super) mobile: Option<bool>,
    pub(super) form_factor: Option<String>,
    pub(super) brands: Vec<(String, String)>,
}

impl Hints {
    pub(super) fn read(client: &ClientFacts, server: &ServerFacts) -> Hints {
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

pub(super) fn os_from_hint(platform: &str) -> Option<Os> {
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

pub(super) fn os_from_ua(ua: &str) -> Option<Os> {
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
pub(super) fn same_os(hint: Os, ua: Os) -> bool {
    hint == ua || (hint == Os::Android && ua == Os::Linux)
}

pub(super) fn browser(
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

pub(super) fn engine(
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

pub(super) fn os_version(
    os: Os,
    hints: &Hints,
    ua: &str,
    browser: Option<Browser>,
) -> Option<Field<String>> {
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
