param(
  [UInt64[]]$Sizes = @(4096, 65536, 1048576),
  [ValidateSet("read","write")]
  [string[]]$Ops = @("write","read"),

  [string[]]$Device = @("\\.\KcomBench","\\.\KcomBenchFused"),
  [ValidateSet("ioctl-echo","ioctl-file","ioctl-file-async","file")]
  [string[]]$Mode = @("ioctl-file","ioctl-file-async","file"),

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
  [switch]$Append
)

$benchScript = Join-Path $PSScriptRoot "bench_io.ps1"
if (-not (Test-Path $benchScript)) {
  throw "bench_io.ps1 not found at $benchScript"
}

foreach ($op in $Ops) {
  foreach ($size in $Sizes) {
    $args = @{
      Mode = $Mode
      Device = $Device
      Op = $op
      Size = [uint64]$size
      Iterations = $Iterations
      Parallelism = $Parallelism
      Warmup = $Warmup
      Runs = $Runs
      Offset = $Offset
      Path = $Path
      KernelCsv = $KernelCsv
      UserCsv = $UserCsv
    }

    if ($PSBoundParameters.ContainsKey('OffsetStep')) { $args.OffsetStep = $OffsetStep }
    if ($Prepare) { $args.Prepare = $true }
    if ($Append) { $args.Append = $true }

    & $benchScript @args
  }
}
