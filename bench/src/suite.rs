//! The list of benchmarks and the code that turns measurements into the result document.
//!
//! Result document (`schema` 1), see `bench/README.md`:
//!
//! ```text
//! { "schema": 1, "meta": { commit, host_id, cpu, rustc, profile, config, ... },
//!   "results": [ { "name", "kind": "wall", "q_perf", "unit", "better", "value" (median),
//!                  "mad", "spread_pct", "min", "max", "samples", "iters_per_sample",
//!                  "work_per_call", "work_unit" },
//!                { "name", "kind": "deterministic", "q_perf", "unit", "better", "value" } ] }
//! ```

use crate::gpu;
use crate::harness::{Better, Config, Scale, summarize, time_it};
use crate::ir;
use crate::json::Json;
use crate::t0;
use crate::virtq::Fixture;
use ritsu_gpu_proto::decode_stream;
use std::hint::black_box;

/// Per-run inputs for benchmark setup.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    /// Seed for generated payload bytes.
    pub seed: u64,
}

type Setup = fn(&Ctx) -> Box<dyn FnMut() -> u64>;

/// A wall-clock benchmark.
#[derive(Clone, Copy)]
pub struct Bench {
    /// Name (`bench_<subject>`).
    pub name: &'static str,
    /// Nearest quality goal of ARCHITECTURE.md section 6.
    pub q_perf: &'static str,
    /// Unit of the reported value.
    pub unit: &'static str,
    /// What one work unit is.
    pub work_unit: &'static str,
    /// Direction of improvement.
    pub better: Better,
    /// Conversion from time per call to the reported value.
    pub scale: Scale,
    /// Builds the closure; setup cost is not measured.
    pub setup: Setup,
}

impl std::fmt::Debug for Bench {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bench").field("name", &self.name).finish()
    }
}

/// A deterministic metric: same code and input give the same number on every machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deterministic {
    /// Name.
    pub name: String,
    /// Nearest quality goal.
    pub q_perf: &'static str,
    /// Unit.
    pub unit: &'static str,
    /// Value.
    pub value: u64,
}

// Sizes: calls last several milliseconds so the timer overhead is negligible and the work per
// call is large enough to dwarf loop set-up.
const COUNTDOWN_N: u32 = 500_000;
const MIX_N: u32 = 100_000;
const VERIFY_SIZES: [usize; 3] = [256, 1024, 4096];

fn t0_countdown(_: &Ctx) -> Box<dyn FnMut() -> u64> {
    let p = t0::countdown(COUNTDOWN_N);
    let limit = 2 * t0::countdown_instructions(COUNTDOWN_N);
    Box::new(move || {
        let (cpu, executed) = p.run(limit);
        assert_eq!(executed, t0::countdown_instructions(COUNTDOWN_N));
        black_box(cpu.x[0]);
        executed
    })
}

fn t0_mix(_: &Ctx) -> Box<dyn FnMut() -> u64> {
    let p = t0::mix(MIX_N);
    let limit = 2 * t0::mix_instructions(MIX_N);
    Box::new(move || {
        let (cpu, executed) = p.run(limit);
        assert_eq!(executed, t0::mix_instructions(MIX_N));
        black_box(cpu.x[1]);
        executed
    })
}

fn ir_verify<const N: usize>(_: &Ctx) -> Box<dyn FnMut() -> u64> {
    let region = ir::cbz_region(N);
    assert_eq!(region.blocks().len(), ir::cbz_region_blocks(N));
    Box::new(move || {
        assert!(black_box(&region).verify().is_ok());
        region.blocks().len() as u64
    })
}

fn ir_lower(_: &Ctx) -> Box<dyn FnMut() -> u64> {
    let insts = ir::cbz_insts(4096);
    Box::new(move || {
        let region = ritsu_dbt::ir::Builder::lower_sequence(black_box(&insts));
        region.blocks().len() as u64
    })
}

fn gpu_decode(stream: fn(u64) -> (Vec<u8>, usize), ctx: &Ctx) -> Box<dyn FnMut() -> u64> {
    let (bytes, records) = stream(ctx.seed);
    assert_eq!(gpu::decode_count(&bytes), records);
    Box::new(move || {
        let cmds = decode_stream(black_box(&bytes)).expect("valid stream");
        black_box(cmds.len());
        bytes.len() as u64
    })
}

fn gpu_decode_mixed(ctx: &Ctx) -> Box<dyn FnMut() -> u64> {
    gpu_decode(gpu::mixed_stream, ctx)
}

fn gpu_decode_bulk(ctx: &Ctx) -> Box<dyn FnMut() -> u64> {
    gpu_decode(gpu::bulk_stream, ctx)
}

fn gpu_decode_reply(_: &Ctx) -> Box<dyn FnMut() -> u64> {
    let (bytes, n) = gpu::reply_stream();
    assert_eq!(gpu::decode_reply_count(&bytes), n);
    Box::new(move || {
        black_box(gpu::decode_reply_count(black_box(&bytes)));
        bytes.len() as u64
    })
}

