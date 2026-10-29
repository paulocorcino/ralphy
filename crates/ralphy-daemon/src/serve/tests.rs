use std::time::Duration;

use super::*;

/// A daemon with no listener bound has nothing to serve: it does not wait
/// for the stop signal.
#[tokio::test]
async fn with_no_listener_the_serving_ends_at_once() {
    let (_stop_tx, stop) = tokio::sync::watch::channel(false);
    let served = tokio::time::timeout(
        Duration::from_secs(5),
        serve_listeners(
            None,
            #[cfg(unix)]
            None,
            axum::Router::new(),
            stop,
        ),
    )
    .await;
    assert!(
        matches!(served, Ok(Ok(()))),
        "serving with no listener returns Ok at once: {served:?}"
    );
}
