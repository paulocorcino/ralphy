//! Put the install folder on the user's own `PATH`, for terminals opened after
//! the install. Nothing here needs root or an administrator: on Unix it appends
//! a guarded block to the shell start-up files in the home directory, and on
//! Windows it edits the per-user `Path` under `HKEY_CURRENT_USER\Environment`.
//!
//! A running shell cannot be changed from a child process, so the caller still
//! tells the operator to open a new terminal.

use std::path::Path;
#[cfg(any(unix, test))]
use std::path::PathBuf;

#[cfg(any(unix, test))]
use anyhow::Context;
use anyhow::{bail, Result};

/// What [`add_to_user_path`] changed.
pub(super) struct Added {
    /// Where the folder was added: start-up files, or the user `Path` in the
    /// registry. Empty when every place already had it.
    pub(super) places: Vec<String>,
    /// A command that loads the change into the shell that ran the install.
    pub(super) reload: Option<String>,
}

#[cfg(unix)]
pub(super) fn add_to_user_path(dir: &Path) -> Result<Added> {
    let home = directories::BaseDirs::new()
        .context("locating the home directory")?
        .home_dir()
        .to_path_buf();
    let shell = std::env::var("SHELL").ok();
    let zdotdir = std::env::var_os("ZDOTDIR").map(PathBuf::from);
    let config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    add_to_profiles(
        dir,
        &ShellEnv {
            home: &home,
            shell: shell.as_deref(),
            zdotdir: zdotdir.as_deref(),
            config_home: config_home.as_deref(),
        },
    )
}

#[cfg(windows)]
pub(super) fn add_to_user_path(dir: &Path) -> Result<Added> {
    let Some(dir) = dir.to_str() else {
        bail!("the folder name is not valid Unicode");
    };
    let mut places = Vec::new();
    if registry::add(dir)? {
        places.push("your user Path (HKEY_CURRENT_USER\\Environment)".to_string());
    }
    Ok(Added {
        places,
        reload: None,
    })
}

/// Which shell reads a start-up file.
#[cfg(any(unix, test))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shell {
    Sh,
    Bash,
    Zsh,
    Fish,
}

#[cfg(any(unix, test))]
impl Shell {
    /// The shell named by `$SHELL` (`/bin/zsh` → `Zsh`); anything else is `Sh`.
    fn from_env(shell: Option<&str>) -> Shell {
        let name = shell
            .and_then(|s| Path::new(s).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        match name {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            _ => Shell::Sh,
        }
    }
}

/// The parts of the environment that decide which start-up files a shell reads.
#[cfg(any(unix, test))]
struct ShellEnv<'a> {
    home: &'a Path,
    shell: Option<&'a str>,
    zdotdir: Option<&'a Path>,
    config_home: Option<&'a Path>,
}

/// The start-up files to write, and the shell each one is for.
///
/// `~/.profile` is always written: `sh`, `dash`, and a login `bash` without a
/// `~/.bash_profile` read it. The other shells get their own file when they are
/// the user's shell or when their configuration already exists, so a user who
/// switches shells later is still covered. `~/.zshenv` is the file every `zsh`
/// reads, login or not; on macOS `/etc/zprofile` runs `path_helper` after it,
/// which keeps entries it did not add.
#[cfg(any(unix, test))]
fn profile_targets(env: &ShellEnv<'_>, exists: &dyn Fn(&Path) -> bool) -> Vec<(PathBuf, Shell)> {
    let shell = Shell::from_env(env.shell);
    let home = env.home;
    let mut targets = vec![(home.join(".profile"), Shell::Sh)];

    // A `~/.bash_profile` stops a login bash from reading `~/.profile`.
    let bash_profile = home.join(".bash_profile");
    if exists(&bash_profile) {
        targets.push((bash_profile, Shell::Bash));
    }
    let bashrc = home.join(".bashrc");
    if shell == Shell::Bash || exists(&bashrc) {
        targets.push((bashrc, Shell::Bash));
    }

    let zdir = env.zdotdir.unwrap_or(home);
    let zshenv = zdir.join(".zshenv");
    if shell == Shell::Zsh || exists(&zshenv) || exists(&zdir.join(".zshrc")) {
        targets.push((zshenv, Shell::Zsh));
    }

    let config = env
        .config_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"));
    let fish = config.join("fish");
    if shell == Shell::Fish || exists(&fish) {
        targets.push((fish.join("conf.d").join("ralphy.fish"), Shell::Fish));
    }
    targets
}

#[cfg(any(unix, test))]
const MARKER: &str = "# Added by `ralphy install`: puts ralphy on PATH.";

