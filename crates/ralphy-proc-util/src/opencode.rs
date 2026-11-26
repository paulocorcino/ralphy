//! Where `OpenCode` is installed. It lives here because the daemon imports no
//! adapter crate; the roster and the console find the binary through this module.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long `opencode --version` may run before the read gives up.
const VERSION_LIMIT: Duration = Duration::from_secs(5);

/// The program to spawn for `OpenCode`: the located path, else the bare name.
pub fn opencode_program() -> OsString {
    opencode_program_with(locate_opencode())
}

/// [`opencode_program`] over a located path.
pub fn opencode_program_with(located: Option<PathBuf>) -> OsString {
    located.map_or_else(|| "opencode".into(), PathBuf::into_os_string)
}

/// The two `OpenCode` majors Ralphy supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeMajor {
    V1,
    V2,
}

/// The version `opencode --version` printed, without a leading `v`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeVersion {
    pub major: OpenCodeMajor,
    pub text: String,
}

/// An `OpenCode` program and the version it reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedOpenCode {
    pub program: OsString,
    pub version: OpenCodeVersion,
}

/// Why the version of `OpenCode` is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenCodeVersionError {
    Unreadable { program: OsString },
    Unsupported { text: String },
}

impl fmt::Display for OpenCodeVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { program } => write!(
                f,
                "could not read the version of {}; is OpenCode installed?",
                Path::new(program).display()
            ),
            Self::Unsupported { text } => write!(
                f,
                "OpenCode version {text} is not supported; Ralphy works with OpenCode 1.x and 2.x"
            ),
        }
    }
}

impl std::error::Error for OpenCodeVersionError {}

/// The version in the first line of `opencode --version`: its last word, a
/// leading `v` removed. The major is the number before the first `.`.
pub fn parse_opencode_version(line: &str) -> Result<OpenCodeVersion, OpenCodeVersionError> {
    let word = line
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().last());
    let text = word.map(|w| w.strip_prefix('v').unwrap_or(w));
    let major = text.and_then(|t| t.split('.').next()?.parse::<u32>().ok());
    match (text, major) {
        (Some(text), Some(1)) => Ok(version(OpenCodeMajor::V1, text)),
        (Some(text), Some(2)) => Ok(version(OpenCodeMajor::V2, text)),
        (Some(text), Some(_)) => Err(OpenCodeVersionError::Unsupported {
            text: text.to_owned(),
        }),
        _ => Err(OpenCodeVersionError::Unreadable {
            program: OsString::from("opencode"),
        }),
    }
}

fn version(major: OpenCodeMajor, text: &str) -> OpenCodeVersion {
    OpenCodeVersion {
        major,
        text: text.to_owned(),
    }
}

/// Run `program --version` (cached per binary and modified time) and parse it.
pub fn read_opencode_version(program: &Path) -> Result<OpenCodeVersion, OpenCodeVersionError> {
    let unreadable = || OpenCodeVersionError::Unreadable {
        program: program.as_os_str().to_owned(),
    };
    let line = crate::hook_form::read_version(program, VERSION_LIMIT).ok_or_else(unreadable)?;
    parse_opencode_version(&line).map_err(|e| match e {
        OpenCodeVersionError::Unreadable { .. } => unreadable(),
        unsupported @ OpenCodeVersionError::Unsupported { .. } => unsupported,
    })
}

/// The program and its version; the bare name when `OpenCode` is not located.
pub fn located_opencode() -> Result<LocatedOpenCode, OpenCodeVersionError> {
    let program = opencode_program();
    let version = read_opencode_version(Path::new(&program))?;
    Ok(LocatedOpenCode { program, version })
}

/// Locate `opencode` against the real environment; `None` when it is not installed.
pub fn locate_opencode() -> Option<PathBuf> {
    locate_opencode_with(
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        crate::home_dir(),
    )
}

/// [`locate_opencode`] over its inputs: `PATH` and the usual user folders first,
/// then the installer folder `<home>/.opencode/bin`.
pub fn locate_opencode_with(
    path_var: Option<OsString>,
    pathext: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(found) = crate::locate_program_with("opencode", path_var, pathext, home.clone()) {
        return Some(found);
    }
    let mut cand = home?.join(".opencode").join("bin").join("opencode");
    if cfg!(windows) {
        cand.set_extension("exe");
    }
    crate::is_executable_file(&cand).then_some(cand)
}

#[cfg(test)]
mod tests;
