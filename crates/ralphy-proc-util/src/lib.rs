//! Path & program resolution, plus the shared process-tree teardown primitives
//! (`own_process_group` at spawn + `kill_tree` on teardown) that both the verify
//! gate and the headless adapter runner rely on to not leak a grandchild.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

pub mod cursor;

/// Put `cmd`'s child into its own process group (Unix) so a later [`kill_tree`]
/// can signal the whole tree via the negative pgid, not just the direct child.
/// A no-op off Unix — Windows walks the tree by parent-PID at kill time instead,
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

/// Suppress the console window a child would otherwise flash. On Windows, a
/// console program spawned by a parent that has **no console** (e.g. the
/// daemon-dispatched `ralphy`, launched `DETACHED_PROCESS`) is given a fresh
/// *visible* console by the OS; `CREATE_NO_WINDOW` gives it a hidden console
/// instead, so no black window flashes. A no-op off Windows.
///
/// Only safe for children whose stdout/stderr are captured or redirected — an
/// inherited-stdio child (one meant to print to the user's terminal) would lose
/// its visible output. Call it on the `Command` before `spawn`/`output`.
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW` — a console app run with a hidden console.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Kill `child` and every descendant it spawned, then reap it. `child.kill()`
/// signals only the direct child, so a grandchild — an agent CLI's helper, or a
/// dev server a `## Verify` command backgrounded — would survive and keep an
/// inherited stdout/stderr pipe open, blocking a reader thread forever. On Windows
/// a native process-tree walk terminates every descendant; on Unix a negative
/// pgid signals the process group the child leads (set via [`own_process_group`]
/// at spawn). Both arms work when the direct child has ALREADY exited — the
/// tree-kill-before-collect callers depend on reaping orphans of a dead parent.
/// Best-effort on every arm; always reaps the direct child so no zombie lingers.
pub fn kill_tree(child: &mut Child) {
    kill_tree_by_pid(child.id());
    let _ = child.kill(); // direct child / fallback
    let _ = child.wait(); // reap so no zombie lingers
}

