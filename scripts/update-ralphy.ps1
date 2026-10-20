<#
.SYNOPSIS
  Build Ralphy from the source folder, replace the installed binary, and
  restart the daemon (Windows, then the WSL peer). No agent needed.

.DESCRIPTION
  The update runs in a hidden process that Windows starts through WMI. It is
  not a child of this window or of the daemon, so it finishes even when the
  restart closes the console you ran this from. It is temporary: it ends when
  the update ends, and a build that runs over 60 minutes is stopped with its
  whole process tree. This window shows the log as it is written; Ctrl+C
  stops the watching, not the update.

  The daemon's own children are not touched: when the daemon stops, its
  consoles stop with it, as always.

  Phases, and what each failure leaves:
    1/4 Build Windows. A failed build changes nothing.
    2/4 Build the WSL peer. A failed build changes nothing on Windows.
        Then: "replacing in 5 s", press C in this window to cancel.
    3/4 Free the daemon port (only a ralphy.exe is stopped), keep a copy of
        the installed binary (ralphy.exe.prev), put the new one in place, and
        run `ralphy daemon restart`. If the new daemon does not answer, the
        previous binary is put back and started again.
    4/4 Restart the WSL peer the same way.

  Other ralphy processes (a `ralphy run` in a terminal, for example) are NOT
  stopped: they keep their version until they end. The log lists them.

  The last log line is `RESULT: <OK|FAILED|CANCELLED|DOWN> - <text>`.
  Exit codes: 0 OK, 1 FAILED (the previous version runs), 2 DOWN (no daemon
  runs), 3 the update process ended with no result, 4 CANCELLED.

.PARAMETER Repo
  The source folder. It builds what is there now, with uncommitted changes.
  Default: the repository this script is in.
.PARAMETER Distro
  The WSL distribution of the peer daemon. Default: the default WSL
  distribution. The peer is updated only when its systemd user unit
  ralphy-daemon is active and runs a cargo build (.../release/ralphy).
.PARAMETER SkipWsl
  Update only Windows.
.PARAMETER Yes
  Do not ask before the restart closes the open consoles. Needed when nobody
  can answer (an agent, a pipe).
.PARAMETER Delay
  Seconds between "build OK" and the replacement, to cancel with C. 0
  replaces at once.
.PARAMETER Rollback
  Put back the binaries of the last update (ralphy.exe.prev and the WSL
  ralphy.prev) and restart.
.PARAMETER Status
  Show the end of the last update's log.

.EXAMPLE
  scripts/update-ralphy.ps1
.EXAMPLE
  scripts/update-ralphy.ps1 -SkipWsl -Yes -Delay 0
.EXAMPLE
  scripts/update-ralphy.ps1 -Rollback
#>
#Requires -Version 7.2
[CmdletBinding()]
param(
    [string]$Repo = (Split-Path $PSScriptRoot -Parent),
    [string]$Distro = '',
    [switch]$SkipWsl,
    [switch]$Yes,
    [ValidateRange(0, 60)][int]$Delay = 5,
    [switch]$Rollback,
    [switch]$Status,
    # Used only by the hidden worker that this script starts.
    [switch]$Worker,
    [string]$Log,
    [string]$Bin,
    [string]$Store,
    [int]$Port = 0,
    [string]$TargetDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$BuildTimeoutMinutes = 60
$Self = $PSCommandPath
# No -Distro: wsl.exe uses the default distribution.
$WslTarget = if ($Distro) { @('-d', $Distro) } else { @() }
$WslName = if ($Distro) { $Distro } else { 'default WSL distribution' }

# ---------------------------------------------------------------- shared

function Get-StoreDir {
    if ($env:RALPHY_DAEMON_DIR) { return $env:RALPHY_DAEMON_DIR }
    return (Join-Path $HOME '.ralphy')
}

# daemon.pid holds `pid=N`, `exe=PATH` and one `arg=X` line per argument.
function Read-PidFile([string]$storeDir) {
    $file = Join-Path $storeDir 'daemon.pid'
    $info = [ordered]@{ Pid = 0; Exe = $null; Args = @() }
    if (-not (Test-Path $file)) { return [pscustomobject]$info }
    foreach ($line in Get-Content $file) {
        if ($line -match '^pid=(\d+)$') { $info.Pid = [int]$Matches[1] }
        elseif ($line -match '^exe=(.+)$') { $info.Exe = $Matches[1] }
        elseif ($line -match '^arg=(.*)$') { $info.Args += $Matches[1] }
    }
    return [pscustomobject]$info
}

function Get-PortFromArgs([string[]]$argv) {
    for ($i = 0; $i -lt $argv.Count; $i++) {
        if ($argv[$i] -eq '--port' -and $i + 1 -lt $argv.Count) { return [int]$argv[$i + 1] }
        if ($argv[$i] -match '^--port=(\d+)$') { return [int]$Matches[1] }
    }
    return 7257
}

# The installed binary: the one the daemon was started from, else the one on PATH.
function Resolve-InstalledBin($pidInfo) {
    if ($pidInfo.Exe) {
        $path = $pidInfo.Exe -replace '^\\\\\?\\', ''
        $path = $path -replace '(\.old)(\.\d+)?$', ''
        if (Test-Path $path) { return $path }
    }
    $cmd = Get-Command ralphy -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return $cmd.Source }
    return (Join-Path $HOME '.cargo\bin\ralphy.exe')
}

