//! The Windows side of autostart (ADR-0032, amendment of 2026-11-23): the
//! Startup shortcut through the shell's COM `IShellLinkW`, the known Startup
//! folder, and the registry values it reads or deletes. No `reg.exe` and no
//! script runs. Each function takes its path or key, so a test points it at a
//! folder and a key of its own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Startup, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT,
};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, HKEY_CURRENT_USER, RRF_RT_ANY,
};

use super::Shortcut;

/// COM for the current thread, ended when dropped if this call started it.
struct Com {
    started: bool,
}

impl Com {
    #[expect(unsafe_code, reason = "FFI: CoInitializeEx")]
    fn start() -> Self {
        // SAFETY: no reserved pointer; a thread already in another apartment
        // answers RPC_E_CHANGED_MODE, and then COM is already usable.
        let started = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        Com { started }
    }
}

impl Drop for Com {
    #[expect(unsafe_code, reason = "FFI: CoUninitialize")]
    fn drop(&mut self) {
        if self.started {
            // SAFETY: pairs the successful CoInitializeEx of `start`.
            unsafe { CoUninitialize() };
        }
    }
}

/// The current user's Startup folder, wherever a policy moved it.
#[expect(unsafe_code, reason = "FFI: SHGetKnownFolderPath")]
pub(super) fn startup_dir() -> Result<PathBuf> {
    // SAFETY: a known folder id and no token; the returned string is ours to free.
    let raw: PWSTR = unsafe { SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None) }
        .context("finding the Startup folder")?;
    // SAFETY: `raw` is the NUL-terminated string the call returned.
    let text = unsafe { raw.to_string() };
    // SAFETY: `raw` came from the shell's allocator and is not used after this.
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    Ok(PathBuf::from(
        text.context("reading the Startup folder path")?,
    ))
}

/// Write `shortcut` to its `link` path, replacing a file already there.
#[expect(unsafe_code, reason = "FFI: COM IShellLinkW and IPersistFile")]
pub(super) fn write_shortcut(shortcut: &Shortcut) -> Result<()> {
    let _com = Com::start();
    // SAFETY: COM is started on this thread; every argument is an owned HSTRING.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .context("creating a shell link")?;
        link.SetPath(&HSTRING::from(shortcut.target.as_os_str()))?;
        link.SetArguments(&HSTRING::from(shortcut.arguments.as_str()))?;
        link.SetWorkingDirectory(&HSTRING::from(shortcut.working_dir.as_os_str()))?;
        link.SetDescription(&HSTRING::from(shortcut.description.as_str()))?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(shortcut.link.as_os_str()), true)
            .with_context(|| format!("writing {}", shortcut.link.display()))?;
    }
    Ok(())
}

/// The target and the arguments of the shortcut at `link`.
#[expect(unsafe_code, reason = "FFI: COM IShellLinkW and IPersistFile")]
pub(super) fn read_shortcut(link: &Path) -> Result<(PathBuf, String)> {
    let _com = Com::start();
    let mut target = [0u16; 1024];
    let mut arguments = [0u16; 1024];
    // SAFETY: COM is started on this thread; the buffers outlive the calls.
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .context("creating a shell link")?;
        let file: IPersistFile = shell.cast()?;
        file.Load(&HSTRING::from(link.as_os_str()), STGM_READ)
            .with_context(|| format!("reading {}", link.display()))?;
        shell.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        shell.GetArguments(&mut arguments)?;
    }
    Ok((PathBuf::from(wide_text(&target)), wide_text(&arguments)))
}

fn wide_text(buf: &[u16]) -> String {
    let text: Vec<u16> = buf.iter().copied().take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&text)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The bytes of the value `name` under `HKCU\<subkey>`, or `None` when the key
/// or the value does not exist.
#[expect(unsafe_code, reason = "FFI: RegGetValueW")]
pub(super) fn read_value(subkey: &str, name: &str) -> Result<Option<Vec<u8>>> {
    let (key, value) = (wide(subkey), wide(name));
    let mut size = 0u32;
    // SAFETY: NUL-terminated names; a null buffer asks only for the size.
    let first = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_ANY,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    if first == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    anyhow::ensure!(
        first == ERROR_SUCCESS,
        "reading the registry value {name}: error {first}"
    );
    let mut bytes = vec![0u8; usize::try_from(size).context("a registry value size")?];
    // SAFETY: `bytes` holds `size` bytes, the size the first call reported.
    let second = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_ANY,
            std::ptr::null_mut(),
            bytes.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    anyhow::ensure!(
        second == ERROR_SUCCESS,
        "reading the registry value {name}: error {second}"
    );
    bytes.truncate(usize::try_from(size).context("a registry value size")?);
    Ok(Some(bytes))
}

/// Delete the value `name` under `HKCU\<subkey>`; `false` when it was not there.
#[expect(unsafe_code, reason = "FFI: RegDeleteKeyValueW")]
pub(super) fn delete_value(subkey: &str, name: &str) -> Result<bool> {
    let (key, value) = (wide(subkey), wide(name));
    // SAFETY: NUL-terminated names that live across the call.
    let code = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    anyhow::ensure!(
        code == ERROR_SUCCESS,
        "deleting the registry value {name}: error {code}"
    );
    Ok(true)
}

#[cfg(test)]
mod tests;
