//! Path & program resolution, plus the shared process-tree teardown primitives
//! (`own_process_group` at spawn + `kill_tree` on teardown) that both the verify
//! gate and the headless adapter runner rely on to not leak a grandchild.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Put `cmd`'s child into its own process group (Unix) so a later [`kill_tree`]
/// can signal the whole tree via the negative pgid, not just the direct child.
/// A no-op off Unix — Windows walks the tree by PID with `taskkill /T` instead,
/// needing nothing at spawn. Call this on the `Command` before `spawn`.
///
/// This is the single source of truth for the spawn-side half of process-tree
/// teardown, shared so the verify gate (core) and the headless adapter runner
/// (adapter-support) can never disagree on how a killable tree is set up.
pub fn own_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Kill `child` and every descendant it spawned, then reap it. `child.kill()`
/// signals only the direct child, so a grandchild — an agent CLI's helper, or a
/// dev server a `## Verify` command backgrounded — would survive and keep an
/// inherited stdout/stderr pipe open, blocking a reader thread forever. On Windows
/// `taskkill /F /T` terminates the whole tree rooted at the PID; on Unix a negative
/// pgid signals the process group the child leads (set via [`own_process_group`] at
/// spawn). Best-effort on every arm; always reaps the direct child so no zombie
/// lingers.
pub fn kill_tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        // `taskkill /T` terminates the whole tree rooted at PID.
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // The child leads its own process group (set at spawn), so a negative pgid
        // signals the whole tree. Dependency-free via the `kill` utility.
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill(); // direct child / fallback
    let _ = child.wait(); // reap so no zombie lingers
}

/// Resolve a home-scoped store/config path: when `override_base` is `Some` (a
/// vendor `$XXX_HOME` env var), the path is `override_base.join(tail)` — the
/// override replaces the whole `home_dir()/home_rel` base, so `home_rel` is
/// ignored. When `None`, it is `home_dir()?/home_rel/tail`. Returns `None` only
/// when no override is given and no home is known.
///
/// The three-part shape (base + `home_rel` + `tail`) is what lets Codex's
/// `$CODEX_HOME/config.toml` and `<home>/.codex/config.toml` share one helper: the
/// `config.toml` tail joins onto whichever base wins.
pub fn home_scoped_path(
    override_base: Option<std::ffi::OsString>,
    home_rel: &Path,
    tail: &Path,
) -> Option<PathBuf> {
    match override_base {
        Some(base) => Some(PathBuf::from(base).join(tail)),
        None => Some(home_dir()?.join(home_rel).join(tail)),
    }
}

/// Resolve `name` to the path of the executable a headless `Command` should
/// spawn, searching `PATH` and — on Windows — every `PATHEXT` extension. Falls
/// back to the bare `name` so the spawn error still names the missing program.
///
/// This exists because Windows ships many agent CLIs as npm shims with **no**
/// `.exe` (e.g. `opencode.cmd`): `Command::new("opencode")` only ever tries
/// `opencode` and `opencode.exe`, so it reports "program not found" even though
/// `opencode.cmd` is on `PATH`. Resolving the full path (including the `.cmd`
/// extension) lets `std`'s `Command` launch the batch shim directly — modern
/// `std` routes `.bat`/`.cmd` through the command processor with safe argument
/// escaping. A native `.exe` (the common case off Windows, and for Codex) is
/// found first and returned unchanged.
pub fn resolve_program(name: &str) -> std::ffi::OsString {
    locate_program(name)
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| name.into())
}

/// Locate `name` as an executable: search `PATH` (+`PATHEXT` on Windows), then
/// fall back to the XDG-conventional `~/.local/bin` where user-installed CLIs (and
/// Ralphy itself, see the cli's `install` module) land but which a non-login shell
/// often omits from `PATH`. Returns the resolved path, or `None` when not found
/// anywhere.
///
/// This is the single source of truth for "is this program available, and where" —
/// `resolve_program` (what every adapter spawns) and `ralphy init`'s environment
/// gate both go through it, so **detection and execution can never disagree**: a
/// CLI under `~/.local/bin` that the gate would otherwise miss on a bare `PATH`
/// probe is both reported present and actually spawned.
pub fn locate_program(name: &str) -> Option<PathBuf> {
    locate_program_with(
        name,
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        home_dir(),
    )
}

