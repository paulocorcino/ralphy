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

mod logins {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{header, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::Router;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::super::*;
    use crate::cookie::LoginId;
    use crate::dispatch::Verb;
    use crate::protocol::Command;
    use crate::routes::{audit_layer, ConsoleKind, ConsoleRecord, SocketAudit};

    fn of_kind(dir: &Path, kind: &str) -> Vec<Value> {
        std::fs::read_to_string(log_path_in(dir))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|l| l["event"] == kind)
            .collect()
    }

    /// One request through the audit layer, with `login` put on the request
    /// as the guard does, and `answer_login` on the response as the login
    /// route does.
    async fn through_the_layer(
        audit: Arc<Audit>,
        path: &str,
        login: Option<LoginId>,
        answer_login: Option<LoginId>,
        cookie: Option<String>,
    ) -> StatusCode {
        let handler = move || async move {
            let mut resp: Response = StatusCode::OK.into_response();
            if let Some(l) = answer_login {
                resp.extensions_mut().insert(l);
            }
            resp
        };
        let guard = move |mut req: Request, next: axum::middleware::Next| async move {
            if let Some(l) = login {
                req.extensions_mut().insert(l);
            }
            next.run(req).await
        };
        let app = Router::new()
            .route(path, axum::routing::post(handler))
            .layer(axum::middleware::from_fn_with_state(audit, audit_layer))
            .layer(axum::middleware::from_fn(guard));
        let mut req = Request::builder().method("POST").uri(path);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        app.oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// A `login_ok` line for `login` signed in from `device`.
    fn signed_in(audit: &Audit, login: LoginId, device: Option<DeviceId>) {
        let mut e = Event::new(EventKind::LoginOk, device, Actor::Device);
        e.login = Some(login.to_string());
        audit.record(&e);
    }

    fn socket(audit: &Arc<Audit>, login: LoginId, device: Option<DeviceId>) -> SocketAudit {
        SocketAudit {
            audit: audit.clone(),
            device,
            login: Some(login),
            actor: Actor::Device,
            ip: Some("203.0.113.9".to_string()),
        }
    }

    fn launch() -> ConsoleRecord {
        ConsoleRecord {
            kind: ConsoleKind::Launch,
            session: Some(7),
            agent: Some("console".to_string()),
            repo: Some("ralphy".to_string()),
            peer: None,
            holder: Some("tab-1".to_string()),
        }
    }

    fn switch() -> Command {
        Command {
            id: 1,
            verb: "branch.switch".to_string(),
            payload: serde_json::json!({"repo": "ralphy", "name": "b"}),
        }
    }

    #[tokio::test]
    async fn a_line_under_a_login_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(Audit::in_dir(Some(dir.path().to_path_buf())));
        let login = LoginId::mint();
        let status = through_the_layer(audit, "/api/ui/thing", Some(login), None, None).await;
        assert_eq!(status, StatusCode::OK);
        let actions = of_kind(dir.path(), "action");
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(actions[0]["login"], login.to_string());
    }

