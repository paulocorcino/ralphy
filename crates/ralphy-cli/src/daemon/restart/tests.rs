use super::*;

#[test]
fn started_returns_once_the_daemon_answers() {
    let asked = std::cell::Cell::new(0);
    let answers = || {
        asked.set(asked.get() + 1);
        asked.get() >= 3
    };
    assert!(started(answers, Duration::from_secs(5), Duration::ZERO));
    assert_eq!(asked.get(), 3);
}

#[test]
fn started_gives_up_after_the_timeout() {
    assert!(!started(
        || false,
        Duration::from_millis(30),
        Duration::from_millis(5)
    ));
}

/// A restart continues the log: appending to what the last daemon wrote,
/// creating the file when there is none, never truncating it.
#[test]
fn the_restart_log_is_appended_never_truncated() {
    use std::io::Write;
    let dir = scratch("log");
    {
        let mut log = open_log(&dir).expect("creating the log");
        log.write_all(
            b"first daemon
",
        )
        .expect("writing");
    }
    {
        let mut log = open_log(&dir).expect("reopening the log");
        log.write_all(
            b"second daemon
",
        )
        .expect("writing");
    }
    let text = std::fs::read_to_string(dir.join("daemon.log")).expect("reading");
    assert_eq!(
        text,
        "first daemon
second daemon
"
    );
}

/// A restarted daemon must outlive the program that ran `daemon restart`,
/// so it may not stay in that program's job. The test joins a job that
/// allows breakaway (no kill-on-close, so the rest of the binary is not at
/// risk) and checks where the detached child ends up.
#[cfg(windows)]
#[test]
#[expect(unsafe_code, reason = "FFI: the job object calls of the test")]
fn a_restarted_daemon_leaves_the_job_of_its_launcher() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: both arguments may be null: no security attributes, no name.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!job.is_null(), "creating a job object");
    // SAFETY: the struct is plain data, and all zero bytes is a valid value.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    // SAFETY: `job` is a live handle, and the pointer and size describe `info`.
    let set = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&info).cast(),
            u32::try_from(size_of_val(&info)).expect("the struct size fits u32"),
        )
    };
    assert_ne!(set, FALSE, "allowing breakaway on the test job");
    // SAFETY: `job` is a live handle; the current-process pseudo handle is always valid.
    let joined = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
    assert_ne!(joined, FALSE, "putting the test process in the job");

    let store = scratch("job");
    let mut child = spawn_detached(
        Path::new("ping"),
        &["-n".into(), "30".into(), "127.0.0.1".into()],
        &store,
    )
    .expect("spawning the stand-in daemon");
    let mut in_job = FALSE;
    // SAFETY: the child is alive until the kill below, and `job` is the job
    // object created above; `in_job` is a valid out-parameter.
    let asked = unsafe { IsProcessInJob(child.as_raw_handle(), job, &raw mut in_job) };
    child.kill().expect("ending the stand-in daemon");
    child.wait().expect("reaping the stand-in daemon");
    // SAFETY: `job` is a live handle this test owns and closes once.
    unsafe { CloseHandle(job) };
    assert_ne!(asked, FALSE, "asking whether the child is in the job");
    assert_eq!(
        in_job, FALSE,
        "the detached daemon stayed in its launcher's job"
    );
}