function Get-PortOwner([int]$port) {
    $conn = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $conn) { return $null }
    return Get-CimInstance Win32_Process -Filter "ProcessId=$($conn.OwningProcess)" -ErrorAction SilentlyContinue
}

function Get-Version([string]$exe) {
    if (-not (Test-Path $exe)) { return 'none' }
    try { return ((& $exe --version 2>$null) | Select-Object -First 1) } catch { return 'unknown' }
}

# ---------------------------------------------------------------- WSL helper

# Written to a file with LF endings, so no quoting crosses the wsl.exe line.
$WslScript = @'
#!/usr/bin/env bash
set -euo pipefail
mode="$1"; repo="${2:-}"
pidf="$HOME/.ralphy/daemon.pid"
B="$HOME/ralphy-target-wsl/release/ralphy"
if [ -f "$pidf" ]; then
  e="$(sed -n 's/^exe=//p' "$pidf" | sed 's/ (deleted)$//')"
  [ -n "$e" ] && B="$e"
fi
case "$B" in
  */release/ralphy) ;;
  *) echo "the WSL daemon runs $B, which is not a cargo build: update it by hand"; exit 3 ;;
esac
T="${B%/release/ralphy}"
CARGO="$HOME/.cargo/bin/cargo"; [ -x "$CARGO" ] || CARGO=cargo
pid="$(systemctl --user show -p MainPID --value ralphy-daemon 2>/dev/null || true)"
# True when the peer daemon runs this file (same inode, so not a replaced one).
runs() { [ "${pid:-0}" -gt 0 ] && [ -e "$1" ] && [ "/proc/$pid/exe" -ef "$1" ]; }
case "$mode" in
  probe)
    systemctl --user is-active ralphy-daemon || true ;;
  build)
    # Cargo replaces $B. If the peer runs it, its exe would become
    # `(deleted)`, and every verb that spawns `ralphy` fails until a restart.
    # A renamed file keeps the inode, so the peer runs `ralphy.old` instead,
    # a name `daemon restart` still knows.
    if runs "$B"; then cp -f "$B" "$B.prev"; mv -f "$B" "$B.old"
    elif runs "$B.old"; then cp -f "$B.old" "$B.prev"
    elif [ -f "$B" ]; then cp -f "$B" "$B.prev"; fi
    cd "$repo"
    CARGO_TARGET_DIR="$T" "$CARGO" build --release --locked -p ralphy-cli --bin ralphy
    "$B" --version ;;
  restore)
    # A failed build: give the running binary its name back, no restart.
    if runs "$B.old"; then mv -f "$B.old" "$B"; fi ;;
  restart)
    "$B" daemon restart --yes
    rm -f "$B.old"
    "$B" --version ;;
  rollback)
    [ -f "$B.prev" ] || { echo "no previous WSL binary at $B.prev"; exit 4; }
    # A running executable cannot be written in place (ETXTBSY): rename over it.
    cp -f "$B.prev" "$B.tmp" && mv -f "$B.tmp" "$B"
    "$B" daemon restart --yes
    rm -f "$B.old"
    "$B" --version ;;
  version)
    if [ -f "$B" ]; then "$B" --version; else echo none; fi ;;
