//! The query a daemon relays to a peer for a console launch or a reattach.

use super::*;

/// The peer relay forwards the checkout on the LAUNCH shape only; a reattach
/// names a record the peer already owns, and the no-checkout launch stays
/// byte-identical for an older peer.
#[test]
fn peer_session_query_forwards_the_checkout_only_when_present() {
    let launch = |checkout: Option<&str>| SessionQuery {
        repo: Some("x".into()),
        agent: Some("claude".into()),
        checkout: checkout.map(str::to_string),
        ..SessionQuery::default()
    };
    assert_eq!(
        peer_session_query(&launch(Some("wt-a")), "owner/repo"),
        "repo=owner%2Frepo&agent=claude&checkout=wt-a"
    );
    assert_eq!(
        peer_session_query(&launch(None), "owner/repo"),
        "repo=owner%2Frepo&agent=claude"
    );
    let reattach = SessionQuery {
        id: Some(7),
        ..launch(Some("wt-a"))
    };
    assert_eq!(
        peer_session_query(&reattach, "owner/repo"),
        "id=7&repo=owner%2Frepo"
    );
}

/// A free console on a peer with no distro is launched there: the peer gets
/// the console shape, the trimmed startup command, and the holder.
#[test]
fn peer_session_query_forwards_a_console_launch() {
    let console = |command: Option<&str>| SessionQuery {
        repo: Some("x".into()),
        console: Some(1),
        command: command.map(str::to_string),
        holder: Some("tab-1".into()),
        ..SessionQuery::default()
    };
    assert_eq!(
        peer_session_query(&console(Some(" htop ")), "owner/repo"),
        "console=1&repo=owner%2Frepo&command=htop&holder=tab-1"
    );
    assert_eq!(
        peer_session_query(
            &SessionQuery {
                holder: None,
                ..console(None)
            },
            "owner/repo"
        ),
        "console=1&repo=owner%2Frepo"
    );
}

/// The owning daemon keeps the writer slot, so the relay forwards the tab's
/// holder on a launch and on a reattach — and drops a malformed one rather
/// than splicing it into the peer's query string.
#[test]
fn peer_session_query_forwards_a_well_formed_holder() {
    let reattach = |holder: &str| SessionQuery {
        repo: Some("x".into()),
        id: Some(7),
        holder: Some(holder.to_string()),
        ..SessionQuery::default()
    };
    assert_eq!(
        peer_session_query(&reattach("tab-1_A"), "owner/repo"),
        "id=7&repo=owner%2Frepo&holder=tab-1_A"
    );
    let launch = SessionQuery {
        id: None,
        agent: Some("claude".into()),
        ..reattach("tab-1_A")
    };
    assert_eq!(
        peer_session_query(&launch, "owner/repo"),
        "repo=owner%2Frepo&agent=claude&holder=tab-1_A"
    );
    for bad in ["", "a&takeover=1", &"x".repeat(65)] {
        assert_eq!(
            peer_session_query(&reattach(bad), "owner/repo"),
            "id=7&repo=owner%2Frepo",
            "{bad:?} is not a holder"
        );
    }
}