#[cfg(windows)]
#[test]
fn only_access_denied_means_the_job_refused_breakaway() {
    assert!(breakaway_refused(&std::io::Error::from_raw_os_error(5)));
    assert!(!breakaway_refused(&std::io::Error::from_raw_os_error(2)));
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-restart-{}-{tag}", std::process::id()));
    remove_scratch(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn remove_scratch(dir: &Path) {
    // the scratch directory may be gone
    match std::fs::remove_dir_all(dir) {
        Ok(()) | Err(_) => {}
    }
}

fn ours() -> PathBuf {
    PathBuf::from("/opt/ralphy/ralphy")
}

fn record(dir: &Path, pid: u32) {
    ralphy_daemon::pidfile::write_in(dir, pid, &ours(), &[]).expect("write");
}

#[test]
fn an_unrecorded_invocation_falls_back_to_a_bare_daemon() {
    assert_eq!(effective(&[]), vec!["daemon".to_owned()]);
    let recorded = vec!["daemon".to_owned(), "--port".to_owned(), "8080".to_owned()];
    assert_eq!(
        effective(&recorded),
        recorded,
        "a daemon on 8080 must come back on 8080, not on the default"
    );
}

#[test]
fn a_reported_path_carries_no_prefix_nobody_typed() {
    assert_eq!(
        readable(Path::new(r"\\?\C:\Program Files\ralphy\ralphy.exe")),
        r"C:\Program Files\ralphy\ralphy.exe"
    );
    assert_eq!(
        readable(Path::new("/usr/local/bin/ralphy")),
        "/usr/local/bin/ralphy"
    );
}

#[test]
fn the_same_program_survives_a_move_and_a_spelling() {
    assert!(same_program(
        Path::new("/opt/ralphy/ralphy"),
        Path::new("/opt/ralphy/ralphy")
    ));
    assert!(
        same_program(
            Path::new(r"C:\bin\ralphy.exe"),
            Path::new(r"C:\bin\RALPHY.EXE")
        ),
        "canonicalization and case must not make a daemon look like a stranger"
    );
    assert!(!same_program(
        Path::new("/bin/ralphy"),
        Path::new("/usr/sbin/sshd")
    ));
}

#[test]
fn a_daemon_whose_image_was_parked_is_still_that_daemon() {
    // The ordinary post-update state: `install` renamed the image the daemon
    // is still executing, so the record says `ralphy.exe` and the OS reports
    // `ralphy.exe.old`. Refusing there would leave the old daemon running
    // and clear the record — defeating the restart entirely.
    assert!(same_program(
        Path::new(r"C:\bin\ralphy.exe"),
        Path::new(r"C:\bin\ralphy.exe.old")
    ));
    assert!(
        same_program(
            Path::new("/usr/local/bin/ralphy"),
            Path::new("/usr/local/bin/ralphy.old.3")
        ),
        "a second install steps the park aside; it is still our binary"
    );
    // And the leniency does not reach past the park suffix.
    assert!(!same_program(
        Path::new("/bin/ralphy"),
        Path::new("/bin/sshd.old")
    ));
    // Linux names an image whose file was replaced or removed this way.
    assert!(same_program(
        Path::new("/home/u/.ralphy/bin/ralphy"),
        Path::new("/home/u/.ralphy/bin/ralphy (deleted)")
    ));
    assert!(same_program(
        Path::new("/usr/local/bin/ralphy"),
        Path::new("/usr/local/bin/ralphy.old (deleted)")
    ));
    assert!(!same_program(
        Path::new("/bin/ralphy"),
        Path::new("/bin/sshd (deleted)")
    ));
    assert_eq!(unparked("ralphy.exe"), "ralphy.exe");
    assert_eq!(unparked("ralphy.exe.old"), "ralphy.exe");
    assert_eq!(unparked("ralphy.old.12"), "ralphy");
}

#[test]
fn no_pid_file_means_nothing_to_stop() {
    let dir = scratch("none");
    assert!(!stop_recorded(
        &dir,
        EXIT_TIMEOUT,
        |_| true,
        |_| Some(ours()),
        |_| unreachable!("nothing to kill")
    )
    .expect("no file is not an error"));
    remove_scratch(&dir);
}

#[test]
fn a_stale_pid_is_cleared_rather_than_killed() {
    let dir = scratch("stale");
    record(&dir, 424_242);
    assert!(!stop_recorded(
        &dir,
        EXIT_TIMEOUT,
        |_| false,
        |_| None,
        |_| unreachable!("a dead pid is not killed")
    )
    .expect("a dead pid is not an error"));
    assert_eq!(
        ralphy_daemon::pidfile::read_in(&dir),
        None,
        "a ghost must not survive to confuse the next restart"
    );
    remove_scratch(&dir);
}

#[test]
fn a_reused_pid_is_never_killed() {
    // The defect this guards: the file outlives a crash or a reboot, the
    // number comes back as something else, and a reader that checked only
    // liveness would end that process and its whole tree.
    let dir = scratch("reused");
    record(&dir, 4242);
    let stopped = stop_recorded(
        &dir,
        EXIT_TIMEOUT,
        |_| true,
        |_| Some(PathBuf::from("/usr/sbin/sshd")),
        |_| unreachable!("a stranger must never be killed"),
    )
    .expect("a reused pid is not an error");
    assert!(!stopped);
    assert_eq!(
        ralphy_daemon::pidfile::read_in(&dir),
        None,
        "and the worthless record is dropped"
    );
    remove_scratch(&dir);
}

#[test]
fn an_unprovable_identity_refuses_rather_than_guessing() {
    // A pid file from an older build records no exe, and some platforms
    // cannot name a process's program at all. Neither is permission to kill.
    let dir = scratch("unprovable");
    std::fs::write(ralphy_daemon::pidfile::pid_path_in(&dir), "4242\ndaemon\n")
        .expect("legacy file");
    let err = stop_recorded(
        &dir,
        EXIT_TIMEOUT,
        |_| true,
        |_| Some(ours()),
        |_| unreachable!("nothing is killed on a guess"),
    )
    .expect_err("an unprovable identity must refuse");
    assert!(err.to_string().contains("cannot prove pid 4242"), "{err}");

    let err = stop_recorded(
        &dir,
        EXIT_TIMEOUT,
        |_| true,
        |_| None,
        |_| unreachable!("nothing is killed on a guess"),
    )
    .expect_err("a platform that cannot say must refuse too");
    assert!(err.to_string().contains("stop it yourself"), "{err}");
    remove_scratch(&dir);
}

#[test]
fn a_pid_that_never_dies_is_reported_not_ignored() {
    let dir = scratch("wedged");
    record(&dir, 424_242);
    let killed = std::sync::atomic::AtomicU32::new(0);
    // A short timeout: the branch under test is the give-up, not the wait.
    let err = stop_recorded(
        &dir,
        Duration::from_millis(50),
        |_| true,
        |_| Some(ours()),
        |_| {
            killed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    )
    .expect_err("a wedged daemon must be reported, not silently doubled");
    assert!(err.to_string().contains("did not exit"), "{err}");
    assert_eq!(killed.load(std::sync::atomic::Ordering::SeqCst), 1);
    remove_scratch(&dir);
}

#[test]
fn a_live_matching_pid_is_killed_once_and_the_file_cleared() {
    let dir = scratch("live");
    record(&dir, 4242);
    let killed = std::sync::atomic::AtomicU32::new(0);
    // Alive until the kill lands, gone after — the ordinary path.
    let stopped = stop_recorded(
        &dir,
        EXIT_TIMEOUT,
        |_| killed.load(std::sync::atomic::Ordering::SeqCst) == 0,
        |_| Some(ours()),
        |_| {
            killed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    )
    .expect("stop");
    assert!(stopped);
    assert_eq!(killed.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(ralphy_daemon::pidfile::read_in(&dir), None);
    remove_scratch(&dir);
}
