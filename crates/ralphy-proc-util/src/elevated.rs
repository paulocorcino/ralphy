//! Whether this process runs as root (UID 0) or, on Windows, with an elevated
//! token. Ralphy needs neither; `ralphy install` and `ralphy daemon install`
//! only warn, because `host add` runs them over ssh with no terminal (ADR-0056
//! §12).

/// Root is the effective uid 0: `sudo` sets it and keeps the real uid.
#[cfg(unix)]
#[expect(unsafe_code, reason = "FFI: libc::geteuid")]
pub fn runs_elevated() -> bool {
    // SAFETY: `geteuid` takes no arguments, cannot fail, and touches no memory.
    unsafe { libc::geteuid() == 0 }
}

/// An elevated token is what "Run as administrator" gives. A token that cannot
/// be read counts as not elevated: the answer only chooses a warning.
#[cfg(windows)]
#[expect(unsafe_code, reason = "FFI: OpenProcessToken and GetTokenInformation")]
pub fn runs_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `token` is a valid out pointer, checked before use and closed
    // once; `elevation` is a POD of the size passed.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            std::ptr::from_mut(&mut elevation).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        read != 0 && elevation.TokenIsElevated != 0
    }
}
