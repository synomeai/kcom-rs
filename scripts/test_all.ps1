$ErrorActionPreference = "Stop"

function Run-Test {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $false)][string[]]$Args = @(),
        [Parameter(Mandatory = $true)][string]$Package
    )

    Write-Host ""
    Write-Host "==> $Name ($Package)"
    if ($Args.Count -gt 0) {
        Write-Host " cargo +nightly miri test -p $Package $($Args -join ' ')"
    } else {
        Write-Host " cargo +nightly miri test -p $Package"
    }

    & cargo +nightly miri test -p $Package @Args
}

function Run-TestPair {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $false)][string[]]$Args = @()
    )

    Run-Test -Name $Name -Package "kcom" -Args $Args
    Run-Test -Name $Name -Package "kcom-tests" -Args $Args
}

Run-TestPair -Name "default"
Run-TestPair -Name "async-com" -Args @("--features", "async-com")
Run-TestPair -Name "kernel-unicode" -Args @("--features", "kernel-unicode")
Run-TestPair -Name "refcount-hardening" -Args @("--features", "refcount-hardening")
Run-TestPair -Name "combo" -Args @("--features", "async-com kernel-unicode refcount-hardening")
Run-TestPair -Name "wdk-alloc-align" -Args @("--features", "driver wdk-alloc-align driver-test-stub")
Run-TestPair -Name "driver-miri" -Args @("--features", "driver async-com-kernel driver-test-stub")
