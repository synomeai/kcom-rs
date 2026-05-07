param(
  [ValidateSet("immediate","yield1","yieldN")]
  [string[]]$Scenario,

  [ValidateSet("p1","p2","p3","comparison")]
  [string]$Preset = "p1",

  [UInt64]$Iterations,
  [UInt64]$Parallelism,
  [UInt32]$YieldCount,
  [UInt64]$Warmup,

  [string]$DeviceA = "\\.\KcomBench",
  [string]$DeviceB = "\\.\KcomBenchFused",

  [int]$Pairs = 5,

  [string]$OutCsv,
  [string]$SummaryCsv,
  [switch]$Append,
  [switch]$StartWithB
)

$benchScript = Join-Path $PSScriptRoot "bench.ps1"
if (-not (Test-Path $benchScript)) {
  throw "bench.ps1 not found at $benchScript"
}

$scenarioBound = $PSBoundParameters.ContainsKey('Scenario')
if (-not $scenarioBound) {
  if ($Preset -eq "comparison") {
    $Scenario = @("immediate")
  } else {
    $Scenario = @("immediate","yield1","yieldN")
  }
}

if ($OutCsv -and -not $Append -and (Test-Path $OutCsv)) {
  Remove-Item $OutCsv -Force
}

$order = if ($StartWithB) { @($DeviceB, $DeviceA) } else { @($DeviceA, $DeviceB) }

$baseArgs = @{
  Scenario = $Scenario
  Preset = $Preset
  Runs = 1
}

if ($PSBoundParameters.ContainsKey('Iterations'))  { $baseArgs.Iterations = $Iterations }
if ($PSBoundParameters.ContainsKey('Parallelism')) { $baseArgs.Parallelism = $Parallelism }
if ($PSBoundParameters.ContainsKey('YieldCount'))  { $baseArgs.YieldCount = $YieldCount }
if ($PSBoundParameters.ContainsKey('Warmup'))      { $baseArgs.Warmup = $Warmup }

$first = $true

for ($i = 1; $i -le $Pairs; $i++) {
  foreach ($dev in $order) {
    $args = @{} + $baseArgs
    $args.Device = $dev

    if ($OutCsv) {
      $args.OutCsv = $OutCsv
      $args.Append = (-not $first) -or $Append
    }

    & $benchScript @args
    $first = $false
  }
}

if ($OutCsv -and (Test-Path $OutCsv)) {
  $rows = Import-Csv $OutCsv | Where-Object { $_.device -in @($DeviceA, $DeviceB) }
  if ($rows.Count -gt 0) {
    $groups = $rows | Group-Object device,scenario,iterations,parallelism,yield_count
    $stats = @{}

    foreach ($g in $groups) {
      $items = $g.Group
      $firstRow = $items[0]
      $elapsed = $items | ForEach-Object { [double]$_.elapsed_ns }
      $thr = $items | ForEach-Object { [double]$_.throughput_per_s }
      $key = "{0}|{1}|{2}|{3}|{4}" -f $firstRow.device, $firstRow.scenario, $firstRow.iterations, $firstRow.parallelism, $firstRow.yield_count
      $stats[$key] = [pscustomobject]@{
        device = $firstRow.device
        scenario = $firstRow.scenario
        iterations = $firstRow.iterations
        parallelism = $firstRow.parallelism
        yield_count = $firstRow.yield_count
        elapsed_ns_mean = ($elapsed | Measure-Object -Average).Average
        throughput_mean = ($thr | Measure-Object -Average).Average
      }
    }

    $keys = $rows | Select-Object scenario,iterations,parallelism,yield_count -Unique
    $summary = @()

    foreach ($k in $keys) {
      $kA = "{0}|{1}|{2}|{3}|{4}" -f $DeviceA, $k.scenario, $k.iterations, $k.parallelism, $k.yield_count
      $kB = "{0}|{1}|{2}|{3}|{4}" -f $DeviceB, $k.scenario, $k.iterations, $k.parallelism, $k.yield_count
      if (-not ($stats.ContainsKey($kA) -and $stats.ContainsKey($kB))) { continue }

      $a = $stats[$kA]
      $b = $stats[$kB]

      $elapsedDiffPct = if ($a.elapsed_ns_mean -ne 0) { (($b.elapsed_ns_mean - $a.elapsed_ns_mean) / $a.elapsed_ns_mean) * 100.0 } else { $null }
      $thrDiffPct = if ($a.throughput_mean -ne 0) { (($b.throughput_mean - $a.throughput_mean) / $a.throughput_mean) * 100.0 } else { $null }

      $elapsedWinner = if ($a.elapsed_ns_mean -lt $b.elapsed_ns_mean) { "A" } elseif ($a.elapsed_ns_mean -gt $b.elapsed_ns_mean) { "B" } else { "tie" }
      $thrWinner = if ($a.throughput_mean -gt $b.throughput_mean) { "A" } elseif ($a.throughput_mean -lt $b.throughput_mean) { "B" } else { "tie" }

      $summary += [pscustomobject]@{
        scenario = $k.scenario
        parallelism = $k.parallelism
        yield_count = $k.yield_count
        elapsed_ns_mean_a = [math]::Round($a.elapsed_ns_mean, 3)
        elapsed_ns_mean_b = [math]::Round($b.elapsed_ns_mean, 3)
        elapsed_diff_pct_b_vs_a = if ($null -ne $elapsedDiffPct) { [math]::Round($elapsedDiffPct, 3) } else { $null }
        throughput_mean_a = [math]::Round($a.throughput_mean, 3)
        throughput_mean_b = [math]::Round($b.throughput_mean, 3)
        throughput_diff_pct_b_vs_a = if ($null -ne $thrDiffPct) { [math]::Round($thrDiffPct, 3) } else { $null }
        elapsed_winner = $elapsedWinner
        throughput_winner = $thrWinner
      }
    }

    $summary | Sort-Object scenario,parallelism | Format-Table -AutoSize scenario,parallelism,yield_count,elapsed_ns_mean_a,elapsed_ns_mean_b,elapsed_diff_pct_b_vs_a,throughput_mean_a,throughput_mean_b,throughput_diff_pct_b_vs_a

    $elapsedWins = $summary | Group-Object elapsed_winner | Sort-Object Name
    $throughputWins = $summary | Group-Object throughput_winner | Sort-Object Name

    Write-Host ""
    Write-Host "Elapsed winner counts (A=$DeviceA, B=$DeviceB):"
    $elapsedWins | ForEach-Object { "{0}: {1}" -f $_.Name, $_.Count } | Write-Host
    Write-Host "Throughput winner counts (A=$DeviceA, B=$DeviceB):"
    $throughputWins | ForEach-Object { "{0}: {1}" -f $_.Name, $_.Count } | Write-Host

    if ($SummaryCsv) {
      $summary | Export-Csv -NoTypeInformation -Path $SummaryCsv
    }
  }
}
