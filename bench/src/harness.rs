//! Minimal measurement harness: warm-up, calibration, repeated samples, median and MAD.
//!
//! Why not `criterion`: it would add some 40 transitive crates (and a registry download) for a
//! suite whose needs are small; its output format is not ours; and a 150-line harness is easy to
//! audit. The method is the same: warm up, size the inner loop so that one sample takes about
//! `sample_time`, take `samples` samples, report median and MAD. What it does not have is
//! criterion's outlier classification and HTML reports; the regression decision lives in
//! `tools/bench_compare.py`, not here.
//!
//! Every benchmark closure returns the number of work units it performed (instructions executed,
//! bytes decoded, queue operations). The count must be identical in every call; it is stored in the
//! result as the deterministic fingerprint of the workload, so a changed workload cannot be
//! mistaken for a performance change.

use crate::stats::Summary;
use std::hint::black_box;
use std::time::{Duration, Instant};

/// Measurement parameters, recorded in the result file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Time spent calling the closure before sampling starts.
    pub warmup: Duration,
    /// Target duration of one sample (the inner loop is sized to reach it).
    pub sample_time: Duration,
    /// Number of samples.
    pub samples: usize,
}

impl Config {
    /// Default: about 2 s per benchmark.
    pub const DEFAULT: Self = Self {
        warmup: Duration::from_millis(300),
        sample_time: Duration::from_millis(100),
        samples: 15,
    };

    /// Short run for smoke tests (numbers are not meaningful).
    pub const QUICK: Self = Self {
        warmup: Duration::from_millis(10),
        sample_time: Duration::from_millis(5),
        samples: 5,
    };
}

/// Raw timings of one benchmark.
#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    /// Seconds per closure call, one entry per sample.
    pub secs_per_call: Vec<f64>,
    /// Closure calls per sample.
    pub iters_per_sample: u64,
    /// Work units per closure call.
    pub work_per_call: u64,
}

/// Times `f`. Panics if `f` reports different work in different calls (the workload would not be
/// comparable between runs) or if `cfg.samples == 0`.
pub fn time_it(cfg: &Config, f: &mut dyn FnMut() -> u64) -> Timing {
    assert!(cfg.samples > 0, "at least one sample is required");
    let work = black_box(f());
    assert!(work > 0, "a benchmark must report non-zero work");

    // Warm-up doubles as calibration: the average over the warm-up window sizes the inner loop.
    let start = Instant::now();
    let mut calls = 0u64;
    while calls == 0 || start.elapsed() < cfg.warmup {
        assert_eq!(black_box(f()), work, "work per call is not deterministic");
        calls += 1;
    }
    let per_call = start.elapsed().as_secs_f64() / calls as f64;
    let iters = ((cfg.sample_time.as_secs_f64() / per_call).ceil() as u64).max(1);

    let mut secs_per_call = Vec::with_capacity(cfg.samples);
    for _ in 0..cfg.samples {
        let t = Instant::now();
        for _ in 0..iters {
            black_box(f());
        }
        secs_per_call.push(t.elapsed().as_secs_f64() / iters as f64);
    }
    Timing {
        secs_per_call,
        iters_per_sample: iters,
        work_per_call: work,
    }
}

/// Which direction is an improvement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Better {
    /// Larger values are better (throughput).
    Higher,
    /// Smaller values are better (latency, size).
    Lower,
}

impl Better {
    /// Name used in the result file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Higher => "higher",
            Self::Lower => "lower",
        }
    }
}

/// How a per-call time becomes the reported value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    /// `work / seconds / divisor` (e.g. divisor 1e6 for Minst/s).
    PerSecond(f64),
    /// Microseconds per call.
    MicrosPerCall,
}

impl Scale {
    /// Value for one sample.
    #[must_use]
    pub fn value(self, work_per_call: u64, secs_per_call: f64) -> f64 {
        match self {
            Self::PerSecond(div) => work_per_call as f64 / secs_per_call / div,
            Self::MicrosPerCall => secs_per_call * 1e6,
        }
    }
}

/// Summary of a timed benchmark in its reporting unit.
#[must_use]
pub fn summarize(t: &Timing, scale: Scale) -> Summary {
    let values: Vec<f64> = t
        .secs_per_call
        .iter()
        .map(|&s| scale.value(t.work_per_call, s))
        .collect();
    Summary::of(&values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_work_and_calibrates() {
        let mut calls = 0u64;
        let mut f = || {
            calls += 1;
            let mut x = 0u64;
            for i in 0..1000u64 {
                x = black_box(x.wrapping_add(i));
            }
            1000
        };
        let t = time_it(&Config::QUICK, &mut f);
        assert_eq!(t.work_per_call, 1000);
        assert_eq!(t.secs_per_call.len(), 5);
        assert!(t.iters_per_sample >= 1);
        assert!(t.secs_per_call.iter().all(|&s| s > 0.0));
        // 1 initial + warm-up + 5 samples * iters
        assert!(calls > 5 * t.iters_per_sample);
    }

    #[test]
    #[should_panic(expected = "not deterministic")]
    fn rejects_varying_work() {
        let mut n = 0;
        let mut f = || {
            n += 1;
            n
        };
        let _ = time_it(&Config::QUICK, &mut f);
    }

    #[test]
    fn scale_converts_hand_computed_values() {
        // 2e6 work units in 0.5 s = 4e6 units/s = 4 M/s.
        assert_eq!(Scale::PerSecond(1e6).value(2_000_000, 0.5), 4.0);
        assert_eq!(Scale::MicrosPerCall.value(1, 0.000_25), 250.0);
    }
}
