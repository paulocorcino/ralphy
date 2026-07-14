//! The OPTIONAL browser-login password (issue #179, ADR-0032 §4) — the weakest,
//! opt-in "something you know" layered on top of the TOTP core factor. Hashed
//! with PBKDF2-HMAC-SHA1 (600k iterations, 16-byte random salt), stored as
//! `pbkdf2-sha1$<iter>$<b64 salt>$<b64 hash>` in the global store
//! (`<home>/.ralphy/daemon-password`, mode 0600). No argon2/bcrypt in the tree
//! and the non-goals forbid overengineering; the password is explicitly modest
//! defense-in-depth, not the core factor.
//!
//! Verification compares the derived key constant-time via `auth::ct_eq`.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};

use crate::auth;

/// PBKDF2 iteration count for NEW records: OWASP's PBKDF2-HMAC-SHA1 floor
/// (~1.3M; ADR-0032 amendment §D). Existing hashes carry their own count in the
/// stored `scheme$iter$salt$hash` header and keep verifying under it, so raising
/// this is a forward migration — an older 600k hash is re-hashed at this count on
/// the next `set`, never silently rejected.
const ITERATIONS: u32 = 1_300_000;
/// Derived-key length in bytes (SHA-1 output size).
const DK_LEN: usize = 20;
/// Salt length in bytes.
const SALT_LEN: usize = 16;

/// A stored password verifier: salt + iteration count + derived key. Never holds
/// the plaintext.
#[derive(Clone)]
pub struct Hash {
    salt: [u8; SALT_LEN],
    iterations: u32,
    dk: [u8; DK_LEN],
}

/// Derive the PBKDF2-HMAC-SHA1 key for `pw` under `salt`/`iterations`.
fn derive(pw: &str, salt: &[u8], iterations: u32) -> [u8; DK_LEN] {
    let mut dk = [0u8; DK_LEN];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(pw.as_bytes(), salt, iterations, &mut dk);
    dk
}

impl Hash {
    /// Hash `pw` with a fresh CSPRNG salt at the fixed iteration count.
    pub fn hash_password(pw: &str) -> Hash {
        let mut salt = [0u8; SALT_LEN];
        getrandom::getrandom(&mut salt)
            .expect("the OS CSPRNG must be available to salt a password");
        let dk = derive(pw, &salt, ITERATIONS);
        Hash {
            salt,
            iterations: ITERATIONS,
            dk,
        }
    }

    /// Whether `pw` reproduces the stored derived key (constant-time compare).
    pub fn verify(&self, pw: &str) -> bool {
        let got = derive(pw, &self.salt, self.iterations);
        auth::ct_eq(&got, &self.dk)
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pbkdf2-sha1${}${}${}",
            self.iterations,
            data_encoding::BASE64.encode(&self.salt),
            data_encoding::BASE64.encode(&self.dk),
        )
    }
}

impl FromStr for Hash {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Hash> {
        let mut parts = s.trim().split('$');
        let scheme = parts.next().unwrap_or("");
        if scheme != "pbkdf2-sha1" {
            return Err(anyhow!("unknown password hash scheme: {scheme}"));
        }
        let iterations: u32 = parts
            .next()
            .ok_or_else(|| anyhow!("missing iteration count"))?
            .parse()
            .context("parsing the PBKDF2 iteration count")?;
        let salt_b64 = parts.next().ok_or_else(|| anyhow!("missing salt"))?;
        let dk_b64 = parts.next().ok_or_else(|| anyhow!("missing derived key"))?;
        let salt_vec = data_encoding::BASE64
            .decode(salt_b64.as_bytes())
            .context("decoding the password salt")?;
        let dk_vec = data_encoding::BASE64
            .decode(dk_b64.as_bytes())
            .context("decoding the password derived key")?;
        let salt: [u8; SALT_LEN] = salt_vec
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("password salt is not {SALT_LEN} bytes"))?;
        let dk: [u8; DK_LEN] = dk_vec
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("password derived key is not {DK_LEN} bytes"))?;
        Ok(Hash {
            salt,
            iterations,
            dk,
        })
    }
}

/// The `daemon-password` path inside `dir`. Path-explicit so the security
/// routes/tests point it at a temp dir without touching the env.
pub fn password_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-password")
}

/// The production path of `daemon-password`. Mirrors [`auth::token_path`].
pub fn password_path() -> Result<PathBuf> {
    Ok(password_path_in(&auth::store_dir()?))
}

/// Remove the password file at `path` (clear the optional factor). `Ok(())` when
/// already absent — clearing is idempotent.
pub fn clear_at(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Load a password hash from `path`, or `Ok(None)` when unset (no password
/// enrolled — the common case, password being opt-in).
pub fn load_from(path: &Path) -> Result<Option<Hash>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.parse()?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `hash` to `path` owner-only, creating the parent dir.
pub fn save_to(hash: &Hash, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, hash.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    auth::set_owner_only(path)?;
    Ok(())
}

/// Load the current password hash from its production path.
pub fn load() -> Result<Option<Hash>> {
    load_from(&password_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6070 PBKDF2-HMAC-SHA1 vector: P=`password`, S=`salt`, c=1, dkLen=20 →
    /// `0c60c80f961f0e71f3a9b524af6012062fe037a6`. An EXTERNAL oracle.
    #[test]
    fn rfc6070_pbkdf2_vector() {
        let dk = derive("password", b"salt", 1);
        let mut hex = String::new();
        for b in dk {
            hex.push_str(&format!("{b:02x}"));
        }
        assert_eq!(hex, "0c60c80f961f0e71f3a9b524af6012062fe037a6");
    }

    #[test]
    fn hash_verifies_correct_password_only() {
        let h = Hash::hash_password("hunter2");
        assert!(h.verify("hunter2"), "the right password verifies");
        assert!(!h.verify("wrong"), "a wrong password does not");
    }

    #[test]
    fn hash_round_trips_through_string() {
        let h = Hash::hash_password("hunter2");
        let s = h.to_string();
        assert!(s.starts_with("pbkdf2-sha1$1300000$"));
        let parsed: Hash = s.parse().unwrap();
        assert!(parsed.verify("hunter2"), "a parsed hash still verifies");
        assert!(!parsed.verify("wrong"));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("daemon-password");
        let h = Hash::hash_password("s3cret");
        save_to(&h, &path).unwrap();
        let loaded = load_from(&path).unwrap().expect("just saved");
        assert!(loaded.verify("s3cret"));
    }

    #[test]
    fn clear_at_deletes_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = password_path_in(dir.path());
        save_to(&Hash::hash_password("pw"), &path).unwrap();
        assert!(path.exists(), "password written");
        clear_at(&path).unwrap();
        assert!(!path.exists(), "clear removes the password file");
        // Idempotent: clearing an absent password is Ok.
        clear_at(&path).unwrap();
        assert!(load_from(&path).unwrap().is_none(), "cleared → unset");
    }

    #[test]
    fn an_older_lower_iteration_hash_still_verifies() {
        // A hash stored before the ADR-0032 §D bump carries its own count; it must
        // keep verifying (forward migration, not a silent lockout).
        let salt = [7u8; SALT_LEN];
        let legacy = Hash {
            salt,
            iterations: 600_000,
            dk: derive("hunter2", &salt, 600_000),
        };
        let round_tripped: Hash = legacy.to_string().parse().unwrap();
        assert!(round_tripped.verify("hunter2"), "legacy 600k hash verifies");
        assert!(!round_tripped.verify("wrong"));
    }

    #[test]
    fn load_from_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(&dir.path().join("absent")).unwrap().is_none());
    }
}
