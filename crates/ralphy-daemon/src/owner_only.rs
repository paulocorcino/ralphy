//! Restrict a daemon store file (token, registry, identity, markers) to the
//! current user: mode `0o600` on unix, and on Windows a protected DACL with one
//! ACE for the current user (ADR-0067: Windows OpenSSH also checks the ACL of a
//! key file). Called on every write, including `desk::save_to` on each drag, so
//! Windows uses the Win32 API in-process rather than spawning `icacls`.

use std::path::Path;

use anyhow::{Context, Result};

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
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use anyhow::{bail, Context, Result};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AddAccessAllowedAce, GetLengthSid, GetTokenInformation, InitializeAcl, TokenUser,
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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
mod tests {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        EqualSid, GetAce, GetSecurityDescriptorControl, ACCESS_ALLOWED_ACE, ACL,
        DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;

    use super::win::CurrentUser;

    /// What the file's DACL says, read back from the file system.
    #[derive(Debug, PartialEq)]
    struct Dacl {
        protected: bool,
        ace_count: u16,
        /// The first ACE allows everything to the current user.
        first_is_user_full_access: bool,
    }

    fn read_dacl(path: &Path) -> Dacl {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut dacl: *mut ACL = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `wide` is NUL-terminated; the out pointers are valid. `sd` is
        // freed with LocalFree below, and `dacl` points inside it.
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
        assert_eq!(
            code,
            ERROR_SUCCESS,
            "reading the DACL of {}",
            path.display()
        );

        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: `sd` is the descriptor returned above.
        let ok = unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) };
        assert_ne!(ok, 0, "reading the descriptor control");

        let user = CurrentUser::query().expect("the current user");
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
        // SAFETY: `sd` was allocated by GetNamedSecurityInfoW and is freed once.
        unsafe { LocalFree(sd) };

        Dacl {
            protected: control & SE_DACL_PROTECTED != 0,
            ace_count,
            first_is_user_full_access,
        }
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
