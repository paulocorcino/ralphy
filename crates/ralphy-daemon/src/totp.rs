//! TOTP (RFC 6238) — the core browser-login factor for a network bind (issue
//! #179, ADR-0032 §4). A mint-once 20-byte seed lives in the global store
//! (`<home>/.ralphy/daemon-totp`, mode 0600) beside `daemon-token`, so its
//! lifecycle survives a re-`daemon setup`. Enrolment shows the `otpauth://` URI
//! (QR + base32) exactly once; a login verifies a 6-digit code against the seed
//! over a ±1-step window (clock skew, RFC 6238 §5.2).
//!
//! Pure sync, path-explicit like `auth`/`identity`: tests pass a temp path and
//! never mutate the process-global env (the `RALPHY_*_DIR` env-race trap). Code
//! comparison routes through `auth::ct_eq` — the ONE constant-time compare.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::auth;

/// The TOTP time step in seconds (RFC 6238 default period, `T0 = 0`).
const PERIOD: u64 = 30;

/// A raw TOTP seed: 20 bytes (the RFC 6238 SHA-1 HMAC key size).
#[derive(Clone)]
pub struct Seed(Vec<u8>);

impl Seed {
    /// Wrap raw seed bytes (used by tests with the RFC 6238 vector seed).
    pub fn from_bytes(bytes: Vec<u8>) -> Seed {
        Seed(bytes)
    }

    /// The base32 (no padding) encoding of the seed — what an authenticator app
    /// stores and what the `otpauth://` URI carries.
    pub fn secret_base32(&self) -> String {
        data_encoding::BASE32_NOPAD.encode(&self.0)
    }

    /// The `otpauth://totp/…` enrolment URI: what the QR encodes and what a
    /// pasted base32 secret reconstructs. SHA-1 / 6 digits / 30s period are the
    /// RFC 6238 defaults every authenticator assumes.
    pub fn otpauth_uri(&self, issuer: &str, account: &str) -> String {
        let secret = self.secret_base32();
        format!(
            "otpauth://totp/{issuer}:{account}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits=6&period=30"
        )
    }

    /// The 6-digit code for an explicit counter (RFC 6238 dynamic truncation).
    /// `pub(crate)` so the router integration test can compute the current code
    /// without exposing the primitive publicly.
    pub(crate) fn code_at(&self, counter: u64) -> String {
        let mut mac =
            Hmac::<Sha1>::new_from_slice(&self.0).expect("HMAC accepts a key of any length");
        mac.update(&counter.to_be_bytes());
        let hs = mac.finalize().into_bytes();
        // RFC 6238 §5.3 dynamic truncation: low 4 bits of the last byte pick the
        // 4-byte window; mask the top bit, take mod 10^6, zero-pad to 6.
        let offset = (hs[hs.len() - 1] & 0x0f) as usize;
        let bin = ((hs[offset] as u32 & 0x7f) << 24)
            | ((hs[offset + 1] as u32) << 16)
            | ((hs[offset + 2] as u32) << 8)
            | (hs[offset + 3] as u32);
        format!("{:06}", bin % 1_000_000)
    }

    /// The time step `code` matches for `unix_secs` within `±skew_steps` of clock
    /// skew, or `None`. The step is the anti-replay key (amendment §D): a login
    /// records it, and a later code whose step is not strictly greater is a replay.
    /// Each candidate compares constant-time via `auth::ct_eq`.
    pub fn matched_step(&self, code: &str, unix_secs: u64, skew_steps: i64) -> Option<u64> {
        let counter = (unix_secs / PERIOD) as i64;
        for s in -skew_steps..=skew_steps {
            let c = counter + s;
            if c < 0 {
                continue;
            }
            if auth::ct_eq(self.code_at(c as u64).as_bytes(), code.as_bytes()) {
                return Some(c as u64);
            }
        }
        None
    }

    /// Whether `code` is a valid TOTP for `unix_secs`, accepting `±skew_steps`
    /// time steps of clock skew (no anti-replay — see [`matched_step`](Self::matched_step)).
    pub fn verify(&self, code: &str, unix_secs: u64, skew_steps: i64) -> bool {
        self.matched_step(code, unix_secs, skew_steps).is_some()
    }
}

/// Generate a fresh 20-byte seed from the OS CSPRNG (mirrors
/// `auth::generate_token`).
pub fn generate_seed() -> Seed {
    let mut bytes = vec![0u8; 20];
    getrandom::getrandom(&mut bytes).expect("the OS CSPRNG must be available to mint a TOTP seed");
    Seed(bytes)
}

