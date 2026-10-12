use super::*;
use axum::extract::Query;

/// The query as axum reads it from the upgrade request.
fn read(query: &str) -> SessionQuery {
    let uri = format!("/ws/session?{query}").parse().expect("a test URI");
    Query::<SessionQuery>::try_from_uri(&uri)
        .expect("the query is read, whatever the link values")
        .0
}

#[test]
fn the_tab_carries_the_link_and_a_bad_value_is_absent() {
    let tab = read("id=7&holder=tab-1&gzip=1&net=4g&net_rtt=100&net_down=1.75").tab(None);
    assert_eq!(tab.holder.as_deref(), Some("tab-1"));
    assert!(tab.gzip);
    let reported = Link {
        effective_type: Some(EffectiveType::G4),
        rtt_ms: Some(100),
        downlink_mbps: Some(1.75),
    };
    assert_eq!(tab.link, reported);
    // Safari sends no keys; a value no browser gives is absent alone.
    assert_eq!(read("id=7").tab(None).link, Link::default());
    let mixed = read("id=7&net=3g&net_rtt=x&net_down=NaN").tab(None);
    let only_type = Link {
        effective_type: Some(EffectiveType::G3),
        ..Link::default()
    };
    assert_eq!(mixed.link, only_type);
    assert_eq!(
        read("id=7&net=5g&net_rtt=-1&net_down=-2").tab(None).link,
        Link::default()
    );
}
