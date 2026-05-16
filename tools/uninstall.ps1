# oxdm uninstaller — Windows.
#
# Usage:
#   irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.ps1 | iex
#   # also wipe config / queue DB:
#   $env:OXDM_PURGE = "1"; irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/uninstall.ps1 | iex

[CmdletBinding()]
param(
  [string]$Dir = $env:OXDM_INSTALL_DIR,
  [switch]$Purge = ([bool]$env:OXDM_PURGE)
)

$ErrorActionPreference = 'Stop'
if (-not $Dir) { $Dir = Join-Path $env:LOCALAPPDATA 'Programs\oxdm' }

function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Ok($m)   { Write-Host "✓ $m" -ForegroundColor Green }
function Warn($m) { Write-Host "! $m" -ForegroundColor Yellow }

Step 'Removing binaries'
foreach ($n in 'oxdm.exe', 'oxdm-native-host.exe') {
  $p = Join-Path $Dir $n
  if (Test-Path $p) { Remove-Item $p -Force; Ok "removed $p" }
}
if ((Test-Path $Dir) -and -not (Get-ChildItem $Dir -Force | Where-Object { $_ })) {
  Remove-Item $Dir -Force; Ok "removed empty $Dir"
}

# Strip from user PATH.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath) {
  $cleaned = ($userPath -split ';' | Where-Object { $_ -and ($_ -ine $Dir) }) -join ';'
  if ($cleaned -ne $userPath) {
    [Environment]::SetEnvironmentVariable('Path', $cleaned, 'User')
    Ok "removed $Dir from user PATH"
  }
}

# Start menu shortcut.
$lnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\oxdm.lnk'
if (Test-Path $lnk) { Remove-Item $lnk -Force; Ok "removed $lnk" }

if ($Purge) {
  Step 'Purging user data'
  $cfg = Join-Path $env:APPDATA 'oxdm'
  if (Test-Path $cfg) { Remove-Item $cfg -Recurse -Force; Ok "removed $cfg" }
  $cfg2 = Join-Path $env:LOCALAPPDATA 'oxdm'
  if (Test-Path $cfg2) { Remove-Item $cfg2 -Recurse -Force; Ok "removed $cfg2" }
} else {
  Warn 'user data preserved (set $env:OXDM_PURGE = "1" to also delete settings + queue)'
}
