//! `ritsu-bench`: runs the benchmark suite and writes the JSON result document.
//!
//! ```text
//! cargo run --release --manifest-path bench/Cargo.toml -- [options]
//!   --output FILE     write the JSON document here (default: stdout)
//!   --host-id ID      machine class recorded in the result (default: env RITSU_BENCH_HOST_ID or "dev")
//!   --filter TEXT     run only benchmarks whose name contains TEXT
//!   --quick           very short run for smoke tests (numbers are not meaningful)
//!   --samples N       samples per benchmark (default 15)
//!   --list            print the benchmark names and exit
//!   --allow-debug     run in a debug build (numbers are meaningless; for development only)
//! ```

use ritsu_bench::harness::Config;
use ritsu_bench::{meta, suite};
use std::process::ExitCode;

fn usage() -> &'static str {
    "usage: ritsu-bench [--output FILE] [--host-id ID] [--filter TEXT] [--quick] [--samples N] [--list] [--allow-debug]"
}

fn write_output(path: &str, text: &str) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(path).parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))
}

fn main() -> ExitCode {
    let mut output = None;
    let mut host_id = std::env::var("RITSU_BENCH_HOST_ID").unwrap_or_else(|_| "dev".to_owned());
    let mut filter = None;
    let mut quick = false;
    let mut samples = None;
    let mut allow_debug = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| {
            args.next().unwrap_or_else(|| {
                eprintln!("error: {name} needs a value\n{}", usage());
                std::process::exit(2)
            })
        };
        match a.as_str() {
            "--output" => output = Some(value("--output")),
            "--host-id" => host_id = value("--host-id"),
            "--filter" => filter = Some(value("--filter")),
            "--samples" => match value("--samples").parse::<usize>() {
                Ok(n) if n > 0 => samples = Some(n),
                _ => {
                    eprintln!("error: --samples needs a positive integer");
                    return ExitCode::from(2);
                }
            },
            "--quick" => quick = true,
            "--allow-debug" => allow_debug = true,
            "--list" => {
                for b in suite::benches() {
                    println!("{}", b.name);
                }
                for d in suite::deterministic() {
                    println!("{}", d.name);
                }
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                println!("{}", usage());
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown argument {other:?}\n{}", usage());
                return ExitCode::from(2);
            }
        }
    }

    if cfg!(debug_assertions) && !allow_debug {
        eprintln!(
            "error: this is a debug build; benchmarks must run with --release (docs/process/testing.md section 8). \
             Use --allow-debug to override."
        );
        return ExitCode::from(2);
    }

    let mut cfg = if quick {
        Config::QUICK
    } else {
        Config::DEFAULT
    };
    if let Some(n) = samples {
        cfg.samples = n;
    }
    let seed = meta::seed_from_env(0x5249_5453_5500_0001);
    let meta = meta::collect(&host_id, &cfg, quick, seed, filter.as_deref());
    eprintln!(
        "ritsu-bench: host-id {host_id}, seed {seed:#x}, {} samples",
        cfg.samples
    );
    let doc = suite::run(
        &cfg,
        &suite::Ctx { seed },
        filter.as_deref(),
        meta,
        |line| {
            eprintln!("{line}");
        },
    );
    let text = doc.to_pretty_string();
    match output {
        Some(path) => {
            if let Err(e) = write_output(&path, &text) {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!("wrote {path}");
        }
        None => print!("{text}"),
    }
    ExitCode::SUCCESS
}
