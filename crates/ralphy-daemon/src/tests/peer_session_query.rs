//! The console open a daemon relays to a peer for a launch or a reattach.

use super::*;
use serde_json::json;

fn base() -> SessionQuery {
    SessionQuery {
        repo: Some("x".into()),
        ..SessionQuery::default()
    }
}

type Row = (&'static str, SessionQuery, serde_json::Value);

fn launch() -> SessionQuery {
    SessionQuery {
        agent: Some("claude".into()),
        ..base()
    }
}

fn reattach() -> SessionQuery {
    SessionQuery {
        id: Some(7),
        ..base()
    }
}

fn console() -> SessionQuery {
    SessionQuery {
        console: Some(1),
        ..base()
    }
}

fn checkout_and_console_rows() -> Vec<Row> {
    vec![
        (
            "launch with a checkout",
            SessionQuery {
                checkout: Some("wt-a".into()),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude", "checkout": "wt-a"}),
        ),
        (
            "launch with no checkout",
            launch(),
            json!({"repo": "owner/repo", "agent": "claude"}),
        ),
        (
            "reattach drops the checkout",
            SessionQuery {
                checkout: Some("wt-a".into()),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo"}),
        ),
        (
            "console launch trims the command and keeps the holder",
            SessionQuery {
                command: Some(" htop ".into()),
                holder: Some("tab-1".into()),
                ..console()
            },
            json!({"console": 1, "repo": "owner/repo", "command": "htop", "holder": "tab-1"}),
        ),
        (
            "console launch with no holder",
            SessionQuery {
                command: None,
                ..console()
            },
            json!({"console": 1, "repo": "owner/repo"}),
        ),
    ]
}

fn holder_and_name_rows() -> Vec<Row> {
    let long_name = "é".repeat(41);
    let cut_name = "é".repeat(40);
    vec![
        (
            "reattach holder",
            SessionQuery {
                holder: Some("tab-1_A".into()),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "holder": "tab-1_A"}),
        ),
        (
            "launch holder",
            SessionQuery {
                holder: Some("tab-1_A".into()),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude", "holder": "tab-1_A"}),
        ),
        (
            "launch name",
            SessionQuery {
                name: Some("fincal #1".into()),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude", "name": "fincal #1"}),
        ),
        (
            "launch with an empty name",
            SessionQuery {
                name: Some(String::new()),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude"}),
        ),
        (
            "launch name is cut to 40 characters",
            SessionQuery {
                name: Some(long_name),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude", "name": cut_name}),
        ),
        (
            "reattach drops the name",
            SessionQuery {
                name: Some("fincal #1".into()),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo"}),
        ),
    ]
}

fn record_and_gzip_rows() -> Vec<Row> {
    vec![
        (
            "console record",
            SessionQuery {
                record: Some("w-a_1".into()),
                ..console()
            },
            json!({"console": 1, "repo": "owner/repo", "record": "w-a_1"}),
        ),
        (
            "agent record",
            SessionQuery {
                record: Some("w-a_1".into()),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude", "record": "w-a_1"}),
        ),
        (
            "reattach drops the record",
            SessionQuery {
                record: Some("w-a_1".into()),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo"}),
        ),
        (
            "launch gzip",
            SessionQuery {
                gzip: Some(1),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude", "gzip": 1}),
        ),
        (
            "watch gzip",
            SessionQuery {
                watch: Some(1),
                gzip: Some(1),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "watch": 1, "gzip": 1}),
        ),
        (
            "console gzip",
            SessionQuery {
                gzip: Some(1),
                ..console()
            },
            json!({"console": 1, "repo": "owner/repo", "gzip": 1}),
        ),
        (
            "watch without gzip",
            SessionQuery {
                watch: Some(1),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "watch": 1}),
        ),
        (
            "only gzip 1 asks",
            SessionQuery {
                watch: Some(1),
                gzip: Some(0),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "watch": 1}),
        ),
    ]
}

fn attempt_and_since_rows() -> Vec<Row> {
    vec![
        (
            "reattach holder and attempt",
            SessionQuery {
                holder: Some("tab-A".into()),
                attempt: Some(12),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "holder": "tab-A", "attempt": 12}),
        ),
        (
            "launch holder and attempt",
            SessionQuery {
                holder: Some("tab-A".into()),
                attempt: Some(12),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude", "holder": "tab-A", "attempt": 12}),
        ),
        (
            "console holder and attempt",
            SessionQuery {
                holder: Some("tab-A".into()),
                attempt: Some(12),
                ..console()
            },
            json!({"console": 1, "repo": "owner/repo", "holder": "tab-A", "attempt": 12}),
        ),
        (
            "no holder, no attempt",
            SessionQuery {
                attempt: Some(12),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo"}),
        ),
        (
            "reattach since",
            SessionQuery {
                since: Some(4096),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "since": 4096}),
        ),
        (
            "watch since",
            SessionQuery {
                watch: Some(1),
                since: Some(4096),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "watch": 1, "since": 4096}),
        ),
        (
            "a terminal that shows no byte yet resumes at 0",
            SessionQuery {
                since: Some(0),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo", "since": 0}),
        ),
        (
            "a launch sends no since",
            SessionQuery {
                since: Some(4096),
                ..launch()
            },
            json!({"repo": "owner/repo", "agent": "claude"}),
        ),
    ]
}

/// What the relay sends the peer for each shape of open. The owning daemon
/// keeps the writer slot, the session output and the console record, so the
/// relay forwards the checkout, name and record on a launch only, the holder
/// and attempt together, `since` on a reattach, and `gzip: 1` on every shape;
/// a malformed holder or record is dropped, not passed on.
#[test]
fn peer_session_query_forwards_each_shape() {
    let mut rows = [
        checkout_and_console_rows(),
        holder_and_name_rows(),
        record_and_gzip_rows(),
        attempt_and_since_rows(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let bad_id = "x".repeat(65);
    for bad in ["", "a&takeover=1", bad_id.as_str()] {
        rows.push((
            "malformed holder",
            SessionQuery {
                holder: Some(bad.into()),
                ..reattach()
            },
            json!({"id": 7, "repo": "owner/repo"}),
        ));
        rows.push((
            "malformed record",
            SessionQuery {
                record: Some(bad.into()),
                ..console()
            },
            json!({"console": 1, "repo": "owner/repo"}),
        ));
    }
    for (name, query, want) in rows {
        assert_eq!(peer_session_open(&query, "owner/repo"), want, "{name}");
    }
}

/// The console name is cut to the desk's 40 characters on a char boundary,
/// and an empty one is no name at all.
#[test]
fn session_query_name_is_cut_to_40_and_empty_is_absent() {
    let query = |name: Option<String>| SessionQuery {
        repo: Some("x".into()),
        agent: Some("claude".into()),
        id: None,
        takeover: None,
        watch: None,
        console: None,
        checkout: None,
        command: None,
        holder: None,
        name,
        record: None,
        gzip: None,
        attempt: None,
        since: None,
        net: None,
        net_rtt: None,
        net_down: None,
    };
    let long = "é".repeat(40);
    assert_eq!(query(Some("é".repeat(41))).name(), Some(long.as_str()));
    assert_eq!(query(Some(String::new())).name(), None);
    assert_eq!(query(None).name(), None);
    assert_eq!(query(Some("fincal #1".into())).name(), Some("fincal #1"));
}

/// A peer on an older build sends no `checkout`; the listing must still
/// parse, and a row without one serialises without the key.
#[test]
fn a_peer_sessions_body_without_checkout_still_parses() {
    let body = concat!(
        r#"[{"id":1,"repo":"o/r","agent":"claude","kind":"agent","#,
        r#""started_at":1,"daemon_id":"d","environment":"Windows"}]"#
    );
    let rows: Vec<HostedSessionInfo> = serde_json::from_str(body).unwrap();
    assert_eq!(rows[0].checkout, None);
    let back = serde_json::to_value(&rows[0]).unwrap();
    assert!(back.get("checkout").is_none(), "{back}");

    let with = body.replacen(
        r#""environment":"Windows""#,
        r#""environment":"Windows","checkout":"wt-a""#,
        1,
    );
    let rows: Vec<HostedSessionInfo> = serde_json::from_str(&with).unwrap();
    assert_eq!(rows[0].checkout.as_deref(), Some("wt-a"));
}