fn virtq<const DESC: u16>(_: &Ctx) -> Box<dyn FnMut() -> u64> {
    const ROUNDS: u64 = 64;
    let mut f = Fixture::new(DESC);
    Box::new(move || {
        let mut n = 0;
        for _ in 0..ROUNDS {
            n += f.round();
        }
        assert_eq!(n, ROUNDS * u64::from(f.chains()));
        n
    })
}

/// All wall-clock benchmarks, in report order.
#[must_use]
pub fn benches() -> Vec<Bench> {
    let throughput = |name, q_perf, unit, work_unit, div, setup| Bench {
        name,
        q_perf,
        unit,
        work_unit,
        better: Better::Higher,
        scale: Scale::PerSecond(div),
        setup,
    };
    let latency = |name, setup| Bench {
        name,
        q_perf: "Q-PERF-1",
        unit: "us",
        work_unit: "blocks",
        better: Better::Lower,
        scale: Scale::MicrosPerCall,
        setup,
    };
    vec![
        throughput(
            "bench_t0_countdown",
            "Q-PERF-1",
            "Minst/s",
            "instructions",
            1e6,
            t0_countdown,
        ),
        throughput(
            "bench_t0_mix",
            "Q-PERF-1",
            "Minst/s",
            "instructions",
            1e6,
            t0_mix,
        ),
        latency("bench_ir_verify_cbz_256", ir_verify::<256>),
        latency("bench_ir_verify_cbz_1024", ir_verify::<1024>),
        latency("bench_ir_verify_cbz_4096", ir_verify::<4096>),
        latency("bench_ir_lower_cbz_4096", ir_lower),
        throughput(
            "bench_gpu_proto_decode_mixed",
            "Q-PERF-3",
            "MB/s",
            "bytes",
            1e6,
            gpu_decode_mixed,
        ),
        throughput(
            "bench_gpu_proto_decode_bulk",
            "Q-PERF-3",
            "MB/s",
            "bytes",
            1e6,
            gpu_decode_bulk,
        ),
        throughput(
            "bench_gpu_proto_decode_reply",
            "Q-PERF-3",
            "MB/s",
            "bytes",
            1e6,
            gpu_decode_reply,
        ),
        throughput(
            "bench_virtq_pop_push_1desc",
            "Q-PERF-2",
            "Mop/s",
            "pop+push pairs",
            1e6,
            virtq::<1>,
        ),
        throughput(
            "bench_virtq_pop_push_3desc",
            "Q-PERF-2",
            "Mop/s",
            "pop+push pairs",
            1e6,
            virtq::<3>,
        ),
    ]
}

/// Deterministic metrics (sizes of generated IR), independent of the machine.
#[must_use]
pub fn deterministic() -> Vec<Deterministic> {
    let mut out = Vec::new();
    for n in VERIFY_SIZES {
        let s = ir::shape(&ir::cbz_region(n));
        for (what, value) in [("blocks", s.blocks), ("values", s.values), ("ops", s.ops)] {
            out.push(Deterministic {
                name: format!("ir_cbz_{n}_{what}"),
                q_perf: "Q-PERF-1",
                unit: what,
                value,
            });
        }
    }
    out
}

/// Closure calls per process for the instruction-count mode (`tools/bench_icount.py`): the driver
/// runs each workload with `reps` and `2 * reps` calls under callgrind and takes the difference.
/// The values keep one call-count step in the order of 10^8 host instructions at most (callgrind
/// runs 20-50x slower than native) while the difference stays far above the 1-instruction
/// resolution; they are not tuned per machine. `None` for unknown names.
#[must_use]
pub fn icount_reps(name: &str) -> Option<u64> {
    Some(match name {
        "bench_t0_countdown" | "bench_t0_mix" => 1,
        "bench_ir_verify_cbz_256" | "bench_ir_verify_cbz_1024" => 8,
        "bench_ir_verify_cbz_4096" | "bench_ir_lower_cbz_4096" => 2,
        "bench_gpu_proto_decode_mixed"
        | "bench_gpu_proto_decode_bulk"
        | "bench_gpu_proto_decode_reply" => 2,
        "bench_virtq_pop_push_1desc" | "bench_virtq_pop_push_3desc" => 2,
        _ => return None,
    })
}

/// Instruction-count mode: builds the workload `name` (setup, not repeated) and calls its closure
/// `reps` times. Returns the work units of one call; fails if the work differs between calls or
/// the name is unknown. Nothing is timed, the process is meant to run under `valgrind`.
pub fn icount_run(name: &str, ctx: &Ctx, reps: u64) -> Result<u64, String> {
    let bench = benches()
        .into_iter()
        .find(|b| b.name == name)
        .ok_or_else(|| format!("unknown benchmark {name:?}"))?;
    if reps == 0 {
        return Err("reps must be positive".to_owned());
    }
    let mut f = (bench.setup)(ctx);
    let work = black_box(f());
    for _ in 1..reps {
        let again = black_box(f());
        if again != work {
            return Err(format!("{name}: work per call differs ({work} vs {again})"));
        }
    }
    Ok(work)
}

