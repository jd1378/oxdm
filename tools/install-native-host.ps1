# Register oxdm as a native-messaging host with the browsers on this
# machine, so an extension can hand downloads to the app.
#
# Usage:
#   .\install-native-host.ps1 [-ChromiumId <ID>[,<ID>...]] `
#                             [-FirefoxId <ID>[,<ID>...]] `
#                             [-HostBinary <PATH>] [-Oxdm <PATH>] [-DryRun]
#
# The work itself lives in the app (`oxdm --install-native-host`),
# which writes the manifests under %LOCALAPPDATA%\oxdm and registers
# them under HKCU. This script only locates the binary and forwards the
# flags — a script carrying its own copy of the registry layout is a
# second source of truth waiting to disagree with the first.
#
# oxdm also does this by itself: on first run, and again on every start
# if a registration has gone missing or stale.
#
# Per-user, no admin rights required.

[CmdletBinding()]
param(
    [string[]] $ChromiumId = @(),
    [string[]] $FirefoxId  = @(),
    [string]   $HostBinary = "",
    [string]   $Oxdm       = "",
    [switch]   $DryRun
)

$ErrorActionPreference = 'Stop'

if (-not $Oxdm) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    $candidate = Join-Path $scriptDir 'oxdm.exe'
    if (Test-Path $candidate) {
        $Oxdm = $candidate
    } else {
        $cmd = Get-Command oxdm -ErrorAction SilentlyContinue
        if ($cmd) { $Oxdm = $cmd.Source }
        else { throw "Could not find oxdm.exe; pass -Oxdm <PATH>." }
    }
}
if (-not (Test-Path $Oxdm)) { throw "oxdm not found: $Oxdm" }

$argv = @('--install-native-host')
if ($ChromiumId.Count -gt 0) { $argv += @('--chromium-id', ($ChromiumId -join ',')) }
if ($FirefoxId.Count  -gt 0) { $argv += @('--firefox-id',  ($FirefoxId  -join ',')) }
if ($HostBinary)             { $argv += @('--host-binary', $HostBinary) }
if ($DryRun)                 { $argv += '--dry-run' }

& $Oxdm @argv
exit $LASTEXITCODE
