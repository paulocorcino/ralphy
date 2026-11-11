//! Path and program resolution, and the process-tree teardown that the verify
//! gate and the headless adapter runner share.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

pub mod cursor;
pub mod elevated;
pub mod exe;
pub mod hook_form;
pub mod pid;

pub use pid::pid_is_alive;

/// Put `cmd`'s child into its own process group (Unix), so [`kill_tree`] can
/// signal the whole tree; a no-op elsewhere. Call it before `spawn`.
pub fn own_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    let _: &mut Command = cmd;
}

/// Prepend `dir` to a `PATH` value, or `None` when it is already there.
/// Entries compare by path equality, so pass a resolved directory.
pub fn path_with_dir(dir: &Path, current: Option<&std::ffi::OsStr>) -> Option<std::ffi::OsString> {
    let current = current.unwrap_or_else(|| std::ffi::OsStr::new(""));
    if std::env::split_paths(current).any(|entry| entry == dir) {
        return None;
    }
    let joined = std::iter::once(dir.to_path_buf())
        .chain(std::env::split_paths(current))
        .collect::<Vec<_>>();
    std::env::join_paths(joined).ok()
}

/// Put the directory `cmd`'s program was resolved from on the child's `PATH`:
/// an npm CLI found off `PATH` (ADR-0043 D16) runs `#!/usr/bin/env node`, and
/// `node` is next to it, also off `PATH` (#353). A `PATH` set on `cmd` is the base.
pub fn program_dir_on_path(cmd: &mut Command) {
    let program = PathBuf::from(cmd.get_program());
    let Some(dir) = program.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return;
    };
    let existing = cmd
        .get_envs()
        .find(|(key, _)| key.eq_ignore_ascii_case(std::ffi::OsStr::new("PATH")))
        .and_then(|(_, value)| value)
        .map(std::ffi::OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"));
    if let Some(widened) = path_with_dir(dir, existing.as_deref()) {
        cmd.env("PATH", widened);
    }
}

/// Give a child a hidden console on Windows, so no window flashes; a no-op
/// elsewhere. Only for a child whose output is captured or redirected.
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _: &mut Command = cmd;
}

/// Keep this process's standard handles out of the children it starts from now
/// on; a no-op off Windows. A detached daemon held the stdout pipe of its caller
/// (measured 2026-10-02: `ralphy daemon restart | tail` waited for the daemon).
/// A handle a `Command` passes is a duplicate, so it still works.
#[cfg_attr(
    windows,
    expect(unsafe_code, reason = "FFI: GetStdHandle and SetHandleInformation")
)]
pub fn keep_std_handles_from_children() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{
            SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: GetStdHandle takes a constant and returns a handle this
            // process owns, or null or INVALID_HANDLE_VALUE, which are skipped.
            // SetHandleInformation changes only the flag of that handle.
            unsafe {
                let handle = GetStdHandle(which);
                if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                    continue;
                }
                if SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) == 0 {
                    tracing::debug!(
                        error = %std::io::Error::last_os_error(),
                        "could not keep a standard handle from child processes"
                    );
                }
            }
        }
    }
}

/// Kill `child` and every descendant, then reap it. `child.kill()` alone leaves
/// a grandchild that keeps an inherited pipe open and blocks its reader. Works
/// when `child` has already exited. Best-effort.
pub fn kill_tree(child: &mut Child) {
    kill_tree_by_pid(child.id());
    // The child may have exited already; the tree kill above is the one that counts.
    if let Err(e) = child.kill() {
        tracing::debug!("kill of the child failed: {e}");
    }
    // The child may have been reaped already; this call only prevents a zombie.
    if let Err(e) = child.wait() {
        tracing::debug!("reap of the child failed: {e}");
    }
}

