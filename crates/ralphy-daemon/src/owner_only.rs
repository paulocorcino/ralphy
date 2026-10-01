//! Restrict a store file (token, registry, identity, markers, and the CLI's
//! events and Telegram stores) to the current user: mode `0o600` on unix, and
//! on Windows a protected DACL with one ACE for the current user (ADR-0067:
//! Windows OpenSSH also checks the ACL of a key file). Called on every write,
//! including `desk::save_to` on each drag, so Windows uses the Win32 API
//! in-process rather than spawning `icacls`.
//!
//! Public because the CLI's secret stores share this one implementation
//! (ADR-0072 D7).

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Write `bytes` to `path` so the file is owner-only from the moment it
/// exists. The bytes go to a sibling temporary file that is created owner-only
/// (mode `0o600` at creation on unix; on Windows the DACL is set while the file
/// is still empty), then the file is synced and renamed over `path`.
/// Invariant: no temporary file is left behind on any return path.
pub fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    write_owner_only_probed(path, bytes, |_| {})
}

/// [`write_owner_only`], with `after_create` called on the temporary file
/// right after it is created, before any byte is written: the test hook that
/// reads its mode at that instant.
fn write_owner_only_probed(
    path: &Path,
    bytes: &[u8],
    after_create: impl FnOnce(&Path),
) -> Result<()> {
    let name = path
        .file_name()
        .with_context(|| format!("{} names no file", path.display()))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let file = match create_owner_only_file(&tmp) {
        // A temporary file of a dead process with the same pid: replace it.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(&tmp)
                .with_context(|| format!("removing the stale {}", tmp.display()))?;
            create_owner_only_file(&tmp)
        }
        other => other,
    }
    .with_context(|| format!("creating {}", tmp.display()))?;
    let written = (|| -> Result<()> {
        let mut file = file;
        protect_new_file(&tmp)?;
        after_create(&tmp);
        file.write_all(bytes)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", tmp.display()))?;
        drop(file);
        std::fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
    })();
    if let Err(e) = written {
        if let Err(rm) = std::fs::remove_file(&tmp) {
            if rm.kind() != std::io::ErrorKind::NotFound {
                return Err(e.context(format!("also removing {}: {rm}", tmp.display())));
            }
        }
        return Err(e);
    }
    Ok(())
}

/// Create `path` new, readable and writable by the owner only.
#[cfg(unix)]
fn create_owner_only_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Create `path` new; [`protect_new_file`] sets its DACL before any write.
#[cfg(windows)]
fn create_owner_only_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// On unix the mode was set at creation; nothing is left to do.
#[cfg(unix)]
fn protect_new_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn protect_new_file(path: &Path) -> Result<()> {
    set_owner_only(path)
}

/// Create the store directory `path` (and its parents). On unix it gets mode
/// `0o700`; on Windows it keeps the per-user profile's ACL, and each secret
/// file carries its own DACL.
pub fn create_owner_only_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("setting owner-only permissions on {}", path.display()))?;
    }
    Ok(())
}

/// Whether `path` is owner-only: mode `0o600` on unix; on Windows a protected
/// DACL whose one ACE gives the current user full access.
#[cfg(unix)]
pub fn is_owner_only(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(meta.permissions().mode() & 0o777 == 0o600)
}

/// Whether `path` is owner-only: mode `0o600` on unix; on Windows a protected
/// DACL whose one ACE gives the current user full access.
#[cfg(windows)]
pub fn is_owner_only(path: &Path) -> Result<bool> {
    let dacl = win::read_dacl(path)?;
    Ok(dacl.protected && dacl.ace_count == 1 && dacl.first_is_user_full_access)
}

/// Restrict `path` to the current user, replacing whatever it inherited.
#[cfg(unix)]
pub(crate) fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting owner-only permissions on {}", path.display()))
}

