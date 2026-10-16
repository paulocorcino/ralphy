//! The stall report (CONTEXT.md → *Stall report*, ADR-0082): the page tells
//! its daemon that a read, a replay decode or a console waited past its
//! deadline, so the next wedge leaves one line in `daemon.log`.

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router};

use super::RouterShared;
use crate::device::DeviceId;

/// The largest stall body the route reads (ADR-0072 D11). The four fields of
/// a report fit in a few hundred bytes.
pub(crate) const STALL_MAX_BYTES: usize = 2048;

/// The longest `what` the log line keeps, in characters.
pub(crate) const WHAT_MAX_CHARS: usize = 200;

/// What waited past its deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StallKind {
    ReadDeadline,
    DecodeDeadline,
    ConsoleGaveUp,
}

impl StallKind {
    fn as_str(self) -> &'static str {
        match self {
            StallKind::ReadDeadline => "read-deadline",
            StallKind::DecodeDeadline => "decode-deadline",
            StallKind::ConsoleGaveUp => "console-gave-up",
        }
    }
}

/// The body of `POST /api/device/stall`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StallReport {
    pub(crate) kind: StallKind,
    pub(crate) what: String,
    pub(crate) waited_ms: u64,
    pub(crate) holder: Option<String>,
}

/// Write the one `daemon.log` line of a stall report.
pub(crate) fn log_stall(report: &StallReport, device: Option<&DeviceId>) {
    let what: String = report.what.chars().take(WHAT_MAX_CHARS).collect();
    tracing::warn!(
        device = %device.map(ToString::to_string).unwrap_or_else(|| "none".to_string()),
        holder = report.holder.as_deref().unwrap_or("none"),
        kind = report.kind.as_str(),
        what = %what,
        waited_ms = report.waited_ms,
        "the page reported a stall"
    );
}

async fn post_stall(
    device: Option<Extension<DeviceId>>,
    Json(report): Json<StallReport>,
) -> StatusCode {
    log_stall(&report, device.as_ref().map(|Extension(id)| id));
    StatusCode::NO_CONTENT
}

fn router() -> Router {
    Router::new()
        .route("/api/device/stall", post(post_stall))
        .layer(DefaultBodyLimit::max(STALL_MAX_BYTES))
}

pub(crate) fn stall_routes(_s: &RouterShared) -> Router {
    router()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[derive(Clone, Default)]
    struct Written(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Written {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("a test lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Written {
        type Writer = Written;
        fn make_writer(&'a self) -> Written {
            self.clone()
        }
    }

    fn logged(f: impl FnOnce()) -> String {
        let written = Written::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(written.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = written.0.lock().expect("a test lock").clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Send `body` through the route on a current-thread runtime, so the
    /// subscriber of `logged` sees the handler's line.
    fn post_body(body: String) -> (StatusCode, String) {
        let mut status = StatusCode::OK;
        let line = logged(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a test runtime");
            status = rt.block_on(async {
                router()
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/api/device/stall")
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .expect("a test request"),
                    )
                    .await
                    .expect("the router answers")
                    .status()
            });
        });
        (status, line)
    }

    fn report(what: &str) -> StallReport {
        StallReport {
            kind: StallKind::ReadDeadline,
            what: what.to_string(),
            waited_ms: 20000,
            holder: Some("tab1".to_string()),
        }
    }

    #[test]
    fn a_stall_report_is_answered_no_content() {
        let body =
            r#"{"kind":"read-deadline","what":"GET /api/repos","waited_ms":20000,"holder":null}"#;
        let (status, line) = post_body(body.to_string());
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(line.contains("GET /api/repos"), "{line}");
    }

    #[test]
    fn a_stall_line_names_the_device_holder_kind_what_and_wait() {
        let device = DeviceId::mint();
        let line = logged(|| log_stall(&report("GET /api/repos"), Some(&device)));
        for part in [
            format!("device={device}"),
            "holder=\"tab1\"".to_string(),
            "kind=\"read-deadline\"".to_string(),
            "what=GET /api/repos".to_string(),
            "waited_ms=20000".to_string(),
        ] {
            assert!(line.contains(&part), "{part} missing in {line}");
        }
    }

    #[test]
    fn a_stall_report_of_an_unknown_kind_is_refused() {
        let body = r#"{"kind":"nap","what":"x","waited_ms":1,"holder":null}"#;
        let (status, line) = post_body(body.to_string());
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(line.is_empty(), "{line}");
    }

    #[test]
    fn a_stall_report_over_the_cap_is_refused() {
        let head = r#"{"kind":"read-deadline","waited_ms":1,"holder":null,"what":""#;
        let tail = r#""}"#;
        let what = "x".repeat(STALL_MAX_BYTES + 1 - head.len() - tail.len());
        let body = format!("{head}{what}{tail}");
        assert_eq!(body.len(), 2049);
        let (status, _) = post_body(body);
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn a_long_what_is_clipped_in_the_stall_line() {
        let long = "w".repeat(500);
        let line = logged(|| log_stall(&report(&long), None));
        assert!(line.contains(&"w".repeat(WHAT_MAX_CHARS)), "{line}");
        assert!(!line.contains(&"w".repeat(WHAT_MAX_CHARS + 1)), "{line}");
    }
}
