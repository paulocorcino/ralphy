//! The clipboard drop writer (ADR-0055): a pasted raster image becomes a file
//! under `.ralphy/clipboard/` with a name the DAEMON chooses, and the path is
//! what the console pastes into the agent's prompt.
//!
//! This is its own module rather than a `fswrite` growth because it is a
//! distinct responsibility — a daemon-named write under a directory the verb
//! fixes — that needs only the public [`confine::confine_write`] kernel and the
//! shared [`WriteError`]. The client names nothing: no path, no filename, no
//! media type. It writes under `.ralphy/` — a directory `fswrite`'s
//! `PROTECTED_DIRS` denies to every CLIENT-named path — the way `plan.discard`
//! does: the verb fixes the target, so the denylist stays closed and needs no
//! hole.
//!
//! Validation ([`decode_image`]) is separate from the write ([`write_image`]) so
//! the write is only ever handed bytes already known to be an allowlisted
//! image, and so the handler can refuse before spending a blocking task.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::confine;
use crate::fswrite::{map_confine, WriteError};
use crate::tree::{ImageType, MAX_IMAGE_BYTES};

/// The run-state directory the drops live under — gitignored by `ralphy` on its
/// first run, filtered out of the Change set by definition, so a screenshot can
/// never be committed by accident (ADR-0055 §3). Created here when a repo no run
/// has touched yet lacks it.
pub const PARENT: &str = ".ralphy";

/// The landing directory. Inside `.ralphy/` on purpose: the operator chose
/// "never in the tree, never committed" over "every vendor can read it" — Gemini
/// refuses gitignored reads (#275), and that limitation is accepted and recorded
/// in the ADR rather than worked around.
pub const DIR: &str = ".ralphy/clipboard";

/// How many same-stamp collisions to step past before giving up. Two pastes in
/// one millisecond is already unusual; a hundred is a bug, not a burst.
const MAX_SUFFIX: u32 = 100;

/// Decode and verify a paste payload: the base64 text the browser sent becomes
/// `(verified type, bytes)` or a wire-literal refusal. The refusal reasons are
/// exactly the read direction's (`not an image`, `too large`); the vocabulary
/// does not grow.
///
/// The ENCODED length is bounded first (`4 × cap / 3`, plus padding), so an
/// oversized payload is refused without ever allocating its decode.
pub fn decode_image(base64: Option<&str>) -> Result<(ImageType, Vec<u8>), &'static str> {
    let text = base64.ok_or("not an image")?;
    let max_encoded = (MAX_IMAGE_BYTES as usize).div_ceil(3) * 4;
    if text.len() > max_encoded {
        return Err("too large");
    }
    let bytes = data_encoding::BASE64
        .decode(text.as_bytes())
        .map_err(|_| "not an image")?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err("too large");
    }
    let kind = ImageType::detect_raster(&bytes).ok_or("not an image")?;
    Ok((kind, bytes))
}

/// The wire literal for a write failure — the same mapping the generic Write
/// verbs answer with, so a drop refuses in the vocabulary the console knows.
pub fn write_reason(e: WriteError) -> &'static str {
    match e {
        WriteError::Confined => "refused",
        WriteError::Conflict => "exists",
        WriteError::NotFound => "not found",
        WriteError::Io => "io error",
    }
}

/// Write `bytes` — already verified to be a `kind` image — as a new file under
/// [`DIR`], creating the directory on first use, and return the repo-relative
/// path with forward slashes on every host.
///
/// The file is opened with `create_new`: a drop never overwrites. A name that
/// already exists (two pastes in the same millisecond) steps to `-2`, `-3`, …
/// Both the directory and the file go through [`confine::confine_write`], which
/// refuses a symlink sitting where either should be.
pub fn write_image(root: &Path, kind: ImageType, bytes: &[u8]) -> Result<String, WriteError> {
    // `.ralphy/` may not exist yet (a repo no run has touched), so each level is
    // confined and created on its own — never `create_dir_all`, which would walk
    // through a symlink it did not check.
    ensure_dir(root, PARENT)?;
    ensure_dir(root, DIR)?;

    let stamp = utc_stamp(SystemTime::now());
    let ext = kind.extension();
    for n in 1..=MAX_SUFFIX {
        let name = if n == 1 {
            format!("paste-{stamp}.{ext}")
        } else {
            format!("paste-{stamp}-{n}.{ext}")
        };
        let rel = format!("{DIR}/{name}");
        let path = confine::confine_write(root, &rel).map_err(map_confine)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|_| WriteError::Io)?;
                return Ok(rel);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(WriteError::Io),
        }
    }
    Err(WriteError::Conflict)
}

/// Create the confined directory `rel` under `root` if it is missing. A file or
/// a symlink squatting on the name is refused, never replaced.
fn ensure_dir(root: &Path, rel: &str) -> Result<(), WriteError> {
    let dir = confine::confine_write(root, rel).map_err(map_confine)?;
    match std::fs::create_dir(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            if dir.is_dir() {
                Ok(())
            } else {
                Err(WriteError::Conflict)
            }
        }
        Err(_) => Err(WriteError::Io),
    }
}

