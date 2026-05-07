param(
  [ValidateSet("immediate","yield1","yieldN")]
  [string[]]$Scenario = @("yield1"),

  [ValidateSet("p1","p2","p3","comparison")]
  [string]$Preset = "p1",

  [UInt64]$Iterations,
  [UInt64]$Parallelism,
  [UInt32]$YieldCount,
  [UInt64]$Warmup,

  [string]$Device = "\\.\KcomBench",
  [int]$Runs = 1,
  [string]$OutCsv,
  [switch]$Append
)

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
if ($Preset -eq "comparison" -and -not $scenarioExplicit) {
  $Scenario = @("immediate")
}

if (-not ([System.Management.Automation.PSTypeName]'KcomBench').Type) {
  Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class KcomBench {
  [StructLayout(LayoutKind.Sequential)]
  public struct BenchRequest {
    public UInt32 scenario;
    public UInt32 yield_count;
    public UInt64 iterations;
    public UInt64 parallelism;
    public UInt64 warmup_iterations;
  }

  [StructLayout(LayoutKind.Sequential)]
  public struct BenchResult {
    public UInt64 iterations;
    public UInt64 parallelism;
    public UInt32 scenario;
    public UInt32 yield_count;
    public UInt64 qpc_freq;
    public UInt64 elapsed_ticks;
    public UInt64 avg_latency_ticks;
    public UInt64 min_latency_ticks;
    public UInt64 max_latency_ticks;
    public UInt64 dpc_enqueued;
    public UInt64 dpc_skipped;
    public UInt64 dpc_run;
    public UInt64 poll_total;
    public UInt64 poll_ready;
    public UInt64 poll_pending;
    public UInt64 slab_hit;
    public UInt64 slab_miss;
  }

  [DllImport("kernel32.dll", SetLastError=true)]
  public static extern IntPtr CreateFile(
    string name, UInt32 access, UInt32 share,
    IntPtr security, UInt32 creation, UInt32 flags, IntPtr template);

  [DllImport("kernel32.dll", SetLastError=true)]
  public static extern bool DeviceIoControl(
    IntPtr h, UInt32 code,
    ref BenchRequest inBuf, UInt32 inLen,
    out BenchResult outBuf, UInt32 outLen,
    out UInt32 bytes, IntPtr overlapped);

  [DllImport("kernel32.dll", SetLastError=true)]
  public static extern bool CloseHandle(IntPtr h);
}
"@
}

$IOCTL = [uint32]0x0022E000
$GENERIC_READ  = [uint32]::Parse("80000000","HexNumber")
$GENERIC_WRITE = [uint32]::Parse("40000000","HexNumber")
$FILE_SHARE_READ  = [uint32]1
$FILE_SHARE_WRITE = [uint32]2
$OPEN_EXISTING = [uint32]3

$h = [KcomBench]::CreateFile(
  $Device,
  $GENERIC_READ -bor $GENERIC_WRITE,
  $FILE_SHARE_READ -bor $FILE_SHARE_WRITE,
  [IntPtr]::Zero, $OPEN_EXISTING, 0, [IntPtr]::Zero)

if ($h -eq [IntPtr]::Zero -or $h -eq [IntPtr](-1)) {
  throw "CreateFile failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
}

$results = @()

try {
  foreach ($sc in $Scenario) {
    switch ($sc) {
      "immediate" { $scenarioId = 0; $yieldOverride = $cfg.YieldCount }
      "yield1"    { $scenarioId = 1; $yieldOverride = 1 }
      "yieldN"    { $scenarioId = 2; $yieldOverride = $cfg.YieldCount }
      default     { throw "Unknown scenario: $sc" }
    }

    for ($i = 1; $i -le $Runs; $i++) {
      $req = New-Object KcomBench+BenchRequest
      $req.scenario    = [uint32]$scenarioId
      $req.yield_count = [uint32]$yieldOverride
      $req.iterations  = [uint64]$cfg.Iterations
      $req.parallelism = [uint64]$cfg.Parallelism
      $req.warmup_iterations = [uint64]$cfg.Warmup

      [UInt32]$bytes = 0
      $r = New-Object KcomBench+BenchResult
      $ok = [KcomBench]::DeviceIoControl(
        $h, $IOCTL,
        [ref]$req, [System.Runtime.InteropServices.Marshal]::SizeOf($req),
        [ref]$r,   [System.Runtime.InteropServices.Marshal]::SizeOf($r),
        [ref]$bytes, [IntPtr]::Zero)

      if (-not $ok) {
        throw "DeviceIoControl failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
      }

      $elapsed_ns = [double]$r.elapsed_ticks * 1e9 / $r.qpc_freq
      $avg_ns = [double]$r.avg_latency_ticks * 1e9 / $r.qpc_freq
      $min_ns = [double]$r.min_latency_ticks * 1e9 / $r.qpc_freq
      $max_ns = [double]$r.max_latency_ticks * 1e9 / $r.qpc_freq
      $throughput = [double]$r.iterations * 1e9 / $elapsed_ns

      $row = [pscustomobject]@{
        timestamp        = (Get-Date).ToString("s")
        device           = $Device
        scenario         = $sc
        iterations       = $r.iterations
        parallelism      = $r.parallelism
        yield_count      = $r.yield_count
        elapsed_ns       = [math]::Round($elapsed_ns, 3)
        avg_latency_ns   = [math]::Round($avg_ns, 3)
        min_latency_ns   = [math]::Round($min_ns, 3)
        max_latency_ns   = [math]::Round($max_ns, 3)
        throughput_per_s = [math]::Round($throughput, 3)
        dpc_enqueued     = $r.dpc_enqueued
        dpc_skipped      = $r.dpc_skipped
        dpc_run          = $r.dpc_run
        poll_total       = $r.poll_total
        poll_ready       = $r.poll_ready
        poll_pending     = $r.poll_pending
        slab_hit         = $r.slab_hit
        slab_miss        = $r.slab_miss
      }

      $results += $row
      $row | Format-List
    }
  }
}
finally {
  [KcomBench]::CloseHandle($h) | Out-Null
}

if ($OutCsv) {
  $append = $Append -and (Test-Path $OutCsv)
  $results | Export-Csv -NoTypeInformation -Path $OutCsv -Append:$append
}
