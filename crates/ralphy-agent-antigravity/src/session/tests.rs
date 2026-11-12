use super::*;

fn session<'a>(dir: &'a Path, log_path: &'a Path) -> Session<'a> {
    Session {
        program: Path::new("agy"),
        root: dir,
        work_dir: dir,
        run_dir: dir,
        log_path,
        model: None,
        effort: None,
        waiver: false,
        timeout: Duration::from_secs(60),
        idle_minutes: 0,
    }
}

#[test]
fn the_proof_message_is_one_fixed_ndjson_line() {
    let line = proof_line();
    assert_eq!(
        line,
        "{\"event\":\"user\",\"message\":{\"content\":\"Reply with the single word READY.\",\
         \"role\":\"user\"}}\n"
    );
    assert_eq!(line.matches('\n').count(), 1, "one line: {line:?}");
    assert!(line.ends_with('\n'));
    assert!(std::ptr::eq(line, proof_line()), "the same text every time");
}

#[test]
fn the_child_is_pointed_at_a_fresh_status_file_in_the_run_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stale = dir.path().join(status::STATUS_FILE);
    fs::write(&stale, "{\"event\":\"SessionStart\"}\n").expect("stale line");
    let log = dir.path().join("session.log");
    let (cmd, status_file) = session(dir.path(), &log).command().expect("command");
    assert_eq!(status_file, stale);
    let env: Vec<_> = cmd
        .get_envs()
        .filter(|(name, _)| *name == status::STATUS_ENV)
        .map(|(_, value)| value.map(PathBuf::from))
        .collect();
    assert_eq!(env, [Some(status_file.clone())]);
    assert_eq!(fs::read(&status_file).expect("status file"), b"");
}

#[test]
fn a_refused_task_gets_no_start_context() {
    let refused = anyhow::Error::new(HooksUnproven {
        cause: "no SessionStart".into(),
    })
    .context("the prompt was held and not sent");
    assert!(!format!("{:#}", spawn_context(refused)).contains(SPAWN_ERR));
    let signed_out = anyhow::Error::new(SignedOut("sign in".into())).context("held");
    assert!(!format!("{:#}", spawn_context(signed_out)).contains(SPAWN_ERR));
    let missing = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::NotFound));
    assert!(format!("{:#}", spawn_context(missing)).contains(SPAWN_ERR));
}
