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

    switch ($env:PROCESSOR_ARCHITECTURE) {
        'AMD64' { $target = 'windows-x64' }
        default { throw "there is no Ralphy build for Windows on $($env:PROCESSOR_ARCHITECTURE)" }
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

    if ($WithDaemon) {
        # From an administrator window, the binary registers the daemon and
        # does not start it.
        & $ralphy daemon install
        if ($LASTEXITCODE -ne 0) { throw 'ralphy daemon install failed' }
    } else {
        Write-Host 'To start the daemon at sign-in, run: ralphy daemon install'
    }

    Write-Host ''
    # `doctor` exits 1 when a tool is missing; that never stops the install.
    & $ralphy doctor
    if ($LASTEXITCODE -gt 1) { throw "could not run $ralphy (exit $LASTEXITCODE)" }
    $global:LASTEXITCODE = 0
}

Install-Ralphy -WithDaemon ($Daemon.IsPresent -or $env:RALPHY_DAEMON -eq '1')
