//! Building the headless `gemini` invocation: resolving a binary npm installs
//! without an executable extension (ADR-0043 D16), fixing the argv that refuses
//! this vendor's default blast radius (D2/D12), and scrubbing every inherited
//! authentication variable outside an explicit allowlist (D7).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;

/// Mint the session id Ralphy hands the CLI with `--session-id`. A v4 UUID, so the
/// session is addressable before the child is spawned.
pub(crate) fn mint_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The vendor's binary name — also its `PATH` name, unlike Cursor's.
const NAME: &str = "gemini";

/// The vendor's stdin ceiling: the CLI reads at most 8 MiB from standard input and
/// silently truncates beyond it (ADR-0043 D2). A charter that would cross this
/// must fail loudly — a truncated charter produces a plausible-looking session
/// that was never given its rules.
pub(crate) const MAX_STDIN_BYTES: usize = 8 * 1024 * 1024;

/// Locate the Gemini CLI against the real environment. `None` means the vendor is
/// not installed — `ralphy init`'s gate reports presence through this.
///
/// npm installs it on Windows as an extensionless shim plus `gemini.cmd`, and
/// under a version-managed Node on Linux into a path a non-login shell omits;
/// both cases are handled inside `locate_program` (ADR-0043 D16).
pub fn locate_gemini() -> Option<PathBuf> {
    ralphy_proc_util::locate_program(NAME)
}

/// What a `Command` is constructed with. Falls back to the bare name so the spawn
/// failure names the vendor rather than an empty path.
pub(crate) fn resolve_gemini_program() -> OsString {
    locate_gemini()
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| NAME.into())
}

/// Refuse a prompt the vendor would silently truncate (D2).
///
/// The check is on BYTES, not characters: the ceiling is a read limit, and a
/// non-ASCII charter is longer in bytes than in `chars()`.
pub(crate) fn check_stdin_ceiling(prompt: &str) -> Result<()> {
    if prompt.len() > MAX_STDIN_BYTES {
        anyhow::bail!(
            "the charter is {} bytes, over the gemini CLI's 8 MiB stdin ceiling — \
             it would arrive truncated and the session would run without its rules",
            prompt.len()
        );
    }
    Ok(())
}

/// Build the headless `gemini` command both `plan` and `execute` go through.
///
/// The charter is NEVER on argv: the assembled planning charter is ~24 KB before
/// any issue body against a Windows argv ceiling of ~32 KB, and stdin is
/// **prepended** to any argv prompt with a blank line between (D2 — the vendor's
/// own documentation states this backwards; the shipped source is authoritative).
/// So no prompt flag appears here at all: not `-p`/`--prompt`, not
/// `-i`/`--prompt-interactive`.
///
/// `--approval-mode yolo` is the non-interactive autonomy this run needs; the
/// deprecated `--yolo` spelling is not used. The vendor's native **plan mode** is
/// refused by absence: it writes its plan into a vendor-private directory whatever
/// it is instructed and whatever a policy permits, so Ralphy's planner writes
/// `.ralphy/plan.md` itself (D12).
///
/// `--skip-trust` bypasses the interactive folder-trust prompt, which is fatal
/// headless. `--output-format stream-json` selects the record stream the fold
/// reads. `--policy` carries Ralphy's own policy document, which is sovereign over
/// the user tier (D5). `--resume`/`--session-file` are absent: this adapter drives
/// one turn per invocation.
///
/// `auth_type` is the operator's declared authentication mode, read from their
/// `settings.json` as a non-secret pointer — it selects D7's allowlist, and
/// nothing else about their root is consulted.
pub(crate) fn build_gemini_command(
    session_id: &str,
    model: Option<&str>,
    work_dir: &Path,
    home: &Path,
    policy: &Path,
    auth_type: Option<&str>,
) -> Command {
    let mut cmd = Command::new(resolve_gemini_program());
    cmd.current_dir(work_dir)
        .arg("--approval-mode")
        .arg("yolo")
        .arg("--skip-trust")
        .arg("--session-id")
        .arg(session_id)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--policy")
        .arg(policy)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    apply_auth_env(&mut cmd, std::env::vars().map(|(k, _)| k), auth_type, home);
    cmd
}