/// Kill the process tree rooted at `pid`, a child not owned as a [`Child`].
/// Windows walks the table by parent pid, Unix signals the group `pid` leads;
/// both reach the orphans of a dead root, which `taskkill /F /T` did not (#156).
#[cfg_attr(
    unix,
    expect(unsafe_code, reason = "FFI: libc::kill on the process group")
)]
pub fn kill_tree_by_pid(pid: u32) {
    #[cfg(windows)]
    kill_tree_windows(pid);
    #[cfg(unix)]
    {
        // Not `kill(1)`: procps-ng read `-<pgid>` as an option and killed our own
        // group. A pgid of 1 is refused: `kill(-1, ...)` signals every process.
        if pid > 1 {
            // Negating a u32 that fits pid_t: pids are well under i32::MAX.
            let pgid = -pid.cast_signed();
            // SAFETY: kill(2) takes plain integers and touches no memory.
            unsafe { libc::kill(pgid, libc::SIGKILL) };
        }
    }
}

/// The Windows arm of [`kill_tree_by_pid`]: snapshot the process table and end
/// each pid [`tree_to_kill`] gives, root first so it cannot start new children.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "FFI: Toolhelp32 snapshot walk and TerminateProcess"
)]
fn kill_tree_windows(root: u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    let mut table: Vec<(u32, u32)> = Vec::new();
    // SAFETY: the snapshot handle is checked before use and closed once;
    // `entry` is a zeroed PROCESSENTRY32 with `dwSize` set, as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize =
            u32::try_from(size_of::<PROCESSENTRY32>()).expect("the struct size fits in u32");
        if Process32First(snap, &raw mut entry) != 0 {
            loop {
                table.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32Next(snap, &raw mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }

    let doomed = tree_to_kill(root, &table, std::process::id(), pid::process_created);
    for pid in doomed {
        // SAFETY: the handle is checked for null before use and closed once.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
}

/// The pids a tree kill rooted at `root` ends: `root`, then breadth-first each
/// process whose recorded parent is in the set. Windows reuses pids, so `me` is
/// never in the set nor walked through (a reused launcher pid made the walk end
/// the daemon: three deaths, measured 2026-10-05), and a process created before
/// its recorded parent is skipped; an unknown time keeps the edge, so the
/// orphans of a dead root still end. The set is also the visited list.
#[cfg(any(windows, test))]
fn tree_to_kill(
    root: u32,
    table: &[(u32, u32)],
    me: u32,
    created: impl Fn(u32) -> Option<u64>,
) -> Vec<u32> {
    if root == me {
        return Vec::new();
    }
    let mut doomed: Vec<u32> = vec![root];
    let mut queue: Vec<u32> = vec![root];
    while let Some(parent) = queue.pop() {
        let parent_created = created(parent);
        for &(pid, ppid) in table {
            if ppid != parent
                || pid == parent
                || pid == me
                || doomed.contains(&pid)
                || created_before_parent(parent_created, created(pid))
            {
                continue;
            }
            doomed.push(pid);
            queue.push(pid);
        }
    }
    doomed
}

/// Whether a child was created before its recorded parent; an unknown time is `false`.
#[cfg(any(windows, test))]
fn created_before_parent(parent_created: Option<u64>, child_created: Option<u64>) -> bool {
    matches!(
        (parent_created, child_created),
        (Some(parent_at), Some(child_at)) if child_at < parent_at
    )
}

/// A home-scoped path: `override_base/tail` when a vendor `$XXX_HOME` is set,
/// else `home_dir()/home_rel/tail`; `None` when neither is known.
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

/// Resolve `name` to the program a headless `Command` spawns, such as an npm
/// shim `opencode.cmd` that `Command::new` does not find, else the bare `name`.
/// Only `std`'s `Command` escapes a `.cmd` command line; `ralphy-pty` refuses
/// a batch program whose command line cmd.exe would not read as text.
pub fn resolve_program(name: &str) -> std::ffi::OsString {
    locate_program(name).map_or_else(|| name.into(), PathBuf::into_os_string)
}

/// Locate `name` on `PATH`, then in `~/.local/bin` and an nvm bin. Both
/// [`resolve_program`] and `ralphy init` use it, so they never disagree.
pub fn locate_program(name: &str) -> Option<PathBuf> {
    locate_program_with(
        name,
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        home_dir(),
    )
}

/// [`locate_program`] over its inputs.
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
    nvm_candidates(&home, name)
        .into_iter()
        .find(|c| is_executable_file(c))
}

/// Whether `dir` is a Windows drive in WSL (`/mnt/<letter>/…`): a Windows CLI
/// there cannot run as a Linux program (ADR-0043 D16).
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
            if drive.to_str().is_some_and(|d| d.len() == 1
                && d.chars().all(|c| c.is_ascii_alphabetic())))
}

