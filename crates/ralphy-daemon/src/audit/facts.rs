//! The device facts a page reports about itself (ADR-0074 D5): a fixed shape
//! where every field may be missing, every string is cut at a fixed length,
//! and every list at a fixed count. Fields outside the shape are dropped.
//! Source `client`: a page can report false facts (D2).

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// The longest string a fact keeps.
const MAX_TEXT_CHARS: usize = 256;
/// The longest list a fact keeps.
const MAX_LIST_LEN: usize = 16;

/// A string cut at [`MAX_TEXT_CHARS`].
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Text(String);

impl Text {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Text(s.chars().take(MAX_TEXT_CHARS).collect()))
    }
}

/// A list cut at [`MAX_LIST_LEN`]; `null` reads as empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct List<T>(pub Vec<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for List<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut v = Option::<Vec<T>>::deserialize(d)?.unwrap_or_default();
        v.truncate(MAX_LIST_LEN);
        Ok(List(v))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ClientFacts {
    /// The tab's holder ID (D11).
    pub holder: Option<Text>,
    pub ua: Option<Text>,
    pub ua_data: Option<UaData>,
    pub platform: Option<Text>,
    pub vendor: Option<Text>,
    pub languages: List<Text>,
    pub intl: Option<IntlFacts>,
    pub tz_offset_min: Option<i32>,
    pub screen: Option<ScreenFacts>,
    pub dpr: Option<f64>,
    pub window: Option<WindowFacts>,
    pub safe_area: Option<SafeArea>,
    pub touch_points: Option<u32>,
    pub cores: Option<u32>,
    pub memory_gb: Option<f64>,
    pub media: MediaFacts,
    pub engine_signals: EngineSignals,
    pub gpu: Option<GpuFacts>,
    pub voices: Option<Voices>,
    pub automation: Option<bool>,
    pub plugins: Option<u32>,
    pub pdf_viewer: Option<bool>,
    pub cookies: Option<bool>,
    pub do_not_track: Option<Text>,
    pub storage: Option<Storage>,
    pub connection: Option<Connection>,
    pub battery: Option<Battery>,
    pub keyboard: Option<BTreeMap<Text, Text>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct UaData {
    pub platform: Option<Text>,
    #[serde(rename = "platformVersion")]
    pub platform_version: Option<Text>,
    pub model: Option<Text>,
    pub mobile: Option<bool>,
    pub architecture: Option<Text>,
    pub bitness: Option<Text>,
    pub brands: List<Brand>,
    #[serde(rename = "fullVersionList")]
    pub full_version_list: List<Brand>,
    #[serde(rename = "formFactors")]
    pub form_factors: List<Text>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Brand {
    pub brand: Text,
    pub version: Text,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct IntlFacts {
    pub time_zone: Option<Text>,
    pub locale: Option<Text>,
    pub calendar: Option<Text>,
    pub numbering_system: Option<Text>,
    pub hour_cycle: Option<Text>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ScreenFacts {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub avail_width: Option<u32>,
    pub avail_height: Option<u32>,
    pub color_depth: Option<u32>,
    pub orientation: Option<Text>,
    pub is_extended: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct WindowFacts {
    pub inner: List<u32>,
    pub outer: List<u32>,
    pub visual_scale: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SafeArea {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MediaFacts {
    pub pointer: Option<Text>,
    pub any_pointer: Option<Text>,
    pub hover: Option<Text>,
    pub color_scheme: Option<Text>,
    pub reduced_motion: Option<Text>,
    pub contrast: Option<Text>,
    pub forced_colors: Option<Text>,
    pub inverted_colors: Option<Text>,
    pub color_gamut: Option<Text>,
    pub dynamic_range: Option<Text>,
    pub display_mode: Option<Text>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct EngineSignals {
    pub webkit_touch_callout: Option<bool>,
    pub gesture_event: Option<bool>,
    pub safari_object: Option<bool>,
    pub ios_standalone: Option<bool>,
    pub moz_appearance: Option<bool>,
    pub chrome_object: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct GpuFacts {
    pub vendor: Option<Text>,
    pub renderer: Option<Text>,
    pub unmasked_vendor: Option<Text>,
    pub unmasked_renderer: Option<Text>,
}

/// The speech voices: their count and the first few, never the whole list.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Voices {
    pub count: u32,
    pub sample: List<Text>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Storage {
    pub quota: Option<f64>,
    pub usage: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Connection {
    #[serde(rename = "type")]
    pub kind: Option<Text>,
    pub effective_type: Option<Text>,
    pub rtt: Option<f64>,
    pub downlink: Option<f64>,
    pub save_data: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Battery {
    pub level: Option<f64>,
    pub charging: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body the page sends (a measured Android phone), shared with
    /// `ui-tests/wb-device.test.mjs`.
    const REPORT: &str = include_str!("../../ui-tests/fixtures/api-device-facts--report.json");

    /// The object keys of `v` as dotted paths. `keyboard` is a map of keys
    /// the page chose, not a field list.
    fn paths(v: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
        if let serde_json::Value::Object(map) = v {
            for (k, child) in map {
                let path = format!("{prefix}{k}");
                if k != "keyboard" {
                    paths(child, &format!("{path}."), out);
                }
                out.push(path);
            }
        }
    }

    #[test]
    fn every_field_the_page_sends_is_a_field_the_daemon_reads() {
        let sent: serde_json::Value = serde_json::from_str(REPORT).unwrap();
        let read: ClientFacts = serde_json::from_str(REPORT).unwrap();
        let kept = serde_json::to_value(&read).unwrap();
        let (mut want, mut got) = (Vec::new(), Vec::new());
        paths(&sent, "", &mut want);
        paths(&kept, "", &mut got);
        let dropped: Vec<&String> = want.iter().filter(|p| !got.contains(p)).collect();
        assert!(dropped.is_empty(), "the daemon drops {dropped:?}");
        assert_eq!(
            read.holder.unwrap().as_str(),
            "0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn a_long_string_and_a_long_list_are_cut() {
        let body = serde_json::json!({
            "ua": "u".repeat(1000),
            "languages": vec!["pt-BR"; 40],
            "voices": null,
        });
        let facts: ClientFacts = serde_json::from_value(body).unwrap();
        assert_eq!(facts.ua.unwrap().as_str().len(), MAX_TEXT_CHARS);
        assert_eq!(facts.languages.0.len(), MAX_LIST_LEN);
    }
}