/// The block for `sh`, `bash` and `zsh`. It checks `PATH` first, so a file
/// read twice (or two files read by one shell) adds the folder once.
#[cfg(any(unix, test))]
fn posix_block(dir: &str) -> String {
    let q = format!("'{}'", dir.replace('\'', r"'\''"));
    format!(
        "{MARKER}\ncase \":${{PATH}}:\" in\n    *:{q}:*) ;;\n    *) export PATH={q}:\"$PATH\" ;;\nesac\n"
    )
}

/// The block for `fish`, which does not read POSIX syntax.
#[cfg(any(unix, test))]
fn fish_block(dir: &str) -> String {
    let q = format!("'{}'", dir.replace('\\', r"\\").replace('\'', r"\'"));
    format!("{MARKER}\nif not contains -- {q} $PATH\n    set -gx PATH {q} $PATH\nend\n")
}

/// Append `block` to `file` unless the file already has it. Returns whether it
/// wrote. A missing file, and a missing parent folder, are created.
#[cfg(any(unix, test))]
fn append_once(file: &Path, block: &str) -> Result<bool> {
    let existing = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    if existing.contains(block) {
        return Ok(false);
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut text = String::new();
    if !existing.is_empty() {
        if !existing.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(block);

    use std::io::Write;
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .with_context(|| format!("opening {}", file.display()))?;
    out.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(true)
}

#[cfg(any(unix, test))]
fn add_to_profiles(dir: &Path, env: &ShellEnv<'_>) -> Result<Added> {
    let Some(dir) = dir.to_str() else {
        bail!("the folder name is not valid Unicode");
    };
    let shell = Shell::from_env(env.shell);
    let targets = profile_targets(env, &|p| p.exists());

    let mut places = Vec::new();
    for (file, for_shell) in &targets {
        let block = match for_shell {
            Shell::Fish => fish_block(dir),
            Shell::Sh | Shell::Bash | Shell::Zsh => posix_block(dir),
        };
        if append_once(file, &block)? {
            places.push(file.display().to_string());
        }
    }

    // The file the current shell reads, else `~/.profile`, which every target
    // list starts with.
    let reload = targets
        .iter()
        .find(|(_, s)| *s == shell)
        .or(targets.first())
        .map(|(file, s)| {
            let verb = if *s == Shell::Fish { "source" } else { "." };
            format!("{verb} '{}'", file.display())
        });
    Ok(Added { places, reload })
}

/// The user `Path` value with `dir` put first, or `None` when it is already
/// there. Windows compares paths without case, and a trailing `\` names the
/// same folder.
#[cfg(any(windows, test))]
fn path_with(current: &str, dir: &str) -> Option<String> {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_lowercase();
    let want = norm(dir);
    if current.split(';').any(|entry| norm(entry) == want) {
        return None;
    }
    let rest = current.trim().trim_matches(';');
    Some(if rest.is_empty() {
        dir.to_string()
    } else {
        format!("{dir};{rest}")
    })
}

#[cfg(windows)]
mod registry {
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
        #[allow(unsafe_code, reason = "FFI: RegCloseKey")]
        fn drop(&mut self) {
            // SAFETY: `self.0` is a key that `RegOpenKeyExW` opened, closed once here.
            // A failed close leaves nothing to undo, so its result is not used.
            unsafe { RegCloseKey(self.0) };
        }
    }

    /// Put `dir` first in the user `Path`. Returns `false` when it is already
    /// there. The value keeps its type, so `%VAR%` entries stay unexpanded.
    #[allow(unsafe_code, reason = "FFI: the registry calls for the user Path")]
    pub(super) fn add(dir: &str) -> Result<bool> {
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
        let current = match status {
            ERROR_FILE_NOT_FOUND => {
                kind = REG_EXPAND_SZ;
                String::new()
            }
            ERROR_SUCCESS => {
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
                    Ok(s) => s,
                    Err(_) => bail!("the user Path is not valid text, so it is left unchanged"),
                }
            }
            other => bail!("reading the user Path failed (error {other})"),
        };

        let Some(next) = super::path_with(&current, dir) else {
            return Ok(false);
        };
        let data = wide(&next);
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
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ralphy-user-path-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn the_start_up_files_follow_the_shell() {
        let home = Path::new("/h");
        let zdot = Path::new("/z");
        let xdg = Path::new("/x");
        let p = |s: &str| PathBuf::from(s);
        #[allow(clippy::type_complexity)]
        let rows: &[(
            Option<&str>,
            Option<&Path>,
            Option<&Path>,
            &[&str],
            Vec<(PathBuf, Shell)>,
        )] = &[
            // macOS default: zsh, nothing configured yet.
            (
                Some("/bin/zsh"),
                None,
                None,
                &[],
                vec![
                    (home.join(".profile"), Shell::Sh),
                    (home.join(".zshenv"), Shell::Zsh),
                ],
            ),
            // A common Linux desktop: bash with a ~/.bashrc.
            (
                Some("/bin/bash"),
                None,
                None,
                &["/h/.bashrc"],
                vec![
                    (home.join(".profile"), Shell::Sh),
                    (home.join(".bashrc"), Shell::Bash),
                ],
            ),
            // ~/.bash_profile hides ~/.profile from a login bash.
            (
                Some("/usr/bin/bash"),
                None,
                None,
                &["/h/.bash_profile"],
                vec![
                    (home.join(".profile"), Shell::Sh),
                    (home.join(".bash_profile"), Shell::Bash),
                    (home.join(".bashrc"), Shell::Bash),
                ],
            ),
            // zsh with its files somewhere else.
            (
                Some("zsh"),
                Some(zdot),
                None,
                &[],
                vec![
                    (home.join(".profile"), Shell::Sh),
                    (zdot.join(".zshenv"), Shell::Zsh),
                ],
            ),
            // fish, with a custom config folder.
            (
                Some("/opt/homebrew/bin/fish"),
                None,
                Some(xdg),
                &[],
                vec![
                    (home.join(".profile"), Shell::Sh),
                    (p("/x/fish/conf.d/ralphy.fish"), Shell::Fish),
                ],
            ),
            // No $SHELL, but zsh and fish are set up: they are covered too.
            (
                None,
                None,
                None,
                &["/h/.zshrc", "/h/.config/fish"],
                vec![
                    (home.join(".profile"), Shell::Sh),
                    (home.join(".zshenv"), Shell::Zsh),
                    (p("/h/.config/fish/conf.d/ralphy.fish"), Shell::Fish),
                ],
            ),
        ];
        for (shell, zdotdir, config_home, present, want) in rows {
            let env = ShellEnv {
                home,
                shell: *shell,
                zdotdir: *zdotdir,
                config_home: *config_home,
            };
            let present: Vec<PathBuf> = present.iter().map(PathBuf::from).collect();
            let got = profile_targets(&env, &|f| present.iter().any(|q| q == f));
            assert_eq!(&got, want, "shell {shell:?}, present {present:?}");
        }
    }

    #[test]
    fn the_block_is_written_once_after_the_existing_text() {
        let home = scratch("once");
        std::fs::write(home.join(".profile"), "export EDITOR=vi").expect("seed");
        let env = ShellEnv {
            home: &home,
            shell: Some("/bin/zsh"),
            zdotdir: None,
            config_home: None,
        };
        let dir = Path::new("/Users/me/.local/bin");
        let block = posix_block("/Users/me/.local/bin");

        let first = add_to_profiles(dir, &env).expect("first");
        assert_eq!(
            first.places,
            vec![
                home.join(".profile").display().to_string(),
                home.join(".zshenv").display().to_string(),
            ]
        );
        assert_eq!(
            first.reload,
            Some(format!(". '{}'", home.join(".zshenv").display()))
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".profile")).expect("read"),
            format!("export EDITOR=vi\n\n{block}")
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".zshenv")).expect("read"),
            block
        );

        let second = add_to_profiles(dir, &env).expect("second");
        assert_eq!(second.places, Vec::<String>::new());
        assert_eq!(
            std::fs::read_to_string(home.join(".profile")).expect("read"),
            format!("export EDITOR=vi\n\n{block}"),
            "a second install must not add a second block"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The block is real shell code: `sh` runs it, puts the folder first, and a
    /// second run adds nothing. The folder name has a space and a quote.
    #[cfg(unix)]
    #[test]
    fn sh_puts_the_folder_on_path_once() {
        let home = scratch("sh");
        let dir = "/opt/it's mine/bin";
        let file = home.join("profile");
        std::fs::write(&file, posix_block(dir)).expect("write");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#". "$1"; . "$1"; printf %s "$PATH""#)
            .arg("sh")
            .arg(&file)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{dir}:/usr/bin:/bin"),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_user_path_gets_the_folder_first_and_only_once() {
        let dir = r"C:\Users\me\.local\bin";
        let rows: &[(&str, Option<&str>)] = &[
            ("", Some(r"C:\Users\me\.local\bin")),
            (
                r"C:\tools;%USERPROFILE%\go\bin",
                Some(r"C:\Users\me\.local\bin;C:\tools;%USERPROFILE%\go\bin"),
            ),
            (r"C:\tools;", Some(r"C:\Users\me\.local\bin;C:\tools")),
            (r"C:\tools;c:\users\ME\.local\bin\", None),
            (r"C:\Users\me\.local\bin", None),
        ];
        for (current, want) in rows {
            assert_eq!(
                path_with(current, dir).as_deref(),
                *want,
                "current {current:?}"
            );
        }
    }
}
