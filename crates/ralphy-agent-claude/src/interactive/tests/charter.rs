use super::*;

#[test]
fn the_interactive_argv_carries_no_prompt() {
    for remote_control in [None, Some("ralphy-1")] {
        let args = interactive_args(
            Path::new("settings.json"),
            Path::new("plugin"),
            "sonnet",
            Some("high"),
            remote_control,
        );
        assert!(!args.iter().any(|arg| arg == EXEC_CHARTER));
    }
}

#[test]
fn the_charter_is_typed_as_measured() {
    let keys = charter_keys();
    assert_eq!(
        keys.text,
        b"Read .ralphy/exec.md and follow it exactly to implement .ralphy/plan.md \
         for this issue. Emit RALPHY_DONE_EXIT when finished."
    );
    assert_eq!(keys.submit, b"\r");
    assert!(keys.pause >= Duration::from_millis(100));
    assert_eq!(keys.pause, Duration::from_millis(300));
}

#[test]
fn the_gate_types_the_charter_once_after_the_first_hook_event() {
    let start = Instant::now();
    let mut gate = ProofGate::new(start, Duration::from_secs(30));
    assert_eq!(gate.poll(false, start), GateAction::Wait);
    assert_eq!(
        gate.poll(false, start + Duration::from_secs(29)),
        GateAction::Wait
    );
    assert_eq!(
        gate.poll(true, start + Duration::from_secs(29)),
        GateAction::Type
    );
    for later in [29, 30, 60] {
        assert_eq!(
            gate.poll(true, start + Duration::from_secs(later)),
            GateAction::Wait
        );
        assert_eq!(
            gate.poll(false, start + Duration::from_secs(later)),
            GateAction::Wait
        );
    }
}

#[test]
fn the_gate_refuses_a_session_with_no_hook_event() {
    let start = Instant::now();
    let mut gate = ProofGate::new(start, Duration::from_secs(30));
    assert_eq!(
        gate.poll(false, start + Duration::from_secs(30)),
        GateAction::Refuse
    );
    for later in [30, 31, 90] {
        assert_ne!(
            gate.poll(true, start + Duration::from_secs(later)),
            GateAction::Type
        );
    }
}

#[test]
fn an_unproven_interactive_session_types_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let status = dir.path().join("agent-status.jsonl");
    std::fs::write(&status, "{\"event\":\"PreToolUse\"}\n").expect("status file");
    let proof = ProofWait::new(
        status,
        0,
        dir.path().join("settings.json"),
        dir.path().to_path_buf(),
        Duration::from_secs(1),
    );
    let agent = ClaudeAgent::new(None, None, dir.path().to_path_buf());
    let (_tx, rx) = mpsc::channel();
    let flag = dir.path().join("status.flag");
    let files = SessionFiles {
        flag_file: &flag,
        transcript_dir: None,
        transcript_since: SystemTime::now(),
    };
    let mut pump = SessionPump::new(&agent, rx, files, proof);
    let mut typed: Vec<u8> = Vec::new();
    let give_up = Instant::now() + Duration::from_secs(10);
    let err = loop {
        let mut keys = |bytes: &[u8]| -> Result<()> {
            typed.extend_from_slice(bytes);
            Ok(())
        };
        if let Err(e) = pump.advance_proof(Instant::now(), &mut keys) {
            break e;
        }
        assert!(Instant::now() < give_up, "the pump never refused");
        thread::sleep(Duration::from_millis(100));
    };
    assert!(err.downcast_ref::<ralphy_core::HooksUnproven>().is_some());
    assert!(typed.is_empty(), "typed {} bytes", typed.len());
}

#[test]
fn the_waiver_types_the_charter_of_an_unproven_interactive_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let status = dir.path().join("agent-status.jsonl");
    std::fs::write(&status, "{\"event\":\"PreToolUse\"}\n").expect("status file");
    let mut proof = ProofWait::new(
        status,
        0,
        dir.path().join("settings.json"),
        dir.path().to_path_buf(),
        Duration::from_millis(200),
    );
    proof.waiver = true;
    let mut typed: Vec<u8> = Vec::new();
    let give_up = Instant::now() + Duration::from_secs(10);
    while typed.is_empty() {
        let mut keys = |bytes: &[u8]| -> Result<()> {
            typed.extend_from_slice(bytes);
            Ok(())
        };
        proof
            .advance(Instant::now(), &mut keys)
            .expect("the waiver lets the session go on");
        assert!(Instant::now() < give_up, "the charter was never typed");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(typed, charter_keys().text);
}
