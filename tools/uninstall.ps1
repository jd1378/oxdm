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
$exe = Join-Path $Dir 'oxdm.exe'

function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Ok($m)   { Write-Host "✓ $m" -ForegroundColor Green }
function Warn($m) { Write-Host "! $m" -ForegroundColor Yellow }

# Windows refuses to delete a program that is running. Every oxdm
# process runs from $Dir: the daemon, its windows, and the browser
# bridge, which the browser starts on its own.
function Get-OxdmProcess {
  $full = [IO.Path]::GetFullPath($Dir).TrimEnd('\')
  Get-Process -Name 'oxdm*' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ((Split-Path $_.Path -Parent) -ieq $full) }
}

if (Get-OxdmProcess) {
  Step 'Stopping oxdm'
  # Asked first so downloads are paused with their progress saved. Not
  # relied on: the kill below covers a daemon that does not answer.
  try {
    $q = Start-Process -FilePath $exe -ArgumentList '--quit' -WindowStyle Hidden -PassThru
    $null = $q.WaitForExit(10000)
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-OxdmProcess | Where-Object ProcessName -eq 'oxdm') -and (Get-Date) -lt $deadline) {
      Start-Sleep -Milliseconds 250
    }
  } catch {
    Warn "could not ask oxdm to quit: $($_.Exception.Message)"
  }
  Get-OxdmProcess | Stop-Process -Force -ErrorAction SilentlyContinue
  Ok 'oxdm stopped'
}

Step 'Removing binaries'
# `*.oxdm-old` are programs an update renamed aside while they ran.
$bins = @('oxdm.exe', 'oxdm-native-host.exe' | ForEach-Object { Join-Path $Dir $_ })
if (Test-Path -LiteralPath $Dir) {
  $bins += @(Get-ChildItem -LiteralPath $Dir -Filter '*.oxdm-old' -Force | ForEach-Object FullName)
}
foreach ($p in $bins) {
  if (-not (Test-Path -LiteralPath $p)) { continue }
  # A killed process can hold its file for a moment, and a browser can
  # restart the bridge in between.
  for ($i = 1; ; $i++) {
    try { Remove-Item -LiteralPath $p -Force; break }
    catch {
      if ($i -ge 10) {
        throw "could not remove ${p}: $($_.Exception.Message) Quit oxdm from its tray icon, close browsers using the oxdm extension, and run this again."
      }
      Get-OxdmProcess | Stop-Process -Force -ErrorAction SilentlyContinue
      Start-Sleep -Milliseconds 500
    }
  }
  Ok "removed $p"
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

# Login autostart (Settings > start with system), only when it names
# the binary just removed.
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$auto = (Get-ItemProperty -Path $run -Name 'oxdm' -ErrorAction SilentlyContinue).oxdm
if ($auto -and $auto.IndexOf($exe, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
  Remove-ItemProperty -Path $run -Name 'oxdm'; Ok 'removed login autostart'
}

if ($Purge) {
  Step 'Purging user data'
  $cfg = Join-Path $env:APPDATA 'oxdm'
  if (Test-Path $cfg) { Remove-Item $cfg -Recurse -Force; Ok "removed $cfg" }
  $cfg2 = Join-Path $env:LOCALAPPDATA 'oxdm'
  if (Test-Path $cfg2) { Remove-Item $cfg2 -Recurse -Force; Ok "removed $cfg2" }

  # The browser registrations name a binary that is now gone. The
  # manifests went with %LOCALAPPDATA%\oxdm above; these are the keys
  # that point at them.
  $hostName = 'io.github.jd1378.oxdm.host'
  $vendors = @(
    'Software\Google\Chrome', 'Software\Chromium', 'Software\Microsoft\Edge',
    'Software\BraveSoftware\Brave-Browser', 'Software\Vivaldi',
    'Software\Mozilla', 'Software\LibreWolf'
  )
  foreach ($v in $vendors) {
    $key = "HKCU:\$v\NativeMessagingHosts\$hostName"
    if (Test-Path $key) { Remove-Item $key -Force -Recurse; Ok "removed $key" }
  }
} else {
  Warn 'user data preserved (set $env:OXDM_PURGE = "1" to also delete settings + queue)'
}
