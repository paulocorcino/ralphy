use super::*;

const SESSION_START_LINE: &str = r#"{"event":"SessionStart","session_id":"c1","ts":"t"}"#;
const PRE_INVOCATION_LINE: &str = r#"{"event":"PreInvocation","session_id":"c1","ts":"t"}"#;

fn proof(dir: &Path, waiver: bool, window: Duration) -> Proof {
    Proof {
        status_file: fresh_status_file(dir).expect("status file"),
        log_file: dir.join("session.log"),
        root: dir.join("antigravity-root"),
        waiver,
        window,
    }
}

fn append(path: &Path, line: &str) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .expect("open");
    writeln!(file, "{line}").expect("append");
}

#[test]
fn the_proof_gate_waits_for_session_start_in_the_status_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proof = proof(dir.path(), false, Duration::from_secs(20));
    let status_file = proof.status_file.clone();
    append(&status_file, PRE_INVOCATION_LINE);
    let delay = Duration::from_millis(600);
    let writer = std::thread::spawn(move || {
        std::thread::sleep(delay);
        append(&status_file, SESSION_START_LINE);
    });
    let started = Instant::now();
    let released = proof.gate()();
    let waited = started.elapsed();
    writer.join().expect("writer");
    assert!(released.is_ok(), "{released:?}");
    assert!(waited >= delay, "released before the line came: {waited:?}");
    assert!(
        waited < Duration::from_secs(10),
        "waited the whole window: {waited:?}"
    );
}

#[test]
fn no_session_start_in_the_window_refuses_with_the_waiver_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proof = proof(dir.path(), false, Duration::from_millis(300));
    append(&proof.status_file, PRE_INVOCATION_LINE);
    let mut warnings = Vec::new();
    let err = proof
        .hold(|cause| warnings.push(cause.to_owned()))
        .expect_err("an unproven session is refused");
    let refusal = err
        .downcast_ref::<HooksUnproven>()
        .expect("a hooks refusal");
    assert!(refusal.cause.contains("SessionStart"), "{}", refusal.cause);
    assert!(
        refusal
            .cause
            .contains(&format!("ralphy config set {WAIVER_KEY} true")),
        "{}",
        refusal.cause
    );
    assert_eq!(
        WAIVER_KEY,
        "antigravity.run_without_guard_proof_i_understand_the_risk"
    );
    assert!(warnings.is_empty(), "a refusal warns nothing: {warnings:?}");
}

#[test]
fn the_waiver_lets_an_unproven_session_go_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proof = proof(dir.path(), true, Duration::from_millis(300));
    let mut warnings = Vec::new();
    let went_on = proof.hold(|cause| warnings.push(cause.to_owned()));
    assert!(went_on.is_ok(), "{went_on:?}");
    assert_eq!(warnings.len(), 1, "one warning: {warnings:?}");
    assert!(
        warnings.iter().all(|cause| cause.contains("SessionStart")),
        "{warnings:?}"
    );
}

#[test]
fn a_sign_in_line_ends_the_wait_with_the_sign_in_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proof = proof(dir.path(), true, Duration::from_secs(20));
    append(
        &proof.log_file,
        "Authentication required. Please visit the URL to log in:",
    );
    let started = Instant::now();
    let err = proof.gate()().expect_err("a signed-out session ends");
    assert!(started.elapsed() < Duration::from_secs(10));
    let signed_out = err.downcast_ref::<SignedOut>().expect("the sign-in error");
    assert!(
        signed_out.0.contains("antigravity-root"),
        "{}",
        signed_out.0
    );
}

#[test]
fn the_status_file_is_emptied_for_each_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = fresh_status_file(&dir.path().join("run")).expect("first");
    append(&first, SESSION_START_LINE);
    assert!(has_session_start(&first));
    let second = fresh_status_file(&dir.path().join("run")).expect("second");
    assert_eq!(first, second);
    assert!(
        !has_session_start(&second),
        "an earlier session's line is no proof"
    );
}
