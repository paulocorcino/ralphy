//! `host add` and `host check` take the tool checks from `ralphy doctor` on a
//! host that has it, and from the probe on a host that does not.

use super::tests::{linux_host, local, no_keygen, LINUX_PROBE};
use super::*;
use crate::host::checks::tests::description;
use crate::host::checks::CheckId;
use crate::host::report::Report;
use crate::host::ssh::tests::{out, FakeHost};

// format of `ralphy doctor --json` on a host with git, gh signed out, no
// python and one agent CLI; it exits 1 because tools are missing.
const DOCTOR: &str = r#"{"tools":[{"tool":"git","name":"git","ok":true,"required":true,"version":"git version 2.43.0","text":"git version 2.43.0","fix":null},{"tool":"gh","name":"gh","ok":false,"required":true,"version":"gh version 2.45.0","text":"installed, but not signed in","fix":"gh auth login"},{"tool":"python","name":"python","ok":false,"required":true,"version":null,"text":"not found as `python` or `python3`","fix":null},{"tool":"agent","name":"claude","ok":true,"required":true,"version":"2.1.0","text":"2.1.0","fix":null},{"tool":"linger","name":"lingering","ok":true,"required":false,"version":null,"text":"lingering is on","fix":null}]}"#;

fn checks_of(fake: &mut FakeHost) -> Vec<HostCheck> {
    let store = tempfile::tempdir().unwrap();
    check(
        fake,
        &local(store.path()),
        Destination {
            dest: "svrapp",
            key_file: None,
        },
        None,
        no_keygen,
        &mut Report::text(Vec::new()),
    )
    .unwrap()
}

fn get(checks: &[HostCheck], id: CheckId) -> &HostCheck {
    checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no {id:?} in {checks:?}"))
}

#[test]
fn a_host_with_doctor_shows_the_doctor_result() {
    let mut fake = FakeHost::default().answer("doctor --json", out(1, &format!("{DOCTOR}\n"), ""));
    fake.answers
        .append(&mut linux_host(LINUX_PROBE, description("linux")).answers);
    let checks = checks_of(&mut fake);
    assert!(fake.index_of("doctor --json").is_some());
    for id in [CheckId::Git, CheckId::Gh] {
        let rows = checks.iter().filter(|c| c.id == id).count();
        assert_eq!(rows, 1, "one {id:?} row, not a doctor row and a probe row");
    }
    assert_eq!(get(&checks, CheckId::Git).status, CheckStatus::Pass);
    assert_eq!(get(&checks, CheckId::Git).text, "git version 2.43.0");
    assert_eq!(
        get(&checks, CheckId::Gh).status,
        CheckStatus::Copy("gh auth login".to_string())
    );
    assert_eq!(get(&checks, CheckId::Python).status, CheckStatus::Warn);
    assert_eq!(get(&checks, CheckId::Agent).text, "claude (2.1.0)");
    assert!(
        !checks.iter().any(HostCheck::is_blocking),
        "a missing tool never blocks the add: {checks:?}"
    );
}

#[test]
fn a_host_older_than_doctor_shows_the_probe_result() {
    let mut fake = linux_host(LINUX_PROBE, description("linux"));
    let checks = checks_of(&mut fake);
    assert!(fake.index_of("doctor --json").is_some());
    assert!(
        !checks.iter().any(|c| c.id == CheckId::Python),
        "{checks:?}"
    );
    let git = get(&checks, CheckId::Git);
    assert!(git.text.contains("checked once Ralphy runs"), "{git:?}");
}

#[test]
fn a_host_with_no_ralphy_is_not_asked_for_doctor() {
    let mut fake = FakeHost::default()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, LINUX_PROBE, ""))
        .answer("describe", out(127, "", "sh: 1: ralphy: not found\n"));
    let checks = checks_of(&mut fake);
    assert!(fake.index_of("doctor").is_none(), "{:?}", fake.commands());
    assert!(get(&checks, CheckId::Gh)
        .text
        .contains("checked once Ralphy runs"));
}
