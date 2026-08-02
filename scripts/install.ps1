# Krate installer for Windows.
#
# Downloads the krate binary for this machine from the latest GitHub release,
# verifies its checksum, and installs it to a directory on PATH. Set
# $env:KRATE_VERSION to pin a release, or $env:KRATE_INSTALL_DIR to choose
# where it lands.
#
#   irm https://krate.tech/install.ps1 | iex
#
# The Unix half of this lives in scripts/install.sh and behaves the same way:
# same release lookup, same refusal to install anything whose checksum does not
# match, same "already up to date" short-circuit.

$ErrorActionPreference = 'Stop'

$repo = 'incyashraj/krate'
$binary = 'krate.exe'

function Write-Say { param([string]$Message) Write-Host $Message }
function Stop-Install { param([string]$Message) Write-Error "error: $Message"; exit 1 }

# Only x86_64 Windows is published today. Say so plainly rather than
# downloading an archive that cannot run.
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64') {
    Stop-Install "unsupported architecture: $arch (install from source instead)"
}
$target = 'x86_64-pc-windows-msvc'

$version = $env:KRATE_VERSION
if (-not $version) {
    Write-Say 'Finding the latest release...'
    # /releases/latest excludes pre-releases and Krate is pre-release only, so
    # query the list and take the newest v-tag instead.
    try {
        $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases?per_page=30" `
            -Headers @{ 'User-Agent' = 'krate-installer' }
        $version = ($releases | Where-Object { $_.tag_name -like 'v*' } | Select-Object -First 1).tag_name
    } catch {
        $version = $null
    }

    # The API is rate limited to 60 requests an hour per address and does not
    # care that you are only reading. Anyone behind a shared address -- an
    # office, a university, a cafe -- can hit that without having run this
    # before, and the raw failure explains nothing. The releases page is plain
    # HTML and is not limited the same way, so read a tag out of it instead.
    if (-not $version) {
        Write-Say 'The GitHub API did not answer; reading the releases page instead...'
        try {
            $page = Invoke-WebRequest -Uri "https://github.com/$repo/releases" `
                -Headers @{ 'User-Agent' = 'krate-installer' } -UseBasicParsing
            # Built by concatenation with a single-quoted pattern, so the
            # quote inside the character class needs no backtick escape --
            # that escaping is easy to get wrong and fails as a parse error
            # rather than a wrong answer.
            $pattern = '/' + $repo + '/releases/tag/(v[0-9][^"]*)'
            $match = [regex]::Match($page.Content, $pattern)
            if ($match.Success) { $version = $match.Groups[1].Value }
        } catch {
            $version = $null
        }
    }

    if (-not $version) {
        Write-Host 'Could not work out the latest version.'
        Write-Host ''
        Write-Host 'This usually means GitHub is rate limiting your address, which happens'
        Write-Host 'on shared networks and clears within the hour.'
        Write-Host ''
        Write-Host 'To install right now, pin a version. Take the newest tag from'
        Write-Host "https://github.com/$repo/releases and use it like this:"
        Write-Host '  $env:KRATE_VERSION="v0.1.0-rc7"; irm https://krate.tech/install.ps1 | iex'
        exit 1
    }
}

# Re-running the installer should be safe and boring, not a surprise reinstall.
$installed = $null
$existing = Get-Command krate -ErrorAction SilentlyContinue
if ($existing) {
    try { $installed = (& krate --version 2>$null) -split '\s+' | Select-Object -Last 1 } catch { }
}
if ($installed) {
    if ($installed -eq $version -or $installed -eq $version.TrimStart('v')) {
        Write-Say "krate $installed is already installed and up to date."
        Write-Say 'Re-running will reinstall the same version.'
    } else {
        Write-Say "Updating krate $installed -> $version."
    }
}

$archiveVersion = $version.TrimStart('v')
$archive = "krate-$archiveVersion-$target.zip"
$base = "https://github.com/$repo/releases/download/$version"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("krate-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    Write-Say "Downloading $archive..."
    $archivePath = Join-Path $tmp $archive
    try {
        Invoke-WebRequest -Uri "$base/$archive" -OutFile $archivePath -UseBasicParsing
    } catch {
        Stop-Install "download failed. Does a binary exist for $target in release $version?"
    }

    # A checksum that cannot be matched is a refusal, not a warning: installing
    # an unverified binary is the one outcome worth failing over.
    $sumsPath = Join-Path $tmp 'SHA256SUMS'
    $haveSums = $true
    try {
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing
    } catch {
        $haveSums = $false
    }
    if ($haveSums) {
        $line = Select-String -Path $sumsPath -Pattern ([regex]::Escape($archive)) |
            Select-Object -First 1
        if (-not $line) {
            Stop-Install "SHA256SUMS has no entry for $archive; refusing to install unverified"
        }
        $expected = ($line.Line -split '\s+')[0]
        $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $expected.ToLower()) {
            Stop-Install "checksum mismatch for $archive; refusing to install"
        }
        Write-Say 'Checksum verified.'
    } else {
        Write-Say 'Note: no SHA256SUMS published for this release, skipping checksum.'
    }

    Expand-Archive -Path $archivePath -DestinationPath $tmp -Force
    $binaryPath = Get-ChildItem -Path $tmp -Filter $binary -Recurse -File |
        Select-Object -First 1
    if (-not $binaryPath) {
        Stop-Install "the archive did not contain a '$binary' binary"
    }

    $dir = $env:KRATE_INSTALL_DIR
    if (-not $dir) {
        # Per-user by default: no administrator rights, nothing outside the
        # user's own profile.
        $dir = Join-Path $env:LOCALAPPDATA 'Krate\bin'
    }
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    Copy-Item -Path $binaryPath.FullName -Destination (Join-Path $dir $binary) -Force

    Write-Say "Installed krate $version to $dir\$binary"

    # Being on PATH is what makes the next command work, so fix it rather than
    # leaving the person to discover the problem themselves.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$dir*") {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
        Write-Say "Added $dir to your PATH. Open a new terminal for it to take effect."
    }

    Write-Say ''
    Write-Say 'You can now open a .krate someone sends you.'
    Write-Say "To *make* your own apps with 'krate create', you also need the Rust"
    Write-Say "build tools. 'krate create' checks for them and offers to install"
    Write-Say "them on first use; 'krate doctor' shows what is present at any time."
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
