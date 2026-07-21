//! Gemini authentication detection (ADR-0043 D6), in two tiers: a free preflight
//! judged on the vendor's own exit code, and an in-flight text matcher for a
//! credential that expires mid-run.
//!
//! **No credential file is ever read** (D17). The vendor's credential lives in the
//! OS credential store or its own root; Ralphy neither reads, copies nor replays
//! it — the verdict comes from spawning the vendor and reading how it exited.

use std::time::Duration;

/// The vendor's own instruction, reproduced verbatim. Quoting it rather than
/// paraphrasing matters: the operator who hits this needs the exact variable names
/// the CLI itself would have printed, and `<root>` is Ralphy's owned root, not
/// theirs.
pub const GEMINI_AUTH_ERROR_MSG: &str =
    "Gemini is not authenticated — Please set an Auth method in your \
     <root>/settings.json or specify one of the following environment variables \
     before running: GEMINI_API_KEY, GOOGLE_GENAI_USE_VERTEXAI, GOOGLE_GENAI_USE_GCA";

/// The exit code the CLI uses for "no authentication method" (ADR-0043 D3). The
/// PRIMARY signal: the sentence is localizable and has already been reworded
/// upstream, whereas the code is part of the documented taxonomy.
pub(crate) const AUTH_EXIT: i32 = 41;

/// Return `true` when `text` shows a Gemini authentication failure — the
/// secondary, in-flight tier, for a credential that expires after the preflight.
pub(crate) fn is_gemini_auth_error(text: &str) -> bool {
    ralphy_adapter_support::auth_error(text, &[&["please set an auth method"]])
}

/// Ask the CLI itself whether the operator is authenticated — the ADR-0013
/// preflight, and what `ralphy init`'s gate reports.
///
/// `--list-sessions` is the probe because it is the only auth-sensitive verb the
/// spike observed exiting 41 while logged out that makes **no paid model call**
/// (`skills list`, `mcp list` and `extensions list` all exit 0 logged out and so
/// discriminate nothing).
///
/// Observed on an authenticated host (2026-07-21, gemini 0.51.0): exit 0. The
/// verdict keys on `== AUTH_EXIT` alone regardless, so a future non-zero
/// success code does not turn every authenticated operator into a logged-out one.
///
/// The probe runs in a throwaway cwd — outside any repository, so nothing
/// repo-local is read — against Ralphy's own root under `<home>/.ralphy`, which is
/// the same `root::ensure` the run path uses rather than a second implementation.
/// A missing binary, a wedged probe or a timeout all read as "not authenticated":
/// the gate's job is to tell the operator what to fix.
pub fn probe_gemini_login() -> bool {
    // Before any side effect: `ralphy init` walks every agent in `Agent::ALL`, so
    // materialising Ralphy's gemini root here would create it on every machine,
    // including for operators who have never installed this vendor.
    if crate::command::locate_gemini().is_none() {
        return false;
    }
    let Some(base) = ralphy_proc_util::home_dir().map(|h| h.join(".ralphy")) else {
        return false;
    };
    let Ok(root) = crate::root::ensure(&base) else {
        return false;
    };
    let Ok(scratch) = tempfile::tempdir() else {
        return false;
    };
    let auth_type = crate::root::operator_auth_type(crate::root::operator_root().as_deref());

    let mut cmd = std::process::Command::new(crate::command::resolve_gemini_program());
    cmd.current_dir(scratch.path())
        .arg("--list-sessions")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::command::apply_auth_env(
        &mut cmd,
        std::env::vars().map(|(k, _)| k),
        auth_type.as_deref(),
        &root.home,
    );

    let verdict = match ralphy_adapter_support::run_headless(cmd, "", Duration::from_secs(30)) {
        Ok(out) if !out.timed_out => {
            gemini_login_verdict(out.exit.and_then(|s| s.code()), &out.stderr)
        }
        _ => false,
    };
    drop(scratch);
    verdict
}

