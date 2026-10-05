use axum::http::{HeaderMap, HeaderName, HeaderValue};

use super::*;

/// A device as the probe saw it: the request headers and the page's facts.
fn fixture(name: &str) -> (ClientFacts, ServerFacts) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/devices")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut headers = HeaderMap::new();
    for (k, v) in json["headers"].as_object().unwrap() {
        headers.insert(
            HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v.as_str().unwrap()).unwrap(),
        );
    }
    let client = serde_json::from_value(json["client"].clone()).unwrap();
    (client, ServerFacts::from_headers(&headers))
}

fn of(name: &str) -> Normalized {
    let (client, server) = fixture(name);
    normalize(&client, &server)
}

fn value<T: Copy>(f: &Option<Field<T>>) -> Option<T> {
    f.as_ref().map(|f| f.value)
}

fn text(f: &Option<Field<String>>) -> Option<&str> {
    f.as_ref().map(|f| f.value.as_str())
}

#[test]
fn each_measured_device_reads_as_what_it_is() {
    struct Want {
        name: &'static str,
        os: Os,
        version: Option<&'static str>,
        browser: Browser,
        engine: Engine,
        form: Form,
        model: Option<&'static str>,
        gpu: GpuVendor,
        screen: &'static str,
    }
    let wants = [
        Want {
            name: "windows-edge",
            os: Os::Windows,
            version: Some("11"),
            browser: Browser::Edge,
            engine: Engine::Blink,
            form: Form::Desktop,
            model: None,
            gpu: GpuVendor::Nvidia,
            screen: "1080x2560@1",
        },
        Want {
            name: "android-chrome",
            os: Os::Android,
            version: Some("12"),
            browser: Browser::Chrome,
            engine: Engine::Blink,
            form: Form::Phone,
            model: Some("moto g(30)"),
            gpu: GpuVendor::Qualcomm,
            screen: "485x1076@1.49",
        },
        Want {
            name: "iphone-safari",
            os: Os::Ios,
            version: Some("26.6"),
            browser: Browser::Safari,
            engine: Engine::Webkit,
            form: Form::Phone,
            model: Some("iPhone 14 Pro/15/15 Pro/16"),
            gpu: GpuVendor::Apple,
            screen: "393x852@3",
        },
        Want {
            name: "ipad-safari",
            os: Os::Ipados,
            version: Some("26.6"),
            browser: Browser::Safari,
            engine: Engine::Webkit,
            form: Form::Tablet,
            model: None,
            gpu: GpuVendor::Apple,
            screen: "954x1373@2",
        },
        Want {
            name: "macos-safari",
            os: Os::Macos,
            version: None,
            browser: Browser::Safari,
            engine: Engine::Webkit,
            form: Form::Desktop,
            model: None,
            gpu: GpuVendor::Apple,
            screen: "800x1280@2",
        },
    ];
    for w in wants {
        let n = of(w.name);
        let name = w.name;
        assert_eq!(value(&n.os), Some(w.os), "{name} os");
        assert_eq!(text(&n.os_version), w.version, "{name} os version");
        assert_eq!(value(&n.browser), Some(w.browser), "{name} browser");
        assert_eq!(value(&n.engine), Some(w.engine), "{name} engine");
        assert_eq!(value(&n.form), Some(w.form), "{name} form");
        assert_eq!(text(&n.model_hint), w.model, "{name} model");
        assert_eq!(n.gpu.as_ref().map(|g| g.vendor), Some(w.gpu), "{name} gpu");
        assert_eq!(n.screen.as_deref(), Some(w.screen), "{name} screen");
        assert!(n.conflicts.is_empty(), "{name}: {:?}", n.conflicts);
    }
}

#[test]
fn the_gpu_model_loses_the_angle_wrapping() {
    assert_eq!(
        of("windows-edge").gpu.unwrap().model,
        "NVIDIA GeForce RTX 3060"
    );
    assert_eq!(of("android-chrome").gpu.unwrap().model, "Adreno (TM) 610");
}

#[test]
fn the_profile_stays_when_only_the_network_or_the_keyboard_changes() {
    assert_eq!(
        of("iphone-safari").profile,
        of("iphone-safari-mobile-network").profile
    );
    assert_eq!(
        of("ipad-safari").profile,
        of("ipad-safari-keyboard").profile
    );
    assert_ne!(of("iphone-safari").profile, of("ipad-safari").profile);
}

fn from_ua(ua: &str) -> Normalized {
    let client = ClientFacts {
        ua: serde_json::from_value(serde_json::json!(ua)).unwrap(),
        ..ClientFacts::default()
    };
    normalize(&client, &ServerFacts::default())
}

#[test]
fn chrome_on_an_iphone_is_chrome_on_webkit() {
    let n = from_ua(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/148.0.7778.73 Mobile/15E148 Safari/604.1",
    );
    assert_eq!(value(&n.browser), Some(Browser::Chrome));
    assert_eq!(n.browser_version.as_deref(), Some("148"));
    assert_eq!(value(&n.engine), Some(Engine::Webkit));
    assert_eq!(text(&n.os_version), Some("18.7"));
}

#[test]
fn an_android_app_view_and_a_frozen_android_version() {
    let n = from_ua(
        "Mozilla/5.0 (Linux; Android 10; K; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/148.0.0.0 Mobile Safari/537.36",
    );
    assert_eq!(value(&n.browser), Some(Browser::Webview));
    assert_eq!(value(&n.form), Some(Form::Phone));
    let n = from_ua(
        "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Mobile Safari/537.36",
    );
    assert_eq!(text(&n.os_version), None, "Chrome freezes `Android 10; K`");
    let n = from_ua("Mozilla/5.0 (Android 14; Mobile; rv:131.0) Gecko/131.0 Firefox/131.0");
    assert_eq!(text(&n.os_version), Some("14"));
    assert_eq!(value(&n.browser), Some(Browser::Firefox));
    assert_eq!(value(&n.engine), Some(Engine::Gecko));
}

#[test]
fn an_iphone_user_agent_on_chromium_is_a_conflict() {
    // Device emulation in a desktop browser: the user agent says iPhone, the
    // client hints say Windows.
    let (mut client, mut server) = fixture("windows-edge");
    client.ua = serde_json::from_value(serde_json::json!(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.6 Mobile/15E148 Safari/604.1"
    ))
    .unwrap();
    server.user_agent = None;
    let n = normalize(&client, &server);
    assert_eq!(value(&n.os), Some(Os::Windows), "the hint wins");
    assert_eq!(n.conflicts, ["os"]);
}
