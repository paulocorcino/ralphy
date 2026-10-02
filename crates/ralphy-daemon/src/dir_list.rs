//! The folder lister behind the `dir.list` verb: one level of directory names,
//! the only read outside a registered repo root (ADR-0036 amendment "the
//! registry verbs" §3 and §5). Each limit here is part of the security
//! boundary of that exception and has its own test.
//!
//! Pure sync: the route runs [`list`] in `spawn_blocking`. No `git` process
//! runs; the `repo` bits come from file metadata only.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf, Prefix};

use serde::Serialize;

use crate::registry::RegistryStore;

/// At most this many entries return; `more` counts the rest.
pub const LIMIT: usize = 200;

/// The longest `path` accepted, in bytes — the same bound `project.add` uses.
pub const MAX_PATH_BYTES: usize = 4096;

/// One child folder of the listed folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub name: String,
    /// A `.git` entry (file or directory) exists in this folder.
    pub repo: bool,
    /// This folder is in the daemon's registry.
    pub added: bool,
    /// Set when the folder cannot be read; the other bits are then `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The listed folder itself (§5 of the amendment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirInfo {
    pub path: String,
    /// The nearest folder, this one or an ancestor, that holds a `.git` entry.
    pub root: Option<String>,
    /// `root`, or the listed folder when there is no root, is registered.
    pub added: bool,
}

/// The reply of one `dir.list` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub more: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    pub dir: DirInfo,
}

/// Why a `dir.list` call was refused as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TooLong,
    ControlCharacter,
    Relative,
    Network,
    NotFound,
    Unreadable,
}

impl Refusal {
    /// The message of the error frame.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::TooLong => "the path is too long",
            Refusal::ControlCharacter => "the path holds a control character",
            Refusal::Relative => "the path must be absolute",
            Refusal::Network => "network paths are not supported",
            Refusal::NotFound => "this folder does not exist",
            Refusal::Unreadable => "cannot read this folder",
        }
    }
}

/// Whether `path` names a network location: on Windows a UNC path
/// (`\\server\share`, its verbatim form, or a `\\.\` device path). Reading one
/// makes Windows authenticate over SMB to the named server, which sends the
/// user's NTLM hash there. Always `false` off Windows.
pub fn is_network_path(path: &Path) -> bool {
    match path.components().next() {
        Some(Component::Prefix(p)) => matches!(
            p.kind(),
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(..)
        ),
        _ => false,
    }
}

/// List the folder that `path` names. `path` is what the operator typed:
///
/// - `None` lists `start`: the parent folder shared by most registered
///   projects, else `home`.
/// - A text that ends with a separator lists that folder.
/// - Any other text lists its parent, filtered by the last component.
/// - A leading `~` is `home`. On Windows `""` lists the drives and `D:` is
///   `D:\`.
pub fn list(
    path: Option<&str>,
    registry: &RegistryStore,
    home: Option<&Path>,
) -> Result<Listing, Refusal> {
    let Some(text) = path else {
        let start = start_folder(registry, home).ok_or(Refusal::NotFound)?;
        let mut listing = list_folder(&with_separator(&start), "", registry)?;
        listing.start = Some(start);
        return Ok(listing);
    };
    if text.len() > MAX_PATH_BYTES {
        return Err(Refusal::TooLong);
    }
    if text.chars().any(char::is_control) {
        return Err(Refusal::ControlCharacter);
    }
    #[cfg(windows)]
    if text.is_empty() {
        return Ok(drives());
    }
    let text = expand_home(text, home);
    let (folder, prefix) = split(&text);
    let folder = drive_root(folder);
    let folder_path = Path::new(&folder);
    if is_network_path(folder_path) {
        return Err(Refusal::Network);
    }
    if !folder_path.is_absolute() {
        return Err(Refusal::Relative);
    }
    list_folder(&folder, prefix, registry)
}

/// `~` and `~/…` (or `~\…`) on the daemon user's home. Any other text is
/// returned unchanged.
fn expand_home(text: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return text.to_string();
    };
    let Some(rest) = text.strip_prefix('~') else {
        return text.to_string();
    };
    if rest.is_empty() {
        return home.to_string_lossy().into_owned();
    }
    if rest.starts_with(is_separator) {
        return format!(
            "{}{rest}",
            home.to_string_lossy().trim_end_matches(is_separator)
        );
    }
    text.to_string()
}

fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Split the typed text into the folder to list and the name prefix.
fn split(text: &str) -> (&str, &str) {
    match text.rfind(is_separator) {
        Some(i) => (&text[..=i], &text[i + 1..]),
        None => (text, ""),
    }
}

/// On Windows `D:` alone is drive-relative; the operator means `D:\`.
fn drive_root(folder: &str) -> String {
    let bytes = folder.as_bytes();
    if cfg!(windows) && bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return format!("{folder}\\");
    }
    folder.to_string()
}

fn with_separator(folder: &str) -> String {
    if folder.ends_with(is_separator) {
        folder.to_string()
    } else {
        format!("{folder}{}", std::path::MAIN_SEPARATOR)
    }
}

