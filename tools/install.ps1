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
# Not `exit`: under `irm | iex` that closes the window, error and all.
function Fail($m) { throw "error: $m" }

# Windows refuses to overwrite a program that is running. Every oxdm
# process runs from $Dir: the daemon, its windows, and the browser
# bridge, which the browser starts on its own.
function Get-OxdmProcess {
  $full = [IO.Path]::GetFullPath($Dir).TrimEnd('\')
  Get-Process -Name 'oxdm*' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ((Split-Path $_.Path -Parent) -ieq $full) }
}

# Renaming a running program is allowed where overwriting it is not;
# oxdm deletes the `.oxdm-old` copy on its next start, as after an
# update.
function Install-Binary($src, $name) {
  $dst = Join-Path $Dir $name
  try { Copy-Item $src $dst -Force }
  catch {
    if (-not (Test-Path -LiteralPath $dst)) { throw }
    $old = [IO.Path]::ChangeExtension($dst, 'oxdm-old')
    Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
    Move-Item -LiteralPath $dst -Destination $old
    # Put the old program back rather than leave none.
    try { Copy-Item $src $dst -Force }
    catch { Move-Item -LiteralPath $old -Destination $dst -Force; throw }
  }
  Ok "installed: $dst"
}

# A .NET task, waited on in short slices: a blocking wait would hold
# off Ctrl+C until the network answered, which may be never. Gives up
# after a minute without progress.
function Wait-Task($task) {
  $idle = [Diagnostics.Stopwatch]::StartNew()
  while (-not $task.AsyncWaitHandle.WaitOne(100)) {
    if ($idle.Elapsed.TotalSeconds -ge 60) { throw 'no data received for 60 seconds' }
  }
  $task.GetAwaiter().GetResult()
}

# Streamed by hand for a progress bar: Windows PowerShell 5.1's
# `Invoke-WebRequest` shows only a byte count, and redraws it so often
# that it slows the download down several times over.
function Get-File($uri, $out, [switch]$Quiet) {
  Add-Type -AssemblyName System.Net.Http
  $client = New-Object System.Net.Http.HttpClient
  $cancel = New-Object System.Threading.CancellationTokenSource
  $activity = "Downloading $(Split-Path $out -Leaf)"
  $err = $null
  try {
    $resp = Wait-Task ($client.GetAsync($uri, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead, $cancel.Token))
    if (-not $resp.IsSuccessStatusCode) {
      $err = "HTTP $([int]$resp.StatusCode)"
    } else {
      $total = $resp.Content.Headers.ContentLength
      $in = Wait-Task ($resp.Content.ReadAsStreamAsync())
      $fs = [IO.File]::Create($out)
      try {
        $buf = New-Object byte[] 65536
        $done = 0L
        $tick = [Diagnostics.Stopwatch]::StartNew()
        while (($n = Wait-Task ($in.ReadAsync($buf, 0, $buf.Length, $cancel.Token))) -gt 0) {
          $fs.Write($buf, 0, $n)
          $done += $n
          if ($Quiet -or $tick.ElapsedMilliseconds -lt 200) { continue }
          $tick.Restart()
          if ($total) {
            $status = '{0:N1} of {1:N1} MB' -f ($done / 1MB), ($total / 1MB)
            Write-Progress -Activity $activity -Status $status -PercentComplete ([int](100 * $done / $total))
          } else {
            Write-Progress -Activity $activity -Status ('{0:N1} MB' -f ($done / 1MB))
          }
        }
      } finally {
        $fs.Dispose()
        $in.Dispose()
        if (-not $Quiet) { Write-Progress -Activity $activity -Completed }
      }
      if (-not $Quiet) { Info ('{0:N1} MB' -f ($done / 1MB)) }
    }
  } catch {
    # The base exception: on .NET Framework the outer one says only
    # "An error occurred while sending the request".
    $err = $_.Exception.GetBaseException().Message
  } finally {
    # Also reached on Ctrl+C, which leaves the request running unless
    # it is cancelled here.
    $cancel.Cancel()
    $client.Dispose()
    $cancel.Dispose()
  }
  if ($err) { Fail "download failed: $err ($uri)" }
}

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