/// Every `<home>/.nvm/versions/node/*/bin/<name>`, newest version by number
/// first (ADR-0043 D16): a string sort puts `v10` before `v9`.
pub fn nvm_candidates(home: &Path, name: &str) -> Vec<PathBuf> {
    let versions = home.join(".nvm").join("versions").join("node");
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };
    let mut found: Vec<(Vec<u64>, PathBuf)> = entries
        .filter_map(Result::ok)
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
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The numbers of a version name like `v22.22.2`; a part that is not a number is `0`.
fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches('v')
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Whether `path` is git-bash as `cursor-agent` detects it, `/git.*bash\.exe$/i`
/// (ADR-0042 D20). `System32\bash.exe` is the WSL launcher, not git-bash.
pub fn is_git_bash_shape(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    lower
        .strip_suffix("bash.exe")
        .is_some_and(|head| head.contains("git"))
}

/// Locate git-bash, which `cursor-agent` prefers (ADR-0042 D20): the two Git
/// roots, then an [`is_git_bash_shape`] hit on `PATH`. `None` leaves `SHELL`.
pub fn locate_git_bash_with(
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
    program_files: Option<PathBuf>,
    program_files_x86: Option<PathBuf>,
) -> Option<PathBuf> {
    // `Git\cmd\`, sometimes the only Git folder on PATH, holds no bash.
    for root in [program_files, program_files_x86].into_iter().flatten() {
        let cand = root.join("Git").join("bin").join("bash.exe");
        if cand.is_file() {
            return Some(cand);
        }
    }
    find_program("bash", path_var, pathext).filter(|p| is_git_bash_shape(p))
}

/// [`locate_git_bash_with`] over the real environment.
pub fn locate_git_bash() -> Option<PathBuf> {
    locate_git_bash_with(
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
    )
}

/// The home directory: `USERPROFILE`, else `HOME`.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Whether `p` is a regular file the OS would run: on Unix it needs an execute
/// bit; on Windows `PATHEXT` decides elsewhere.
#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Search `path_var` for `name`, trying each `PATHEXT` extension on Windows.
pub fn find_program(
    name: &str,
    path_var: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let path_var = path_var?;
    let exts = pathext_list(pathext);
    for dir in std::env::split_paths(&path_var) {
        if cfg!(unix) && is_windows_mount_path(&dir) {
            continue;
        }
        let direct = dir.join(name);
        if direct_runnable(&direct, &exts) {
            return Some(direct);
        }
        if let Some(found) = with_pathext(&dir, name, &exts) {
            return Some(found);
        }
    }
    None
}

/// The `PATHEXT` extensions to try, empty off Windows; unset or not UTF-8 is `.EXE`.
fn pathext_list(pathext: Option<std::ffi::OsString>) -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    pathext
        .and_then(|p| p.into_string().ok())
        .unwrap_or_else(|| ".EXE".into())
        .split(';')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether `direct` runs as named. On Windows its extension must be in `PATHEXT`:
/// npm's extensionless shim gives "not a valid Win32 application" (os error 193).
fn direct_runnable(direct: &Path, exts: &[String]) -> bool {
    if !cfg!(windows) {
        return is_executable_file(direct);
    }
    direct.is_file()
        && direct
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                exts.iter()
                    .any(|x| x.trim_start_matches('.').eq_ignore_ascii_case(e))
            })
}

/// The first `name` + extension file in `dir`, trying `exts` in order.
fn with_pathext(dir: &Path, name: &str, exts: &[String]) -> Option<PathBuf> {
    exts.iter()
        .map(|ext| dir.join(format!("{name}.{}", ext.trim_start_matches('.'))))
        .find(|cand| cand.is_file())
}

#[cfg(test)]
mod tests;
