# stop_exit_hook.ps1 — Stop hook for the Ralph execution session.
#
# Claude Code runs this each time the interactive session finishes a turn and
# would wait for user input. We DON'T kill the process here — we just record
# the agent's exit signal to $env:RALPH_FLAG_FILE. The orchestrator is polling
# that file and owns the actual process termination (it holds the PID).
#
# No-op unless RALPH_FLAG_FILE is set, so this hook is harmless if it ever
# leaks into a normal interactive session.

$ErrorActionPreference = 'SilentlyContinue'

$flag = $env:RALPH_FLAG_FILE
if (-not $flag) { exit 0 }

# Pull the last assistant message either from the payload or from the
# transcript JSONL (version-robust: older/newer CLIs differ on whether
# last_assistant_message is included in the Stop payload).
function Get-LastAssistantText([string]$transcript) {
    if (-not $transcript -or -not (Test-Path $transcript)) { return '' }
    $text = ''
    foreach ($line in Get-Content -LiteralPath $transcript) {
        try { $o = $line | ConvertFrom-Json } catch { continue }
        if ($o.type -eq 'assistant' -and $o.message -and $o.message.content) {
            foreach ($c in $o.message.content) {
                if ($c.type -eq 'text' -and $c.text) { $text = [string]$c.text }
            }
        }
    }
    return $text
}

$msg = ''
try {
    $raw = [Console]::In.ReadToEnd()
    $payload = $raw | ConvertFrom-Json
    $msg = [string]$payload.last_assistant_message
    if ([string]::IsNullOrWhiteSpace($msg)) {
        $msg = Get-LastAssistantText ([string]$payload.transcript_path)
    }
} catch { exit 0 }

if ($msg -match 'RALPH_DONE_EXIT') {
    'DONE' | Set-Content -LiteralPath $flag -Encoding utf8
    exit 0
}
$m = [regex]::Match($msg, 'RALPH_BLOCKED_EXIT\s*(.*)')
if ($m.Success) {
    "BLOCKED $($m.Groups[1].Value.Trim())" | Set-Content -LiteralPath $flag -Encoding utf8
    exit 0
}
exit 0