esac
'@

function Get-WslHelper([string]$storeDir) {
    $file = Join-Path $storeDir 'update-ralphy-wsl.sh'
    [System.IO.File]::WriteAllText($file, ($WslScript -replace "`r`n", "`n"))
    $linux = (& wsl.exe @WslTarget -- wslpath -a ($file -replace '\\', '/')) | Select-Object -First 1
    return $linux.Trim()
}

function Test-WslPeer([string]$storeDir) {
    if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) { return $false }
    try {
        $helper = Get-WslHelper $storeDir
        $state = (& wsl.exe @WslTarget -- bash $helper probe 2>$null) | Select-Object -Last 1
        return ($LASTEXITCODE -eq 0 -and $state -and $state.Trim() -eq 'active')
    } catch { return $false }
}

# ---------------------------------------------------------------- -Status

if ($Status) {
    $last = Join-Path (Get-StoreDir) 'update-ralphy.last'
    if (-not (Test-Path $last)) { Write-Host 'no update has run yet'; exit 0 }
    $logPath = (Get-Content $last -Raw).Trim()
    Write-Host "log: $logPath"
    Get-Content $logPath -Tail 40
    exit 0
}

# ---------------------------------------------------------------- foreground

function Write-LogLine([string]$line) {
    $color = 'Gray'
    if ($line -match '^RESULT: OK') { $color = 'Green' }
    elseif ($line -match '^RESULT: CANCELLED') { $color = 'Yellow' }
    elseif ($line -match '^RESULT: ') { $color = 'Red' }
    elseif ($line -match '^PHASE: ') { $color = 'Cyan' }
    elseif ($line -match '^\s*error(\[E\d+\])?:' -or $line -match '\] ERROR: ') { $color = 'Red' }
    elseif ($line -match '^\s*warning:' -or $line -match '\] WARNING: ') { $color = 'Yellow' }
    Write-Host $line -ForegroundColor $color
}

# "Build OK, replacing in N s": C in this window cancels, through a file the
# worker reads. With no keyboard (an agent, a pipe) it only waits.
function Wait-ReplaceCountdown([int]$seconds, [string]$cancelFile) {
    $interactive = -not [Console]::IsInputRedirected
    Write-Host ''
    Write-Host "Build OK. Replacing in $seconds s: the daemon restarts and its consoles close." -ForegroundColor Green
    if ($interactive) { Write-Host 'Press C to cancel (the build is kept, nothing is replaced).' -ForegroundColor Green }
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if ($interactive -and [Console]::KeyAvailable) {
            $key = [Console]::ReadKey($true)
            if ($key.Key -eq [ConsoleKey]::C) {
                Set-Content -Path $cancelFile -Value 'cancel'
                Write-Host 'cancel requested' -ForegroundColor Yellow
                return
            }
        }
        Start-Sleep -Milliseconds 100
    }
    Write-Host ''
}