/// The `daemon-totp` seed path inside `dir`. Path-explicit so the security
/// routes/tests point it at a temp dir without touching the env.
pub fn seed_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-totp")
}

/// The production path of `daemon-totp`. Mirrors [`auth::token_path`].
pub fn seed_path() -> Result<PathBuf> {
    Ok(seed_path_in(&auth::store_dir()?))
}

/// The in-flight (pending) enrolment seed path inside `dir`
/// (`daemon-totp.pending`). A seed here has NOT been confirmed: it never counts
/// as enrolled and never gates login until [`confirm_pending_at`] promotes it
/// (ADR-0032 amendment §C — prove possession before the factor is armed).
pub fn pending_seed_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-totp.pending")
}

/// The `daemon-totp-laststep` path inside `dir`: the last TOTP step consumed by a
/// successful login (anti-replay, amendment §D). Path-explicit like the seed.
pub fn last_step_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-totp-laststep")
}

/// The last consumed step, or `None` when no login has happened yet / the file is
/// absent or malformed (malformed → `None` is safe: it only means the next code
/// is not treated as a replay).
pub fn load_last_step_from(path: &Path) -> Result<Option<u64>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text.trim().parse().ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Record `step` as the last consumed step, owner-only.
pub fn save_last_step_to(step: u64, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, step.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    auth::set_owner_only(path)?;
    Ok(())
}

/// Confirm a pending enrolment: verify `code` (±1 step) against the seed at
/// `pending_path` and, on success, promote it to `live_path` and delete the
/// pending file. Returns whether the code verified. `Ok(false)` when no pending
/// seed exists or the code is wrong — the pending seed is left in place on a
/// wrong code so the operator can retry from the same QR.
pub fn confirm_pending_at(
    pending_path: &Path,
    live_path: &Path,
    code: &str,
    now_unix: u64,
) -> Result<bool> {
    let Some(seed) = load_seed_from(pending_path)? else {
        return Ok(false);
    };
    if !seed.verify(code, now_unix, 1) {
        return Ok(false);
    }
    save_seed_to(&seed, live_path)?;
    revoke_seed_at(pending_path)?;
    Ok(true)
}

