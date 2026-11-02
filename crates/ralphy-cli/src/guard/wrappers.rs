//! What a command segment runs once the words in front of the command are
//! skipped (#810). A wrapper word (`sudo`, `env`, `time`, `nice`, `nohup`)
//! runs the command after it, and a shell given `-c` (or `cmd /c`,
//! `pwsh -Command`) runs a script of its own, so the guard judges what runs,
//! not the word that starts the segment.

/// The shell that runs a command. Some words mean different things in each:
/// `rm -r` is the POSIX flag in one and `Remove-Item -Recurse` in the other.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Shell {
    /// bash, sh, zsh, and `cmd`, whose delete words the rules read the same.
    Posix,
    PowerShell,
}

/// What one segment runs.
pub(super) enum Runs<'a> {
    /// A command: its words, from the command word on.
    Command(&'a [String]),
    /// A script that a shell runs: the guard judges it as a command of its own.
    Script(String, Shell),
    /// A script the guard cannot read (`pwsh -EncodedCommand`).
    Unreadable,
    /// `xargs` and its words: the arguments of the command come from the input.
    FromInput(&'a [String]),
}

/// What the segment of `tokens` runs.
pub(super) fn what_runs(tokens: &[String]) -> Runs<'_> {
    let words = strip_wrappers(tokens);
    let Some(first) = words.first() else {
        return Runs::Command(words);
    };
    let rest = &words[1..];
    match command_name(first).as_str() {
        "xargs" => Runs::FromInput(rest),
        "bash" | "sh" | "zsh" | "dash" => posix_script(rest)
            .map_or(Runs::Command(words), |script| {
                Runs::Script(script, Shell::Posix)
            }),
        "cmd" => cmd_script(rest).map_or(Runs::Command(words), |script| {
            Runs::Script(script, Shell::Posix)
        }),
        "powershell" | "pwsh" => powershell_script(rest).unwrap_or(Runs::Command(words)),
        _ => Runs::Command(words),
    }
}

/// The name of a command word, as the rules compare it: lower case, with no
/// path (`/bin/rm`) and no `.exe`.
pub(super) fn command_name(word: &str) -> String {
    let lower = word.to_lowercase();
    let name = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    name.strip_suffix(".exe").unwrap_or(name).to_string()
}

/// `tokens` without the wrapper words in front of the command, each with
/// the options it takes.
fn strip_wrappers(tokens: &[String]) -> &[String] {
    let mut words = tokens;
    while let Some(first) = words.first() {
        let skip = match command_name(first).as_str() {
            "sudo" | "nohup" => 1,
            "time" => 1 + options_len(&words[1..], &[]),
            "env" => 1 + env_options_len(&words[1..]),
            "nice" => 1 + options_len(&words[1..], &["-n"]),
            _ => break,
        };
        words = &words[skip.min(words.len())..];
    }
    words
}

/// The count of leading option words, where an option in `with_value` takes
/// the next word as its value.
fn options_len(words: &[String], with_value: &[&str]) -> usize {
    let mut n = 0;
    while let Some(word) = words.get(n) {
        if !word.starts_with('-') {
            break;
        }
        n += if with_value.contains(&word.as_str()) {
            2
        } else {
            1
        };
    }
    n
}

/// The count of leading `env` options and `NAME=value` words.
fn env_options_len(words: &[String]) -> usize {
    let mut n = 0;
    while let Some(word) = words.get(n) {
        if word.starts_with('-') {
            n += if matches!(word.as_str(), "-u" | "-C" | "-S") {
                2
            } else {
                1
            };
        } else if word.contains('=') {
            n += 1;
        } else {
            break;
        }
    }
    n
}

/// The script of `bash -c SCRIPT` (also `-lc`, `-ec`, …), or `None` when the
/// shell runs a file or reads its input.
fn posix_script(args: &[String]) -> Option<String> {
    let at = args
        .iter()
        .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))?;
    args.get(at + 1).cloned()
}

/// The script of `cmd /c …` or `cmd /k …`: every word after the switch.
fn cmd_script(args: &[String]) -> Option<String> {
    let at = args
        .iter()
        .position(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k"))?;
    Some(args[at + 1..].join(" "))
}

/// The script of `powershell -Command …` (any prefix of the parameter name,
/// as PowerShell takes it): every word after it. `-EncodedCommand` is a
/// script the guard cannot read.
fn powershell_script(args: &[String]) -> Option<Runs<'static>> {
    for (i, arg) in args.iter().enumerate() {
        let lower = arg.to_lowercase();
        if is_prefix_of(&lower, "-encodedcommand") || lower == "-ec" {
            return Some(Runs::Unreadable);
        }
        if is_prefix_of(&lower, "-command") {
            return Some(Runs::Script(args[i + 1..].join(" "), Shell::PowerShell));
        }
    }
    None
}

/// Is `word` a PowerShell abbreviation of the parameter `name`: a prefix of
/// it with at least one letter after the dash?
pub(super) fn is_prefix_of(word: &str, name: &str) -> bool {
    word.len() >= 2 && name.starts_with(word)
}
