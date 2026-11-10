use super::*;
use std::sync::Arc;

/// The bridge learns it was evicted by waking, and announces WHY by reading
/// the token — so a wake that arrives before the reason does would announce
/// nothing. `fire` records first and wakes second; this pins that order from
/// the waiter's side.
#[tokio::test]
async fn evict_token_carries_its_reason_before_waking() {
    let token = Arc::new(EvictToken::new());
    assert_eq!(token.reason(), None, "a fresh token names no reason");

    // Observed from a SECOND task, the way the bridge does: it reads the
    // reason at the moment its wait resolves. Asserting after a synchronous
    // `fire` returned would pass even with the order reversed, since both
    // writes are complete by then.
    let waiter = {
        let token = token.clone();
        tokio::spawn(async move {
            token.notify.notified().await;
            token.reason()
        })
    };
    // Let the waiter park before firing; `notify_one` stores a permit either
    // way, so this only makes the intended interleaving the common one.
    tokio::task::yield_now().await;

    token.fire(EndReason::TakenOver);

    let seen = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("the pre-registered waiter must be woken")
        .expect("the waiter task must not panic");
    assert_eq!(
        seen,
        Some(EndReason::TakenOver),
        "a woken waiter must ALREADY see the reason — otherwise the bridge \
         announces `child-exited` for a takeover (the bridge's unwrap_or fallback)"
    );
    assert_eq!(EndReason::TakenOver.as_wire(), "taken-over");
    assert_eq!(EndReason::ChildExited.as_wire(), "child-exited");
    assert_eq!(EndReason::DaemonShutdown.as_wire(), "daemon-shutdown");
}

/// The record serialises the checkout only when there is one: an older
/// peer's listing (and a primary-tree console) keeps its exact shape.
#[test]
fn session_info_serialises_checkout_only_when_present() {
    let info = |checkout: Option<&str>| SessionInfo {
        id: 1,
        repo: "owner/ralphy".to_string(),
        agent: "claude".to_string(),
        kind: "agent".to_string(),
        started_at: 1,
        environment: None,
        name: None,
        checkout: checkout.map(str::to_string),
        record: None,
        agent_state: None,
    };
    let primary = serde_json::to_value(info(None)).unwrap();
    assert!(primary.get("checkout").is_none(), "{primary}");
    let linked = serde_json::to_value(info(Some("wt-a"))).unwrap();
    assert_eq!(linked["checkout"], "wt-a");
}

/// A PTY input that blocks its first write until the test sends `()`, then
/// appends every write to `sink`: a child that does not read its input.
struct Stuck {
    gate: Option<std::sync::mpsc::Receiver<()>>,
    sink: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for Stuck {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(gate) = self.gate.take() {
            gate.recv().map_err(std::io::Error::other)?;
        }
        self.sink.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn input_send_does_not_wait_for_a_child_that_does_not_read() {
    let (open, gate) = std::sync::mpsc::channel();
    let sink = Arc::new(Mutex::new(Vec::new()));
    let input = Input::start(Box::new(Stuck {
        gate: Some(gate),
        sink: sink.clone(),
    }))
    .unwrap();
    let (sent_tx, sent) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let both = input.send(b"ab").and_then(|()| input.send(b"cd"));
        sent_tx.send(both.map_err(|e| e.to_string())).unwrap();
        // Keep the queue open until the writes land.
        std::thread::sleep(std::time::Duration::from_secs(3));
    });
    sent.recv_timeout(std::time::Duration::from_secs(1))
        .expect("two sends must return while the child does not read")
        .unwrap();
    open.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while sink.lock().unwrap().as_slice() != b"abcd" {
        assert!(
            std::time::Instant::now() < deadline,
            "the input thread must write abcd in order, got {:?}",
            sink.lock().unwrap()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
