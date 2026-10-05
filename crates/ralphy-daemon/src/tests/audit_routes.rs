//! The device cookie and the audit log through the router (ADR-0074).

use super::*;

fn audit_lines(dir: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(dir.join("daemon-audit.jsonl")).unwrap_or_default();
    text.lines()
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect()
}

fn device_set_cookie(res: &Response) -> Option<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ralphy_device="))
        .map(str::to_string)
}

fn pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_string()
}

async fn send(app: Router, req: Request<Body>) -> Response {
    app.oneshot(req).await.unwrap()
}

#[tokio::test]
async fn a_browser_gets_one_device_cookie_and_a_forged_one_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let app = desk_router(dir.path());
    let first = send(
        app.clone(),
        Request::builder()
            .uri("/api/session")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let set = device_set_cookie(&first).expect("the first API request sets the device cookie");
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Strict"),
        "{set}"
    );

    let again = send(
        app.clone(),
        Request::builder()
            .uri("/api/session")
            .header(header::COOKIE, pair(&set))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(device_set_cookie(&again), None, "a valid cookie is kept");

    let value = pair(&set);
    let id = value.split('.').nth(1).unwrap();
    let forged = value.replace(id, &"0".repeat(32));
    let replaced = send(
        app,
        Request::builder()
            .uri("/api/session")
            .header(header::COOKIE, forged)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        device_set_cookie(&replaced).is_some(),
        "a cookie whose MAC does not match gets a new ID"
    );
}

#[tokio::test]
async fn an_action_line_keeps_the_path_and_drops_the_query_and_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let res = send(
        desk_router(dir.path()),
        Request::builder()
            .method("POST")
            .uri("/api/nope?token=query-secret")
            .header("x-real-ip", "203.0.113.7")
            .body(Body::from("body-secret"))
            .unwrap(),
    )
    .await;
    let lines = audit_lines(dir.path());
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert_eq!(line["event"], "action");
    assert_eq!(line["method"], "POST");
    assert_eq!(line["path"], "/api/nope");
    assert_eq!(line["status"], res.status().as_u16());
    assert_eq!(line["ip"], "203.0.113.7");
    assert_eq!(line["actor"], "device");
    let set = device_set_cookie(&res).unwrap();
    assert_eq!(
        line["device"],
        pair(&set).split('.').nth(1).unwrap(),
        "the line names the device the cookie carries"
    );
    let text = std::fs::read_to_string(dir.path().join("daemon-audit.jsonl")).unwrap();
    assert!(!text.contains("secret"), "{text}");
    assert!(
        owner_only::is_owner_only(&dir.path().join("daemon-audit.jsonl")).unwrap(),
        "the log is owner-only"
    );
}

#[tokio::test]
async fn a_read_writes_no_line() {
    let dir = tempfile::tempdir().unwrap();
    send(
        desk_router(dir.path()),
        Request::builder()
            .uri("/api/session")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(audit_lines(dir.path()).is_empty());
}

#[tokio::test]
async fn a_login_and_a_replayed_login_are_recorded_with_the_server_facts() {
    let dir = tempfile::tempdir().unwrap();
    let app = session_router_at(dir.path().join("repos.toml"), "tok");
    let code = rfc_seed().code_at(now_unix() / 30);
    let login = || {
        Request::builder()
            .method("POST")
            .uri("/api/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("x-real-ip", "198.51.100.4")
            .header("sec-ch-ua-platform", "\"Android\"")
            .body(Body::from(format!("code={code}")))
            .unwrap()
    };
    assert_eq!(send(app.clone(), login()).await.status(), StatusCode::OK);
    assert_eq!(
        send(app, login()).await.status(),
        StatusCode::UNAUTHORIZED,
        "the same code again is a replay"
    );
    let lines = audit_lines(dir.path());
    let events: Vec<_> = lines.iter().map(|l| l["event"].as_str().unwrap()).collect();
    assert_eq!(events, ["login_ok", "login_failed"]);
    assert_eq!(lines[1]["reason"], "bad_credential");
    assert_eq!(lines[0]["server"]["real_ip"], "198.51.100.4");
    assert_eq!(lines[0]["server"]["client_hints"][0][0], "platform");
    let text = std::fs::read_to_string(dir.path().join("daemon-audit.jsonl")).unwrap();
    assert!(!text.contains(&format!("code={code}")), "{text}");
}

#[tokio::test]
async fn a_router_with_no_store_directory_sets_no_cookie_and_keeps_no_log() {
    let res = get("/api/session").await;
    assert_eq!(device_set_cookie(&res), None);
}
