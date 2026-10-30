//! FFI: the per-user `Path` under `HKEY_CURRENT_USER\Environment`, and the
//! broadcast that tells Explorer to reload it (ADR-0072 D13).

use anyhow::{bail, Result};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Key(HKEY);

impl Drop for Key {
    #[expect(unsafe_code, reason = "FFI: RegCloseKey")]
    fn drop(&mut self) {
        // SAFETY: `self.0` is a key that `RegOpenKeyExW` opened, closed once here.
        // A failed close leaves nothing to undo, so its result is not used.
        unsafe { RegCloseKey(self.0) };
    }
}

/// Put `dir` first in the user `Path`. Returns `false` when it is already
/// there. The value keeps its type, so `%VAR%` entries stay unexpanded.
pub(in crate::install) fn add(dir: &str) -> Result<bool> {
    change(|current| super::path_with(current, dir))
}

/// Open the key, read the user `Path`, and write what `next` makes of it.
/// `next` returns `None` when nothing changes. Returns whether it wrote.
#[expect(unsafe_code, reason = "FFI: the registry calls for the user Path")]
pub(in crate::install) fn change(next: impl FnOnce(&str) -> Option<String>) -> Result<bool> {
    let subkey = wide("Environment");
    let name = wide("Path");
    let mut raw: HKEY = std::ptr::null_mut();
    // SAFETY: `subkey` is NUL-terminated and outlives the call; `raw` is a
    // valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut raw,
        )
    };
    if status != ERROR_SUCCESS {
        bail!("opening HKEY_CURRENT_USER\\Environment failed (error {status})");
    }
    let key = Key(raw);
    let (kind, current) = read(&key, &name)?;
    let Some(next) = next(&current) else {
        return Ok(false);
    };
    write(&key, &name, kind, &next)?;
    broadcast();
    Ok(true)
}

/// The type and the text of the user `Path`. A missing value is an empty
/// `REG_EXPAND_SZ`, the type Windows itself gives it.
#[expect(unsafe_code, reason = "FFI: RegQueryValueExW")]
fn read(key: &Key, name: &[u16]) -> Result<(REG_VALUE_TYPE, String)> {
    let mut kind: REG_VALUE_TYPE = 0;
    let mut bytes: u32 = 0;
    // SAFETY: a null data pointer asks only for the type and the size.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND => return Ok((REG_EXPAND_SZ, String::new())),
        ERROR_SUCCESS => {}
        other => bail!("reading the user Path failed (error {other})"),
    }
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        bail!("the user Path is not a text value (type {kind})");
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf` holds `size` bytes and outlives the call.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        bail!("reading the user Path failed (error {status})");
    }
    buf.truncate((size as usize) / 2);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    match String::from_utf16(&buf) {
        Ok(s) => Ok((kind, s)),
        Err(_) => bail!("the user Path is not valid text, so it is left unchanged"),
    }
}

#[expect(unsafe_code, reason = "FFI: RegSetValueExW")]
fn write(key: &Key, name: &[u16], kind: REG_VALUE_TYPE, value: &str) -> Result<()> {
    let data = wide(value);
    // SAFETY: `data` is NUL-terminated UTF-16 of `data.len() * 2` bytes.
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            kind,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    if status != ERROR_SUCCESS {
        bail!("writing the user Path failed (error {status})");
    }
    Ok(())
}

#[expect(unsafe_code, reason = "FFI: SendMessageTimeoutW")]
fn broadcast() {
    let area = wide("Environment");
    let mut result: usize = 0;
    // SAFETY: `area` is NUL-terminated and outlives the call, which returns
    // after at most 5 s. Without the broadcast, only terminals started after
    // the next sign-in see the change, so a failure here is not an error.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            area.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        )
    };
}
