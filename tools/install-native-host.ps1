# Install the oxdm native-messaging manifest for one or more browsers (Windows).
#
# Windows uses HKCU registry keys to point browsers at native-messaging
# manifests. This script:
#   1. Writes the manifest JSON to
#      %LOCALAPPDATA%\oxdm\io.github.jd1378.oxdm.host.<browser>.json
#   2. Registers it under
#      HKCU:\Software\<vendor>\<browser>\NativeMessagingHosts\io.github.jd1378.oxdm.host
#
# Usage:
#   .\install-native-host.ps1 -ChromiumId <ID>[,<ID>...] `
#                             [-FirefoxId <ID>[,<ID>...]] `
#                             [-HostBinary <PATH>] [-DryRun]
#
# Per-user, no admin rights required.

[CmdletBinding()]
param(
    [string[]] $ChromiumId = @(),
    [string[]] $FirefoxId  = @(),
    [string]   $HostBinary = "",
    [switch]   $DryRun
)

$ErrorActionPreference = 'Stop'
$HostName = 'io.github.jd1378.oxdm.host'

if (-not $ChromiumId -and -not $FirefoxId) {
    throw "Supply at least one of -ChromiumId / -FirefoxId."
}

if (-not $HostBinary) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    $candidate = Join-Path $scriptDir 'oxdm-native-host.exe'
    if (Test-Path $candidate) {
        $HostBinary = $candidate
    } else {
        $cmd = Get-Command oxdm-native-host -ErrorAction SilentlyContinue
        if ($cmd) { $HostBinary = $cmd.Source }
        else { throw "Could not locate oxdm-native-host.exe; pass -HostBinary <PATH>." }
    }
}
if (-not (Test-Path $HostBinary)) {
    throw "Host binary not found: $HostBinary"
}
if (-not [System.IO.Path]::IsPathRooted($HostBinary)) {
    throw "-HostBinary must be an absolute path: $HostBinary"
}

$manifestDir = Join-Path $env:LOCALAPPDATA 'oxdm'
if (-not $DryRun) { New-Item -ItemType Directory -Force -Path $manifestDir | Out-Null }

function Write-Manifest {
    param([string]$Variant, [hashtable]$Body)
    $path = Join-Path $manifestDir "$HostName.$Variant.json"
    $json = $Body | ConvertTo-Json -Depth 4
    if ($DryRun) {
        Write-Output "would write $path :`n$json`n"
    } else {
        Set-Content -Path $path -Value $json -Encoding UTF8
        Write-Output "wrote $path"
    }
    return $path
}

function Register-RegistryKey {
    param([string]$RegPath, [string]$ManifestPath)
    if ($DryRun) {
        Write-Output "would register $RegPath -> $ManifestPath"
        return
    }
    New-Item -Path $RegPath -Force | Out-Null
    Set-ItemProperty -Path $RegPath -Name '(default)' -Value $ManifestPath
    Write-Output "registered $RegPath"
}

# Chromium family
if ($ChromiumId.Count -gt 0) {
    $origins = @($ChromiumId | ForEach-Object { "chrome-extension://$_/" })
    $body = @{
        name = $HostName
        description = 'oxdm download capture host'
        path = $HostBinary
        type = 'stdio'
        allowed_origins = $origins
    }
    $manifestPath = Write-Manifest 'chromium' $body
    $vendors = @(
        'Software\Google\Chrome',
        'Software\Chromium',
        'Software\Microsoft\Edge',
        'Software\BraveSoftware\Brave-Browser',
        'Software\Vivaldi'
    )
    foreach ($v in $vendors) {
        Register-RegistryKey "HKCU:\$v\NativeMessagingHosts\$HostName" $manifestPath
    }
}

# Firefox family
if ($FirefoxId.Count -gt 0) {
    $body = @{
        name = $HostName
        description = 'oxdm download capture host'
        path = $HostBinary
        type = 'stdio'
        allowed_extensions = $FirefoxId
    }
    $manifestPath = Write-Manifest 'firefox' $body
    $vendors = @(
        'Software\Mozilla',
        'Software\LibreWolf'
    )
    foreach ($v in $vendors) {
        Register-RegistryKey "HKCU:\$v\NativeMessagingHosts\$HostName" $manifestPath
    }
}
