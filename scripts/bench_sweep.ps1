param(
  [string[]]$Device = @("\\.\KcomBench","\\.\KcomBenchFused"),

  [ValidateSet("immediate","yield1","yieldN")]
  [string[]]$Scenario,

  [ValidateSet("p1","p2","p3","comparison")]
  [string]$Preset = "p1",

  [UInt64]$Iterations,
  [UInt64]$ParallelismDefault,
  [UInt32]$YieldCount,
  [UInt64]$Warmup,

  [UInt64[]]$Parallelism = @(1,4,8,16),
  [int]$Runs = 5,

  [string]$RawCsv = "bench_raw.csv",
  [string]$SummaryCsv = "bench_summary.csv",
  [switch]$Clean
)

$benchScript = Join-Path $PSScriptRoot "bench.ps1"
if (-not (Test-Path $benchScript)) {
  throw "bench.ps1 not found at $benchScript"
}

if ($Clean) {
  if (Test-Path $RawCsv) { Remove-Item $RawCsv -Force }
  if (Test-Path $SummaryCsv) { Remove-Item $SummaryCsv -Force }
}

$scenarioBound = $PSBoundParameters.ContainsKey('Scenario')
if (-not $scenarioBound) {
  if ($Preset -eq "comparison") {
    $Scenario = @("immediate")
  } else {
    $Scenario = @("immediate","yield1","yieldN")
  }
}

foreach ($dev in $Device) {
  foreach ($par in $Parallelism) {
    $benchArgs = @{
      Scenario = $Scenario
      Preset = $Preset
      Parallelism = $par
      Runs = $Runs
      Device = $dev
      OutCsv = $RawCsv
      Append = $true
    }

    if ($PSBoundParameters.ContainsKey('Iterations'))  { $benchArgs.Iterations = $Iterations }
    if ($PSBoundParameters.ContainsKey('YieldCount'))  { $benchArgs.YieldCount = $YieldCount }
    if ($PSBoundParameters.ContainsKey('Warmup'))      { $benchArgs.Warmup = $Warmup }
    if ($PSBoundParameters.ContainsKey('ParallelismDefault')) { $benchArgs.Parallelism = $ParallelismDefault }

    & $benchScript @benchArgs
  }
}

if (-not (Test-Path $RawCsv)) {
  throw "Raw CSV not found: $RawCsv"
}

function Get-Percentile {
  param(
    [double[]]$Values,
    [double]$P
  )
  if (-not $Values -or $Values.Count -eq 0) {
    return $null
  }
  $sorted = $Values | Sort-Object
  $n = $sorted.Count
  $rank = [math]::Ceiling($P * $n)
  if ($rank -lt 1) { $rank = 1 }
  if ($rank -gt $n) { $rank = $n }
  return [double]$sorted[$rank - 1]
}

$rows = Import-Csv $RawCsv
$groups = $rows | Group-Object device,scenario,iterations,parallelism,yield_count
$summary = @()

foreach ($g in $groups) {
  $items = $g.Group
  $elapsed = $items | ForEach-Object { [double]$_.elapsed_ns }
  $avg = $items | ForEach-Object { [double]$_.avg_latency_ns }
  $min = $items | ForEach-Object { [double]$_.min_latency_ns }
  $max = $items | ForEach-Object { [double]$_.max_latency_ns }
  $thr = $items | ForEach-Object { [double]$_.throughput_per_s }

  $first = $items[0]
  $summary += [pscustomobject]@{
    device = $first.device
    scenario = $first.scenario
    iterations = $first.iterations
    parallelism = $first.parallelism
    yield_count = $first.yield_count
    runs = $items.Count
    elapsed_ns_mean = [math]::Round(($elapsed | Measure-Object -Average).Average, 3)
    elapsed_ns_median = Get-Percentile $elapsed 0.50
    elapsed_ns_p90 = Get-Percentile $elapsed 0.90
    elapsed_ns_p99 = Get-Percentile $elapsed 0.99
    avg_latency_ns_mean = [math]::Round(($avg | Measure-Object -Average).Average, 3)
    avg_latency_ns_p90 = Get-Percentile $avg 0.90
    avg_latency_ns_p99 = Get-Percentile $avg 0.99
    min_latency_ns_mean = [math]::Round(($min | Measure-Object -Average).Average, 3)
    max_latency_ns_mean = [math]::Round(($max | Measure-Object -Average).Average, 3)
    max_latency_ns_p90 = Get-Percentile $max 0.90
    max_latency_ns_p99 = Get-Percentile $max 0.99
    throughput_per_s_mean = [math]::Round(($thr | Measure-Object -Average).Average, 3)
    throughput_per_s_median = Get-Percentile $thr 0.50
    throughput_per_s_p90 = Get-Percentile $thr 0.90
    throughput_per_s_p99 = Get-Percentile $thr 0.99
  }
}

$summary | Export-Csv -NoTypeInformation -Path $SummaryCsv
$summary | Sort-Object device,scenario,parallelism | Format-Table -AutoSize device,scenario,parallelism,elapsed_ns_mean,throughput_per_s_mean,max_latency_ns_p99