/// Kill the process tree rooted at `pid` by OS pid alone — for a child this crate
/// does not own as a [`Child`] (e.g. a `ralphy-pty` session, whose PTY child is
/// not a `std::process::Child`). On Windows a Toolhelp snapshot walk finds every
/// descendant by parent-PID and terminates each; on Unix the pid doubles as a
/// process-group id (the PTY child is a session leader, or was placed in its own
/// group via [`own_process_group`]), so a negative pgid signals the whole group.
///
/// Both arms reap orphans of an already-dead root: living descendants still
/// record the dead parent's PID (Windows) and the process group outlives its
/// leader (Unix). `taskkill /F /T`, the previous Windows arm, aborts its walk
/// when the root PID is no longer running — exactly the exit-leaking-grandchild
/// shape (#156) — which is why the walk is native. Best-effort, and does not
/// reap — the caller owns reaping its handle.
pub fn kill_tree_by_pid(pid: u32) {
    #[cfg(windows)]
    kill_tree_windows(pid);
    #[cfg(unix)]
    {
        use std::process::Stdio;
        // A negative pgid signals the whole process group. Dependency-free via the
        // `kill` utility.
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// The Windows arm of [`kill_tree_by_pid`]: snapshot the process table, walk the
/// parent-PID edges breadth-first from `root`, and terminate every process found
/// (root first, so a live root can't spawn replacements mid-walk). PID-reuse
/// caveat: a stale parent-PID pointing at a reused `root` would drag an unrelated
/// process into the walk — the same exposure `taskkill /T` had, accepted for the
/// same reason (the window is spawn-to-teardown of one gate command).
#[cfg(windows)]
fn kill_tree_windows(root: u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    // One snapshot of the whole (pid, parent-pid) table.
    let mut table: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
        if Process32First(snap, &mut entry) != 0 {
            loop {
                table.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32Next(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }

    // Breadth-first over parent edges. `doomed` doubles as the visited set — a
    // recycled parent PID can make the edges cyclic, so membership is checked
    // before pushing.
    let mut doomed: Vec<u32> = vec![root];
    let mut queue: Vec<u32> = vec![root];
    while let Some(parent) = queue.pop() {
        for &(pid, ppid) in &table {
            if ppid == parent && pid != parent && !doomed.contains(&pid) {
                doomed.push(pid);
                queue.push(pid);
            }
        }
    }

    for pid in doomed {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            // Null on failure — already gone, or access denied. Best-effort.
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
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
    let home = home?;
    let mut cand = home.join(".local").join("bin").join(name);
    if cfg!(windows) {
        cand.set_extension("exe");
    }
    if is_executable_file(&cand) {
        return Some(cand);
    }
    // Last resort: a version-managed Node install, whose global bin is off `PATH`
    // in a non-login shell (ADR-0043 D16).
    nvm_candidates(&home, name)
        .into_iter()
        .find(|c| is_executable_file(c))
}

/// True when `dir` is a Windows drive mounted into a Linux filesystem —
/// `/mnt/<letter>/…`, the WSL interop layout.
///
/// Under WSL the Windows `PATH` leaks into the Linux one, so a vendor CLI
/// installed on Windows (`/mnt/c/Users/x/AppData/Roaming/npm/gemini`) is found by
/// a plain `PATH` search and then fails to execute as a Linux program — or worse,
/// executes the Windows binary against Linux paths (ADR-0043 D16). A Linux search
/// must skip those directories and keep looking.
///
/// Pure over its input and OS-independent, so both directions unit-test on every
/// platform; the `cfg!(unix)` decision of whether to APPLY it lives at the call
/// site in [`find_program`].
pub fn is_windows_mount_path(dir: &Path) -> bool {
    let mut comps = dir.components();
    let Some(std::path::Component::RootDir) = comps.next() else {
        return false;
    };
    let Some(std::path::Component::Normal(mnt)) = comps.next() else {
        return false;
    };
    if mnt != "mnt" {
        return false;
    }
    matches!(comps.next(),
        Some(std::path::Component::Normal(drive))
            if drive.to_str().is_some_and(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_alphabetic())))
}

/// Every `<home>/.nvm/versions/node/*/bin/<name>`, NEWEST version first.
///
/// A version-managed Node install puts npm's global bin under the active Node
/// version rather than on a stable path, and a non-login shell (which is what a
/// daemon or a CI step gets) often carries neither the nvm shims nor the active
/// version's bin on `PATH` (ADR-0043 D16).
///
/// The order is load-bearing, not cosmetic: `locate_program_with` takes the FIRST
/// hit, and a plain lexicographic sort hands it `v10.24.0` over `v9.11.2` and
/// `v20` over `v22` — systematically the older runtime. Ordering on the parsed
/// numeric components picks the newest and stays deterministic rather than
/// filesystem-order dependent.
///
/// Unix-shaped by construction; nvm-windows' `%APPDATA%\nvm\vX\` layout differs
/// and is not covered, so this fallback is inert on Windows — where npm's global
/// bin is on `PATH` anyway.
///
/// Pure over its inputs — it only reads the directory listing — so it unit-tests
/// against a temp home on every platform.
pub fn nvm_candidates(home: &Path, name: &str) -> Vec<PathBuf> {
    let versions = home.join(".nvm").join("versions").join("node");
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };
    let mut found: Vec<(Vec<u64>, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let dir = e.path();
            let key = dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(version_key)
                .unwrap_or_default();
            (key, dir.join("bin").join(name))
        })
        .collect();
    // Newest first; the path breaks ties so the order is total.
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The numeric components of a version directory name like `v22.22.2`, for
/// ordering. A non-numeric segment contributes `0`, so an unparsable name sorts
/// low rather than panicking or winning.
fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches('v')
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

/// True when `path` is the shape `cursor-agent`'s shell classifier accepts as
/// git-bash — its own detector keys on `/git.*bash\.exe$/i` (ADR-0042 D20). A
/// `bash.exe` that is not under a `git` path — notably `%SystemRoot%\System32\
/// bash.exe`, the **WSL launcher** — is NOT git-bash: pinning `SHELL` to it would
/// make the vendor spawn WSL, not a POSIX shell on the host. Pure and
/// OS-independent so both directions unit-test on every platform.
pub fn is_git_bash_shape(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    lower
        .strip_suffix("bash.exe")
        .is_some_and(|head| head.contains("git"))
}

/// Locate git-bash (`bash.exe`) — the shell `cursor-agent`'s classifier picks over
/// PowerShell for POSIX shell tool calls (ADR-0042 D20). `None` means no git-bash
/// is present, and the caller must then leave `SHELL` unset rather than point it at
/// a missing binary.
///
/// The two standard Git-for-Windows install roots are probed first — the exact
/// paths the vendor's own detector uses — then `PATH`. Only a git-bash-*shaped* hit
/// is accepted ([`is_git_bash_shape`]): a bare `bash.exe` on `PATH` is as likely to
/// be the WSL launcher, which is not git-bash.
///
/// Pure over its inputs so all shapes unit-test against temp trees with an empty
/// `PATH`, matching [`locate_cursor_with`](cursor::locate_cursor_with).
pub fn locate_git_bash_with(
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
    program_files: Option<PathBuf>,
    program_files_x86: Option<PathBuf>,
) -> Option<PathBuf> {
    // `Git\bin\bash.exe` is git-bash; `Git\cmd\` (sometimes the only dir on PATH)
    // carries git.exe but no bash — hence the explicit `bin` probe here.
    for root in [program_files, program_files_x86].into_iter().flatten() {
        let cand = root.join("Git").join("bin").join("bash.exe");
        if cand.is_file() {
            return Some(cand);
        }
    }
    find_program("bash", path_var, pathext).filter(|p| is_git_bash_shape(p))
}

/// Locate git-bash against the real environment (ADR-0042 D20). `None` when no
/// git-bash is installed — the run then leaves `SHELL` as the operator has it.
pub fn locate_git_bash() -> Option<PathBuf> {
    locate_git_bash_with(
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
    )
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
        // Under WSL the Windows PATH leaks in: a `/mnt/c/…` hit is a Windows
        // binary that cannot run as a Linux program (ADR-0043 D16). Skip it and
        // keep searching rather than returning an unrunnable path.
        if cfg!(unix) && is_windows_mount_path(&dir) {
            continue;
        }
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

    /// ADR-0043 D16: a `/mnt/<drive>/…` directory is a Windows mount, and a Linux
    /// `PATH` search must skip it. The helper is pure and OS-independent so both
    /// directions are asserted on Windows and Linux CI alike.
    #[test]
    fn a_windows_mount_path_is_rejected_off_windows() {
        assert!(is_windows_mount_path(Path::new(
            "/mnt/c/Users/x/AppData/Roaming/npm"
        )));
        assert!(is_windows_mount_path(Path::new("/mnt/d")));
        // `/mnt/data` is an ordinary Linux mount: the second component must be a
        // SINGLE letter, or every `/mnt/*` volume would be skipped.
        assert!(!is_windows_mount_path(Path::new("/mnt/data")));
        assert!(!is_windows_mount_path(Path::new("/usr/local/bin")));
        assert!(!is_windows_mount_path(Path::new("/mnt")));
        // Relative paths never qualify — the layout is rooted by definition.
        assert!(!is_windows_mount_path(Path::new("mnt/c/npm")));
    }

    /// ADR-0043 D16: npm's global bin under a version-managed Node install is off
    /// `PATH` in a non-login shell, so the locator has to look there by hand.
    #[test]
    fn nvm_candidates_cover_a_version_managed_install() {
        let home = tempfile::tempdir().unwrap();
        assert!(
            nvm_candidates(home.path(), "gemini").is_empty(),
            "no .nvm at all is not an error"
        );

        // Deliberately spanning the decade boundary a lexicographic sort gets
        // wrong (`v10` sorts before `v9` as strings).
        for v in ["v9.11.2", "v22.22.2"] {
            let bin = home.path().join(".nvm/versions/node").join(v).join("bin");
            fs::create_dir_all(&bin).unwrap();
            let exe = bin.join("gemini");
            fs::write(&exe, b"#!/usr/bin/env node\n").unwrap();
            mark_executable(&exe);
        }

        let got = nvm_candidates(home.path(), "gemini");
        assert_eq!(got.len(), 2, "{got:?}");
        // NEWEST first: a lexicographic sort would put v10 before v9 and v20
        // before v22, handing the locator the older runtime every time.
        assert!(
            got[0].to_string_lossy().contains("v22.22.2"),
            "newest version must come first: {got:?}"
        );
        assert!(got[1].to_string_lossy().contains("v9.11.2"), "{got:?}");
        // Every candidate is `<version>/bin/<name>` — the `bin` segment is what a
        // path built from the wrong join would silently lose.
        for p in &got {
            assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("gemini"));
            assert_eq!(
                p.parent()
                    .and_then(|d| d.file_name())
                    .and_then(|n| n.to_str()),
                Some("bin"),
                "{p:?}"
            );
        }

        // …and the locator actually reaches them: `PATH` is empty, `~/.local/bin`
        // holds nothing, so only the nvm fallback can answer. (Windows gates
        // executability on PATHEXT, so the extensionless fixture only resolves on
        // Unix — the CANDIDATE list above is what this asserts cross-platform.)
        if cfg!(unix) {
            let found = locate_program_with(
                "gemini",
                Some(std::ffi::OsString::new()),
                None,
                Some(home.path().to_path_buf()),
            );
            assert_eq!(found, got.first().cloned(), "the nvm fallback must be hit");
        }
    }

    /// ADR-0042 D20: git-bash under either standard root is the shape the vendor's
    /// classifier accepts; the WSL launcher (a `System32\bash.exe`) and git.exe are
    /// not, and pinning `SHELL` to the WSL bash would spawn WSL instead of a POSIX
    /// shell on the host.
    #[test]
    fn is_git_bash_shape_matches_the_vendor_regex() {
        assert!(is_git_bash_shape(Path::new(
            r"C:\Program Files\Git\bin\bash.exe"
        )));
        // Case-insensitive, like the vendor's `/…/i` regex.
        assert!(is_git_bash_shape(Path::new(
            r"C:\Program Files (x86)\Git\bin\BASH.EXE"
        )));
        // The WSL launcher: a bash.exe that is NOT git-bash.
        assert!(!is_git_bash_shape(Path::new(
            r"C:\Windows\System32\bash.exe"
        )));
        // git.exe is not a shell.
        assert!(!is_git_bash_shape(Path::new(
            r"C:\Program Files\Git\cmd\git.exe"
        )));
    }

    /// D20: the two standard Git-for-Windows roots are probed first, `bin\bash.exe`
    /// under each. `is_file()` is OS-independent, so this resolves on every platform.
    #[test]
    fn locate_git_bash_prefers_the_program_files_roots() {
        for from_x86 in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let want = root.path().join("Git").join("bin").join("bash.exe");
            fs::create_dir_all(want.parent().unwrap()).unwrap();
            fs::write(&want, b"").unwrap();
            let (pf, pf86) = if from_x86 {
                (None, Some(root.path().to_path_buf()))
            } else {
                (Some(root.path().to_path_buf()), None)
            };
            let got = locate_git_bash_with(Some(std::ffi::OsString::new()), None, pf, pf86);
            assert_eq!(got.as_deref(), Some(want.as_path()), "x86={from_x86}");
        }
    }

    /// No install anywhere resolves to nothing — the caller then leaves `SHELL`
    /// alone rather than pinning it to a missing binary.
    #[test]
    fn locate_git_bash_is_none_when_nothing_is_installed() {
        assert_eq!(
            locate_git_bash_with(Some(std::ffi::OsString::new()), None, None, None),
            None
        );
    }

    /// D20: a `bash.exe` on `PATH` that is not git-bash (the WSL launcher shape) is
    /// rejected, while a git-shaped one is accepted. Windows-only: the `PATH` search
    /// keys on `PATHEXT`, which is a no-op off Windows.
    #[test]
    #[cfg(windows)]
    fn locate_git_bash_rejects_a_non_git_bash_on_path() {
        let base = tempfile::tempdir().unwrap();

        // A `System32\bash.exe` on PATH — the WSL launcher — must be rejected.
        let sys = base.path().join("System32");
        fs::create_dir_all(&sys).unwrap();
        fs::write(sys.join("bash.exe"), b"").unwrap();
        assert_eq!(
            locate_git_bash_with(
                Some(sys.clone().into_os_string()),
                Some(".EXE".into()),
                None,
                None
            ),
            None,
            "a non-git bash.exe on PATH must be rejected"
        );

        // A git-shaped `Git\bin\bash.exe` on PATH IS accepted.
        let gitbin = base.path().join("Git").join("bin");
        fs::create_dir_all(&gitbin).unwrap();
        let want = gitbin.join("bash.exe");
        fs::write(&want, b"").unwrap();
        let got = locate_git_bash_with(
            Some(gitbin.into_os_string()),
            Some(".EXE".into()),
            None,
            None,
        )
        .expect("a git-shaped bash.exe on PATH must resolve");
        assert!(is_git_bash_shape(&got), "resolved {got:?}");
        assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("bash"));
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