/// Remove the seed file at `path` (revoke enrolment); mint-once means a fresh
/// enrol mints a NEW seed afterwards. `Ok(())` when already absent — revocation
/// is idempotent.
pub fn revoke_seed_at(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Load a seed from `path` (base32 text), or `Ok(None)` when the file does not
/// exist yet (an un-enrolled seed). Trims trailing whitespace/newlines.
pub fn load_seed_from(path: &Path) -> Result<Option<Seed>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let trimmed = text.trim();
            let bytes = data_encoding::BASE32_NOPAD
                .decode(trimmed.as_bytes())
                .with_context(|| format!("decoding the base32 TOTP seed in {}", path.display()))?;
            Ok(Some(Seed(bytes)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `seed` to `path` owner-only (base32 text), creating the parent dir.
pub fn save_seed_to(seed: &Seed, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, seed.secret_base32())
        .with_context(|| format!("writing {}", path.display()))?;
    auth::set_owner_only(path)?;
    Ok(())
}

/// Mint-once at `path`: return the existing seed with `false`, or generate,
/// save, and return a fresh one with `true`. The `bool` ("was newly minted")
/// lets `daemon setup` show the enrolment URI exactly once.
pub fn ensure_seed_at(path: &Path) -> Result<(Seed, bool)> {
    match load_seed_from(path)? {
        Some(seed) => Ok((seed, false)),
        None => {
            let seed = generate_seed();
            save_seed_to(&seed, path)?;
            Ok((seed, true))
        }
    }
}

/// Load the current TOTP seed from its production path.
pub fn load_seed() -> Result<Option<Seed>> {
    load_seed_from(&seed_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B SHA-1 vector: seed = ASCII `12345678901234567890`,
    /// T = 59s (counter 1) → `287082`. An EXTERNAL oracle — a wrong truncation
    /// cannot reproduce it.
    #[test]
    fn rfc6238_sha1_vector() {
        let seed = Seed::from_bytes(b"12345678901234567890".to_vec());
        assert!(
            seed.verify("287082", 59, 0),
            "the RFC vector code must verify"
        );
        assert!(
            !seed.verify("999999", 59, 0),
            "a wrong code must not verify"
        );
    }

    /// The ±1-step window accepts a code from the neighbouring step (clock skew,
    /// RFC 6238 §5.2). At T=89 (counter 2) the counter-1 code `287082` is
    /// accepted only within the window, not at skew 0.
    #[test]
    fn verify_honors_skew_window() {
        let seed = Seed::from_bytes(b"12345678901234567890".to_vec());
        assert!(
            seed.verify("287082", 89, 1),
            "±1 step accepts the prior code"
        );
        assert!(
            !seed.verify("287082", 89, 0),
            "no skew rejects the prior step's code"
        );
    }

    #[test]
    fn otpauth_uri_carries_secret_and_params() {
        let seed = Seed::from_bytes(b"12345678901234567890".to_vec());
        let uri = seed.otpauth_uri("ralphy", "anvil");
        assert!(uri.starts_with("otpauth://totp/ralphy:anvil?"));
        assert!(uri.contains(&format!("secret={}", seed.secret_base32())));
        assert!(
            uri.contains("algorithm=SHA1") && uri.contains("digits=6") && uri.contains("period=30")
        );
    }

    #[test]
    fn ensure_seed_is_mint_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("daemon-totp");
        let (first, minted) = ensure_seed_at(&path).unwrap();
        assert!(minted, "first call mints");
        let (second, minted_again) = ensure_seed_at(&path).unwrap();
        assert!(!minted_again, "second call does not re-mint");
        assert_eq!(
            first.secret_base32(),
            second.secret_base32(),
            "the same seed is returned"
        );
    }

    #[test]
    fn revoke_seed_at_deletes_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = seed_path_in(dir.path());
        save_seed_to(&generate_seed(), &path).unwrap();
        assert!(path.exists(), "seed written");
        revoke_seed_at(&path).unwrap();
        assert!(!path.exists(), "revoke removes the seed file");
        // Idempotent: revoking an absent seed is Ok, not an error.
        revoke_seed_at(&path).unwrap();
    }

    #[test]
    fn load_seed_from_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_seed_from(&dir.path().join("absent"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn confirm_pending_promotes_only_on_a_valid_code() {
        let dir = tempfile::tempdir().unwrap();
        let pending = pending_seed_path_in(dir.path());
        let live = seed_path_in(dir.path());
        // Enrol into the PENDING slot with the RFC vector seed; live stays empty.
        let seed = Seed::from_bytes(b"12345678901234567890".to_vec());
        save_seed_to(&seed, &pending).unwrap();
        assert!(
            !live.exists(),
            "pending enrolment does not arm the live seed"
        );

        // A wrong code leaves the pending seed in place and never arms live.
        assert!(!confirm_pending_at(&pending, &live, "999999", 59).unwrap());
        assert!(
            pending.exists() && !live.exists(),
            "wrong code changes nothing"
        );

        // The RFC vector code (T=59 → 287082) promotes pending → live.
        assert!(confirm_pending_at(&pending, &live, "287082", 59).unwrap());
        assert!(live.exists(), "a valid code arms the live seed");
        assert!(!pending.exists(), "the pending seed is consumed");
        assert_eq!(
            load_seed_from(&live).unwrap().unwrap().secret_base32(),
            seed.secret_base32(),
            "the confirmed seed is exactly the pending one",
        );
    }

    #[test]
    fn matched_step_returns_the_counter() {
        // RFC vector: T=59 → counter 1.
        let seed = Seed::from_bytes(b"12345678901234567890".to_vec());
        assert_eq!(seed.matched_step("287082", 59, 0), Some(1));
        assert_eq!(seed.matched_step("999999", 59, 0), None);
    }

    #[test]
    fn last_step_store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = last_step_path_in(dir.path());
        assert_eq!(load_last_step_from(&path).unwrap(), None, "absent → None");
        save_last_step_to(42, &path).unwrap();
        assert_eq!(load_last_step_from(&path).unwrap(), Some(42));
    }

    #[test]
    fn confirm_pending_with_no_pending_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let pending = pending_seed_path_in(dir.path());
        let live = seed_path_in(dir.path());
        assert!(
            !confirm_pending_at(&pending, &live, "287082", 59).unwrap(),
            "nothing to confirm when no enrolment is in flight"
        );
        assert!(!live.exists());
    }

    #[cfg(unix)]
    #[test]
    fn saved_seed_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon-totp");
        save_seed_to(&generate_seed(), &path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "seed file must be mode 0600");
    }
}