/// The verdict mapping, split from the spawn so it is testable: exit 41 — and only
/// exit 41 — means "not authenticated". Anything else, including a code the
/// taxonomy does not yet assign, is not an authentication answer, so the operator
/// is not told to log in over an unrelated failure.
///
/// `stderr` is the SECONDARY tier, for the case where the vendor prints its own
/// sentence under a different code.
pub(crate) fn gemini_login_verdict(exit_code: Option<i32>, stderr: &str) -> bool {
    if exit_code == Some(AUTH_EXIT) {
        return false;
    }
    !is_gemini_auth_error(stderr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D6: the message reproduces the vendor's own sentence, so the operator gets
    /// the exact variable names the CLI would have printed.
    #[test]
    fn the_auth_message_reproduces_the_vendor_sentence() {
        assert!(
            GEMINI_AUTH_ERROR_MSG.contains("Please set an Auth method in your"),
            "{GEMINI_AUTH_ERROR_MSG}"
        );
        assert!(
            GEMINI_AUTH_ERROR_MSG
                .contains("GEMINI_API_KEY, GOOGLE_GENAI_USE_VERTEXAI, GOOGLE_GENAI_USE_GCA"),
            "{GEMINI_AUTH_ERROR_MSG}"
        );
    }

    /// The exit code is the primary signal, and the sentence is the fallback.
    #[test]
    fn auth_exit_41_is_the_primary_signal() {
        assert_eq!(AUTH_EXIT, 41);
        assert!(!gemini_login_verdict(Some(41), ""));
        assert!(gemini_login_verdict(Some(0), ""));
        // An unassigned code is not an authentication answer.
        assert!(gemini_login_verdict(Some(999), ""));
        assert!(gemini_login_verdict(None, ""));

        const VENDOR: &str = "Please set an Auth method in your /root/.gemini/settings.json or \
             specify one of the following environment variables before running: \
             GEMINI_API_KEY, GOOGLE_GENAI_USE_VERTEXAI, GOOGLE_GENAI_USE_GCA";
        assert!(is_gemini_auth_error(VENDOR));
        assert!(!is_gemini_auth_error("everything is fine"));
    }

    /// The regression this guards: a text-only matcher misses a localized or
    /// reworded sentence, so exit 41 must decide even when stderr says nothing.
    #[test]
    fn gemini_login_verdict_reads_exit_41_not_the_text() {
        assert!(
            !gemini_login_verdict(Some(41), "Bitte legen Sie eine Authentifizierung fest"),
            "exit 41 is the verdict whatever language the sentence is in"
        );
        assert!(
            !gemini_login_verdict(Some(41), ""),
            "a silent exit 41 is still logged out"
        );
    }

    /// D17: the credential is never read, copied or replayed — the probe reaches
    /// the vendor only through the shared headless runner, and the ONE file
    /// anything in this crate reads out of the operator's root is
    /// `settings.json` (the non-secret auth-mode pointer, via
    /// `root::operator_auth_type`).
    ///
    /// The ban list is over the whole crate, not just this file: pinning a naming
    /// taboo in `auth.rs` alone would be satisfied by moving the read one module
    /// over, which is exactly what the invariant must not permit. Fragments
    /// assembled with `concat!` so the assertion cannot match itself.
    #[test]
    fn the_auth_probe_reads_no_credential() {
        let crate_src = [
            include_str!("auth.rs"),
            include_str!("root.rs"),
            include_str!("command.rs"),
            include_str!("policy.rs"),
            include_str!("outcome.rs"),
            include_str!("lib.rs"),
        ]
        .map(|s| s.split("#[cfg(test)]").next().unwrap().to_string())
        .join("\n");
        for banned in [
            "oauth_creds",
            "google_accounts",
            "keytar",
            "access_token",
            "refresh_token",
        ] {
            assert!(
                !crate_src.contains(banned),
                "the credential is never read (D17); found {banned}"
            );
        }
        // Every read of the operator's root names `settings.json` and nothing
        // else — a second filename here would be a second thing being read.
        let operator_reads: Vec<&str> = crate_src
            .match_indices("root?.join(")
            .map(|(i, _)| &crate_src[i..crate_src[i..].find(')').unwrap() + i])
            .collect();
        assert_eq!(
            operator_reads,
            [concat!("root?.", "join(\"settings.json\"")],
            "only the non-secret auth pointer may be read from the operator's root"
        );

        let production = include_str!("auth.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            production.contains(concat!("run_", "headless(")),
            "the probe must reach the vendor through the shared runner"
        );
    }
}
