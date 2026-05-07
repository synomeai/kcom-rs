use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Immediate,
    Yield1,
    YieldN,
}

impl Scenario {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "immediate" => Some(Self::Immediate),
            "yield1" => Some(Self::Yield1),
            "yieldN" | "yieldn" => Some(Self::YieldN),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Yield1 => "yield1",
            Self::YieldN => "yieldN",
        }
    }
}

struct Counters {
    sum_ns: AtomicU64,
    min_ns: AtomicU64,
    max_ns: AtomicU64,
}

impl Counters {
    fn new() -> Self {
        Self {
            sum_ns: AtomicU64::new(0),
            min_ns: AtomicU64::new(u64::MAX),
            max_ns: AtomicU64::new(0),
        }
    }

    fn record(&self, ns: u64) {
        self.sum_ns.fetch_add(ns, Ordering::Relaxed);
        update_min(&self.min_ns, ns);
        update_max(&self.max_ns, ns);
    }
}

#[derive(Debug)]
struct BenchResult {
    device: String,
    iterations: u64,
    parallelism: u64,
    scenario: Scenario,
    yield_count: u32,
    elapsed_ns: u64,
    avg_latency_ns: u64,
    min_latency_ns: u64,
    max_latency_ns: u64,
    throughput_per_s: f64,
}

#[derive(Debug)]
struct Args {
    scenario: Scenario,
    iterations: u64,
    parallelism: u64,
    yield_count: u32,
    warmup: u64,
    runs: u64,
    threads: usize,
    device: String,
    out_csv: Option<String>,
    append: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut scenario = Scenario::Yield1;
        let mut iterations = 200_000u64;
        let mut parallelism = 8u64;
        let mut yield_count = 8u32;
        let mut warmup = 0u64;
        let mut runs = 1u64;
        let mut threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let mut device = "tokio".to_string();
        let mut out_csv = None;
        let mut append = false;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--scenario" => {
                    let value = args.next().ok_or("missing value for --scenario")?;
                    scenario = Scenario::from_str(&value)
                        .ok_or_else(|| format!("invalid scenario: {}", value))?;
                }
                "--iterations" => {
                    let value = args.next().ok_or("missing value for --iterations")?;
                    iterations = parse_u64(&value, "--iterations")?;
                }
                "--parallelism" => {
                    let value = args.next().ok_or("missing value for --parallelism")?;
                    parallelism = parse_u64(&value, "--parallelism")?;
                }
                "--yield-count" => {
                    let value = args.next().ok_or("missing value for --yield-count")?;
                    let parsed = parse_u64(&value, "--yield-count")?;
                    yield_count = parsed
                        .try_into()
                        .map_err(|_| "yield-count out of range".to_string())?;
                }
                "--warmup" => {
                    let value = args.next().ok_or("missing value for --warmup")?;
                    warmup = parse_u64(&value, "--warmup")?;
                }
                "--runs" => {
                    let value = args.next().ok_or("missing value for --runs")?;
                    runs = parse_u64(&value, "--runs")?;
                }
                "--threads" => {
                    let value = args.next().ok_or("missing value for --threads")?;
                    let parsed = parse_u64(&value, "--threads")?;
                    threads = parsed
                        .try_into()
                        .map_err(|_| "threads out of range".to_string())?;
                }
                "--device" => {
                    let value = args.next().ok_or("missing value for --device")?;
                    device = value;
                }
                "--out-csv" => {
                    let value = args.next().ok_or("missing value for --out-csv")?;
                    out_csv = Some(value);
                }
                "--append" => {
                    append = true;
                }
                "--help" | "-h" => {
                    return Err(usage());
                }
                other => {
                    return Err(format!("unknown argument: {}", other));
                }
            }
        }

        if iterations == 0 || parallelism == 0 || runs == 0 {
            return Err("iterations/parallelism/runs must be > 0".to_string());
        }

        if yield_count == 0 {
            yield_count = 1;
        }

        Ok(Self {
            scenario,
            iterations,
            parallelism,
            yield_count,
            warmup,
            runs,
            threads,
            device,
            out_csv,
            append,
        })
    }
}

fn usage() -> String {
    [
        "tokio-bench",
        "",
        "Usage:",
        "  cargo run -p tokio-bench -- [options]",
        "",
        "Options:",
        "  --scenario <immediate|yield1|yieldN>",
        "  --iterations <n>",
        "  --parallelism <n>",
        "  --yield-count <n>",
        "  --warmup <n>",
        "  --runs <n>",
        "  --threads <n>",
        "  --device <name>",
        "  --out-csv <path>",
        "  --append",
    ]
    .join("\n")
}

fn parse_u64(value: &str, name: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid value for {}: {}", name, value))
}

fn now_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0));
    let secs = now.as_secs();
    let nanos = now.subsec_nanos();
    format!("{}.{:09}", secs, nanos)
}