/// Runs every benchmark whose name contains `filter` (all if `None`) and returns the result
/// document. `progress` is called with a line of text after each benchmark.
pub fn run(
    cfg: &Config,
    ctx: &Ctx,
    filter: Option<&str>,
    meta: Json,
    mut progress: impl FnMut(&str),
) -> Json {
    let selected = |name: &str| filter.is_none_or(|f| name.contains(f));
    let mut results = Vec::new();
    for b in benches().into_iter().filter(|b| selected(b.name)) {
        let mut f = (b.setup)(ctx);
        let timing = time_it(cfg, &mut *f);
        let s = summarize(&timing, b.scale);
        progress(&format!(
            "{:<32} {:>12.3} {:<8} +-{:>5.2}%  (min {:.3}, max {:.3}; {} x {} calls)",
            b.name,
            s.median,
            b.unit,
            s.spread_pct(),
            s.min,
            s.max,
            timing.secs_per_call.len(),
            timing.iters_per_sample,
        ));
        results.push(Json::obj([
            ("name", Json::str(b.name)),
            ("kind", Json::str("wall")),
            ("q_perf", Json::str(b.q_perf)),
            ("unit", Json::str(b.unit)),
            ("better", Json::str(b.better.as_str())),
            ("value", Json::Num(s.median)),
            ("mad", Json::Num(s.mad)),
            ("spread_pct", Json::Num(s.spread_pct())),
            ("min", Json::Num(s.min)),
            ("max", Json::Num(s.max)),
            ("samples", Json::Int(timing.secs_per_call.len() as i64)),
            (
                "iters_per_sample",
                Json::Int(timing.iters_per_sample as i64),
            ),
            ("work_per_call", Json::Int(timing.work_per_call as i64)),
            ("work_unit", Json::str(b.work_unit)),
        ]));
    }
    for d in deterministic().into_iter().filter(|d| selected(&d.name)) {
        progress(&format!("{:<32} {:>12} {}", d.name, d.value, d.unit));
        results.push(Json::obj([
            ("name", Json::str(&d.name)),
            ("kind", Json::str("deterministic")),
            ("q_perf", Json::str(d.q_perf)),
            ("unit", Json::str(d.unit)),
            ("better", Json::str("lower")),
            ("value", Json::Int(d.value as i64)),
        ]));
    }
    Json::obj([
        ("schema", Json::Int(1)),
        ("meta", meta),
        ("results", Json::Arr(results)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_follow_the_convention() {
        let b = benches();
        let mut names: Vec<_> = b.iter().map(|b| b.name).collect();
        assert!(names.iter().all(|n| n.starts_with("bench_")));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), b.len());
    }

    #[test]
    fn every_benchmark_runs_and_reports_stable_work() {
        // Quick config: checks that the closures work and the document is well-formed, not speed.
        let ctx = Ctx { seed: 1 };
        let doc = run(&Config::QUICK, &ctx, None, Json::Null, |_| ());
        let text = doc.to_pretty_string();
        for b in benches() {
            assert!(text.contains(b.name), "{} missing", b.name);
        }
        assert!(text.contains("\"ir_cbz_4096_blocks\""));
        assert!(text.contains("8193"), "full page block count");
    }

    #[test]
    fn deterministic_metrics_are_reproducible() {
        assert_eq!(deterministic(), deterministic());
        let blocks = deterministic()
            .into_iter()
            .find(|d| d.name == "ir_cbz_4096_blocks")
            .expect("metric exists");
        assert_eq!(blocks.value, 8193);
    }

    #[test]
    fn every_benchmark_has_icount_reps() {
        for b in benches() {
            assert!(
                icount_reps(b.name).is_some_and(|r| r > 0),
                "{} has no icount_reps entry",
                b.name
            );
        }
        assert_eq!(icount_reps("nope"), None);
    }

    #[test]
    fn icount_run_returns_the_work_of_one_call() {
        let ctx = Ctx { seed: 1 };
        // Hand-derived: 2 + 2n instructions for the countdown; 64 rounds of floor(256 / 1) and
        // floor(256 / 3) = 85 chains for a queue of 256 descriptors.
        assert_eq!(
            icount_run("bench_t0_countdown", &ctx, 2),
            Ok(2 + 2 * u64::from(COUNTDOWN_N))
        );
        assert_eq!(
            icount_run("bench_virtq_pop_push_1desc", &ctx, 3),
            Ok(64 * 256)
        );
        assert_eq!(
            icount_run("bench_virtq_pop_push_3desc", &ctx, 1),
            Ok(64 * 85)
        );
        assert!(icount_run("nope", &ctx, 1).is_err());
        assert!(icount_run("bench_t0_mix", &ctx, 0).is_err());
    }

    #[test]
    fn filter_selects_by_substring() {
        let ctx = Ctx { seed: 1 };
        let doc = run(
            &Config::QUICK,
            &ctx,
            Some("virtq_pop_push_3"),
            Json::Null,
            |_| (),
        );
        let text = doc.to_pretty_string();
        assert!(text.contains("bench_virtq_pop_push_3desc"));
        assert!(!text.contains("bench_t0_mix"));
    }
}