    #[tokio::test]
    async fn login_ok_names_its_login_and_device() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(Audit::in_dir(Some(dir.path().to_path_buf())));
        let device = DeviceId::mint();
        let value = audit.key(true).expect("a device key").sign(device);
        let cookie = format!("{}={value}", device::COOKIE_NAME);
        let login = LoginId::mint();
        let status = through_the_layer(audit, "/api/login", None, Some(login), Some(cookie)).await;
        assert_eq!(status, StatusCode::OK);
        let ok = of_kind(dir.path(), "login_ok");
        assert_eq!(ok.len(), 1, "{ok:?}");
        assert_eq!(ok[0]["login"], login.to_string());
        assert_eq!(ok[0]["device"], device.to_string());
    }

    #[test]
    fn a_launch_from_another_device_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(Audit::in_dir(Some(dir.path().to_path_buf())));
        let login = LoginId::mint();
        signed_in(&audit, login, Some(DeviceId::mint()));
        let other = DeviceId::mint();
        socket(&audit, login, Some(other)).console(&launch());
        let reported = of_kind(dir.path(), "login_other_device");
        assert_eq!(reported.len(), 1, "{reported:?}");
        let line = &reported[0];
        assert_eq!(line["login"], login.to_string());
        assert_eq!(line["device"], other.to_string());
        assert_eq!(line["did"], "console_launch");
        assert_eq!(line["ip"], "203.0.113.9");
        assert_eq!(line["holder"], "tab-1");
    }

    #[test]
    fn each_pair_is_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(Audit::in_dir(Some(dir.path().to_path_buf())));
        let login = LoginId::mint();
        let own = DeviceId::mint();
        signed_in(&audit, login, Some(own));
        let verb = Verb::from_query("branch.switch").expect("a known verb");
        socket(&audit, login, Some(own)).record(verb, &switch());
        socket(&audit, login, Some(own)).console(&launch());
        assert!(
            of_kind(dir.path(), "login_other_device").is_empty(),
            "the login's own device writes none"
        );
        let other = DeviceId::mint();
        socket(&audit, login, Some(other)).record(verb, &switch());
        socket(&audit, login, Some(other)).console(&launch());
        socket(&audit, login, Some(other)).record(verb, &switch());
        let reported = of_kind(dir.path(), "login_other_device");
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert_eq!(reported[0]["did"], "branch.switch");
        socket(&audit, login, None).record(verb, &switch());
        assert_eq!(
            of_kind(dir.path(), "login_other_device").len(),
            2,
            "no device is a device of its own"
        );
    }

    #[test]
    fn a_reported_pair_stays_reported_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let login = LoginId::mint();
        let other = DeviceId::mint();
        {
            let audit = Arc::new(Audit::in_dir(Some(dir.path().to_path_buf())));
            signed_in(&audit, login, Some(DeviceId::mint()));
            socket(&audit, login, Some(other)).console(&launch());
        }
        assert_eq!(of_kind(dir.path(), "login_other_device").len(), 1);
        let audit = Arc::new(Audit::in_dir(Some(dir.path().to_path_buf())));
        socket(&audit, login, Some(other)).console(&launch());
        assert_eq!(
            of_kind(dir.path(), "login_other_device").len(),
            1,
            "the pair in the log is not reported again"
        );
        socket(&audit, login, Some(DeviceId::mint())).console(&launch());
        assert_eq!(
            of_kind(dir.path(), "login_other_device").len(),
            2,
            "the login's device was read back from the log"
        );
    }

    /// A `login_other_device` line for `login` and `device`, `ts` seconds
    /// after the epoch.
    fn reported_at(ts: i64, login: LoginId, device: DeviceId) -> String {
        let at = chrono::DateTime::from_timestamp(ts, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        serde_json::json!({
            "at": at,
            "event": "login_other_device",
            "device": device.to_string(),
            "login": login.to_string(),
            "actor": "device",
            "did": "console_launch",
        })
        .to_string()
    }

    #[test]
    fn a_notice_older_than_thirty_days_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path_in(dir.path());
        let now = 2_000_000_000;
        let day = 86_400;
        let (fresh, old, edge) = (LoginId::mint(), LoginId::mint(), LoginId::mint());
        let lines = [
            reported_at(now - 31 * day, old, DeviceId::mint()),
            reported_at(now - 30 * day, edge, DeviceId::mint()),
            reported_at(now - 29 * day, fresh, DeviceId::mint()),
        ];
        std::fs::write(
            &path,
            lines.join(
                "
",
            ) + "
",
        )
        .unwrap();
        let listed = super::super::logins::notices(&path, now, None, |_| false).unwrap();
        let logins: Vec<String> = listed.iter().map(|n| n.login.clone()).collect();
        assert_eq!(
            logins,
            [fresh.to_string()],
            "only a notice under 30 days old"
        );
    }

    #[test]
    fn a_notice_change_pushes_logins_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let pushes = tokio::sync::broadcast::channel(8).0;
        let mut bus = pushes.subscribe();
        let audit =
            Arc::new(Audit::in_dir(Some(dir.path().to_path_buf())).with_pushes(pushes.clone()));
        let pushed = |bus: &mut tokio::sync::broadcast::Receiver<crate::routes::Push>| {
            let mut verbs = Vec::new();
            while let Ok(p) = bus.try_recv() {
                verbs.push(p.verb());
            }
            verbs
        };
        let login = LoginId::mint();
        let (own, other) = (DeviceId::mint(), DeviceId::mint());
        signed_in(&audit, login, Some(own));
        socket(&audit, login, Some(own)).console(&launch());
        assert!(
            pushed(&mut bus).is_empty(),
            "a use from the login's own device"
        );
        socket(&audit, login, Some(other)).console(&launch());
        assert_eq!(
            pushed(&mut bus),
            ["logins.dirty"],
            "the login_other_device line"
        );
        let other = other.to_string();
        audit
            .dismiss_notice(&login, Some(&other), Actor::Device)
            .unwrap();
        assert_eq!(pushed(&mut bus), ["logins.dirty"], "the dismissal");
        audit
            .dismiss_notice(&login, Some(&other), Actor::Device)
            .unwrap();
        assert!(
            pushed(&mut bus).is_empty(),
            "a second dismissal changes nothing"
        );
    }
}
