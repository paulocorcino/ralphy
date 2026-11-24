# Install Ralphy for this user on Windows.
#
#   irm https://raw.githubusercontent.com/paulocorcino/ralphy/main/assets/install/install.ps1 | iex
#
# It downloads the newest release, checks its SHA-256, runs `ralphy install`,
# and ends with `ralphy doctor`. Nothing needs an administrator.
#
# Settings, all optional:
#   -Daemon, or $env:RALPHY_DAEMON = '1'      also start the daemon at sign-in
#   $env:RALPHY_VERSION = 'v0.1.0-rc.40'      install this release instead of the newest
#   $env:RALPHY_ALLOW_ROOT = '1'              install as administrator with no question
#   $env:RALPHY_INSTALL_DIR = '<folder>'      put ralphy in this folder (`ralphy install --dir`)
#   $env:RALPHY_RELEASES_URL                  where the release list is read (for tests)
#   $env:RALPHY_DOWNLOAD_URL                  where the archives are read (for tests)

param([switch]$Daemon)

function Install-Ralphy {
    param([bool]$WithDaemon)

    # Set here, not at the top: under `irm | iex` the top of the script runs in
    # the user's own console, and these would stay set there.
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    $repo = 'paulocorcino/ralphy'
    $releasesUrl = if ($env:RALPHY_RELEASES_URL) { $env:RALPHY_RELEASES_URL } else { "https://api.github.com/repos/$repo/releases" }
    $downloadUrl = if ($env:RALPHY_DOWNLOAD_URL) { $env:RALPHY_DOWNLOAD_URL } else { "https://github.com/$repo/releases/download" }
    # Windows PowerShell 5.1 does not offer TLS 1.2 by default.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    # Checked before any question: an install that cannot happen asks nothing.
    # Windows 11 on ARM64 runs the x64 build under emulation.
    $target = 'windows-x64'
    switch ($env:PROCESSOR_ARCHITECTURE) {
        'AMD64' { }
        'ARM64' { Write-Host 'This is Windows on ARM64: the x64 build runs under emulation.' }
        default { throw "there is no Ralphy build for Windows on $($env:PROCESSOR_ARCHITECTURE)" }
    }

    $identity = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    $elevated = $identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($elevated) {
        Write-Host 'warning: this runs as administrator. Ralphy does not need it: it installs for one user.'
        if ($env:RALPHY_ALLOW_ROOT -ne '1') {
            if (-not [Environment]::UserInteractive -or [Console]::IsInputRedirected) {
                throw 'there is no terminal to ask in. Run it in a normal window, or set RALPHY_ALLOW_ROOT=1'
            }
            $answer = Read-Host 'Continue anyway? [y/N]'
            if ($answer -notin @('y', 'Y', 'yes')) {
                throw 'stopped. Run it again in a normal window'
            }
        }
    }

    if ($env:RALPHY_VERSION) {
        $tag = $env:RALPHY_VERSION
    } else {
        # The newest release on the rc channel, which admits every release.
        $releases = Invoke-RestMethod -UseBasicParsing -Uri "${releasesUrl}?per_page=10"
        $tag = @($releases | Where-Object { -not $_.draft })[0].tag_name
        if (-not $tag) { throw "the release list from $releasesUrl names no release" }
    }

    $name = "ralphy-$tag-$target"
    $archive = "$name.zip"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("ralphy-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "Downloading Ralphy $tag for $target..."
        Invoke-WebRequest -UseBasicParsing -Uri "$downloadUrl/$tag/$archive" -OutFile (Join-Path $tmp $archive)
        Invoke-WebRequest -UseBasicParsing -Uri "$downloadUrl/$tag/$archive.sha256" -OutFile (Join-Path $tmp "$archive.sha256")
        $want = ((Get-Content -Raw (Join-Path $tmp "$archive.sha256")).Trim() -split '\s+')[0]
        $got = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $tmp $archive)).Hash
        if ($want -ne $got) { throw "the SHA-256 of $archive is wrong, so nothing was installed" }

        Expand-Archive -LiteralPath (Join-Path $tmp $archive) -DestinationPath $tmp
        $unpacked = Join-Path $tmp "$name\ralphy.exe"
        # Where `ralphy install` puts the binary. The binary answers, so the
        # default folder is decided in one place.
        $dirArgs = if ($env:RALPHY_INSTALL_DIR) { @('--dir', $env:RALPHY_INSTALL_DIR) } else { @() }
        $ralphy = (& $unpacked install --print-path @dirArgs | Select-Object -Last 1).Trim()
        if ($LASTEXITCODE -ne 0 -or -not $ralphy) { throw 'the downloaded ralphy did not run' }
        # A copy, not a link: the download folder is removed when this ends.
        # `--force` lets a second run repair an install.
        & $unpacked install --copy --force --dir (Split-Path -Parent $ralphy)
        if ($LASTEXITCODE -ne 0) { throw 'ralphy install failed' }
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }

    # The launcher starts the daemon at sign-in. An antivirus may remove it.
    $launcher = Join-Path (Split-Path -Parent $ralphy) 'ralphyw.exe'
    if (-not (Test-Path -LiteralPath $launcher -PathType Leaf)) {
        Write-Warning "$launcher is missing, so the daemon cannot start at sign-in. An antivirus may have removed it: check its quarantine, then run this script again."
        if ($WithDaemon) {
            Write-Warning 'The daemon is not set to start at sign-in. When the launcher is back, run: ralphy daemon install'
            $WithDaemon = $false
        }
    }

    if ($WithDaemon) {
        # From an administrator window, the binary registers the daemon and
        # does not start it.
        & $ralphy daemon install
        if ($LASTEXITCODE -ne 0) { throw 'ralphy daemon install failed' }
    }

    Write-Host ''
    # `doctor` exits 1 when a tool is missing; that never stops the install.
    & $ralphy doctor
    $doctorStatus = $LASTEXITCODE
    if ($doctorStatus -gt 1) { throw "could not run $ralphy (exit $doctorStatus)" }
    Write-NextStep -Dir (Split-Path -Parent $ralphy) -ToolsMissing ($doctorStatus -eq 1) -WithDaemon $WithDaemon
    $global:LASTEXITCODE = 0
}

# The last lines of the output are what the person reads, so they list what is
# left to do, in order.
function Write-NextStep {
    param([string]$Dir, [bool]$ToolsMissing, [bool]$WithDaemon)
    $steps = @()
    $onPath = @($env:Path -split ';' | ForEach-Object { $_.TrimEnd('') }) -contains $Dir.TrimEnd('')
    if (-not $onPath) {
        $steps += "Open a new window, or run this in this one: `$env:Path = '$Dir;' + `$env:Path"
    }
    if ($ToolsMissing) { $steps += 'Install each tool marked [miss] above, then run: ralphy doctor' }
    if (-not $WithDaemon) { $steps += 'Start the daemon at sign-in: ralphy daemon install' }
    Write-Host ''
    Write-Host 'Next steps:'
    if ($steps.Count -eq 0) { Write-Host '  None. Ralphy is ready.' }
    for ($i = 0; $i -lt $steps.Count; $i++) { Write-Host "  $($i + 1). $($steps[$i])" }
}

# An expected stop is one line, not a PowerShell error record. Under `irm | iex`
# there is no script file, and `exit` would close the person's own console.
try {
    Install-Ralphy -WithDaemon ($Daemon.IsPresent -or $env:RALPHY_DAEMON -eq '1')
} catch {
    [Console]::Error.WriteLine("ralphy install: $($_.Exception.Message)")
    if ($PSCommandPath) { exit 1 }
    $global:LASTEXITCODE = 1
}
