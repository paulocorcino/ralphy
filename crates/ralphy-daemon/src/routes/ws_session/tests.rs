use super::*;
use serde_json::json;

/// The open payload as a console channel reads it.
fn read(payload: serde_json::Value) -> SessionQuery {
    serde_json::from_value(payload).expect("the payload is read, whatever the link values")
}

#[test]
fn the_tab_carries_the_link_and_a_bad_value_is_absent() {
    let tab = read(json!({
        "id": 7, "holder": "tab-1", "gzip": 1,
        "net": "4g", "net_rtt": "100", "net_down": "1.75"
    }))
    .tab(None);
    assert_eq!(tab.holder.as_deref(), Some("tab-1"));
    assert!(tab.gzip);
    let reported = Link {
        effective_type: Some(EffectiveType::G4),
        rtt_ms: Some(100),
        downlink_mbps: Some(1.75),
    };
    assert_eq!(tab.link, reported);
    // Safari sends no keys; a value no browser gives is absent alone.
    assert_eq!(read(json!({"id": 7})).tab(None).link, Link::default());
    let mixed = read(json!({"id": 7, "net": "3g", "net_rtt": "x", "net_down": "NaN"})).tab(None);
    let only_type = Link {
        effective_type: Some(EffectiveType::G3),
        ..Link::default()
    };
    assert_eq!(mixed.link, only_type);
    assert_eq!(
        read(json!({"id": 7, "net": "5g", "net_rtt": "-1", "net_down": "-2"}))
            .tab(None)
            .link,
        Link::default()
    );
}