/// The name filter, and the hidden-folder rule: a hidden folder is kept only
/// when the prefix starts with `.` or names it exactly (amendment §5).
fn keep(name: &str, hidden: bool, prefix: &str) -> bool {
    let lower = name.to_lowercase();
    let wanted = prefix.to_lowercase();
    if !lower.starts_with(&wanted) {
        return false;
    }
    !hidden || prefix.starts_with('.') || lower == wanted
}

fn list_folder(folder: &str, prefix: &str, registry: &RegistryStore) -> Result<Listing, Refusal> {
    let dir = Path::new(folder);
    let read = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Refusal::NotFound,
        _ => Refusal::Unreadable,
    })?;
    let mut names = Vec::new();
    for item in read {
        // One child that vanished or cannot be stat'ed is skipped: it is not a
        // folder the operator can pick.
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if !is_folder(&item) {
            continue;
        }
        if keep(&name, is_hidden(&name, &item), prefix) {
            names.push(name);
        }
    }
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    let more = names.len().saturating_sub(LIMIT);
    names.truncate(LIMIT);

    let registered = registered_roots(registry);
    let entries = names
        .into_iter()
        .map(|name| entry(dir.join(&name), name, &registered))
        .collect();
    Ok(Listing {
        entries,
        more,
        start: None,
        dir: dir_info(dir, &registered),
    })
}

/// A directory, or a symlink or junction that resolves to one.
fn is_folder(item: &std::fs::DirEntry) -> bool {
    match item.file_type() {
        Ok(t) if t.is_dir() => true,
        Ok(t) if t.is_symlink() => std::fs::metadata(item.path()).is_ok_and(|m| m.is_dir()),
        _ => false,
    }
}

#[cfg(windows)]
fn is_hidden(name: &str, item: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    name.starts_with('.')
        || item
            .metadata()
            .is_ok_and(|m| m.file_attributes() & (HIDDEN | SYSTEM) != 0)
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _item: &std::fs::DirEntry) -> bool {
    name.starts_with('.')
}

fn entry(path: PathBuf, name: String, registered: &HashSet<PathBuf>) -> Entry {
    if std::fs::read_dir(&path).is_err() {
        return Entry {
            name,
            repo: false,
            added: false,
            error: Some(Refusal::Unreadable.message().to_string()),
        };
    }
    Entry {
        repo: has_git(&path),
        added: is_registered(&path, registered),
        name,
        error: None,
    }
}

fn has_git(folder: &Path) -> bool {
    std::fs::symlink_metadata(folder.join(".git")).is_ok()
}

fn registered_roots(registry: &RegistryStore) -> HashSet<PathBuf> {
    registry
        .repos
        .values()
        .filter_map(|e| std::fs::canonicalize(&e.path).ok())
        .collect()
}

fn is_registered(folder: &Path, registered: &HashSet<PathBuf>) -> bool {
    std::fs::canonicalize(folder).is_ok_and(|c| registered.contains(&c))
}

fn dir_info(dir: &Path, registered: &HashSet<PathBuf>) -> DirInfo {
    let root = std::fs::canonicalize(dir).ok().and_then(|canon| {
        canon
            .ancestors()
            .find(|a| has_git(a))
            .map(Path::to_path_buf)
    });
    let added = match &root {
        Some(r) => registered.contains(r),
        None => is_registered(dir, registered),
    };
    DirInfo {
        path: dir.to_string_lossy().into_owned(),
        root: root.map(|r| plain(&r)),
        added,
    }
}

/// A canonical path without the Windows verbatim prefix, which no shell
/// accepts. The same rule as `RepoEntry::root`.
fn plain(canon: &Path) -> String {
    let s = canon.to_string_lossy();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").unwrap_or(&s).to_string(),
    }
}

/// The parent folder shared by most reachable registered projects (a tie goes
/// to the one that sorts first), else `home`. Computed on each call; nothing
/// is stored.
fn start_folder(registry: &RegistryStore, home: Option<&Path>) -> Option<String> {
    let mut counts: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for e in registry.repos.values() {
        let Ok(canon) = std::fs::canonicalize(&e.path) else {
            continue;
        };
        if let Some(parent) = canon.parent() {
            *counts.entry(parent.to_path_buf()).or_default() += 1;
        }
    }
    let best = counts
        .iter()
        .max_by(|(a, x), (b, y)| x.cmp(y).then(b.cmp(a)))
        .map(|(p, _)| plain(p));
    best.or_else(|| home.map(|h| h.to_string_lossy().into_owned()))
}

/// The drive letters, as entries named `C:`. Windows only.
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "FFI: GetLogicalDrives reads the drive bitmask and touches no drive"
)]
fn drives() -> Listing {
    // SAFETY: GetLogicalDrives takes no argument and returns a bitmask.
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    let entries = (0u8..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| Entry {
            name: format!("{}:", char::from(b'A' + i)),
            repo: false,
            added: false,
            error: None,
        })
        .collect();
    Listing {
        entries,
        more: 0,
        start: None,
        dir: DirInfo {
            path: String::new(),
            root: None,
            added: false,
        },
    }
}

#[cfg(test)]
mod tests;
