# guard.ps1 — PreToolUse safety hook for the Ralphy autonomous loop.
#
# Claude Code runs this before every Bash/Edit/Write/MultiEdit/NotebookEdit
# call. Because the loop runs with --dangerously-skip-permissions (no
# interactive prompts), this hook is the ONLY thing standing between the
# agent and a destructive command while you sleep.
#
# Protocol: read the hook payload (JSON) from stdin. To BLOCK a call, write
# the reason to stderr and exit 2 — Claude Code feeds that reason back to the
# model so it can choose a different action. Exit 0 to allow.

$ErrorActionPreference = 'Stop'

try {
    $raw = [Console]::In.ReadToEnd()
    if ([string]::IsNullOrWhiteSpace($raw)) { exit 0 }
    $payload = $raw | ConvertFrom-Json
} catch {
    # If we cannot parse the payload, fail safe by allowing — blocking on
    # parse errors would stall every iteration. The deny-list below is the
    # real protection.
    exit 0
}

$tool = [string]$payload.tool_name
$ti = $payload.tool_input

function Deny([string]$reason) {
    [Console]::Error.WriteLine("BLOCKED by Ralphy guard: $reason")
    exit 2
}

# --- Command (Bash) deny-list -------------------------------------------------
# The agent commits locally; the orchestrator owns push / PR / merge. Anything
# that rewrites history, touches the remote, or destroys files is blocked.
if ($tool -eq 'Bash') {
    $cmd = [string]$ti.command
    if ([string]::IsNullOrWhiteSpace($cmd)) { exit 0 }

    $denied = @(
        @{ rx = '\bgit\s+push\b';                  why = 'pushing is the orchestrator''s job, not the agent''s' },
        @{ rx = '\bgit\s+reset\s+--hard\b';        why = 'hard reset can destroy uncommitted work' },
        @{ rx = '\bgit\s+clean\b';                 why = 'git clean deletes untracked files' },
        @{ rx = '\bgit\s+rebase\b';                why = 'history rewrite is not allowed in the loop' },
        @{ rx = '\bgit\s+(checkout|switch)\b';     why = 'the agent must stay on the run branch the orchestrator created' },
        @{ rx = '\bgit\s+worktree\b';              why = 'worktrees are the orchestrator''s business, not the agent''s' },
        @{ rx = '\bgh\s+pr\s+(merge|close)\b';     why = 'merging/closing PRs is a human decision' },
        @{ rx = '\bgh\s+(release|repo|workflow|secret|auth)\b'; why = 'repo/release/workflow/secret/auth ops are out of scope' },
        @{ rx = '\bcargo\s+publish\b';             why = 'publishing crates is out of scope' },
        @{ rx = '\brm\s+.*-[a-z]*r[a-z]*f|\brm\s+.*-[a-z]*f[a-z]*r'; why = 'recursive force-delete is blocked' },
        @{ rx = 'Remove-Item\b.*-Recurse';         why = 'recursive delete is blocked' },
        @{ rx = '\b(del|rmdir)\s+.*/s\b';          why = 'recursive delete is blocked' },
        @{ rx = '\b(format|mkfs|diskpart)\b';      why = 'disk-level command is blocked' },
        @{ rx = '\bcurl\b.*\|\s*(sh|bash|pwsh|powershell)'; why = 'piping a download into a shell is blocked' },
        @{ rx = 'iwr\b.*\|\s*iex|Invoke-Expression'; why = 'remote code execution is blocked' }
    )
    foreach ($d in $denied) {
        if ($cmd -match $d.rx) { Deny $d.why }
    }
    exit 0
}

# --- File-write deny-list -----------------------------------------------------
# Protect secrets, VCS internals, and the loop's own tooling/state.
if ($tool -in @('Edit','Write','MultiEdit','NotebookEdit')) {
    $path = [string]$ti.file_path
    if ([string]::IsNullOrWhiteSpace($path)) { exit 0 }
    $p = $path.Replace('\','/').ToLowerInvariant()

    $blockedPaths = @(
        '/.git/',
        '/.env',          # .env, .env.local, ...
        '/secrets',
        '/credentials',
        '/id_rsa',
        '.pem',
        '.pfx'
    )
    foreach ($b in $blockedPaths) {
        if ($p -like "*$b*") { Deny "writing to a protected path ($path)" }
    }

    # The agent must never edit the loop's own tooling. Ralphy now lives OUTSIDE
    # the target repo, so anchor this on the tool dir's absolute path ($PSScriptRoot)
    # instead of a fragile substring — works wherever Ralphy is installed.
    $toolDir = $PSScriptRoot.Replace('\','/').ToLowerInvariant().TrimEnd('/')
    if ($p -like "$toolDir/*") { Deny "writing to Ralphy's own tooling is not allowed ($path)" }
}

exit 0
