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
  Remove-Item -Path $extKey  -Recurse -ErrorAction SilentlyContinue
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
foreach ($src in @($DocIcon, "dist\icon\KrateDoc.ico", "docs\landing\krate-document-icon.png")) {
  if ($src -ne "" -and (Test-Path $src)) { $icon = (Resolve-Path $src).Path; break }
}

# 1. The .krate extension points at our ProgID.
New-Item -Path $extKey -Force | Out-Null
Set-ItemProperty -Path $extKey -Name "(default)" -Value $progId
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
