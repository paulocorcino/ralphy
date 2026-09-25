<#
Runs the workspace tests on Windows so that the Test step always ends (#441).

On Windows a child process inherits this script's output pipe. A process that
a test leaves alive keeps that pipe open, and the runner waits for the pipe to
close, so the step never ends: not at nextest's 360s kill, and not at the
step's `timeout-minutes` (run 36117818716). Measured locally with a grandchild
that sleeps 90 s: the script exited after 0.8 s, and the pipe closed after
91 s.

So this script writes the tests' output to files, bounds the run with its own
timer, and before it exits it kills every process that the run left behind,
and names each one in a warning.
#>
param(
    [double] $TimeoutMinutes = 14,
    [string[]] $Command = @('cargo', 'nextest', 'run', '--workspace', '--cargo-profile', 'ci', '--no-fail-fast')
)

$ErrorActionPreference = 'Stop'
$since = Get-Date
$out = Join-Path ([IO.Path]::GetTempPath()) "nextest-$PID.out"
$err = Join-Path ([IO.Path]::GetTempPath()) "nextest-$PID.err"

# Processes this run left alive: descendants of this script, and orphans
# (parent gone) that started after it. An orphan is how a detached child looks
# once its test has exited. Our own conhost is spared: this script writes to it.
function Get-LeftBehind {
    $all = @(Get-CimInstance Win32_Process)
    $byId = @{}
    foreach ($p in $all) { $byId[[int]$p.ProcessId] = $p }
    $children = @{}
    foreach ($p in $all) {
        $parent = [int]$p.ParentProcessId
        if (-not $children.ContainsKey($parent)) { $children[$parent] = @() }
        $children[$parent] += $p
    }
    $found = @{}
    $queue = [Collections.Generic.Queue[int]]::new()
    $queue.Enqueue($PID)
    while ($queue.Count -gt 0) {
        $id = $queue.Dequeue()
        foreach ($c in @($children[$id])) {
            if ($null -eq $c -or $found.ContainsKey([int]$c.ProcessId)) { continue }
            $found[[int]$c.ProcessId] = $c
            $queue.Enqueue([int]$c.ProcessId)
        }
    }
    foreach ($p in $all) {
        $parent = $byId[[int]$p.ParentProcessId]
        # A live parent that started after the child is a reused pid: the real
        # parent is gone.
        $orphan = ($null -eq $parent) -or ($parent.CreationDate -gt $p.CreationDate)
        if ($orphan -and $p.CreationDate -gt $since) { $found[[int]$p.ProcessId] = $p }
    }
    $found.Values | Where-Object {
        $_.ProcessId -ne $PID -and -not ($_.Name -eq 'conhost.exe' -and $_.ParentProcessId -eq $PID)
    }
}

function Stop-LeftBehind([string] $when) {
    foreach ($p in @(Get-LeftBehind)) {
        $line = "$($p.Name) pid $($p.ProcessId) (parent $($p.ParentProcessId)): $($p.CommandLine)"
        if ($line.Length -gt 300) { $line = $line.Substring(0, 300) }
        "::warning title=Process left alive $when::$line"
        try {
            Stop-Process -Id $p.ProcessId -Force -ErrorAction Stop
        } catch {
            "::warning title=Could not stop a process::pid $($p.ProcessId): $($_.Exception.Message)"
        }
    }
}

$run = Start-Process -FilePath $Command[0] -ArgumentList $Command[1..($Command.Length - 1)] `
    -NoNewWindow -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
# Without a handle taken now, ExitCode reads as null once the process is gone.
$null = $run.Handle

if ($run.WaitForExit([int]($TimeoutMinutes * 60 * 1000))) {
    $code = $run.ExitCode
    Get-Content $out
    Get-Content $err
    Stop-LeftBehind 'after the tests'
    exit $code
}

"::error title=Tests did not finish::the run passed $TimeoutMinutes minutes; its last output and every process it started follow"
Get-Content $out -Tail 100
Get-Content $err -Tail 200
Get-LeftBehind | Sort-Object CreationDate |
    Format-Table -AutoSize ProcessId, ParentProcessId, Name,
        @{ n = 'CommandLine'; e = { "$($_.CommandLine)".Substring(0, [Math]::Min(160, "$($_.CommandLine)".Length)) } } |
    Out-String -Width 250
Stop-LeftBehind 'at the timeout'
exit 1