/// The authentication variables that MAY be forwarded, per the operator's declared
/// auth mode (ADR-0043 D7).
///
/// An allowlist rather than a denylist because the failure direction matters: an
/// inherited `GOOGLE_GENAI_USE_VERTEXAI=true` from unrelated cloud tooling silently
/// redirects the run to another account and another bill, and the run still looks
/// green. An unknown or absent auth mode forwards nothing — the vendor then answers
/// with its own exit 41 and its own sentence (D6), which is the actionable failure.
pub(crate) fn allowed_auth_vars(auth_type: Option<&str>) -> &'static [&'static str] {
    match auth_type {
        Some("gemini-api-key") => &["GEMINI_API_KEY"],
        Some("vertex-ai") => &[
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_CLOUD_LOCATION",
            // Application Default Credentials: a Vertex operator authenticating
            // with a service-account key file has this as their ONLY credential
            // pointer. Scrubbing it drops them to exit 41 on every run — the
            // "wrongly dropped" direction the allowlist is supposed to avoid.
            "GOOGLE_APPLICATION_CREDENTIALS",
        ],
        // `oauth-personal` and `cloud-shell` authenticate out of band; no
        // environment variable is theirs to forward.
        _ => &[],
    }
}

/// Every authentication-relevant name in `parent` that is NOT in `keep` — what the
/// child's environment must have removed.
///
/// The namespaces are matched by PREFIX (`GEMINI_`, `GOOGLE_GENAI_`,
/// `GOOGLE_CLOUD_`) plus two exact names, so a variable the vendor adds later is
/// scrubbed by default rather than forwarded by default.
///
/// Matching is CASE-INSENSITIVE, unconditionally. Windows environment lookup is
/// case-insensitive — Node resolves `process.env.GOOGLE_GENAI_USE_VERTEXAI`
/// against a variable stored as `google_genai_use_vertexai` — so a case-sensitive
/// filter would let exactly that spelling survive the scrub while remaining fully
/// effective for the child. Harmless on Unix, where such a name is a different
/// variable the vendor does not read.
pub(crate) fn scrubbed_names<'a>(
    parent: impl Iterator<Item = &'a str>,
    keep: &[&str],
) -> Vec<String> {
    parent
        .filter(|n| {
            let u = n.to_ascii_uppercase();
            u.starts_with("GEMINI_")
                || u.starts_with("GOOGLE_GENAI_")
                || u.starts_with("GOOGLE_CLOUD_")
                || u == "GOOGLE_API_KEY"
                || u == "GOOGLE_APPLICATION_CREDENTIALS"
        })
        .filter(|n| {
            let u = n.to_ascii_uppercase();
            !keep.iter().any(|k| k.eq_ignore_ascii_case(&u))
        })
        .map(str::to_string)
        .collect()
}

/// Apply D7's allowlist to `cmd`, then point the child at Ralphy's owned root.
///
/// **Never `env_clear()`**: the child is a Node process and needs `PATH`,
/// `SystemRoot`, `APPDATA` and friends to start at all. Removal is per name.
///
/// `GEMINI_CLI_HOME` is set LAST and unconditionally — it is the D4 containment,
/// and it must survive the scrub that its own `GEMINI_` prefix would otherwise
/// catch.
pub(crate) fn apply_auth_env<I, S>(
    cmd: &mut Command,
    parent: I,
    auth_type: Option<&str>,
    home: &Path,
) where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let keep = allowed_auth_vars(auth_type);
    let names: Vec<String> = parent.map(|s| s.as_ref().to_string()).collect();
    for name in scrubbed_names(names.iter().map(String::as_str), keep) {
        cmd.env_remove(&name);
    }
    cmd.env("GEMINI_CLI_HOME", home);
}

