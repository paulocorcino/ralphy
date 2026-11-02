//! The `PowerShell` tool of a Claude session on Windows (#810): its command
//! is judged by the same rules as a Bash command, with PowerShell's own
//! delete words.

use super::*;

/// Judge each row as the command of a `PowerShell` call.
fn assert_powershell_rows(rows: &[(&str, &str, bool)]) {
    let rows: Vec<(&str, Value, bool)> = rows
        .iter()
        .map(|(case, cmd, deny)| (*case, bash(cmd), *deny))
        .collect();
    assert_rows("PowerShell", &rows);
}

/// A `PowerShell` call meets the forge-write and delete rules of a Bash
/// call, and an ordinary command runs.
#[test]
fn a_powershell_call_is_judged_by_the_command_rules() {
    // (case, command, denied)
    assert_powershell_rows(&[
        ("git push", "git push origin main", true),
        ("gh pr create", "gh pr create --fill", true),
        (
            "Remove-Item outside",
            r"Remove-Item -Recurse -Force C:\x",
            true,
        ),
        (
            "Remove-Item inside the worktree",
            r"Remove-Item -Recurse -Force .\out",
            false,
        ),
        ("cargo test", "cargo test --workspace", false),
        ("Get-ChildItem", "Get-ChildItem -Recurse src", false),
    ]);
}

/// In PowerShell `rm`, `ri`, `del`, `erase`, `rd` and `rmdir` are
/// `Remove-Item`, and `-Recurse` may be cut short (`-r`, `-Rec`).
#[test]
fn powershell_delete_aliases_are_recursive_deletes() {
    // (case, command, denied)
    assert_powershell_rows(&[
        ("rm -r", r"rm -r C:\x", true),
        ("ri -r", r"ri -r C:\x", true),
        ("del -Rec", r"del -Rec C:\x", true),
        ("rmdir -Recurse", r"rmdir -Recurse C:\x", true),
        (
            "Remove-Item cut short",
            r"Remove-Item -Rec -Force C:\x",
            true,
        ),
        ("rm -r inside the worktree", r"rm -r .\out", false),
        (
            "rd -Recurse under the temp",
            r"rd -Recurse C:\Users\x\AppData\Local\Temp\pw",
            false,
        ),
        ("rm without -Recurse", r"rm C:\x\file.txt", false),
    ]);
}

/// `[System.IO.Directory]::Delete(path, $true)` is a recursive delete,
/// judged by its path like any other.
#[test]
fn a_dotnet_directory_delete_is_judged_by_its_path() {
    // (case, command, denied)
    assert_powershell_rows(&[
        (
            "outside",
            r"[System.IO.Directory]::Delete('C:\x', $true)",
            true,
        ),
        ("a variable", r"[IO.Directory]::Delete($p, $true)", true),
        (
            "inside the worktree",
            r"[System.IO.Directory]::Delete('.\out', $true)",
            false,
        ),
        (
            "not recursive",
            r"[System.IO.Directory]::Delete('C:\x')",
            false,
        ),
    ]);
}

/// A `PowerShell` call without its string `command` is a shape the guard
/// cannot read: the guard fails closed.
#[test]
fn a_powershell_call_without_its_command_is_a_deny() {
    let got = decide(r#"{"tool_name":"PowerShell","tool_input":{}}"#);
    assert!(matches!(got, GuardDecision::Deny(_)), "{got:?}");
}
