//! Take out what `ralphy install` put on `PATH`, for `ralphy uninstall`: the
//! blocks that start with the install marker on Unix, and the install folder in
//! the user `Path` on Windows. Every other line and entry stays as it is.

use std::path::Path;

#[cfg(any(unix, test))]
use anyhow::Context;
use anyhow::Result;

#[cfg(any(unix, test))]
use super::{profile_targets, ShellEnv, MARKER};

/// `text` without the blocks that start with the install marker, and without
/// the blank line `append_once` put before each. `None` when it has none.
#[cfg(any(unix, test))]
fn without_block(text: &str) -> Option<String> {
    let is_marker = |line: &str| line.trim_end_matches(['\r', '\n']) == MARKER;
    if !text.split_inclusive('\n').any(is_marker) {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        if !is_marker(line) {
            kept.push(line);
            continue;
        }
        if kept.last().is_some_and(|l| l.trim().is_empty()) {
            kept.pop();
        }
        skip_block(&mut lines);
    }
    Some(kept.concat())
}

/// Skip the lines of one block after its marker. A POSIX block ends with
/// `esac`, a fish block with `end`.
#[cfg(any(unix, test))]
fn skip_block<'a>(lines: &mut impl Iterator<Item = &'a str>) {
    for line in lines {
        if matches!(line.trim(), "esac" | "end") {
            break;
        }
    }
}

/// Remove the blocks from every start-up file `ralphy install` may have
/// written, whatever the shell is now. Returns the files it changed.
#[cfg(any(unix, test))]
fn remove_from_profiles(env: &ShellEnv<'_>) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for (file, _) in profile_targets(env, &|_| true) {
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
        };
        let Some(rest) = without_block(&text) else {
            continue;
        };
        // `conf.d/ralphy.fish` is a file `ralphy install` made for the block
        // alone.
        if rest.trim().is_empty() && file.file_name().is_some_and(|n| n == "ralphy.fish") {
            std::fs::remove_file(&file).with_context(|| format!("removing {}", file.display()))?;
        } else {
            std::fs::write(&file, rest).with_context(|| format!("writing {}", file.display()))?;
        }
        changed.push(file.display().to_string());
    }
    Ok(changed)
}

/// The user `Path` value without `dir`, or `None` when it is not there.
#[cfg(any(windows, test))]
fn path_without(current: &str, dir: &str) -> Option<String> {
    let want = super::path_entry(dir);
    let entries: Vec<&str> = current.split(';').collect();
    if !entries.iter().any(|e| super::path_entry(e) == want) {
        return None;
    }
    let kept: Vec<&str> = entries
        .into_iter()
        .filter(|e| !e.trim().is_empty() && super::path_entry(e) != want)
        .collect();
    Some(kept.join(";"))
}

#[cfg(unix)]
pub(crate) fn remove_from_user_path(_dir: &Path) -> Result<Vec<String>> {
    let home = directories::BaseDirs::new()
        .context("locating the home directory")?
        .home_dir()
        .to_path_buf();
    let zdotdir = std::env::var_os("ZDOTDIR").map(std::path::PathBuf::from);
    let config_home = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from);
    remove_from_profiles(&ShellEnv {
        home: &home,
        shell: None,
        zdotdir: zdotdir.as_deref(),
        config_home: config_home.as_deref(),
    })
}

#[cfg(windows)]
pub(crate) fn remove_from_user_path(dir: &Path) -> Result<Vec<String>> {
    let Some(dir) = dir.to_str() else {
        anyhow::bail!("the folder name is not valid Unicode");
    };
    Ok(
        if super::registry::change(|current| path_without(current, dir))? {
            vec!["your user Path (HKEY_CURRENT_USER\\Environment)".to_string()]
        } else {
            Vec::new()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::{fish_block, posix_block};
    use super::*;

    #[test]
    fn only_the_marked_block_is_removed() {
        let block = posix_block("/home/me/.local/bin");
        let text = format!("export EDITOR=vi\n\n{block}alias ll='ls -l'\n");
        assert_eq!(
            without_block(&text).as_deref(),
            Some("export EDITOR=vi\nalias ll='ls -l'\n")
        );
        assert_eq!(without_block(&block).as_deref(), Some(""));
        assert_eq!(
            without_block(&fish_block("/home/me/.local/bin")).as_deref(),
            Some("")
        );
        assert_eq!(
            without_block("case x in\nesac\nexport A=1\n"),
            None,
            "a file without the marker is left alone"
        );
    }

    #[test]
    fn uninstall_takes_out_what_install_wrote() {
        let home = tempfile::tempdir().expect("scratch");
        let home = home.path();
        std::fs::write(home.join(".profile"), "export EDITOR=vi\n").expect("seed");
        std::fs::create_dir_all(home.join(".config").join("fish")).expect("fish");
        let env = ShellEnv {
            home,
            shell: Some("/bin/bash"),
            zdotdir: None,
            config_home: None,
        };
        super::super::add_to_profiles(Path::new("/opt/ralphy bin"), &env).expect("install");
        let fish = home.join(".config/fish/conf.d/ralphy.fish");
        assert!(fish.is_file());

        let changed = remove_from_profiles(&ShellEnv { shell: None, ..env }).expect("uninstall");
        assert_eq!(changed.len(), 3, "{changed:?}");
        assert_eq!(
            std::fs::read_to_string(home.join(".profile")).expect("read"),
            "export EDITOR=vi\n"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".bashrc")).expect("read"),
            ""
        );
        assert!(!fish.exists(), "the fish file held only the block");
    }

    #[test]
    fn the_user_path_loses_only_the_install_folder() {
        let dir = r"C:\Users\me\.local\bin";
        let rows: &[(&str, Option<&str>)] = &[
            (
                r"C:\Users\me\.local\bin;C:\tools;%USERPROFILE%\go\bin",
                Some(r"C:\tools;%USERPROFILE%\go\bin"),
            ),
            (r"C:\tools;c:\users\ME\.local\bin\;", Some(r"C:\tools")),
            (r"C:\Users\me\.local\bin", Some("")),
            (r"C:\tools", None),
        ];
        for (current, want) in rows {
            assert_eq!(
                path_without(current, dir).as_deref(),
                *want,
                "current {current:?}"
            );
        }
    }
}
