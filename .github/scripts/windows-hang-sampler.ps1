<#
Sends the state of a slow Windows test run off the runner while it runs (#441).

When the Windows tests hang, the runner stops responding: nextest's kill, the
timer in nextest-windows.ps1 and the step's `timeout-minutes` all failed to end
the step, and the job's log was lost each time. So evidence written on the
runner never survives. This sampler records a sample every minute and, once
the run has lasted longer than a healthy one, posts the samples as one comment
on the tracking issue and edits it each minute. If the runner freezes, the
last edit stays on GitHub. If the edits go on, the runner is alive.

A healthy run posts nothing. Without a token that can write issues (a pull
request from a fork or from Dependabot), it posts nothing either.
#>
param(
    [Parameter(Mandatory)] [string] $NextestLog,
    [Parameter(Mandatory)] [string] $StopFile,
    [double] $AfterMinutes = 8,
    [int] $EverySeconds = 60,
    [int] $Issue = 441
)

$start = Get-Date
$samples = [Collections.Generic.List[string]]::new()
$commentUrl = $null
$api = "$env:GITHUB_API_URL/repos/$env:GITHUB_REPOSITORY"
if (-not $env:GITHUB_API_URL) { $api = "https://api.github.com/repos/$env:GITHUB_REPOSITORY" }
$runUrl = "$env:GITHUB_SERVER_URL/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
$headers = @{
    Authorization          = "Bearer $env:GITHUB_TOKEN"
    Accept                 = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
}

function Get-Sample {
    $now = Get-Date
    $os = Get-CimInstance Win32_OperatingSystem
    $procs = @(Get-Process)
    $handles = ($procs | Measure-Object HandleCount -Sum).Sum
    $disks = Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' |
        ForEach-Object { '{0} {1:N1} GB free' -f $_.DeviceID, ($_.FreeSpace / 1GB) }
    $top = $procs | Sort-Object WorkingSet64 -Descending | Select-Object -First 8 |
        ForEach-Object { '  {0} pid {1}: {2:N0} MB, {3} handles' -f $_.ProcessName, $_.Id, ($_.WorkingSet64 / 1MB), $_.HandleCount }
    $recent = Get-CimInstance Win32_Process | Where-Object { $_.CreationDate -gt $start.AddMinutes(-1) } |
        Sort-Object CreationDate | Select-Object -Last 25 |
        ForEach-Object {
            $cmd = "$($_.CommandLine)"
            if ($cmd.Length -gt 140) { $cmd = $cmd.Substring(0, 140) }
            '  {0} pid {1} (parent {2}): {3}' -f $_.Name, $_.ProcessId, $_.ParentProcessId, $cmd
        }
    $tail = @()
    if (Test-Path $NextestLog) {
        # Read with sharing: nextest still has the file open for writing.
        $stream = [IO.File]::Open($NextestLog, 'Open', 'Read', 'ReadWrite')
        try { $tail = ([IO.StreamReader]::new($stream)).ReadToEnd() -split "`r?`n" | Where-Object { $_ } | Select-Object -Last 15 }
        finally { $stream.Dispose() }
    }
    @(
        ('### {0} UTC, {1:N1} min into the tests' -f $now.ToUniversalTime().ToString('HH:mm:ss'), ($now - $start).TotalMinutes)
        '```'
        ('memory free: {0:N0} MB of {1:N0} MB; commit free: {2:N0} MB' -f ($os.FreePhysicalMemory / 1KB), ($os.TotalVisibleMemorySize / 1KB), ($os.FreeVirtualMemory / 1KB))
        "processes: $($procs.Count); handles: $handles; disks: $($disks -join ', ')"
        'largest processes:'
        $top
        'processes started during the tests (last 25):'
        $recent
        'last nextest lines:'
        $tail | ForEach-Object { "  $_" }
        '```'
    ) -join "`n"
}

function Send-Comment([string] $status) {
    $body = @(
        "**A Windows test run is slower than a healthy one.** [Run $env:GITHUB_RUN_ID, attempt $env:GITHUB_RUN_ATTEMPT]($runUrl), commit ``$env:GITHUB_SHA``."
        ''
        "Status: $status"
        ''
        ($samples | Select-Object -Last 5)
    ) -join "`n"
    $json = @{ body = $body } | ConvertTo-Json -Compress
    try {
        if ($null -eq $script:commentUrl) {
            $reply = Invoke-RestMethod -Method Post -Uri "$api/issues/$Issue/comments" -Headers $headers -Body $json -ContentType 'application/json'
            $script:commentUrl = $reply.url
        } else {
            $null = Invoke-RestMethod -Method Patch -Uri $script:commentUrl -Headers $headers -Body $json -ContentType 'application/json'
        }
    } catch {
        # No write access, or the API is unreachable: the run must not fail
        # because the sampler could not report.
        [Console]::Error.WriteLine("sampler: could not post: $($_.Exception.Message)")
    }
}

$next = $start.AddSeconds($EverySeconds)
while ($true) {
    if (Test-Path $StopFile) {
        if ($null -ne $commentUrl) {
            $samples.Add((Get-Sample))
            Send-Comment ('the tests ended after {0:N1} min ({1}).' -f ((Get-Date) - $start).TotalMinutes, (Get-Content $StopFile -Raw).Trim())
        }
        exit 0
    }
    if ((Get-Date) -ge $next) {
        $next = $next.AddSeconds($EverySeconds)
        $samples.Add((Get-Sample))
        if ($samples.Count -gt 5) { $samples.RemoveAt(0) }
        if (((Get-Date) - $start).TotalMinutes -ge $AfterMinutes) {
            Send-Comment 'still running. If this comment stops changing, the runner stopped responding.'
        }
    }
    Start-Sleep -Seconds 1
}