if (-not $Worker) {
    $storeDir = Get-StoreDir
    $pidInfo = Read-PidFile $storeDir
    $installed = Resolve-InstalledBin $pidInfo
    $daemonPort = Get-PortFromArgs $pidInfo.Args
    $lock = Join-Path $storeDir 'update-ralphy.lock'
    $cancelFile = Join-Path $storeDir 'update-ralphy.cancel'

    if (Test-Path $lock) {
        $holder = [int]((Get-Content $lock -Raw).Trim())
        $proc = Get-Process -Id $holder -ErrorAction SilentlyContinue
        if ($proc -and $proc.ProcessName -eq 'pwsh') {
            Write-Host "an update is already running (pid $holder). Watch it with: $Self -Status" -ForegroundColor Yellow
            exit 1
        }
        Remove-Item $lock -Force
    }
    Remove-Item $cancelFile -Force -ErrorAction SilentlyContinue
    if (-not $Rollback -and -not (Test-Path (Join-Path $Repo 'Cargo.toml'))) {
        throw "no Cargo.toml in $Repo"
    }
    if ($Rollback -and -not (Test-Path "$installed.prev")) {
        throw "no previous binary at $installed.prev: nothing to roll back to"
    }

    Write-Host ''
    if ($Rollback) {
        Write-Host "Roll back to: $(Get-Version "$installed.prev")"
    } else {
        $branch = (& git -C $Repo branch --show-current 2>$null)
        $head = (& git -C $Repo log -1 --format='%h %s' 2>$null)
        $dirty = @(& git -C $Repo status --porcelain 2>$null).Count
        Write-Host "Source:    $Repo"
        Write-Host "Branch:    $branch  ($head)"
        if ($dirty -gt 0) { Write-Host "           $dirty uncommitted file(s) are built too" -ForegroundColor Yellow }
        if ($branch -and $branch -ne 'main') { Write-Host '           this is not main' -ForegroundColor Yellow }
    }
    Write-Host "Installed: $installed  ($(Get-Version $installed))"
    Write-Host "Daemon:    port $daemonPort, pid $($pidInfo.Pid)"
    $wsl = (-not $SkipWsl) -and (Test-WslPeer $storeDir)
    Write-Host "WSL peer:  $(if ($wsl) { "$WslName, updated after Windows" } elseif ($SkipWsl) { 'skipped (-SkipWsl)' } else { 'not found, skipped' })"

    # The restart ends every console of the daemon and the agents inside.
    if (-not $Yes) {
        $headers = @{}
        $tokenFile = Join-Path $storeDir 'daemon-token'
        if (Test-Path $tokenFile) {
            $token = (Get-Content $tokenFile -Raw).Trim()
            if ($token) { $headers['Authorization'] = "Bearer $token" }
        }
        $listening = [bool](Get-PortOwner $daemonPort)
        $consoles = $null
        if ($listening) {
            try {
                # A JSON array comes back as one object: enumerate it, or `[]` counts as one.
                $reply = Invoke-RestMethod -Uri "http://127.0.0.1:$daemonPort/api/sessions?local=1" -Headers $headers -TimeoutSec 3 -NoProxy
                $consoles = @($reply | Where-Object { $null -ne $_ })
            } catch { $consoles = $null }
        }
        if ($listening -and ($null -eq $consoles -or $consoles.Count -gt 0)) {
            Write-Host ''
            if ($null -eq $consoles) {
                Write-Host 'The open consoles could not be counted.' -ForegroundColor Yellow
            } else {
                Write-Host "$($consoles.Count) console(s) open. The restart closes them and stops the agents inside:" -ForegroundColor Yellow
                foreach ($c in $consoles) {
                    $field = { param($o, $n) if ($o.PSObject.Properties[$n]) { $o.$n } else { $null } }
                    $name = & $field $c 'name'
                    if (-not $name) { $name = "$(& $field $c 'repo') ($(& $field $c 'agent'))" }
                    Write-Host "  $name"
                }
            }
            if ($wsl) { Write-Host 'The consoles of the WSL peer close too.' -ForegroundColor Yellow }
            Write-Host 'Consoles opened while it builds are closed as well.'
            if ([Console]::IsInputRedirected) {
                Write-Host 'nobody can answer here: run again with -Yes to go on. Nothing changed.' -ForegroundColor Red
                exit 1
            }
            $answer = Read-Host 'Continue? [y/N]'
            if ($answer.Trim().ToLower() -notin @('y', 'yes', 's', 'sim')) { Write-Host 'nothing changed'; exit 0 }
        }
    }

    # Is this window inside the daemon's process tree (a workbench console)?
    $inside = $false
    if ($pidInfo.Pid -gt 0) {
        $p = $PID
        for ($i = 0; $i -lt 32; $i++) {
            $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$p" -ErrorAction SilentlyContinue
            if (-not $proc -or $proc.ParentProcessId -in @(0, $p)) { break }
            if ($proc.ParentProcessId -eq $pidInfo.Pid) { $inside = $true; break }
            $p = $proc.ParentProcessId
        }
    }

    $logDir = Join-Path $storeDir 'update-logs'
    New-Item -ItemType Directory -Force -Path $logDir | Out-Null
    Get-ChildItem $logDir -Filter 'update-*.log' | Sort-Object LastWriteTime -Descending | Select-Object -Skip 19 | Remove-Item -Force
    $logPath = Join-Path $logDir ('update-{0:yyyyMMdd-HHmmss}.log' -f (Get-Date))
    Set-Content -Path (Join-Path $storeDir 'update-ralphy.last') -Value $logPath
    New-Item -ItemType File -Force -Path $logPath | Out-Null

    $pwsh = (Get-Process -Id $PID).Path
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Repo 'target' }
    $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-Worker',
        '-Log', "`"$logPath`"", '-Repo', "`"$Repo`"", '-Bin', "`"$installed`"", '-Store', "`"$storeDir`"",
        '-Port', $daemonPort, '-TargetDir', "`"$target`"", '-Delay', $Delay)
    if ($Distro) { $argv += @('-Distro', $Distro) }
    if (-not $wsl) { $argv += '-SkipWsl' }
    if ($Rollback) { $argv += '-Rollback' }
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ ShowWindow = [uint16]0 }
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine               = "`"$pwsh`" $($argv -join ' ')"
        CurrentDirectory          = $Repo
        ProcessStartupInformation = $startup
    }
    if ($created.ReturnValue -ne 0) { throw "could not start the update process (Win32_Process.Create returned $($created.ReturnValue))" }
    $workerPid = [int]$created.ProcessId

    Write-Host ''
    Write-Host "Update running in process $workerPid. Log: $logPath"
    Write-Host "Ctrl+C stops watching only; the update goes on. Later: $Self -Status"
    if ($inside) { Write-Host 'This window is a workbench console: it closes when the daemon restarts.' -ForegroundColor Yellow }
    Write-Host ''

    $seen = 0
    $result = $null
    while ($true) {
        $lines = @()
        try { $lines = @(Get-Content -Path $logPath -Encoding utf8 -ErrorAction Stop) } catch { $lines = @() }
        for (; $seen -lt $lines.Count; $seen++) {
            $line = $lines[$seen]
            if ($line -match '^COUNTDOWN: (\d+)$') {
                Wait-ReplaceCountdown ([int]$Matches[1]) $cancelFile
                continue
            }
            Write-LogLine $line
            if ($line -match '^RESULT: (OK|FAILED|CANCELLED|DOWN)') { $result = $Matches[1] }
        }
        if ($result) { break }
        if (-not (Get-Process -Id $workerPid -ErrorAction SilentlyContinue)) {
            Start-Sleep -Milliseconds 500
            $tail = @(Get-Content -Path $logPath -Encoding utf8 -ErrorAction SilentlyContinue)
            if ($tail.Count -gt $seen) { continue }
            Write-Host "the update process ended without a result: see $logPath" -ForegroundColor Red
            exit 3
        }
        Start-Sleep -Milliseconds 500
    }
    switch ($result) { 'OK' { exit 0 } 'FAILED' { exit 1 } 'CANCELLED' { exit 4 } default { exit 2 } }
}

# ---------------------------------------------------------------- worker

$env:RALPHY_DAEMON_DIR = $Store
if ($TargetDir) { $env:CARGO_TARGET_DIR = $TargetDir }
$lock = Join-Path $Store 'update-ralphy.lock'
$cancelFile = Join-Path $Store 'update-ralphy.cancel'
Set-Content -Path $lock -Value $PID

# A build that hangs (a stuck cargo lock, a wedged WSL) must not leave this
# process behind: past the limit it ends with its whole tree (cargo, rustc).
# Armed only around the builds; nothing is replaced while it is armed.
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
public static class UpdateRalphyWatchdog {
    static Timer timer;
    public static void Arm(int minutes, string log, string lockPath) {
        Disarm();
        timer = new Timer(_ => {
            try { File.AppendAllText(log, "RESULT: FAILED - the build ran over " + minutes + " minutes and was stopped. Nothing was replaced.\n"); } catch (IOException) { }
            try { File.Delete(lockPath); } catch (IOException) { }
            Process.GetCurrentProcess().Kill(true);
        }, null, minutes * 60000, Timeout.Infinite);
    }
    public static void Disarm() {
        if (timer != null) { timer.Dispose(); timer = null; }
    }
}
'@

function Write-Log([string]$text) {
    Add-Content -Path $Log -Value ('[{0:HH:mm:ss}] {1}' -f (Get-Date), $text) -Encoding utf8
}
function Write-Phase([string]$text) { Add-Content -Path $Log -Value "PHASE: $text" -Encoding utf8 }
function Write-Result([string]$text) { Add-Content -Path $Log -Value "RESULT: $text" -Encoding utf8 }

# Run a native program with its output in the log; return its exit code.
function Invoke-Logged([string]$what, [scriptblock]$run) {
    Write-Log $what
    & $run *>> $Log
    return $LASTEXITCODE
}

# Repeat the compiler's error lines at the end, so a failed build reads in one place.
function Write-BuildErrors([int]$fromLine) {
    $errors = @(Get-Content -Path $Log -Encoding utf8 | Select-Object -Skip $fromLine |
        Where-Object { $_ -match '^\s*error(\[E\d+\])?:' })
    if ($errors.Count -eq 0) { return }
    Write-Log "the build errors ($($errors.Count)):"
    foreach ($e in $errors) { Add-Content -Path $Log -Value "  $e" -Encoding utf8 }
}

function Get-LogLineCount { return @(Get-Content -Path $Log -Encoding utf8).Count }

# The window counts down and may drop the cancel file; with no window, this only waits.
function Wait-Cancel {
    if ($Delay -le 0) { return $false }
    Add-Content -Path $Log -Value "COUNTDOWN: $Delay" -Encoding utf8
    $deadline = (Get-Date).AddSeconds($Delay)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $cancelFile) { Remove-Item $cancelFile -Force; return $true }
        Start-Sleep -Milliseconds 200
    }
    if (Test-Path $cancelFile) { Remove-Item $cancelFile -Force; return $true }
    return $false
}

$wsl = -not $SkipWsl
$wslHelper = $null
$prev = "$Bin.prev"
$before = Get-Version $Bin
$beforeWsl = $null

function Restart-Windows([string]$why) {
    $code = Invoke-Logged "ralphy daemon restart ($why)" { & $Bin daemon restart --yes }
    if ($code -ne 0) { return $false }
    # The restart said yes; check it: a recorded pid that is alive and is the
    # one listening on the port. The new daemon writes its pid file after the
    # restart returns, so the check waits for it.
    $deadline = (Get-Date).AddSeconds(15)
    do {
        $info = Read-PidFile $Store
        $running = if ($info.Pid -gt 0) { Get-Process -Id $info.Pid -ErrorAction SilentlyContinue } else { $null }
        $owner = if ($running) { Get-PortOwner $Port } else { $null }
        if ($running -and $owner -and $owner.ProcessId -eq $info.Pid) { break }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)
    if (-not $running) { Write-Log 'ERROR: no daemon is recorded as running after the restart'; return $false }
    if (-not $owner -or $owner.ProcessId -ne $info.Pid) {
        $who = if ($owner) { "pid $($owner.ProcessId)" } else { 'nothing' }
        Write-Log "ERROR: daemon pid $($info.Pid) runs, but port $Port is held by $who"
        return $false
    }
    Write-Log "daemon pid $($info.Pid) runs $($running.Path) and listens on port $Port"
    return $true
}

# A name that `ralphy daemon restart` still knows as this program: it strips
# `.old` and `.old.<digits>`. Any other name makes it think the pid was reused,
# and then it does not stop the daemon.
function Get-ParkName {
    $parked = "$Bin.old"
    if (Test-Path $parked) {
        try { Remove-Item -LiteralPath $parked -Force } catch { $parked = "$Bin.old.{0:yyyyMMddHHmmss}" -f (Get-Date) }
    }
    return $parked
}

# Put the previous binary back. The failed one is parked, not deleted: it may
# still be running.
function Restore-Windows {
    if (-not (Test-Path $prev)) { Write-Log "ERROR: no previous binary at $prev"; return $false }
    $failed = Get-ParkName
    if (Test-Path $Bin) { Move-Item -LiteralPath $Bin -Destination $failed -Force }
    Copy-Item -LiteralPath $prev -Destination $Bin -Force
    Write-Log "put back $(Get-Version $Bin); the replaced binary is parked at $failed"
    return (Restart-Windows 'previous version')
}

function Clear-Port {
    $owner = Get-PortOwner $Port
    if (-not $owner) { return $true }
    $info = Read-PidFile $Store
    if ($owner.ProcessId -eq $info.Pid) { return $true }   # the daemon itself; the restart stops it
    $leaf = if ($owner.ExecutablePath) { Split-Path $owner.ExecutablePath -Leaf } else { $owner.Name }
    if ($leaf -match '^ralphy\.exe') {
        Write-Log "port $Port is held by ralphy pid $($owner.ProcessId), not the recorded daemon: stopping it"
        Write-Log "  $($owner.CommandLine)"
        Stop-Process -Id $owner.ProcessId -Force
        Start-Sleep -Seconds 1
        return -not (Get-PortOwner $Port)
    }
    Write-Log "ERROR: port $Port is held by $leaf (pid $($owner.ProcessId)), which is not Ralphy. Stop it by hand."
    return $false
}

try {
    Write-Log "update-ralphy worker pid $PID"
    Write-Log "installed: $Bin ($before)"
    if ($wsl) {
        $wslHelper = Get-WslHelper $Store
        $beforeWsl = (& wsl.exe @WslTarget -- bash $wslHelper version 2>$null) | Select-Object -Last 1
        Write-Log "WSL peer: $beforeWsl"
    }

    if ($Rollback) {
        if (Wait-Cancel) { Write-Result 'CANCELLED - nothing was rolled back'; exit 4 }
        Write-Phase 'roll back Windows'
        if (-not (Restore-Windows)) { Write-Result 'DOWN - the previous version did not start either. See the daemon.log in the store.'; exit 2 }
        if ($wsl) {
            Write-Phase 'roll back the WSL peer'
            $code = Invoke-Logged 'WSL: put back the previous binary and restart' { & wsl.exe @WslTarget -- bash $wslHelper rollback }
            if ($code -ne 0) { Write-Result "FAILED - Windows rolled back to $(Get-Version $Bin); the WSL peer did not (exit $code)"; exit 1 }
        }
        Write-Result "OK - rolled back to $(Get-Version $Bin)"
        exit 0
    }

    # Nothing is replaced until both builds pass.
    [UpdateRalphyWatchdog]::Arm($BuildTimeoutMinutes, $Log, $lock)
    Write-Phase "1/4 build Windows ($Repo)"
    $mark = Get-LogLineCount
    $code = Invoke-Logged 'cargo build --release --locked -p ralphy-cli --bin ralphy' {
        & cargo build --release --locked -p ralphy-cli --bin ralphy --manifest-path (Join-Path $Repo 'Cargo.toml')
    }
    if ($code -ne 0) {
        [UpdateRalphyWatchdog]::Disarm()
        Write-BuildErrors $mark
        Write-Result "FAILED - the Windows build failed (exit $code). Nothing changed."
        exit 1
    }
    $built = Join-Path $TargetDir 'release\ralphy.exe'
    if (-not (Test-Path $built)) { [UpdateRalphyWatchdog]::Disarm(); Write-Result "FAILED - no binary at $built. Nothing changed."; exit 1 }
    $newVersion = Get-Version $built
    Write-Log "built $newVersion"

    if ($wsl) {
        $repoLinux = ((& wsl.exe @WslTarget -- wslpath -a ($Repo -replace '\\', '/')) | Select-Object -First 1).Trim()
        Write-Phase "2/4 build the WSL peer ($WslName, $repoLinux)"
        $mark = Get-LogLineCount
        $code = Invoke-Logged 'cargo build --release --locked -p ralphy-cli --bin ralphy (WSL)' { & wsl.exe @WslTarget -- bash $wslHelper build $repoLinux }
        if ($code -ne 0) {
            [UpdateRalphyWatchdog]::Disarm()
            Write-BuildErrors $mark
            Invoke-Logged 'WSL: giving the running binary its name back' { & wsl.exe @WslTarget -- bash $wslHelper restore } | Out-Null
            Write-Result "FAILED - the WSL build failed (exit $code). Windows was not changed."
            exit 1
        }
    } else {
        Write-Phase '2/4 WSL peer skipped'
    }
    [UpdateRalphyWatchdog]::Disarm()

    $runningInfo = Read-PidFile $Store
    $runningProc = if ($runningInfo.Pid) { Get-Process -Id $runningInfo.Pid -ErrorAction SilentlyContinue } else { $null }
    $same = (Test-Path $Bin) -and ((Get-FileHash $built).Hash -eq (Get-FileHash $Bin).Hash)
    if ($same -and $runningProc -and $runningProc.Path -eq $Bin -and -not $wsl) {
        Write-Result "OK - already current: $newVersion. Nothing was restarted."
        exit 0
    }

    if (Wait-Cancel) { Write-Result "CANCELLED - $newVersion is built in $TargetDir; nothing was replaced"; exit 4 }

    Write-Phase '3/4 replace and restart Windows'
    if (-not (Clear-Port)) { Write-Result 'FAILED - the daemon port is not free. Nothing was replaced.'; exit 1 }

    # A running image can be renamed but not overwritten.
    if (-not $same) {
        if (Test-Path $Bin) {
            Copy-Item -LiteralPath $Bin -Destination $prev -Force
            Move-Item -LiteralPath $Bin -Destination (Get-ParkName) -Force
        }
        Copy-Item -LiteralPath $built -Destination $Bin -Force
        Write-Log "installed $newVersion at $Bin (previous kept at $prev)"
    }

    if (-not (Restart-Windows 'new version')) {
        Write-Log 'WARNING: the new daemon did not come up; putting the previous version back'
        if (Restore-Windows) {
            Write-Result "FAILED - $newVersion did not start; $(Get-Version $Bin) runs again. See the daemon.log in the store."
            exit 1
        }
        Write-Result 'DOWN - neither version started. See the daemon.log in the store.'
        exit 2
    }

    $wslNote = ''
    if ($wsl) {
        Write-Phase '4/4 restart the WSL peer'
        $code = Invoke-Logged 'WSL: ralphy daemon restart' { & wsl.exe @WslTarget -- bash $wslHelper restart }
        if ($code -ne 0) {
            Write-Log 'WARNING: the WSL daemon did not come up; putting its previous binary back'
            $back = Invoke-Logged 'WSL: rollback' { & wsl.exe @WslTarget -- bash $wslHelper rollback }
            $wslNote = if ($back -eq 0) { "; the WSL peer failed and runs $beforeWsl again" } else { '; the WSL peer is DOWN' }
        } else {
            $wslNote = '; WSL peer updated'
        }
    } else {
        Write-Phase '4/4 WSL peer skipped'
    }

    foreach ($old in Get-ChildItem -Path (Split-Path $Bin) -Filter 'ralphy.exe.old*' -ErrorAction SilentlyContinue) {
        try { Remove-Item -LiteralPath $old.FullName -Force } catch { Write-Log "kept $($old.Name): still in use" }
    }
    $daemonPid = (Read-PidFile $Store).Pid
    $others = @(Get-CimInstance Win32_Process -Filter "Name LIKE 'ralphy%'" -ErrorAction SilentlyContinue |
            Where-Object { $_.ProcessId -ne $daemonPid })
    if ($others.Count -gt 0) {
        Write-Log 'these ralphy processes still run the version they started with (not stopped):'
        foreach ($o in $others) { Write-Log "  pid $($o.ProcessId): $($o.CommandLine)" }
    }
    Write-Result "OK - $before -> $newVersion$wslNote"
    exit 0
} catch {
    [UpdateRalphyWatchdog]::Disarm()
    Write-Log "ERROR: $($_.Exception.Message)"
    Write-Log $_.ScriptStackTrace
    $info = Read-PidFile $Store
    if ($info.Pid -and (Get-Process -Id $info.Pid -ErrorAction SilentlyContinue)) {
        Write-Result "FAILED - stopped on an error; the daemon pid $($info.Pid) still runs ($(Get-Version $Bin))"
        exit 1
    }
    Write-Result "DOWN - stopped on an error and no daemon runs. Start the previous version with: $Self -Rollback"
    exit 2
} finally {
    Remove-Item -LiteralPath $lock -Force -ErrorAction SilentlyContinue
}
