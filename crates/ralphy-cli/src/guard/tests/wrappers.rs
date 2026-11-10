//! The words in front of a command and the shells around it (#810): the
//! guard judges what runs.

use super::*;

/// Judge each row as a Bash command.
fn assert_bash_rows(rows: &[(&str, &str, bool)]) {
    let rows: Vec<(&str, Value, bool)> = rows
        .iter()
        .map(|(case, cmd, deny)| (*case, bash(cmd), *deny))
        .collect();
    assert_rows("Bash", &rows);
}

/// A wrapper word runs the command after it, so a recursive delete behind
/// `env`, `time`, `nice`, `nohup` or `sudo` is judged by its targets, and an
/// ordinary command behind them still runs.
#[test]
fn a_delete_behind_a_wrapper_word_is_judged_by_its_targets() {
    // (case, command, denied)
    assert_bash_rows(&[
        ("env", "env rm -rf /etc/nginx", true),
        (
            "env with a variable and -u",
            "env FOO=1 -u BAR rm -rf /etc",
            true,
        ),
        ("time", "time rm -rf /etc/nginx", true),
        ("nice -n", "nice -n 5 rm -rf /etc/nginx", true),
        ("nohup", "nohup rm -rf /var/lib/docker", true),
        ("sudo env", "sudo env rm -rf /etc", true),
        ("format behind time", "time format C: /q", true),
        ("env inside the worktree", "env rm -rf /repo/target", false),
        ("time relative", "time rm -rf node_modules", false),
        ("env with a variable", "env FOO=1 cargo test", false),
        ("time cargo build", "time cargo build", false),
        ("nice cargo test", "nice -n 5 cargo test", false),
        ("nohup a server", "nohup npm run dev", false),
    ]);
}

/// A shell given a script runs it, so the script is judged as a command of
/// its own: `bash -c`, `sh -c`, `cmd /c`, `powershell -c`, `pwsh -Command`,
/// nested too. A script the guard cannot read, or one nested past three
/// shells, is denied.
#[test]
fn the_script_of_a_shell_is_judged_as_a_command() {
    // (case, command, denied)
    assert_bash_rows(&[
        ("bash -c", r#"bash -c "rm -rf /etc""#, true),
        ("sh -c", "sh -c 'rm -rf /etc'", true),
        ("bash -lc", r#"bash -lc "rm -rf /etc""#, true),
        ("cmd /c rd", r"cmd /c rd /s /q C:\Users\x", true),
        (
            "powershell -c",
            r#"powershell -c "Remove-Item -Recurse -Force C:\x""#,
            true,
        ),
        (
            "pwsh -Command",
            r#"pwsh -NoProfile -Command "Remove-Item -Recurse -Force C:\x""#,
            true,
        ),
        (
            "nested bash -c",
            r#"bash -c "bash -c \"rm -rf /etc\"""#,
            true,
        ),
        (
            "git push in a script",
            r#"bash -c "git push origin main""#,
            true,
        ),
        ("format in a script", r"cmd /c format C: /q", true),
        ("four shells deep", "cmd /c cmd /c cmd /c cmd /c dir", true),
        ("encoded command", "pwsh -EncodedCommand SQBFAFgA", true),
        ("encoded, short", "powershell -enc SQBFAFgA", true),
        ("three shells deep", "cmd /c cmd /c cmd /c dir", false),
        ("bash -c under /tmp", r#"bash -c "rm -rf /tmp/x""#, false),
        ("cmd /c rd relative", "cmd /c rd /s /q build", false),
        (
            "powershell relative",
            r#"powershell -c "Remove-Item -Recurse -Force .\out""#,
            false,
        ),
        ("bash -c cargo test", r#"bash -c "cargo test""#, false),
        (
            "an execution policy is not an encoded command",
            r#"powershell -ExecutionPolicy Bypass -Command "cargo test""#,
            false,
        ),
        ("a script file", "bash scripts/check.sh", false),
        ("pwsh -File", "pwsh -NoProfile -File build.ps1", false),
    ]);
}

/// `xargs` takes the arguments of its command from the input, which the
/// guard cannot see: a recursive delete behind it is denied wherever it
/// points, and any other command behind it runs.
#[test]
fn a_recursive_delete_behind_xargs_is_denied() {
    // (case, command, denied)
    assert_bash_rows(&[
        ("xargs rm -rf", "xargs rm -rf < list", true),
        (
            "xargs with flags",
            "find . -name x -print0 | xargs -0 -n 1 rm -rf",
            true,
        ),
        ("xargs grep", "find . -name '*.rs' | xargs grep TODO", false),
        (
            "xargs rm without -r",
            "git ls-files -d | xargs rm -f",
            false,
        ),
    ]);
}