/// Restrict `path` to the current user, replacing whatever it inherited.
#[cfg(windows)]
pub(crate) fn set_owner_only(path: &Path) -> Result<()> {
    win::protect(path)
        .with_context(|| format!("setting owner-only permissions on {}", path.display()))
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "FFI: the Win32 security API that reads and writes a file's DACL"
)]
pub(crate) mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use anyhow::{bail, Context, Result};
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        AddAccessAllowedAce, EqualSid, GetAce, GetLengthSid, GetSecurityDescriptorControl,
        GetTokenInformation, InitializeAcl, TokenUser, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION,
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// What a file's DACL says, read back from the file system.
    #[derive(Debug, PartialEq)]
    pub(crate) struct Dacl {
        pub(crate) protected: bool,
        pub(crate) ace_count: u16,
        /// The first ACE allows everything to the current user.
        pub(crate) first_is_user_full_access: bool,
    }

    /// Frees the security descriptor GetNamedSecurityInfoW allocated, on every
    /// return path.
    struct Descriptor(PSECURITY_DESCRIPTOR);

    impl Drop for Descriptor {
        fn drop(&mut self) {
            // SAFETY: the descriptor came from GetNamedSecurityInfoW and is
            // freed once. A failed free leaks it; there is nothing to report to.
            unsafe { LocalFree(self.0) };
        }
    }

    pub(crate) fn read_dacl(path: &Path) -> Result<Dacl> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut dacl: *mut ACL = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `wide` is NUL-terminated; the out pointers are valid. `sd` is
        // freed by `Descriptor`, and `dacl` points inside it.
        let code = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut sd,
            )
        };
        if code != ERROR_SUCCESS {
            let err = io::Error::from_raw_os_error(code as i32);
            bail!("reading the access list of {}: {err}", path.display());
        }
        let sd = Descriptor(sd);

        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: `sd.0` is the descriptor returned above.
        if unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error()).context("reading the descriptor control");
        }

        let user = CurrentUser::query()?;
        let (ace_count, first_is_user_full_access) = if dacl.is_null() {
            (0, false)
        } else {
            // SAFETY: `dacl` is non-null and points inside `sd`.
            let count = unsafe { (*dacl).AceCount };
            let mut ace: *mut core::ffi::c_void = null_mut();
            // SAFETY: index 0 is checked against `count`; `ace` is a valid out pointer.
            let first = count > 0 && unsafe { GetAce(dacl, 0, &mut ace) } != 0 && {
                let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
                // SAFETY: GetAce returned a pointer to ACE 0 inside the DACL; an
                // ACCESS_ALLOWED ACE holds its SID from `SidStart` on.
                unsafe {
                    u32::from((*ace).Header.AceType) == ACCESS_ALLOWED_ACE_TYPE
                        && (*ace).Mask == FILE_ALL_ACCESS
                        && EqualSid((&raw const (*ace).SidStart).cast_mut().cast(), user.sid()) != 0
                }
            };
            (count, first)
        };
        Ok(Dacl {
            protected: control & SE_DACL_PROTECTED != 0,
            ace_count,
            first_is_user_full_access,
        })
    }

    /// The `TOKEN_USER` of this process, in a buffer that owns the SID it points
    /// to. `u64` elements keep the buffer aligned for the pointer inside.
    pub(super) struct CurrentUser(Vec<u64>);

    impl CurrentUser {
        pub(super) fn query() -> Result<CurrentUser> {
            let mut raw: HANDLE = null_mut();
            // SAFETY: the pseudo-handle of GetCurrentProcess needs no closing, and
            // `raw` is a valid out pointer for the token handle.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
                return Err(io::Error::last_os_error()).context("opening the process token");
            }
            let token = Token(raw);

            let mut len = 0u32;
            // SAFETY: a null buffer of length 0 only asks for the size into `len`.
            let probed =
                unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut len) };
            // The size probe fails by design (ERROR_INSUFFICIENT_BUFFER).
            if probed != 0 || len == 0 {
                return Err(io::Error::last_os_error()).context("sizing the token user");
            }
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: `buf` holds at least `len` writable bytes.
            let filled = unsafe {
                GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
            };
            if filled == 0 {
                return Err(io::Error::last_os_error()).context("reading the token user");
            }
            Ok(CurrentUser(buf))
        }

        /// The user's SID; valid while `self` lives.
        pub(super) fn sid(&self) -> PSID {
            // SAFETY: GetTokenInformation(TokenUser) filled the aligned buffer
            // with a TOKEN_USER whose SID points inside the same buffer.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }

    /// Closes the token handle on every return path.
    struct Token(HANDLE);

    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: the handle came from OpenProcessToken and is closed once.
            // A failed close leaks one handle; there is nothing to report it to.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) fn protect(path: &Path) -> Result<()> {
        let user = CurrentUser::query()?;
        let sid = user.sid();
        // SAFETY: `sid` is a valid SID owned by `user`.
        let sid_len = unsafe { GetLengthSid(sid) } as usize;
        // The ACE's `SidStart` field is the first 4 bytes of the SID.
        let acl_len =
            size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_len;
        // `u32` elements keep the ACL DWORD-aligned, as InitializeAcl requires.
        let mut acl_buf = vec![0u32; acl_len.div_ceil(4)];
        let acl = acl_buf.as_mut_ptr().cast::<ACL>();
        let acl_size = u32::try_from(acl_buf.len() * 4).context("sizing the access list")?;
        // SAFETY: `acl` points to `acl_size` writable, DWORD-aligned bytes.
        if unsafe { InitializeAcl(acl, acl_size, ACL_REVISION) } == 0 {
            return Err(io::Error::last_os_error()).context("creating the access list");
        }
        // SAFETY: `acl` was initialized above with room for this one ACE, and
        // `sid` stays valid while `user` lives.
        if unsafe { AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid) } == 0 {
            return Err(io::Error::last_os_error()).context("adding the owner to the access list");
        }

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and `acl` is a valid ACL; both outlive
        // the call. Owner, group and SACL are left unchanged (null).
        let code = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null(),
            )
        };
        if code != ERROR_SUCCESS {
            let err = io::Error::from_raw_os_error(code as i32);
            bail!("writing the file's access list: {err}");
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
pub(crate) mod tests {
    use std::path::Path;

    pub(crate) use super::win::Dacl;

    pub(crate) fn read_dacl(path: &Path) -> Dacl {
        super::win::read_dacl(path).expect("reading the DACL")
    }

    /// The written file carries the owner-only DACL, a rewrite keeps it, and
    /// no temporary file is left beside it.
    #[test]
    fn write_owner_only_is_protected() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        super::create_owner_only_dir(&store).unwrap();
        let path = store.join("secret.toml");
        let mut seen = None;
        super::write_owner_only_probed(&path, b"one", |tmp| seen = Some(read_dacl(tmp))).unwrap();
        let owner_only = Dacl {
            protected: true,
            ace_count: 1,
            first_is_user_full_access: true,
        };
        assert_eq!(seen, Some(owner_only), "the empty temporary file");
        super::write_owner_only(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(super::is_owner_only(&path).unwrap());
        let plain = store.join("plain.txt");
        std::fs::write(&plain, "x").unwrap();
        assert!(!super::is_owner_only(&plain).unwrap());
        let names: Vec<String> = std::fs::read_dir(&store)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.contains(".tmp-")), "{names:?}");
    }

    #[test]
    fn daemon_files_are_owner_only_with_inheritance_removed() {
        let dir = tempfile::tempdir().unwrap();

        let control = dir.path().join("plain.txt");
        std::fs::write(&control, "x").unwrap();
        assert!(
            !read_dacl(&control).protected,
            "a plain write inherits its folder's ACL, so the reader can tell the two apart"
        );

        let token = dir.path().join("daemon-token");
        crate::auth::save_token_to("tok", &token).unwrap();
        let repos = dir.path().join("repos.toml");
        crate::registry::save_to(&crate::registry::RegistryStore::default(), &repos).unwrap();
        let identity = dir.path().join("daemon.toml");
        crate::identity::save_to(
            &crate::identity::Identity {
                id: ulid::Ulid::nil(),
                name: "anvil".into(),
                avatar: "🐙".into(),
            },
            &identity,
        )
        .unwrap();

        for file in [&token, &repos, &identity] {
            assert_eq!(
                read_dacl(file),
                Dacl {
                    protected: true,
                    ace_count: 1,
                    first_is_user_full_access: true,
                },
                "{} must grant the current user only, with inheritance removed",
                file.display()
            );
        }
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The secret file never exists with a wider mode than `0o600`: the
    /// temporary file has that mode the instant it is created, before a byte
    /// is written. The final file keeps it, the store directory is `0o700`,
    /// and no temporary file is left behind.
    #[test]
    fn write_owner_only_creates_the_file_0600() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        super::create_owner_only_dir(&store).unwrap();
        assert_eq!(mode(&store), 0o700);
        let path = store.join("secret.toml");
        let mut at_creation = None;
        super::write_owner_only_probed(&path, b"one", |tmp| at_creation = Some(mode(tmp))).unwrap();
        assert_eq!(at_creation, Some(0o600), "the temporary file at creation");
        assert_eq!(mode(&path), 0o600);
        assert!(super::is_owner_only(&path).unwrap());
        super::write_owner_only(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let names: Vec<String> = std::fs::read_dir(&store)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["secret.toml"]);
    }
}
