use serde_json::Value;

use super::*;
use crate::host::checks::CheckId;
use crate::host::shell::HostOp;
use crate::host::ssh::{ssh_error, SshFailure};

fn lines(report: Report<Vec<u8>>) -> Vec<Value> {
    String::from_utf8(report.into_inner())
        .expect("the report is UTF-8")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l:?} is not JSON: {e}")))
        .collect()
}

#[test]
fn json_checks_are_one_event_each_with_the_fix_command() {
    let checks = [
        HostCheck {
            id: CheckId::Ralphy,
            status: CheckStatus::Pass,
            text: "Ralphy 0.1".to_string(),
        },
        HostCheck {
            id: CheckId::Autostart,
            status: CheckStatus::Fix(HostOp::InstallAutostart),
            text: "Ralphy installs its autostart".to_string(),
        },
        HostCheck {
            id: CheckId::Name,
            status: CheckStatus::Copy("ralphy host add svrapp --name <name>".to_string()),
            text: "the daemon on the host has no name".to_string(),
        },
    ];
    let mut report = Report::json(Vec::new());
    report.checks("svrapp", HostOs::Linux, &checks).unwrap();
    let events = lines(report);
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[0]["event"], "check");
    assert_eq!(events[0]["id"], "ralphy");
    assert_eq!(events[0]["status"], "pass");
    assert_eq!(events[0]["command"], Value::Null);
    assert_eq!(events[1]["id"], "autostart");
    assert_eq!(events[1]["status"], "fix");
    assert_eq!(
        events[1]["command"],
        render(Some(HostOs::Linux), &HostOp::InstallAutostart).unwrap()
    );
    assert_eq!(events[2]["id"], "name");
    assert_eq!(events[2]["status"], "copy");
    assert_eq!(events[2]["command"], "ralphy host add svrapp --name <name>");
}

#[test]
fn json_failed_names_the_ssh_failure_kind() {
    let mut report = Report::json(Vec::new());
    let e = ssh_error(
        SshFailure::Unreachable,
        "could not reach svrapp".to_string(),
    )
    .context("checking svrapp");
    report.failed(&e).unwrap();
    report.failed(&anyhow::anyhow!("x")).unwrap();
    let events = lines(report);
    assert_eq!(events[0]["event"], "failed");
    assert_eq!(events[0]["kind"], "unreachable");
    assert_eq!(
        events[0]["message"],
        "checking svrapp: could not reach svrapp"
    );
    assert_eq!(events[1]["kind"], "other");
    assert_eq!(events[1]["message"], "x");
}

#[test]
fn text_mode_prints_no_failed_line_and_notes_as_lines() {
    let mut report = Report::text(Vec::new());
    report.note("Forgot vps on this computer.").unwrap();
    report.failed(&anyhow::anyhow!("x")).unwrap();
    let printed = String::from_utf8(report.into_inner()).unwrap();
    assert_eq!(printed, "Forgot vps on this computer.\n");
}
