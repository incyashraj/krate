# Register the .krate double-click handler on Windows (the counterpart of
# scripts/make-macos-app.sh for macOS and install-krate-desktop.sh for Linux).
#
# It writes a per-user file association so double-clicking a .krate opens the
# app behind the consent flow (`krate run <file> --consent`). Per-user means no
# administrator rights are needed; it only touches HKCU.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\install-krate-desktop.ps1
#   powershell ... install-krate-desktop.ps1 -Uninstall
#   Set -KrateBinary to point at krate.exe; otherwise it is found on PATH.

param(
  [string]$KrateBinary = "",
  [string]$DocIcon = "",
  [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$progId  = "Krate.Bundle"
$extKey  = "HKCU:\Software\Classes\.krate"
$progKey = "HKCU:\Software\Classes\$progId"

if ($Uninstall) {
  Remove-Item -Path $progKey -Recurse -ErrorAction SilentlyContinue
  # Only take the extension down with us when it is OURS. Uninstalling the CLI
  # from a machine whose .krate belongs to Krate Studio must not break the
  # Studio's double-click (K-166).
  $currentProgId = (Get-ItemProperty -Path $extKey -Name "(default)" -ErrorAction SilentlyContinue)."(default)"
  if ($currentProgId -eq $progId -or -not $currentProgId) {
    Remove-Item -Path $extKey -Recurse -ErrorAction SilentlyContinue
  }
  Write-Host "removed the Krate .krate association"
  exit 0
}

# Resolve krate.exe: -KrateBinary, else PATH, else the local build.
function Resolve-Krate {
  if ($KrateBinary -ne "") { return (Resolve-Path $KrateBinary).Path }
  $onPath = Get-Command krate.exe -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }
  foreach ($p in @("target\release\krate.exe", "target\debug\krate.exe")) {
    if (Test-Path $p) { return (Resolve-Path $p).Path }
  }
  throw "could not find krate.exe; pass -KrateBinary"
}

$binary = Resolve-Krate
if (-not (Test-Path $binary)) { throw "krate.exe not found: $binary" }

# The document icon, if a source .ico/.png is available.
$icon = ""
# Beside the binary first: that is where a real install puts it, and the repo
# paths below only exist in a checkout.
$besideBinary = Join-Path (Split-Path $binary) "KrateDoc.ico"
foreach ($src in @($DocIcon, $besideBinary, "dist\icon\KrateDoc.ico", "docs\landing\krate-document-icon.png")) {
  if ($src -ne "" -and (Test-Path $src)) { $icon = (Resolve-Path $src).Path; break }
}

# 1. The .krate extension points at our ProgID -- unless Krate Studio already
# owns it and is really installed. The Studio registers Krate.App with its own
# opener; a CLI install (or update) that steals the extension away from a
# working Studio downgrades the person's double-click for no reason, and the
# Studio's next first-run would steal it back -- two installers fighting over
# one key (K-166). Ours still lands in OpenWithProgids either way, so
# "Open with" always offers Krate.
$currentProgId = (Get-ItemProperty -Path $extKey -Name "(default)" -ErrorAction SilentlyContinue)."(default)"
$studioOwnsIt = $false
if ($currentProgId -eq "Krate.App") {
  $studioCmd = (Get-ItemProperty -Path "HKCU:\Software\Classes\Krate.App\shell\open\command" -Name "(default)" -ErrorAction SilentlyContinue)."(default)"
  if ($studioCmd -match '"([^"]+)"') { $studioOwnsIt = Test-Path $Matches[1] }
}
New-Item -Path $extKey -Force | Out-Null
if ($studioOwnsIt) {
  Write-Host "keeping the existing Krate Studio association for .krate"
} else {
  Set-ItemProperty -Path $extKey -Name "(default)" -Value $progId
}
New-Item -Path "$extKey\OpenWithProgids" -Force | Out-Null
Set-ItemProperty -Path "$extKey\OpenWithProgids" -Name $progId -Value ([byte[]]@()) -Type Binary

# The double-click handler is krate-open.exe, not krate.exe.
#
# krate.exe is a console application, so Explorer allocated a console for it
# and a black window sat beside the app for the whole session. krate-open.exe
# is built for the "windows" subsystem, which is the only way to avoid that; it
# hands the file straight to `krate run --consent`, so there is one runner
# rather than two that can drift.
#
# Falls back to krate.exe when the opener is missing (an older install, a build
# that predates it): a console window is worse than nothing happening.
$opener = Join-Path (Split-Path $binary) "krate-open.exe"
if (-not (Test-Path $opener)) { $opener = $binary }

# 2. The ProgID: friendly name, icon, and the open command.
New-Item -Path $progKey -Force | Out-Null
Set-ItemProperty -Path $progKey -Name "(default)" -Value "Krate app bundle"
if ($icon -ne "") {
  New-Item -Path "$progKey\DefaultIcon" -Force | Out-Null
  Set-ItemProperty -Path "$progKey\DefaultIcon" -Name "(default)" -Value "`"$icon`""
}
New-Item -Path "$progKey\shell\open\command" -Force | Out-Null
Set-ItemProperty -Path "$progKey\shell\open\command" -Name "(default)" `
  -Value "`"$opener`" `"%1`""

# 3. Nudge Explorer to pick up the new association.
$sig = '[System.Runtime.InteropServices.DllImport("shell32.dll")] public static extern void SHChangeNotify(int eventId, int flags, System.IntPtr item1, System.IntPtr item2);'
$sh = Add-Type -MemberDefinition $sig -Name KrateShell -Namespace Win32 -PassThru
$sh::SHChangeNotify(0x08000000, 0, [System.IntPtr]::Zero, [System.IntPtr]::Zero)

Write-Host "registered .krate to open with Krate (binary: $binary)"
Write-Host "double-click a .krate in Explorer to try it."
