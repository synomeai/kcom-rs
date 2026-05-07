param(
  [string[]]$Device = @("\\.\KcomBench","\\.\KcomBenchFused"),

  [ValidateSet("ioctl-echo","ioctl-file","ioctl-file-async","file")]
  [string[]]$Mode = @("ioctl-file","ioctl-file-async","file"),

  [ValidateSet("read","write")]
  [string]$Op = "write",

  [UInt64]$Size = 4096,
  [UInt64]$Iterations = 200000,
  [UInt64]$Parallelism = 8,
  [UInt64]$Warmup = 0,
  [int]$Runs = 1,

  [UInt64]$Offset = 0,
  [UInt64]$OffsetStep,
  [switch]$Prepare,

  [string]$Path = "C:\\Driver\\kcom_io_test.bin",
  [string]$KernelCsv = "io_kernel.csv",
  [string]$UserCsv = "io_user.csv",
  [switch]$Append,

  [ValidateSet("debug","release")]
  [string]$Profile = "release",

  [string]$Cargo = "cargo",

  [switch]$SkipKernel,
  [switch]$SkipUser
)

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")

$baseArgs = @(
  "--size", $Size.ToString(),
  "--iterations", $Iterations.ToString(),
  "--parallelism", $Parallelism.ToString(),
  "--warmup", $Warmup.ToString(),
  "--runs", $Runs.ToString(),
  "--op", $Op,
  "--path", $Path,
  "--offset", $Offset.ToString()
)

if ($PSBoundParameters.ContainsKey('OffsetStep')) {
  $baseArgs += @("--offset-step", $OffsetStep.ToString())
}
if ($Prepare) { $baseArgs += "--prepare" }
if ($Append) { $baseArgs += "--append" }

$profileArgs = @("run", "-p", "io-bench")
if ($Profile -eq "release") { $profileArgs += "--release" }

$kernelModes = $Mode | Where-Object { $_ -ne "file" }
$userModes = $Mode | Where-Object { $_ -eq "file" }

if (-not $SkipKernel) {
  foreach ($dev in $Device) {
    foreach ($m in $kernelModes) {
      $args = @($profileArgs + @("--", "--mode", $m, "--device", $dev, "--out-csv", $KernelCsv) + $baseArgs)

      Push-Location $repoRoot
      try {
        & $Cargo @args
      } finally {
        Pop-Location
      }
    }
  }
}

if (-not $SkipUser) {
  foreach ($m in $userModes) {
    $args = @($profileArgs + @("--", "--mode", $m, "--out-csv", $UserCsv) + $baseArgs)

    Push-Location $repoRoot
    try {
      & $Cargo @args
    } finally {
      Pop-Location
    }
  }
}
