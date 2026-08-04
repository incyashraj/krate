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

# Rank a version tag so the newest sorts highest, regardless of API or page
# order. A final release outranks any of its own pre-releases (rc), and a
# higher rc number outranks a lower one. Returns a single sortable number.
function rc_rank {
    param([string]$Tag)
    if ($Tag -match '^v(\d+)\.(\d+)\.(\d+)(?:-rc(\d+))?$') {
        $base = ([int]$Matches[1]) * 1000000 + ([int]$Matches[2]) * 10000 + ([int]$Matches[3]) * 100
        # No rc suffix is the final release: rank it above every rc of the same
        # version by giving it 99, and an rc its own number.
        if ($Matches[4]) { return $base + [int]$Matches[4] } else { return $base + 99 }
    }
    return -1
}

# Intel and ARM Windows both publish a build. ARM matters more than its share
# of desktops suggests: a Windows VM on an Apple Silicon Mac is ARM, and that
# is the ordinary way to try Windows without owning a Windows machine.
#
# PROCESSOR_ARCHITECTURE reports the architecture of the *process*, so a
# 32-bit PowerShell on a 64-bit machine says x86. PROCESSOR_ARCHITEW6432 is
# set only in that case and carries the machine's real architecture, so prefer
# it when present rather than refusing a machine that is perfectly supported.
$arch = $env:PROCESSOR_ARCHITEW6432
if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
switch ($arch) {
    'AMD64' { $target = 'x86_64-pc-windows-msvc' }
    'ARM64' { $target = 'aarch64-pc-windows-msvc' }
    default {
        Stop-Install "unsupported architecture: $arch (install from source instead)"
    }
}

$version = $env:KRATE_VERSION
if (-not $version) {
    Write-Say 'Finding the latest release...'
    # /releases/latest excludes pre-releases and Krate is pre-release only, so
    # query the list and take the newest v-tag instead.
    try {
        $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases?per_page=30" `
            -Headers @{ 'User-Agent' = 'krate-installer' }
        # Sort by version, do not trust the array order. GitHub returns
        # releases newest-created-first, but a re-tag or an out-of-order push
        # can put an older tag ahead of a newer one -- which shipped rc9 to a
        # machine when rc14 was current. Rank by the numeric rc suffix so the
        # highest always wins.
        $version = ($releases |
            Where-Object { $_.tag_name -match '^v\d+\.\d+\.\d+(-rc(\d+))?$' } |
            Sort-Object -Descending { rc_rank $_.tag_name } |
            Select-Object -First 1).tag_name
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
            $tags = [regex]::Matches($page.Content, $pattern) |
                ForEach-Object { $_.Groups[1].Value } |
                Sort-Object -Unique
            # Highest version among every tag on the page, not the first the
            # page happened to render.
            $version = $tags | Sort-Object -Descending { rc_rank $_ } | Select-Object -First 1
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
        Write-Host '  $env:KRATE_VERSION="v0.1.0-rc19"; irm https://krate.tech/install.ps1 | iex'
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
    #
    # Two separate things have to happen, and only doing the first is why
    # `krate --version` came back "not recognized" straight after a successful
    # install: SetEnvironmentVariable writes the *persisted* PATH, which is
    # read by processes started afterwards. The window the installer ran in
    # already has its copy and never sees the change. So update this session
    # too, and the person can carry straight on in the terminal they are
    # already sitting in.
    #
    # Compared entry by entry rather than with -like: a path is matched as a
    # whole segment, so C:\Krate\bin does not count as already present because
    # C:\Krate\bin\extra happens to contain it, and characters PowerShell
    # treats as wildcards cannot corrupt the test.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @()
    if ($userPath) { $entries = $userPath -split ';' | Where-Object { $_ } }
    $already = $entries | Where-Object { $_.TrimEnd('\') -ieq $dir.TrimEnd('\') }
    if (-not $already) {
        $updated = (($entries + $dir) -join ';')
        [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
        Write-Say "Added $dir to your PATH."
    }
    # This session, so the next command works here and not only in a new window.
    if (($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $dir.TrimEnd('\') }).Count -eq 0) {
        $env:Path = "$env:Path;$dir"
    }

    # Register the .krate double-click handler so a bundle someone sends you
    # opens by double-clicking it in Explorer, not only from the command line.
    # The logic lives in scripts/install-krate-desktop.ps1, which writes a
    # per-user (HKCU, no admin) association whose open command is
    # `"<krate.exe>" run "%1" --consent`. A curl/irm install has no repo
    # checkout, so fetch that script from GitHub and run it against the exe we
    # just placed. This is best-effort: a failure here (offline, blocked fetch,
    # odd environment) must not fail the install, which already succeeded as a
    # working CLI. Set $env:KRATE_NO_DESKTOP=1 to skip it (matches the Unix
    # KRATE_NO_DESKTOP=1 opt-out).
    $installedBinary = Join-Path $dir $binary
    if ($env:KRATE_NO_DESKTOP -eq '1') {
        Write-Say 'Skipping the .krate file association (KRATE_NO_DESKTOP=1).'
    } else {
        try {
            $desktopScript = Join-Path $tmp 'install-krate-desktop.ps1'
            $desktopUrl = "https://raw.githubusercontent.com/$repo/main/scripts/install-krate-desktop.ps1"
            Invoke-WebRequest -Uri $desktopUrl -OutFile $desktopScript -UseBasicParsing
            # Run the fetched script in its own PowerShell with the execution
            # policy relaxed for that process only, so a machine whose policy
            # blocks scripts still gets the association. Pass the absolute path
            # to the exe we just installed; install-krate-desktop.ps1 bakes that
            # path into the open command, so double-click works without krate
            # being on PATH.
            & powershell -NoProfile -ExecutionPolicy Bypass -File $desktopScript -KrateBinary $installedBinary | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "association script exited with code $LASTEXITCODE" }
            Write-Say 'Registered the .krate handler -- you can double-click a .krate now.'
        } catch {
            Write-Say 'Note: could not register the .krate double-click handler; the CLI still works.'
            Write-Say "You can run it later: krate is installed, and you can always use 'krate run <file>'."
        }
    }

    Write-Say ''
    Write-Say 'You can now open a .krate someone sends you. Try this:'
    Write-Say '  krate run https://krate.tech/cubes.krate'
    Write-Say ''
    Write-Say 'If a new terminal says "krate is not recognized", run it by full path:'
    Write-Say "  & `"$dir\$binary`" --version"
    Write-Say "To *make* your own apps with 'krate create', you also need the Rust"
    Write-Say "build tools. 'krate create' checks for them and offers to install"
    Write-Say "them on first use; 'krate doctor' shows what is present at any time."
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