# One archive per target on every platform, so the updater and a
# person downloading by hand take the same file. `tar.exe` is bsdtar,
# shipped with Windows since 10 1803.
$asset = "oxdm-$tag-$target.tar.gz"
$url   = "https://github.com/$Repo/releases/download/$tag/$asset"

$tmp = Join-Path $env:TEMP ("oxdm-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
$pkg = Join-Path $tmp $asset

try {
  Step "Downloading $asset"
  Info $url
  Get-File $url $pkg

  # Every asset is published with its digest beside it; checking it is
  # the difference between "downloaded from GitHub" and "downloaded
  # what the release actually built". No digest means no install: an
  # unverified archive is the one case this check exists for.
  Step 'Verifying'
  $shaFile = "$pkg.sha256"
  Get-File "$url.sha256" $shaFile -Quiet
  # `<hash>  <name>`, or `<hash> *<name>` when it was written in
  # binary mode; the digest is the first field either way.
  $want = @((Get-Content -Raw -Path $shaFile) -split '\s+' | Where-Object { $_ })[0]
  if ($want -notmatch '^[0-9a-fA-F]{64}$') { Fail "no sha256 digest in $url.sha256" }
  $got = (Get-FileHash -Path $pkg -Algorithm SHA256).Hash.ToLower()
  if ($want.ToLower() -ne $got) { Fail "checksum mismatch: expected $want, got $got" }
  Ok 'sha256 matches'

  Step 'Extracting'
  tar -xzf $pkg -C $tmp
  if ($LASTEXITCODE -ne 0) {
    throw "could not unpack $pkg (needs tar.exe, present on Windows 10 1803 and later)"
  }

  $oxdm = Get-ChildItem -Path $tmp -Recurse -File -Filter 'oxdm.exe' | Select-Object -First 1
  $host_ = Get-ChildItem -Path $tmp -Recurse -File -Filter 'oxdm-native-host.exe' | Select-Object -First 1
  if (-not $oxdm)  { Fail "oxdm.exe not found in archive" }

  if (Get-OxdmProcess) {
    Step 'Stopping oxdm'
    # Asked first so downloads are paused with their progress saved.
    # Not relied on: the kill below covers a daemon that does not
    # answer, and `Install-Binary` a process that outlives the kill.
    try {
      $q = Start-Process -FilePath (Join-Path $Dir 'oxdm.exe') -ArgumentList '--quit' -WindowStyle Hidden -PassThru
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

  Step "Installing to $Dir"
  if (-not (Test-Path $Dir)) { New-Item -ItemType Directory -Path $Dir | Out-Null }
  Install-Binary $oxdm.FullName 'oxdm.exe'
  # Not fatal when an archive lacks it: oxdm runs without the browser
  # bridge, minus that integration.
  if ($host_) {
    Install-Binary $host_.FullName 'oxdm-native-host.exe'
    # Lets the extension reach oxdm before its first launch. Start-Process
    # because oxdm.exe is a GUI program, which `&` does not wait for.
    Step 'Registering with browsers'
    $log = Join-Path $tmp 'native-host.txt'
    try {
      $p = Start-Process -FilePath (Join-Path $Dir 'oxdm.exe') -ArgumentList '--install-native-host' `
        -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err"
      # oxdm writes UTF-8, which PS 5.1 would otherwise read as ANSI.
      Get-Content -Path $log, "$log.err" -Encoding UTF8 -ErrorAction SilentlyContinue | ForEach-Object { Info $_ }
      if ($p.ExitCode -eq 0) { Ok 'browser integration registered' }
      else { Warn "registration exited with $($p.ExitCode); run 'oxdm --install-native-host' to retry" }
    } catch {
      Warn "could not register with browsers: $($_.Exception.Message); run 'oxdm --install-native-host' to retry"
    }
  } else {
    Warn "'oxdm-native-host.exe' is not in this archive - browser integration will be unavailable."
  }

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
