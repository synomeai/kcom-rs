param(
  [string[]]$Device = @("\\.\KcomBench","\\.\KcomBenchFused"),

  [ValidateSet("immediate","yield1","yieldN")]
  [string[]]$Scenario,

  [ValidateSet("p1","p2","p3","comparison")]
  [string]$Preset = "p1",

  [UInt64]$Iterations,
  [UInt64]$Parallelism,
  [UInt32]$YieldCount,
  [UInt64]$Warmup,

  [int]$Runs = 1,

  [string]$KernelCsv = "kernel_raw.csv",
  [string]$TokioCsv = "tokio_raw.csv",
  [switch]$Append,

  [string]$TokioDevice = "tokio",
  [int]$TokioThreads,

  [ValidateSet("debug","release")]
  [string]$TokioProfile = "release",

  [string]$Cargo = "cargo",

  [switch]$SkipKernel,
  [switch]$SkipTokio
)

$benchScript = Join-Path $PSScriptRoot "bench.ps1"
if (-not (Test-Path $benchScript)) {
  throw "bench.ps1 not found at $benchScript"
}

$presets = @{
  p1 = @{ Iterations = 200000; Parallelism = 8;  YieldCount = 8;  Warmup = 0 }
  p2 = @{ Iterations = 10000;  Parallelism = 1;  YieldCount = 1;  Warmup = 0 }
  p3 = @{ Iterations = 1000000; Parallelism = 16; YieldCount = 8;  Warmup = 0 }
  comparison = @{ Iterations = 10000000; Parallelism = 1; YieldCount = 1; Warmup = 100000 }
}

$cfg = @{
  Iterations  = $presets[$Preset].Iterations
  Parallelism = $presets[$Preset].Parallelism
  YieldCount  = $presets[$Preset].YieldCount
  Warmup      = $presets[$Preset].Warmup
}

if ($env:KCOM_BENCH_ITERS) { $cfg.Iterations = [uint64]$env:KCOM_BENCH_ITERS }
if ($env:KCOM_BENCH_PAR)   { $cfg.Parallelism = [uint64]$env:KCOM_BENCH_PAR }
if ($env:KCOM_BENCH_YIELDS){ $cfg.YieldCount = [uint32]$env:KCOM_BENCH_YIELDS }
if ($env:KCOM_BENCH_WARMUP){ $cfg.Warmup = [uint64]$env:KCOM_BENCH_WARMUP }

if ($PSBoundParameters.ContainsKey('Iterations'))  { $cfg.Iterations  = $Iterations }
if ($PSBoundParameters.ContainsKey('Parallelism')) { $cfg.Parallelism = $Parallelism }
if ($PSBoundParameters.ContainsKey('YieldCount'))  { $cfg.YieldCount  = $YieldCount }
if ($PSBoundParameters.ContainsKey('Warmup'))      { $cfg.Warmup      = $Warmup }

$scenarioExplicit = $PSBoundParameters.ContainsKey('Scenario')
if (-not $scenarioExplicit) {
  if ($Preset -eq "comparison") {
    $Scenario = @("immediate")
  } else {
    $Scenario = @("immediate","yield1","yieldN")
  }
}

if (-not $SkipKernel) {
  foreach ($dev in $Device) {
    $benchArgs = @{
      Scenario = $Scenario
      Preset = $Preset
      Iterations = $cfg.Iterations
      Parallelism = $cfg.Parallelism
      YieldCount = $cfg.YieldCount
      Warmup = $cfg.Warmup
      Runs = $Runs
      Device = $dev
      OutCsv = $KernelCsv
    }

    if ($Append) { $benchArgs.Append = $true }
    & $benchScript @benchArgs
  }
}

if (-not $SkipTokio) {
  $repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")

  foreach ($sc in $Scenario) {
    switch ($sc) {
      "immediate" { $yieldOverride = $cfg.YieldCount }
      "yield1"    { $yieldOverride = 1 }
      "yieldN"    { $yieldOverride = $cfg.YieldCount }
      default     { throw "Unknown scenario: $sc" }
    }

    $tokioArgs = @("run","-p","tokio-bench")
    if ($TokioProfile -eq "release") { $tokioArgs += "--release" }
    $tokioArgs += @("--","--scenario",$sc,
      "--iterations",$cfg.Iterations.ToString(),
      "--parallelism",$cfg.Parallelism.ToString(),
      "--yield-count",$yieldOverride.ToString(),
      "--warmup",$cfg.Warmup.ToString(),
      "--runs",$Runs.ToString(),
      "--device",$TokioDevice,
      "--out-csv",$TokioCsv)

    if ($PSBoundParameters.ContainsKey('TokioThreads')) {
      $tokioArgs += @("--threads",$TokioThreads.ToString())
    }
    if ($Append) {
      $tokioArgs += "--append"
    }

    Push-Location $repoRoot
    try {
      & $Cargo @tokioArgs
    } finally {
      Pop-Location
    }
  }
}
