//! Recursive deletes: which commands are one, and which targets they may touch.

use std::sync::LazyLock;

use regex::Regex;

use super::wrappers::{self, command_name, Shell};
use super::{GuardContext, Judge};

/// PowerShell's .NET recursive delete, `[System.IO.Directory]::Delete(path,
/// $true)`: its path is the first argument.
static DOTNET_DELETE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\[(?:system\.)?io\.directory\]::delete\(",
        r#"\s*['"]?([^,'"()]+?)['"]?\s*,\s*\$true\s*\)"#,
    ))
    .expect("valid guard regex")
});

/// The first path of a .NET recursive delete in `cmd` that a recursive
/// delete may not touch.
pub(super) fn dotnet_delete_target(cmd: &str, ctx: &GuardContext) -> Option<String> {
    DOTNET_DELETE
        .captures_iter(cmd)
        .map(|c| c[1].trim().to_owned())
        .find(|path| !recursive_delete_target_allowed(path, ctx))
}

/// The shape of delete command a segment turned out to be.
pub(super) enum DeleteKind {
    /// `rm` with combined/separate recursive+force flags.
    Rm,
    /// PowerShell `Remove-Item -Recurse`.
    RemoveItem,
    /// cmd.exe `del /s` or `rmdir /s`.
    DelRmdir,
}

/// The first target of the command `words` that a recursive delete may not
/// touch, or `None` when it is not a recursive delete or every target is
/// allowed. A target must stay inside the worktree (`ctx.cwd`), the system
/// temp dir, or `/tmp`; a relative path (no `..`) resolves under the
/// worktree; `~`, `$VAR` and `%VAR%` cannot be resolved here.
pub(super) fn first_disallowed_target(words: &[String], judge: Judge) -> Option<String> {
    let (kind, args) = classify_delete(words, judge.shell)?;
    args.iter()
        .find(|tok| !is_delete_flag(&kind, tok) && !recursive_delete_target_allowed(tok, judge.ctx))
        .cloned()
}

/// Is `tok` an option of the delete command, not a target?
fn is_delete_flag(kind: &DeleteKind, tok: &str) -> bool {
    match kind {
        DeleteKind::Rm | DeleteKind::RemoveItem => tok.starts_with('-'),
        // cmd.exe switches: `/s`, `/q`, `/a:h`, … (also tolerate `-`).
        DeleteKind::DelRmdir => {
            tok.starts_with('-')
                || (tok.len() <= 4
                    && tok.strip_prefix('/').is_some_and(|switch| {
                        switch
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == ':')
                    }))
        }
    }
}

/// Classify a command's words as a recursive delete, returning the kind and
/// the argument words (everything after the command word). The wrapper words
/// in front (`sudo`, `env`, …) are already skipped (`wrappers::what_runs`).
pub(super) fn classify_delete(words: &[String], shell: Shell) -> Option<(DeleteKind, &[String])> {
    let cmd = command_name(words.first()?);
    let args = &words[1..];
    let kind = powershell_delete(&cmd, args, shell).or_else(|| shell_delete(&cmd, args))?;
    Some((kind, args))
}

/// `Remove-Item` and its PowerShell aliases with `-Recurse`, written in full
/// or cut short as PowerShell allows. In a POSIX shell `rm -r` is the POSIX
/// flag, judged by [`shell_delete`], so there an alias needs at least
/// `-re`; in PowerShell `rm -r` is `Remove-Item -Recurse`.
fn powershell_delete(cmd: &str, args: &[String], shell: Shell) -> Option<DeleteKind> {
    let shortest = match (cmd, shell) {
        ("remove-item" | "ri", _)
        | ("rm" | "del" | "erase" | "rd" | "rmdir", Shell::PowerShell) => 2,
        ("rm" | "del" | "erase" | "rd" | "rmdir", Shell::Posix) => 3,
        _ => return None,
    };
    args.iter()
        .any(|t| t.len() >= shortest && wrappers::is_prefix_of(&t.to_lowercase(), "-recurse"))
        .then_some(DeleteKind::RemoveItem)
}

/// A POSIX `rm` that is recursive AND forced (parity with the original
/// rule: `rm -r` alone still prompts per file), or cmd.exe `del /s` and
/// `rmdir /s`.
fn shell_delete(cmd: &str, args: &[String]) -> Option<DeleteKind> {
    match cmd {
        "rm" => {
            let flags: Vec<&str> = args
                .iter()
                .filter(|t| t.starts_with('-'))
                .map(String::as_str)
                .collect();
            let has = |short: char, long: &str| {
                flags
                    .iter()
                    .any(|f| *f == long || (!f.starts_with("--") && f.contains(short)))
            };
            (has('r', "--recursive") && has('f', "--force")).then_some(DeleteKind::Rm)
        }
        "del" | "rmdir" | "rd" if args.iter().any(|t| t.eq_ignore_ascii_case("/s")) => {
            Some(DeleteKind::DelRmdir)
        }
        _ => None,
    }
}

/// Is `raw` an acceptable target for a recursive delete? Allowed: relative
/// paths without `..` (they resolve under the worktree), and absolute paths
/// under `ctx.cwd`, `ctx.temp_dir`, or `/tmp`.
fn recursive_delete_target_allowed(raw: &str, ctx: &GuardContext) -> bool {
    let p = raw.replace('\\', "/").to_lowercase();
    let p = p.trim_end_matches('/');
    // `/` (and `//`) trims to nothing: it is the root, not a relative path.
    if p.is_empty() {
        return false;
    }
    if p.split('/').any(|seg| seg == "..") {
        return false;
    }
    if p.starts_with(['~', '$', '%']) {
        return false;
    }
    let is_drive_absolute =
        p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic();
    if p.starts_with('/') || is_drive_absolute {
        return under(p, &ctx.cwd) || under(p, &ctx.temp_dir) || under(p, "/tmp");
    }
    true
}

/// Is normalised path `p` equal to or below normalised directory `dir`?
fn under(p: &str, dir: &str) -> bool {
    !dir.is_empty() && (p == dir || p.starts_with(&format!("{dir}/")))
}
