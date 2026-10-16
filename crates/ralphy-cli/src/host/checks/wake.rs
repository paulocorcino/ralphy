//! The host checks that keep the daemon running after a sign-out or a
//! restart.

use super::super::shell::{HostOp, HostOs};
use super::{CheckId, CheckStatus, HostCheck, HostFacts};

/// What keeps the daemon running after a sign-out or a restart: lingering on
/// Linux, automatic sign-in and sleep on macOS and Windows.
pub(super) fn wake_checks(facts: &HostFacts, os: HostOs) -> Vec<HostCheck> {
    if os == HostOs::Linux {
        vec![linger_check(facts)]
    } else {
        vec![sign_in_check(facts, os), sleep_check(facts, os)]
    }
}

fn linger_check(facts: &HostFacts) -> HostCheck {
    use CheckStatus::{Fix, Pass, Warn};
    let base = crate::doctor::linger_text(facts.linger);
    let (status, text) = match facts.linger {
        Some(true) => (Pass, base.to_string()),
        Some(false) => (
            Fix(HostOp::EnableLinger),
            format!("{base}. Ralphy turns it on"),
        ),
        None => (Warn, base.to_string()),
    };
    HostCheck::new(CheckId::Linger, status, text)
}

fn sign_in_check(facts: &HostFacts, os: HostOs) -> HostCheck {
    use CheckStatus::{Pass, Warn};
    let after_sign_in = "after a restart, the daemon starts only when someone signs in on the host";
    let (status, text) = match (os, facts.filevault, facts.autologin) {
        (HostOs::MacOs, Some(true), _) => (
            Warn,
            format!("{after_sign_in}. FileVault is on, and it blocks automatic sign-in"),
        ),
        (_, _, Some(true)) => (
            Pass,
            "the host signs in by itself after a restart".to_string(),
        ),
        (HostOs::Windows, _, _) => (
            Warn,
            format!("{after_sign_in}. To sign in by itself, use Autologon from Sysinternals"),
        ),
        _ => (
            Warn,
            format!("{after_sign_in}. Turn on automatic sign-in to change this"),
        ),
    };
    HostCheck::new(CheckId::SignIn, status, text)
}

fn sleep_check(facts: &HostFacts, os: HostOs) -> HostCheck {
    use CheckStatus::{Copy, Pass, Warn};
    let (status, text) = match facts.sleeps {
        Some(false) => (Pass, "the host does not sleep"),
        Some(true) => (
            Copy(
                if os == HostOs::MacOs {
                    "sudo pmset -a sleep 0"
                } else {
                    "powercfg /change standby-timeout-ac 0"
                }
                .to_string(),
            ),
            "the host sleeps when idle, and a sleeping host does not answer",
        ),
        None => (Warn, "could not read the sleep setting"),
    };
    HostCheck::new(CheckId::Sleep, status, text)
}