/// `yyyymmdd-hhmmss-mmm` in UTC, from the system clock, with no date crate:
/// the civil-from-days conversion is Howard Hinnant's, valid for every date
/// this daemon will ever stamp. A clock before the epoch stamps the epoch.
fn utc_stamp(now: SystemTime) -> String {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let millis = since.subsec_millis();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };

    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}-{millis:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

    fn b64(bytes: &[u8]) -> String {
        data_encoding::BASE64.encode(bytes)
    }

    #[test]
    fn utc_stamp_is_the_civil_date_and_time() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101-000000-000");
        // 2001-09-09T01:46:40Z, the billionth second.
        let t = UNIX_EPOCH + Duration::from_millis(1_000_000_000_042);
        assert_eq!(utc_stamp(t), "20010909-014640-042");
        // A leap day, to exercise the era arithmetic.
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc_stamp(t), "20000229-000000-000");
    }

    #[test]
    fn decode_image_accepts_a_raster_and_names_its_type() {
        let (kind, bytes) = decode_image(Some(&b64(PNG))).unwrap();
        assert_eq!(kind, ImageType::Png);
        assert_eq!(bytes, PNG);
    }

    #[test]
    fn decode_image_refuses_what_is_not_an_allowlisted_image() {
        assert_eq!(decode_image(None), Err("not an image"));
        assert_eq!(decode_image(Some("!!!not base64!!!")), Err("not an image"));
        assert_eq!(decode_image(Some("")), Err("not an image"));
        assert_eq!(
            decode_image(Some(&b64(b"<html><script>x</script>"))),
            Err("not an image")
        );
        // SVG is on the READ allowlist and deliberately not on this one.
        assert_eq!(
            decode_image(Some(&b64(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"))),
            Err("not an image")
        );
    }

    #[test]
    fn decode_image_refuses_an_oversized_payload_before_decoding_it() {
        // One byte over the cap, as the browser would encode it.
        let mut big = PNG.to_vec();
        big.resize((MAX_IMAGE_BYTES + 1) as usize, 0);
        assert_eq!(decode_image(Some(&b64(&big))), Err("too large"));
        // Exactly at the cap is accepted — the bound is the read cap, not under it.
        big.truncate(MAX_IMAGE_BYTES as usize);
        assert!(decode_image(Some(&b64(&big))).is_ok());
    }

    #[test]
    fn write_image_creates_the_dir_and_lands_identical_bytes() {
        let root = tempfile::tempdir().unwrap();
        assert!(!root.path().join(DIR).exists());
        let rel = write_image(root.path(), ImageType::Png, PNG).unwrap();
        assert!(rel.starts_with(".ralphy/clipboard/paste-"), "{rel}");
        assert!(rel.ends_with(".png"), "{rel}");
        assert!(!rel.contains('\\'), "forward slashes on every host: {rel}");
        assert_eq!(fs::read(root.path().join(&rel)).unwrap(), PNG);
    }

    #[test]
    fn write_image_never_overwrites_a_same_stamp_drop() {
        let root = tempfile::tempdir().unwrap();
        // Two drops back to back: even inside one millisecond both survive.
        let a = write_image(root.path(), ImageType::Jpeg, b"\xff\xd8\xff\xe0a").unwrap();
        let b = write_image(root.path(), ImageType::Jpeg, b"\xff\xd8\xff\xe0b").unwrap();
        assert_ne!(a, b);
        assert_eq!(
            fs::read(root.path().join(&a)).unwrap(),
            b"\xff\xd8\xff\xe0a"
        );
        assert_eq!(
            fs::read(root.path().join(&b)).unwrap(),
            b"\xff\xd8\xff\xe0b"
        );
        assert!(a.ends_with(".jpg") && b.ends_with(".jpg"));
    }

    #[test]
    fn write_image_lands_inside_an_existing_ralphy_dir_and_leaves_its_siblings_alone() {
        // The usual case: a repo a run has touched already has `.ralphy/`.
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(PARENT)).unwrap();
        fs::write(root.path().join(PARENT).join("plan.md"), b"# plan").unwrap();
        let rel = write_image(root.path(), ImageType::Png, PNG).unwrap();
        assert!(rel.starts_with(".ralphy/clipboard/paste-"), "{rel}");
        assert_eq!(
            fs::read(root.path().join(PARENT).join("plan.md")).unwrap(),
            b"# plan"
        );
    }

    #[test]
    fn write_image_refuses_a_file_squatting_on_the_dir() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(PARENT)).unwrap();
        fs::write(root.path().join(DIR), b"not a dir").unwrap();
        assert_eq!(
            write_image(root.path(), ImageType::Png, PNG),
            Err(WriteError::Conflict)
        );
        assert_eq!(fs::read(root.path().join(DIR)).unwrap(), b"not a dir");
    }

    #[test]
    fn write_image_refuses_a_file_squatting_on_ralphy_itself() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(PARENT), b"not a dir").unwrap();
        assert_eq!(
            write_image(root.path(), ImageType::Png, PNG),
            Err(WriteError::Conflict)
        );
        assert_eq!(fs::read(root.path().join(PARENT)).unwrap(), b"not a dir");
    }

    #[cfg(unix)]
    #[test]
    fn write_image_refuses_a_symlink_at_either_level() {
        use std::os::unix::fs::symlink;
        for level in [PARENT, DIR] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            if level == DIR {
                fs::create_dir(root.path().join(PARENT)).unwrap();
            }
            symlink(outside.path(), root.path().join(level)).unwrap();
            assert_eq!(
                write_image(root.path(), ImageType::Png, PNG),
                Err(WriteError::Confined),
                "{level}"
            );
            assert!(
                fs::read_dir(outside.path()).unwrap().next().is_none(),
                "nothing written through the link at {level}"
            );
        }
    }
}
