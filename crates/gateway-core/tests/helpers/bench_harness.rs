//! Benchmark Harness
//!
//! Provides lightweight benchmark measurement utilities for integration tests.
//! Each test can record timing samples and custom metrics, then output
//! a formatted table at the end.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A single benchmark result with timing samples and custom metrics
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub name: String,
    pub samples: Vec<Duration>,
    pub metrics: HashMap<String, f64>,
}

impl BenchResult {
    /// Create a new BenchResult
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            samples: Vec::new(),
            metrics: HashMap::new(),
        }
    }

    /// Add a timing sample
    pub fn add_sample(&mut self, d: Duration) {
        self.samples.push(d);
    }

    /// Add a custom metric
    pub fn add_metric(&mut self, name: impl Into<String>, value: f64) {
        self.metrics.insert(name.into(), value);
    }

    /// Merge another BenchResult's samples and metrics into this one
    pub fn merge(&mut self, other: &BenchResult) {
        self.samples.extend_from_slice(&other.samples);
        for (k, v) in &other.metrics {
            self.metrics.insert(k.clone(), *v);
        }
    }

    /// Get the median (p50) duration
    pub fn p50(&self) -> Duration {
        self.percentile(50.0)
    }

    /// Get the p95 duration
    pub fn p95(&self) -> Duration {
        self.percentile(95.0)
    }

    /// Get the p99 duration
    pub fn p99(&self) -> Duration {
        self.percentile(99.0)
    }

    /// Get the average duration
    pub fn avg(&self) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        let total: Duration = self.samples.iter().sum();
        total / self.samples.len() as u32
    }

    /// Get the minimum duration
    pub fn min(&self) -> Duration {
        self.samples.iter().copied().min().unwrap_or(Duration::ZERO)
    }

    /// Get the maximum duration
    pub fn max(&self) -> Duration {
        self.samples.iter().copied().max().unwrap_or(Duration::ZERO)
    }

    /// Get the total duration (sum of all samples)
    pub fn total(&self) -> Duration {
        self.samples.iter().sum()
    }

    /// Calculate throughput in operations per second
    pub fn throughput(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let total_secs = self.total().as_secs_f64();
        if total_secs == 0.0 {
            return f64::INFINITY;
        }
        self.samples.len() as f64 / total_secs
    }

    /// Calculate a percentile from the sorted samples
    fn percentile(&self, pct: f64) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = self.samples.clone();
        sorted.sort();
        let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }
}

/// Benchmark timer for easy measurement
pub struct BenchTimer {
    start: Instant,
    name: String,
}

impl BenchTimer {
    /// Start a new timer
    pub fn start(name: impl Into<String>) -> Self {
        Self {
            start: Instant::now(),
            name: name.into(),
        }
    }

    /// Stop the timer and return the elapsed duration
    pub fn stop(self) -> Duration {
        self.start.elapsed()
    }

    /// Stop and record into a BenchResult
    pub fn stop_and_record(self, result: &mut BenchResult) -> Duration {
        let elapsed = self.start.elapsed();
        result.add_sample(elapsed);
        elapsed
    }
}

/// Print a formatted benchmark table to stdout
pub fn print_benchmark_table(results: &[BenchResult]) {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║            CANAL AGENT BENCHMARK REPORT                                ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");

    for result in results {
        println!("║                                                                            ║");
        println!("║ {:74} ║", result.name);
        println!("║ ┌──────────────┬──────────────┬──────────────┬──────────────┬─────────────┐ ║");
        println!("║ │   Total      │   Avg        │   P50        │   P95        │   Ops/s     │ ║");
        println!("║ ├──────────────┼──────────────┼──────────────┼──────────────┼─────────────┤ ║");
        println!(
            "║ │ {:>10}ms │ {:>10}ms │ {:>10}ms │ {:>10}ms │ {:>9.1}   │ ║",
            result.total().as_millis(),
            result.avg().as_millis(),
            result.p50().as_millis(),
            result.p95().as_millis(),
            result.throughput(),
        );
        println!("║ └──────────────┴──────────────┴──────────────┴──────────────┴─────────────┘ ║");

        if !result.metrics.is_empty() {
            println!(
                "║   Metrics:                                                                 ║"
            );
            let mut keys: Vec<_> = result.metrics.keys().collect();
            keys.sort();
            for key in keys {
                let value = result.metrics[key];
                println!(
                    "║   {:30} = {:>12.2}                           ║",
                    key, value
                );
            }
        }
    }

    println!("║                                                                            ║");
    println!("║ Summary:                                                                   ║");
    let total_duration: Duration = results.iter().map(|r| r.total()).sum();
    let total_samples: usize = results.iter().map(|r| r.samples.len()).sum();
    println!(
        "║   Total tests: {} │ Total samples: {} │ Total duration: {:.1}s             ║",
        results.len(),
        total_samples,
        total_duration.as_secs_f64()
    );
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();
}

/// Print a single benchmark result inline (for individual test output)
pub fn print_bench_inline(result: &BenchResult) {
    println!(
        "  [BENCH] {} | total={:.1}ms avg={:.1}ms p50={:.1}ms p95={:.1}ms ops/s={:.1}",
        result.name,
        result.total().as_secs_f64() * 1000.0,
        result.avg().as_secs_f64() * 1000.0,
        result.p50().as_secs_f64() * 1000.0,
        result.p95().as_secs_f64() * 1000.0,
        result.throughput(),
    );
    for (key, value) in &result.metrics {
        println!("          {} = {:.2}", key, value);
    }
}

/// Measure the memory usage delta of a closure (approximate via sysinfo)
pub fn measure_memory_delta_mb<F: FnOnce()>(f: F) -> f64 {
    // Use a simple heap tracking approach
    // Note: This is approximate - for precise measurements use a memory profiler
    let before = get_process_memory_mb();
    f();
    let after = get_process_memory_mb();
    after - before
}

/// Get current process memory in MB (approximate)
pub fn get_process_memory_mb() -> f64 {
    // Use /proc/self/status on Linux or mach APIs on macOS
    // Fallback: use std::alloc stats if available
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if let Some(kb_str) = parts.get(1) {
                        if let Ok(kb) = kb_str.parse::<f64>() {
                            return kb / 1024.0;
                        }
                    }
                }
            }
        }
        0.0
    }

    #[cfg(target_os = "macos")]
    {
        // Use mach task_info
        use std::process::Command;
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output();
        match output {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.trim().parse::<f64>().unwrap_or(0.0) / 1024.0
            }
            Err(_) => 0.0,
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0.0
    }
}