/// Every character (other than `.`, handled separately below)
/// `AT_COMMAND_PATH_REGEX_SOURCE` (`chunk-FAVXT6HW.js:66616`,
/// `(?:(?:"(?:[^"]*)")|(?:\\.|[^ \t\n\r,;!?()\[\]{}.]|\.(?!$|[ \t\n\r])))+`)
/// terminates an UNQUOTED `@`-match at, beyond the space this function
/// originally escaped alone: any of these ends the match unless itself
/// backslash-escaped (the regex's own `\\.` alternative).
const POSIX_AT_TERMINATORS: [char; 14] = [
    ' ', '\t', '\n', '\r', ',', ';', '!', '?', '(', ')', '[', ']', '{', '}',
];

/// Whether an UNESCAPED `.` at index `i` in `chars` would still end the
/// regex's match: only when it is the last character, or immediately
/// followed by whitespace — the regex's own `\.(?!$|[ \t\n\r])` alternative
/// admits every other bare period (an ordinary `name.ext` needs no escaping).
fn posix_period_needs_escape(chars: &[char], i: usize) -> bool {
    matches!(chars.get(i + 1), None | Some(' ' | '\t' | '\n' | '\r'))
}

/// Format `path` as an `@`-reference the vendor's `resolveAtCommandPath`
/// resolves into an inline image attachment (ADR-0043 D14).
///
/// The escaping is per-platform because `unescapePath` (vendor bundle
/// `chunk-AWR3APYV.js:243431`) strips surrounding double quotes ONLY on
/// `win32` and otherwise applies `replace(/\\(.)/g, "$1")` to every
/// backslash-escaped character. On POSIX every char in
/// [`POSIX_AT_TERMINATORS`] is backslash-escaped, plus a bare trailing `.`
/// (see [`posix_period_needs_escape`]) — not only the space alone: a fetched
/// attachment keeps its ORIGINAL upload filename verbatim
/// (`github/attachments.rs::filename_from_url`), and parentheses, commas,
/// semicolons and brackets are valid unencoded characters in a URL path
/// segment — any of them left unescaped truncates the regex match silently,
/// with no error: `handleAtCommand`'s zero-match branch just returns the
/// query text unresolved. Escaping a character the regex would have admitted
/// unescaped is harmless: `unescapePath` strips the backslash either way.
pub(crate) fn at_reference(path: &Path, windows: bool) -> String {
    if windows {
        format!("@\"{}\"", path.display())
    } else {
        let raw = path.display().to_string();
        let chars: Vec<char> = raw.chars().collect();
        let mut escaped = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let needs_escape = POSIX_AT_TERMINATORS.contains(&c)
                || (c == '.' && posix_period_needs_escape(&chars, i));
            if needs_escape {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        format!("@{escaped}")
    }
}

/// The block appended after `req.attachments_manifest` delivering each fetched
/// attachment as an inline `@`-reference (ADR-0043 D14, this vendor only — the
/// manifest itself stays vendor-neutral, ADR-0025 §6).
pub(crate) fn attachment_block(image_paths: &[PathBuf]) -> String {
    if image_paths.is_empty() {
        return String::new();
    }
    let mut block = String::from(
        "\n\n## Attached images\n\nEach path below is delivered to you as an inline \
         image — describe it from the image itself, do not read it with a tool.\n\n",
    );
    for p in image_paths {
        block.push_str(&at_reference(p, cfg!(windows)));
        block.push('\n');
    }
    block
}

/// Each fetched attachment's parent directory, deduplicated, order preserved —
/// what must be passed to `--include-directories` so the vendor's workspace
/// boundary check (`config.validatePathAccess`) admits the `@`-reference.
pub(crate) fn attachment_dirs(image_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for p in image_paths {
        if let Some(parent) = p.parent() {
            if !dirs.iter().any(|d: &PathBuf| d == parent) {
                dirs.push(parent.to_path_buf());
            }
        }
    }
    dirs
}

/// Push one `--include-directories <dir>` pair per entry in `dirs`.
pub(crate) fn add_include_directories(cmd: &mut Command, dirs: &[PathBuf]) {
    for dir in dirs {
        cmd.arg("--include-directories").arg(dir);
    }
}

#[cfg(test)]
mod tests;
