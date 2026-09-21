//! The `*_at` store helpers behind `/api/security/*`: path-explicit reads and
//! writes of the real enrolment files under the daemon store (`daemon-token`,
//! `daemon-totp`, `daemon-totp.pending`, `daemon-password`,
//! `daemon-require-login`), so a test passes a tempdir and the routes stay thin.

use std::path::Path;

use anyhow::Result;

use crate::{auth, password, totp};

/// The daemon's auth-state surface for the Security modal (issue #195): which
/// factors are enrolled in the REAL stores. `require_login` is the PERSISTED
/// `daemon-require-login` flag (ADR-0032 amendment §A): an operator opt-in that
/// gates the browser UI even on a loopback bind, no longer derived from the seed.
#[derive(serde::Serialize)]
pub(crate) struct SecurityState {
    pub(crate) token_set: bool,
    pub(crate) password_set: bool,
    pub(crate) totp_enrolled: bool,
    pub(crate) require_login: bool,
}

/// Read the real store FILES under `dir` and report enrolment. Path-explicit (no
/// env reads) so tests pass a tempdir. `require_login` is now the PERSISTED flag
/// (ADR-0032 amendment §A), no longer derived from the seed.
pub(crate) fn security_state_at(dir: &Path) -> SecurityState {
    let totp_enrolled = totp::load_seed_from(&totp::seed_path_in(dir))
        .ok()
        .flatten()
        .is_some();
    SecurityState {
        token_set: auth::load_token_from(&auth::token_path_in(dir))
            .ok()
            .flatten()
            .is_some(),
        password_set: password::load_from(&password::password_path_in(dir))
            .ok()
            .flatten()
            .is_some(),
        totp_enrolled,
        require_login: auth::require_login_enabled_in(dir),
    }
}

/// Begin enrolment: mint-once a PENDING TOTP seed under `dir` and return its
/// `otpauth://` URI + whether it was newly minted. The seed is NOT armed — it
/// gates nothing until [`confirm_totp_at`] verifies a code (ADR-0032 amendment
/// §C). The URI is shown once (QR + base32); a re-enrol before confirming
/// returns the SAME pending secret with `newly_minted=false`.
pub(crate) fn enroll_totp_at(dir: &Path) -> Result<(String, bool)> {
    let (seed, newly_minted) = totp::ensure_seed_at(&totp::pending_seed_path_in(dir))?;
    Ok((seed.otpauth_uri("ralphy", "daemon"), newly_minted))
}

/// Confirm a pending enrolment: verify `code` against the pending seed and, on
/// success, promote it to the live seed. Returns whether the code verified.
pub(crate) fn confirm_totp_at(dir: &Path, code: &str, now: u64) -> Result<bool> {
    totp::confirm_pending_at(
        &totp::pending_seed_path_in(dir),
        &totp::seed_path_in(dir),
        code,
        now,
    )
}

/// Revoke enrolment: delete the live seed, any in-flight pending seed, and the
/// anti-replay last-step marker, so an abandoned enrolment leaves nothing behind
/// (ADR-0032 amendment §C/§D).
pub(crate) fn revoke_totp_at(dir: &Path) -> Result<()> {
    totp::revoke_seed_at(&totp::seed_path_in(dir))?;
    totp::revoke_seed_at(&totp::pending_seed_path_in(dir))?;
    totp::revoke_seed_at(&totp::last_step_path_in(dir))
}

/// Set (non-empty) or clear (empty/absent) the optional password under `dir`;
/// return whether a password is now enrolled.
pub(crate) fn set_password_at(dir: &Path, password: Option<&str>) -> Result<bool> {
    let path = password::password_path_in(dir);
    match password.filter(|p| !p.is_empty()) {
        Some(pw) => {
            password::save_to(&password::Hash::hash_password(pw), &path)?;
            Ok(true)
        }
        None => {
            password::clear_at(&path)?;
            Ok(false)
        }
    }
}

/// Remint the access token under `dir`, overwriting any prior. The token is
/// never echoed — only its rotation is reported.
pub(crate) fn remint_token_at(dir: &Path) -> Result<()> {
    auth::save_token_to(&auth::generate_token(), &auth::token_path_in(dir))
}

/// The require-login gate (ADR-0032 amendment §A): persist the operator's choice
/// as the `daemon-require-login` flag. Enabling demands an armed TOTP seed
/// (`Err("totp not enrolled")` otherwise) and MINTS the access token if absent —
/// gating a loopback bind needs a signing key, and machine clients then use it as
/// a bearer. Disabling just clears the flag.
pub(crate) fn require_login_at(dir: &Path, enable: bool) -> Result<()> {
    if enable {
        if totp::load_seed_from(&totp::seed_path_in(dir))
            .ok()
            .flatten()
            .is_none()
        {
            anyhow::bail!("totp not enrolled");
        }
        // Ensure a signing key exists (mint-once) so the gate can sign cookies —
        // a loopback bind may never have minted one.
        auth::ensure_token_at(&auth::token_path_in(dir))?;
    }
    auth::set_require_login_in(dir, enable)
}
