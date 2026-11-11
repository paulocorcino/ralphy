use super::tree_to_kill;
#[cfg(unix)]
use super::*;

/// A group kill reaches its target group and stops there — never the caller's.
///
/// `kill_tree_by_pid` used to spend the pgid as *text* on `kill(1)`, which has
/// to guess whether a leading `-` opens an option or names a group. procps-ng
/// guessed wrong and killed the calling process's group, so every
/// `run_headless` teardown sent `SIGKILL` to the process that asked for it: locally the
/// test, on CI the runner itself — which then uploaded no log, leaving the gate
/// red with nothing to read.
///
/// Unix-only because process groups are: the Windows arm walks the process
/// table by parent-PID and never had this exposure.
#[cfg(unix)]
#[test]
fn a_group_kill_spares_the_callers_own_group() {
    use std::time::Duration;

    // A *dead* group leader's pid — the shape production actually hits, since
    // the natural-exit path reaps the child before tearing down its tree.
    let mut leader = Command::new("true");
    own_process_group(&mut leader);
    let mut leader = leader.spawn().expect("spawning `true` should succeed");
    let dead_pgid = leader.id();
    leader.wait().expect("reaping `true` should succeed");

    // The witness deliberately stays in OUR process group (no
    // `own_process_group`), so a kill that escapes its target takes it with us.
    let mut bystander = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawning `sleep` should succeed");

    kill_tree_by_pid(dead_pgid);

    // SIGKILL is delivered without the target getting a say, but delivery is
    // still asynchronous — give it a beat before concluding the witness lived.
    std::thread::sleep(Duration::from_millis(200));
    // `try_wait`, not `pid_is_alive`: a killed-but-unreaped child is a zombie,
    // and `kill(pid, 0)` reports a zombie as alive.
    let died = bystander
        .try_wait()
        .expect("polling the bystander should succeed")
        .is_some();

    #[expect(
        clippy::let_underscore_must_use,
        reason = "cleanup of the witness, which may have died already"
    )]
    let _ = bystander.kill();
    #[expect(
        clippy::let_underscore_must_use,
        reason = "cleanup of the witness, which may have died already"
    )]
    let _ = bystander.wait();

    assert!(
        !died,
        "a kill aimed at group {dead_pgid} escaped and took out a bystander \
             sharing the caller's process group"
    );
}

/// The process doing a tree kill is never ended by it, nor reached through it:
/// a console whose pid reused the daemon launcher's number must not take the
/// daemon (and the daemon's own children) down with it.
#[test]
fn a_tree_kill_never_ends_the_process_doing_it() {
    // 10 is the console being closed; 20 is the caller, whose recorded parent
    // is a launcher that exited and whose pid 10 now names; 30 is the caller's
    // child; 40 is the console's real child.
    let table = [(10, 1), (20, 10), (30, 20), (40, 10)];
    let doomed = tree_to_kill(10, &table, 20, |_| None);
    assert_eq!(doomed, vec![10, 40]);
    assert_eq!(tree_to_kill(20, &table, 20, |_| None), Vec::<u32>::new());
}

/// A process created before the parent it records is not that parent's child:
/// the number was reused. A real child, and a child whose time is unknown,
/// stay in the walk.
#[test]
fn a_process_older_than_its_recorded_parent_is_not_its_child() {
    let table = [(10, 1), (20, 10), (30, 10), (40, 10)];
    let created = |pid: u32| match pid {
        10 => Some(500),
        20 => Some(100), // older than 10: 10 reused 20's dead parent number
        30 => Some(900),
        _ => None,
    };
    let doomed = tree_to_kill(10, &table, 99, created);
    assert_eq!(doomed, vec![10, 30, 40]);
}