/// The relay forwards the console name on a LAUNCH only: the owning daemon
/// folds it. A reattach names a session that already has its name, and an
/// empty name leaves the query byte-identical for an older peer.
#[test]
fn peer_session_query_forwards_the_console_name_on_a_launch_only() {
    let launch = |name: Option<String>| SessionQuery {
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
    assert_eq!(
        peer_session_query(&launch(Some("fincal #1".into())), "owner/repo"),
        "repo=owner%2Frepo&agent=claude&name=fincal%20%231"
    );
    for none in [None, Some(String::new())] {
        assert_eq!(
            peer_session_query(&launch(none), "owner/repo"),
            "repo=owner%2Frepo&agent=claude"
        );
    }
    let long = peer_session_query(&launch(Some("é".repeat(41))), "owner/repo");
    assert!(long.ends_with(&format!(
        "&name={}",
        crate::routes::encode_query_value(&"é".repeat(40))
    )));
    let reattach = SessionQuery {
        id: Some(7),
        ..launch(Some("fincal #1".into()))
    };
    assert_eq!(
        peer_session_query(&reattach, "owner/repo"),
        "id=7&repo=owner%2Frepo"
    );
}

/// The relay forwards the window record on a LAUNCH only, so the owning
/// daemon keeps one session per record; a reattach names its session already,
/// and a malformed record is dropped rather than spliced into the query.
#[test]
fn peer_session_query_forwards_the_record_on_a_launch_only() {
    let console = |record: &str| SessionQuery {
        repo: Some("x".into()),
        console: Some(1),
        record: Some(record.to_string()),
        ..SessionQuery::default()
    };
    assert_eq!(
        peer_session_query(&console("w-a_1"), "owner/repo"),
        "console=1&repo=owner%2Frepo&record=w-a_1"
    );
    let agent = SessionQuery {
        console: None,
        agent: Some("claude".into()),
        ..console("w-a_1")
    };
    assert_eq!(
        peer_session_query(&agent, "owner/repo"),
        "repo=owner%2Frepo&agent=claude&record=w-a_1"
    );
    let reattach = SessionQuery {
        id: Some(7),
        ..console("w-a_1")
    };
    assert_eq!(
        peer_session_query(&reattach, "owner/repo"),
        "id=7&repo=owner%2Frepo"
    );
    for bad in ["", "w&takeover=1", &"x".repeat(65)] {
        assert_eq!(
            peer_session_query(&console(bad), "owner/repo"),
            "console=1&repo=owner%2Frepo",
            "{bad:?} is not a record"
        );
    }
}

/// The daemon that hosts the session compresses the replay, so the relay
/// forwards `gzip=1` on every shape, and nothing when the page did not ask.
#[test]
fn peer_session_query_forwards_gzip_on_every_shape() {
    let launch = |gzip: Option<u32>| SessionQuery {
        repo: Some("x".into()),
        agent: Some("claude".into()),
        id: None,
        takeover: None,
        watch: None,
        console: None,
        checkout: None,
        command: None,
        holder: None,
        name: None,
        record: None,
        gzip,
        attempt: None,
        since: None,
        net: None,
        net_rtt: None,
        net_down: None,
    };
    assert_eq!(
        peer_session_query(&launch(Some(1)), "owner/repo"),
        "repo=owner%2Frepo&agent=claude&gzip=1"
    );
    let watch = |gzip| SessionQuery {
        id: Some(7),
        watch: Some(1),
        ..launch(gzip)
    };
    assert_eq!(
        peer_session_query(&watch(Some(1)), "owner/repo"),
        "id=7&repo=owner%2Frepo&watch=1&gzip=1"
    );
    let console = SessionQuery {
        console: Some(1),
        agent: None,
        ..launch(Some(1))
    };
    assert_eq!(
        peer_session_query(&console, "owner/repo"),
        "console=1&repo=owner%2Frepo&gzip=1"
    );
    assert_eq!(
        peer_session_query(&watch(None), "owner/repo"),
        "id=7&repo=owner%2Frepo&watch=1"
    );
    assert_eq!(
        peer_session_query(&watch(Some(0)), "owner/repo"),
        "id=7&repo=owner%2Frepo&watch=1",
        "only `1` asks"
    );
}

/// The owning daemon orders the claims of one holder by their attempt, so the
/// relay forwards it beside the holder on every shape that sends one, and
/// never alone: an attempt without a holder orders nothing.
#[test]
fn peer_session_query_forwards_the_attempt_with_the_holder() {
    let reattach = |holder: Option<&str>| SessionQuery {
        repo: Some("x".into()),
        id: Some(7),
        holder: holder.map(str::to_string),
        attempt: Some(12),
        ..SessionQuery::default()
    };
    assert_eq!(
        peer_session_query(&reattach(Some("tab-A")), "owner/repo"),
        "id=7&repo=owner%2Frepo&holder=tab-A&attempt=12"
    );
    let launch = SessionQuery {
        id: None,
        agent: Some("claude".into()),
        ..reattach(Some("tab-A"))
    };
    assert_eq!(
        peer_session_query(&launch, "owner/repo"),
        "repo=owner%2Frepo&agent=claude&holder=tab-A&attempt=12"
    );
    let console = SessionQuery {
        console: Some(1),
        ..launch
    };
    assert_eq!(
        peer_session_query(&console, "owner/repo"),
        "console=1&repo=owner%2Frepo&holder=tab-A&attempt=12"
    );
    assert_eq!(
        peer_session_query(&reattach(None), "owner/repo"),
        "id=7&repo=owner%2Frepo",
        "no holder, no attempt"
    );
}

/// The owning daemon keeps the session's output, so the relay forwards the
/// byte count of a reattach, on the reattach and the watch alike. A launch
/// opens a new terminal and sends none.
#[test]
fn peer_session_query_forwards_since_on_a_reattach() {
    let reattach = |watch: Option<u32>| SessionQuery {
        repo: Some("x".into()),
        agent: None,
        id: Some(7),
        takeover: None,
        watch,
        console: None,
        checkout: None,
        command: None,
        holder: None,
        name: None,
        record: None,
        gzip: None,
        attempt: None,
        since: Some(4096),
        net: None,
        net_rtt: None,
        net_down: None,
    };
    assert_eq!(
        peer_session_query(&reattach(None), "owner/repo"),
        "id=7&repo=owner%2Frepo&since=4096"
    );
    assert_eq!(
        peer_session_query(&reattach(Some(1)), "owner/repo"),
        "id=7&repo=owner%2Frepo&watch=1&since=4096"
    );
    let at_zero = SessionQuery {
        since: Some(0),
        ..reattach(None)
    };
    assert_eq!(
        peer_session_query(&at_zero, "owner/repo"),
        "id=7&repo=owner%2Frepo&since=0",
        "a terminal that shows no byte yet resumes at 0"
    );
    let launch = SessionQuery {
        id: None,
        agent: Some("claude".into()),
        ..reattach(None)
    };
    assert_eq!(
        peer_session_query(&launch, "owner/repo"),
        "repo=owner%2Frepo&agent=claude"
    );
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
    let body = r#"[{"id":1,"repo":"o/r","agent":"claude","kind":"agent","started_at":1,"daemon_id":"d","environment":"Windows"}]"#;
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