async fn run_bench_loop(
    device: &str,
    scenario: Scenario,
    iterations: u64,
    parallelism: u64,
    yield_count: u32,
) -> BenchResult {
    let counters = Arc::new(Counters::new());
    let semaphore = Arc::new(Semaphore::new(parallelism as usize));
    let mut join_set = JoinSet::new();

    let bench_start = Instant::now();

    for _ in 0..iterations {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let counters = counters.clone();
        join_set.spawn(async move {
            let _permit = permit;
            let start = Instant::now();
            match scenario {
                Scenario::Immediate => {}
                Scenario::Yield1 => {
                    tokio::task::yield_now().await;
                }
                Scenario::YieldN => {
                    let mut remaining = yield_count.max(1);
                    while remaining > 0 {
                        tokio::task::yield_now().await;
                        remaining -= 1;
                    }
                }
            }
            let elapsed = start.elapsed().as_nanos() as u64;
            counters.record(elapsed);
        });
    }

    while join_set.join_next().await.is_some() {}

    let elapsed_ns = bench_start.elapsed().as_nanos() as u64;
    let sum_ns = counters.sum_ns.load(Ordering::Relaxed);
    let min_ns = counters.min_ns.load(Ordering::Relaxed);
    let max_ns = counters.max_ns.load(Ordering::Relaxed);
    let avg_latency_ns = if iterations == 0 {
        0
    } else {
        sum_ns / iterations
    };
    let min_latency_ns = if min_ns == u64::MAX { 0 } else { min_ns };
    let throughput_per_s = if elapsed_ns == 0 {
        0.0
    } else {
        (iterations as f64) * 1_000_000_000.0 / (elapsed_ns as f64)
    };

    BenchResult {
        device: device.to_string(),
        iterations,
        parallelism,
        scenario,
        yield_count,
        elapsed_ns,
        avg_latency_ns,
        min_latency_ns,
        max_latency_ns: max_ns,
        throughput_per_s,
    }
}

fn print_result(result: &BenchResult) {
    println!("timestamp        : {}", now_timestamp());
    println!("device           : {}", result.device);
    println!("scenario         : {}", result.scenario.as_str());
    println!("iterations       : {}", result.iterations);
    println!("parallelism      : {}", result.parallelism);
    println!("yield_count      : {}", result.yield_count);
    println!("elapsed_ns       : {}", result.elapsed_ns);
    println!("avg_latency_ns   : {}", result.avg_latency_ns);
    println!("min_latency_ns   : {}", result.min_latency_ns);
    println!("max_latency_ns   : {}", result.max_latency_ns);
    println!("throughput_per_s : {:.3}", result.throughput_per_s);
    println!();
}

fn csv_escape(value: &str) -> String {
    let escaped = value.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

fn write_csv_header<W: std::io::Write>(writer: &mut W) -> std::io::Result<()> {
    writeln!(
        writer,
        "timestamp,device,scenario,iterations,parallelism,yield_count,elapsed_ns,avg_latency_ns,min_latency_ns,max_latency_ns,throughput_per_s"
    )
}

fn write_csv_row<W: std::io::Write>(writer: &mut W, result: &BenchResult) -> std::io::Result<()> {
    let timestamp = csv_escape(&now_timestamp());
    let device = csv_escape(&result.device);
    let scenario = csv_escape(result.scenario.as_str());
    let iterations = csv_escape(&result.iterations.to_string());
    let parallelism = csv_escape(&result.parallelism.to_string());
    let yield_count = csv_escape(&result.yield_count.to_string());
    let elapsed_ns = csv_escape(&result.elapsed_ns.to_string());
    let avg_latency_ns = csv_escape(&result.avg_latency_ns.to_string());
    let min_latency_ns = csv_escape(&result.min_latency_ns.to_string());
    let max_latency_ns = csv_escape(&result.max_latency_ns.to_string());
    let throughput = csv_escape(&format!("{:.3}", result.throughput_per_s));
    writeln!(
        writer,
        "{},{},{},{},{},{},{},{},{},{},{}",
        timestamp,
        device,
        scenario,
        iterations,
        parallelism,
        yield_count,
        elapsed_ns,
        avg_latency_ns,
        min_latency_ns,
        max_latency_ns,
        throughput
    )
}

fn prepare_csv(path: &str, append: bool) -> std::io::Result<()> {
    if append {
        if !std::path::Path::new(path).exists() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?;
            write_csv_header(&mut file)?;
        }
    } else {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        write_csv_header(&mut file)?;
    }
    Ok(())
}

fn append_csv(path: &str, result: &BenchResult) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    write_csv_row(&mut file, result)
}

fn main() {
    let args = match Args::parse() {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("{}", msg);
            std::process::exit(2);
        }
    };

    if let Some(ref path) = args.out_csv {
        if let Err(err) = prepare_csv(path, args.append) {
            eprintln!("failed to prepare csv {}: {}", path, err);
            std::process::exit(2);
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.threads)
        .enable_all()
        .build()
        .expect("build tokio runtime");

    runtime.block_on(async {
        if args.warmup > 0 {
            let _ = run_bench_loop(
                &args.device,
                args.scenario,
                args.warmup,
                args.parallelism,
                args.yield_count,
            )
            .await;
        }

        for _ in 0..args.runs {
            let result = run_bench_loop(
                &args.device,
                args.scenario,
                args.iterations,
                args.parallelism,
                args.yield_count,
            )
            .await;
            print_result(&result);
            if let Some(ref path) = args.out_csv {
                if let Err(err) = append_csv(path, &result) {
                    eprintln!("failed to append csv {}: {}", path, err);
                    std::process::exit(2);
                }
            }
        }
    });
}

fn update_min(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value < current {
        match target.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}

fn update_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}
