# oxdm installer — Windows (PowerShell 5+).
#
# Usage:
#   irm https://raw.githubusercontent.com/jd1378/oxdm/main/tools/install.ps1 | iex
#
# Optional env vars (set before piping):
#   $env:OXDM_INSTALL_DIR  = "C:\Tools\oxdm"     # default: %LOCALAPPDATA%\Programs\oxdm
#   $env:OXDM_VERSION      = "v0.1.0"             # default: latest

[CmdletBinding()]
param(
  [string]$Dir = $env:OXDM_INSTALL_DIR,
  [string]$Version = $env:OXDM_VERSION
)

$ErrorActionPreference = 'Stop'
$Repo = 'jd1378/oxdm'
if (-not $Dir)     { $Dir = Join-Path $env:LOCALAPPDATA 'Programs\oxdm' }
if (-not $Version) { $Version = 'latest' }

function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Info($m) { Write-Host "    $m" -ForegroundColor DarkGray }
function Ok($m)   { Write-Host "✓ $m" -ForegroundColor Green }
function Warn($m) { Write-Host "! $m" -ForegroundColor Yellow }
function Fail($m) { Write-Host "error: $m" -ForegroundColor Red; exit 1 }

Step 'Detecting platform'
$arch = (Get-CimInstance Win32_OperatingSystem).OSArchitecture
switch -Wildcard ($arch) {
  '64-bit*ARM*' { $target = 'aarch64-pc-windows-msvc' }
  '*ARM64*'     { $target = 'aarch64-pc-windows-msvc' }
  '64-bit*'     { $target = 'x86_64-pc-windows-msvc' }
  default       { Fail "unsupported windows arch: $arch" }
}
Info "Windows / $arch -> $target"

Step 'Resolving release'
if ($Version -eq 'latest') {
  try {
    $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
  } catch { Fail "failed to query latest release: $($_.Exception.Message)" }
  $tag = $rel.tag_name
  if (-not $tag) { Fail 'could not resolve latest tag' }
  Info "latest = $tag"
} else {
  $tag = $Version
  Info "pinned = $tag"
}

$asset = "oxdm-$tag-$target.zip"
$url   = "https://github.com/$Repo/releases/download/$tag/$asset"

$tmp = Join-Path $env:TEMP ("oxdm-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
$pkg = Join-Path $tmp $asset

try {
  Step "Downloading $asset"
  Info $url
  Invoke-WebRequest -Uri $url -OutFile $pkg -UseBasicParsing

  Step 'Extracting'
  Expand-Archive -Path $pkg -DestinationPath $tmp -Force

  $oxdm = Get-ChildItem -Path $tmp -Recurse -File -Filter 'oxdm.exe' | Select-Object -First 1
  $host_ = Get-ChildItem -Path $tmp -Recurse -File -Filter 'oxdm-native-host.exe' | Select-Object -First 1
  if (-not $oxdm)  { Fail "oxdm.exe not found in archive" }
  if (-not $host_) { Fail "oxdm-native-host.exe not found in archive" }

  Step "Installing to $Dir"
  if (-not (Test-Path $Dir)) { New-Item -ItemType Directory -Path $Dir | Out-Null }
  Copy-Item $oxdm.FullName  (Join-Path $Dir 'oxdm.exe')             -Force
  Copy-Item $host_.FullName (Join-Path $Dir 'oxdm-native-host.exe') -Force
  Ok "installed: $Dir\oxdm.exe"
  Ok "installed: $Dir\oxdm-native-host.exe"

  # Add to user PATH if missing.
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not ($userPath -split ';' | Where-Object { $_ -ieq $Dir })) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$Dir", 'User')
    Ok "added $Dir to user PATH (open a new terminal to use 'oxdm')"
  }

  # Start menu shortcut.
  try {
    $startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
    $lnk = Join-Path $startMenu 'oxdm.lnk'
    $w = New-Object -ComObject WScript.Shell
    $s = $w.CreateShortcut($lnk)
    $s.TargetPath = (Join-Path $Dir 'oxdm.exe')
    $s.IconLocation = (Join-Path $Dir 'oxdm.exe') + ',0'
    $s.Save()
    Ok "start-menu shortcut: $lnk"
  } catch {
    Warn "could not create start-menu shortcut: $($_.Exception.Message)"
  }

  # No WebView2 or toolkit check: the UI is rendered in software and
  # links nothing beyond the Windows API.

  Ok 'done. Run: oxdm'
}
finally {
  Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