/// Pure core of [`locate_program`] over its inputs, so the `~/.local/bin` fallback
/// unit-tests against a temp home without touching the real environment.
pub fn locate_program_with(
    name: &str,
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(found) = find_program(name, path_var, pathext) {
        return Some(found);
    }
    let mut cand = home?.join(".local").join("bin").join(name);
    if cfg!(windows) {
        cand.set_extension("exe");
    }
    is_executable_file(&cand).then_some(cand)
}

/// The home directory, from the platform's usual env var (`USERPROFILE` on
/// Windows, else `HOME`). Exported so every adapter shares one definition instead
/// of re-deriving the `USERPROFILE`-or-`HOME` dance.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// True when `p` is a regular file the OS would actually run. On Unix this
/// requires an execute bit, so a non-executable data file sharing the program's
/// name on `PATH` can't shadow a real binary later in the search; off Unix, being
/// a regular file is enough (Windows gates executability on `PATHEXT` separately).
#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Search `path_var` for `name`, trying each `PATHEXT` extension on Windows. Pure
/// over its inputs so it unit-tests against a temp dir. Mirrors the Claude
/// adapter's private resolver; lives here so every headless adapter shares it.
pub fn find_program(
    name: &str,
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let path_var = path_var?;
    let exts: Vec<String> = if cfg!(windows) {
        pathext
            .and_then(|p| p.into_string().ok())
            .unwrap_or_else(|| ".EXE".into())
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    for dir in std::env::split_paths(&path_var) {
        let direct = dir.join(name);
        // On Windows a file is only executable when its extension is in PATHEXT,
        // so a bare extensionless `direct` must be skipped — npm ships agent CLIs
        // as a pair (`opencode` shell shim + `opencode.cmd`), and returning the
        // extensionless shim yields "not a valid Win32 application" (os error 193).
        // Off Windows, any existing file on PATH is a candidate as-is.
        let direct_ok = if cfg!(windows) {
            direct.is_file()
                && direct
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| {
                        exts.iter()
                            .any(|x| x.trim_start_matches('.').eq_ignore_ascii_case(e))
                    })
        } else {
            is_executable_file(&direct)
        };
        if direct_ok {
            return Some(direct);
        }
        for ext in &exts {
            let cand = dir.join(name).with_extension(ext.trim_start_matches('.'));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Mark a freshly-written fixture executable so `find_program`'s Unix
    /// execute-bit check accepts it. A no-op off Unix (Windows gates on PATHEXT).
    fn mark_executable(p: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(p).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(p, perms).unwrap();
        }
        #[cfg(not(unix))]
        let _ = p;
    }

    #[test]
    fn home_scoped_path_override_replaces_the_home_base() {
        // The override branch: `base.join(tail)`, with `home_rel` ignored.
        assert_eq!(
            home_scoped_path(
                Some("X".into()),
                Path::new(".codex"),
                Path::new("config.toml")
            ),
            Some(PathBuf::from("X").join("config.toml"))
        );
    }

    #[test]
    fn home_scoped_path_home_branch_joins_home_rel_and_tail() {
        // The home branch: `home_dir()/home_rel/tail`. Deterministic against the
        // same `home_dir()` the helper uses — no env mutation.
        assert_eq!(
            home_scoped_path(None, Path::new(".codex"), Path::new("config.toml")),
            home_dir().map(|h| h.join(".codex").join("config.toml"))
        );
    }

    #[test]
    fn find_program_locates_a_file_on_the_search_path() {
        let tmp = std::env::temp_dir().join(format!("ralphy-find-prog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        // A file with the searched extension (".EXE" via PATHEXT on Windows; on
        // non-Windows the bare name is what is matched).
        let bare = tmp.join("tool");
        let exe = tmp.join("tool.exe");
        let target = if cfg!(windows) { &exe } else { &bare };
        fs::write(target, b"x").unwrap();
        mark_executable(target);

        let path_var = tmp.clone().into_os_string();
        let got = find_program("tool", Some(path_var), Some(".EXE".into()))
            .expect("must locate the file on PATH");
        // The resolved path must point at a real file whose stem is `tool` (the
        // extension casing may follow PATHEXT, which is harmless on Windows'
        // case-insensitive filesystem).
        assert!(got.is_file(), "resolved path must exist: {got:?}");
        assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("tool"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_program_resolves_a_windows_cmd_shim() {
        // The defect this guards: an npm CLI present only as `name.cmd` (no
        // `.exe`) must still resolve, since `Command::new("name")` would not find
        // it. On non-Windows there is no PATHEXT, so this asserts the bare-name
        // branch instead.
        let tmp = std::env::temp_dir().join(format!("ralphy-find-cmd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path_var = tmp.clone().into_os_string();

        if cfg!(windows) {
            let shim = tmp.join("opencode.cmd");
            fs::write(&shim, b"@echo off").unwrap();
            let got = find_program("opencode", Some(path_var), Some(".EXE;.CMD".into()))
                .expect("must resolve the .cmd shim");
            assert!(got.is_file(), "resolved shim must exist: {got:?}");
            assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("opencode"));
            assert!(
                got.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("cmd")),
                "must resolve the .cmd extension, not .exe: {got:?}"
            );
        } else {
            let bare = tmp.join("opencode");
            fs::write(&bare, b"#!/bin/sh").unwrap();
            mark_executable(&bare);
            let got = find_program("opencode", Some(path_var), None);
            assert_eq!(got.as_deref(), Some(bare.as_path()));
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    #[cfg(windows)]
    fn find_program_skips_extensionless_shim_when_cmd_present() {
        // The exact npm-on-Windows layout: a bare `opencode` shell shim sits next
        // to `opencode.cmd`. The bare file is not a valid Win32 application
        // (os error 193), so the resolver must return the `.cmd`, not the shim.
        let tmp = std::env::temp_dir().join(format!("ralphy-find-pair-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("opencode"), b"#!/bin/sh\n").unwrap();
        fs::write(tmp.join("opencode.cmd"), b"@echo off\n").unwrap();

        let got = find_program(
            "opencode",
            Some(tmp.clone().into_os_string()),
            Some(".EXE;.CMD".into()),
        )
        .expect("must resolve a runnable candidate");
        assert!(
            got.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cmd")),
            "must return the .cmd, not the extensionless shim: {got:?}"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_program_returns_none_when_absent() {
        let path_var = std::env::temp_dir().into_os_string();
        assert!(find_program(
            "definitely-not-a-real-prog-xyz",
            Some(path_var),
            Some(".EXE".into())
        )
        .is_none());
    }

    #[test]
    fn locate_program_falls_back_to_local_bin_when_off_path() {
        // A program absent from PATH but present in ~/.local/bin must still be
        // located — this is the gate/execution unification: the env gate would
        // otherwise miss it, while the adapter (resolve_program) would still run it.
        let home = std::env::temp_dir().join(format!("ralphy-locate-home-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let bin = home.join(".local").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let name = "myagent";
        let file = if cfg!(windows) {
            bin.join("myagent.exe")
        } else {
            bin.join("myagent")
        };
        fs::write(&file, b"x").unwrap();
        mark_executable(&file);

        // Empty PATH so only the ~/.local/bin fallback can match.
        let got = locate_program_with(
            name,
            Some(std::ffi::OsString::new()),
            Some(".EXE".into()),
            Some(home.clone()),
        );
        assert_eq!(got.as_deref(), Some(file.as_path()));

        // Without a home, the fallback can't fire → None.
        assert!(locate_program_with(
            name,
            Some(std::ffi::OsString::new()),
            Some(".EXE".into()),
            None
        )
        .is_none());

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn locate_program_prefers_path_over_local_bin() {
        // When the program is on PATH, that wins — the fallback is only consulted
        // when PATH has nothing.
        let tmp = std::env::temp_dir().join(format!("ralphy-locate-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let on_path = if cfg!(windows) {
            tmp.join("tool.exe")
        } else {
            tmp.join("tool")
        };
        fs::write(&on_path, b"x").unwrap();
        mark_executable(&on_path);

        let got = locate_program_with(
            "tool",
            Some(tmp.clone().into_os_string()),
            Some(".EXE".into()),
            // A bogus home whose ~/.local/bin doesn't exist — PATH must win anyway.
            Some(tmp.join("nonexistent-home")),
        )
        .expect("PATH hit must win");
        // Compare by parent + stem: on Windows the resolved extension casing follows
        // PATHEXT (`.EXE`) rather than the file's `.exe`, which is harmless.
        assert_eq!(got.parent(), on_path.parent());
        assert_eq!(got.file_stem(), on_path.file_stem());
        let _ = fs::remove_dir_all(&tmp);
    }
}
