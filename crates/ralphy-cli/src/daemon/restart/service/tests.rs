use super::*;

#[test]
fn a_unit_that_is_not_running_has_no_main_pid() {
    assert_eq!(
        parse_main_pid(
            "4242
"
        ),
        Some(4242)
    );
    assert_eq!(
        parse_main_pid(
            "0
"
        ),
        None,
        "systemd prints 0 for a stopped unit; no daemon has pid 0"
    );
    assert_eq!(parse_main_pid(""), None);
    assert_eq!(parse_main_pid("Failed to connect to bus"), None);
}

#[test]
fn a_launchd_agent_reports_its_pid_only_while_it_runs() {
    // `launchctl list dev.ralphy.daemon` on macOS 12.7.6, running and not.
    let running = "{\n\t\"Label\" = \"dev.ralphy.daemon\";\n\t\"LastExitStatus\" = 256;\n\t\
        \"PID\" = 3136;\n\t\"Program\" = \"/Users/user/.ralphy/bin/ralphy\";\n};\n";
    let stopped = "{\n\t\"Label\" = \"dev.ralphy.daemon\";\n\t\"LastExitStatus\" = 256;\n};\n";
    assert_eq!(parse_launchd_pid(running), Some(3136));
    assert_eq!(parse_launchd_pid(stopped), None);
    assert_eq!(parse_launchd_pid(""), None);
    assert_eq!(
        kickstart_argv("501"),
        vec!["launchctl", "kickstart", "-k", "gui/501/dev.ralphy.daemon"]
    );
}

#[test]
fn a_daemon_outside_a_loaded_agent_is_stopped_before_the_kickstart() {
    assert_eq!(launchd_plan(Some(3136), None), LaunchdPlan::NotTheAgent);
    assert_eq!(
        launchd_plan(Some(3136), Some(Some(3136))),
        LaunchdPlan::Kickstart
    );
    // What a restart before this fix left: the agent retrying, not running.
    assert_eq!(
        launchd_plan(Some(3061), Some(None)),
        LaunchdPlan::StopThenKickstart
    );
    assert_eq!(
        launchd_plan(Some(3061), Some(Some(3136))),
        LaunchdPlan::StopThenKickstart
    );
    // The record was erased while the agent's daemon serves (rc.31 on the Mac).
    assert_eq!(launchd_plan(None, Some(Some(3307))), LaunchdPlan::Kickstart);
    assert_eq!(launchd_plan(None, Some(None)), LaunchdPlan::NotTheAgent);
}
