param(
    [string]$ReleaseDir = (Resolve-Path (Join-Path $PSScriptRoot "..\target\release")).Path,
    [string]$OutDir = "C:\Driver",
    [string]$PfxPath = "C:\Driver\kcombench.pfx",
    [string]$PfxPassword = "",
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [string[]]$Only = @("KcomBench", "KcomBenchFused"),
    [switch]$SkipSign
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Test-Admin {
    $current = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($current)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Ensure-Admin {
    if (-not (Test-Admin)) {
        throw "This script must be run as Administrator."
    }
}

function Get-PlainTextFromSecureString([Security.SecureString]$secure) {
    $ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try {
        return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr)
    } finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr)
    }
}

function Invoke-Sc([string[]]$Arguments, [switch]$IgnoreErrors) {
    $scPath = Join-Path $env:SystemRoot "System32\\sc.exe"
    $cmd = "$scPath $($Arguments -join ' ')"
    Write-Host $cmd
    & $scPath @Arguments | Out-Host
    if (-not $IgnoreErrors -and $LASTEXITCODE -ne 0) {
        throw "sc.exe failed: $($Arguments -join ' ') (exit $LASTEXITCODE)"
    }
}

function Ensure-Dir([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Path $Path | Out-Null
    }
}

function Ensure-Signtool {
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if (-not $cmd) {
        throw "signtool.exe not found in PATH. Install Windows SDK or add signtool to PATH."
    }
}

function Sign-File([string]$Path, [string]$PfxPath, [string]$PfxPassword, [string]$TimestampUrl) {
    $args = @(
        "sign",
        "/fd", "sha256",
        "/f", $PfxPath
    )
    if ($PfxPassword -ne "") {
        $args += @("/p", $PfxPassword)
    }
    if ($TimestampUrl -ne "") {
        $args += @("/tr", $TimestampUrl, "/td", "sha256")
    }
    $args += $Path

    Write-Host "signtool $($args -join ' ')"
    & signtool.exe @args | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed for $Path (exit $LASTEXITCODE)"
    }
}

Ensure-Admin
Ensure-Dir $OutDir

$driverMap = @(
    [pscustomobject]@{
        Name = "KcomBench"
        Dll  = "driver_test.dll"
        Sys  = "KcomBench.sys"
        DisplayName = "KcomBench"
    },
    [pscustomobject]@{
        Name = "KcomBenchFused"
        Dll  = "driver_test_fused.dll"
        Sys  = "KcomBenchFused.sys"
        DisplayName = "KcomBenchFused"
    }
)

$selected = $driverMap | Where-Object { $Only -contains $_.Name }
if (-not $selected) {
    throw "No drivers selected. Use -Only KcomBench and/or KcomBenchFused."
}

if (-not $SkipSign) {
    if ($PfxPath -eq "") {
        $PfxPath = Read-Host "PFX path (e.g. C:\\Driver\\kcombench.pfx)"
    }
    if (-not (Test-Path -LiteralPath $PfxPath)) {
        throw "PFX not found: $PfxPath"
    }
    if ($PfxPassword -eq "") {
        $secure = Read-Host -AsSecureString "PFX password"
        $PfxPassword = Get-PlainTextFromSecureString $secure
    }
    Ensure-Signtool
}

# Best-effort stop/delete up front in case a driver is still running.
foreach ($drv in $selected) {
    Invoke-Sc @("stop", $drv.Name) -IgnoreErrors
    Invoke-Sc @("delete", $drv.Name) -IgnoreErrors
}
Start-Sleep -Milliseconds 200

foreach ($drv in $selected) {
    $src = Join-Path $ReleaseDir $drv.Dll
    $dst = Join-Path $OutDir $drv.Sys

    if (-not (Test-Path -LiteralPath $src)) {
        throw "Source DLL not found: $src"
    }

    Write-Host "Copy: $src -> $dst"
    Copy-Item -LiteralPath $src -Destination $dst -Force

    if (-not $SkipSign) {
        Sign-File -Path $dst -PfxPath $PfxPath -PfxPassword $PfxPassword -TimestampUrl $TimestampUrl
    }

    Invoke-Sc @("query", $drv.Name) -IgnoreErrors
    Invoke-Sc @("stop", $drv.Name) -IgnoreErrors
    Invoke-Sc @("delete", $drv.Name) -IgnoreErrors
    Start-Sleep -Milliseconds 200

    $binPath = $dst
    Invoke-Sc @(
        "create",
        $drv.Name,
        "type=", "kernel",
        "start=", "demand",
        "binPath=", $binPath,
        "DisplayName=", $drv.DisplayName
    )
    Invoke-Sc @("start", $drv.Name)
}
