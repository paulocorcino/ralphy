//! The device ID (ADR-0074 D1): 16 random bytes the daemon issues in the
//! `ralphy_device` cookie, signed so a client cannot pick another device's
//! ID. The cookie has no session epoch, so the ID outlives a logout. It is a
//! record for the audit log; no access decision reads it (D2).
//!
//! The signing key is the daemon's own `daemon-device-key`, not the access
//! token, so a new token does not give every device a new ID.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::{cookie, owner_only};

/// The device cookie name.
pub const COOKIE_NAME: &str = "ralphy_device";

/// The device cookie's lifetime: 400 days, the longest a browser keeps a
/// cookie (Chrome caps `Max-Age` there).
pub const COOKIE_MAX_AGE_SECS: u64 = 400 * 24 * 3600;

/// One device's ID.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId([u8; 16]);

impl DeviceId {
    /// A new random ID.
    pub fn mint() -> DeviceId {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS CSPRNG must be available to mint a device ID");
        DeviceId(bytes)
    }

    fn parse(hex: &str) -> Option<DeviceId> {
        let bytes = from_hex(hex)?;
        Some(DeviceId(bytes.try_into().ok()?))
    }
}

/// The ID as 32 lowercase hex digits: the form every record writes (D10).
impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_hex(&self.0))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({self})")
    }
}

/// The key that signs device cookies.
pub struct DeviceKey(Vec<u8>);

/// The `daemon-device-key` path inside `dir`.
pub fn key_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-device-key")
}

impl DeviceKey {
    /// Read the key at `path`, or create it owner-only when it is absent.
    pub fn load_or_create(path: &Path) -> Result<DeviceKey> {
        match std::fs::read_to_string(path) {
            Ok(text) => from_hex(text.trim())
                .filter(|b| b.len() == 32)
                .map(DeviceKey)
                .with_context(|| format!("{} is not a 32-byte hex key", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = vec![0u8; 32];
                getrandom::fill(&mut bytes)
                    .expect("the OS CSPRNG must be available to mint a device key");
                if let Some(parent) = path.parent() {
                    owner_only::create_owner_only_dir(parent)?;
                }
                owner_only::write_owner_only(path, to_hex(&bytes).as_bytes())?;
                Ok(DeviceKey(bytes))
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn mac_hex(&self, id: DeviceId) -> String {
        let mut mac =
            Hmac::<Sha1>::new_from_slice(&self.0).expect("HMAC accepts a key of any length");
        mac.update(format!("d1|{id}").as_bytes());
        to_hex(&mac.finalize().into_bytes())
    }

    /// The cookie value for `id`: `1.<id>.<mac>`.
    pub fn sign(&self, id: DeviceId) -> String {
        format!("1.{id}.{}", self.mac_hex(id))
    }

    /// The ID a cookie value carries, or `None` when it is malformed or its MAC
    /// does not match.
    pub fn verify(&self, value: &str) -> Option<DeviceId> {
        let mut parts = value.splitn(3, '.');
        let (Some("1"), Some(hex), Some(mac)) = (parts.next(), parts.next(), parts.next()) else {
            return None;
        };
        let id = DeviceId::parse(hex)?;
        crate::auth::ct_eq(mac.as_bytes(), self.mac_hex(id).as_bytes()).then_some(id)
    }

    /// The device ID of a request's `Cookie:` header, or `None`.
    pub fn id_in_cookie_header(&self, header: Option<&str>) -> Option<DeviceId> {
        self.verify(cookie::named_value(header, COOKIE_NAME)?)
    }
}

/// The `Set-Cookie` value that gives a browser its device cookie. `Secure`
/// follows the same rule as the session cookie.
pub fn set_cookie_value(value: &str, secure: bool) -> String {
    format!(
        "{COOKIE_NAME}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}{}",
        if secure { "; Secure" } else { "" }
    )
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(dir: &Path) -> DeviceKey {
        DeviceKey::load_or_create(&key_path_in(dir)).unwrap()
    }

    #[test]
    fn a_signed_id_verifies_and_a_changed_one_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let key = key(dir.path());
        let id = DeviceId::mint();
        let value = key.sign(id);
        assert_eq!(key.verify(&value), Some(id));

        let other = DeviceId::mint();
        let mac = value.rsplit('.').next().unwrap();
        let forged = format!("1.{other}.{mac}");
        assert_eq!(key.verify(&forged), None, "another ID under this MAC");
        assert_eq!(
            key.verify(&format!("2.{id}.{mac}")),
            None,
            "unknown version"
        );
        assert_eq!(key.verify("1.zz.00"), None, "not hex");
    }

    #[test]
    fn the_key_is_created_owner_only_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = key_path_in(dir.path());
        let id = DeviceId::mint();
        let value = DeviceKey::load_or_create(&path).unwrap().sign(id);
        assert!(owner_only::is_owner_only(&path).unwrap());
        assert_eq!(
            DeviceKey::load_or_create(&path).unwrap().verify(&value),
            Some(id),
            "a second load reads the same key"
        );
    }
}
