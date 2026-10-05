use super::*;

fn line_at(ts: i64, tag: &str) -> String {
    let at = chrono::DateTime::from_timestamp(ts, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    format!("{{\"at\":\"{at}\",\"event\":\"action\",\"actor\":\"device\",\"path\":\"{tag}\"}}")
}

#[test]
fn prune_removes_the_lines_older_than_the_retention() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_path_in(dir.path());
    let now = 2_000_000_000;
    let text = [
        line_at(now - RETENTION_SECS - 1, "old"),
        "not json".to_string(),
        line_at(now - RETENTION_SECS, "edge"),
        line_at(now, "new"),
    ]
    .join("\n")
        + "\n";
    std::fs::write(&path, text).unwrap();
    prune(&path, now).unwrap();
    let kept = std::fs::read_to_string(&path).unwrap();
    assert!(!kept.contains("old"), "{kept}");
    assert!(kept.contains("not json"), "a line with no time is kept");
    assert!(kept.contains("edge") && kept.contains("new"), "{kept}");
}

#[test]
fn prune_cuts_the_oldest_lines_when_the_file_is_over_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = log_path_in(dir.path());
    let now = 2_000_000_000;
    let pad = "x".repeat(1024 * 1024);
    let lines: Vec<String> = (0..21)
        .map(|i| line_at(now - 100 + i, &format!("{i:02}{pad}")))
        .collect();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    prune(&path, now).unwrap();
    let kept = std::fs::read_to_string(&path).unwrap();
    assert!(kept.len() as u64 <= PRUNE_TO_BYTES, "{}", kept.len());
    assert!(kept.contains("\"20x"), "the newest line stays");
    assert!(!kept.contains("\"00x"), "the oldest line goes");
}

#[test]
fn server_facts_read_the_front_and_the_client_hints() {
    let mut headers = HeaderMap::new();
    headers.insert("x-real-ip", "203.0.113.7".parse().unwrap());
    headers.insert("sec-ch-ua", "\"Chromium\";v=\"148\"".parse().unwrap());
    headers.insert("sec-ch-ua-model", "\"moto g(30)\"".parse().unwrap());
    headers.insert("user-agent", "u".repeat(2000).parse().unwrap());
    let facts = ServerFacts::from_headers(&headers);
    assert_eq!(facts.real_ip.as_deref(), Some("203.0.113.7"));
    assert_eq!(
        facts.client_hints,
        [
            ("brands".to_string(), "\"Chromium\";v=\"148\"".to_string()),
            ("model".to_string(), "\"moto g(30)\"".to_string()),
        ]
    );
    assert_eq!(facts.user_agent.unwrap().len(), MAX_HEADER_CHARS);
}
