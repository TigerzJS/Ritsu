//! `ritsu-dbt-ref`: command line of the differential-test infrastructure.
//!
//! ```text
//! ritsu-dbt-ref classes
//! ritsu-dbt-ref gen   [--seed S] [--units K] [--class NAME]... --out vectors.bin
//! ritsu-dbt-ref show  --class NAME --index I [--seed S] [--units K]
//! ritsu-dbt-ref t0    --vectors vectors.bin --out results.raw
//! ritsu-dbt-ref pack  --vectors vectors.bin --raw results.raw --oracle hardware|qemu [--meta k=v]... --out x.golden
//! ritsu-dbt-ref check --golden x.golden [--limit N]            T0 vs a golden file
//! ritsu-dbt-ref check-raw --vectors vectors.bin --raw results.raw [--limit N]   T0 vs a runner result
//! ritsu-dbt-ref golden-diff A.golden B.golden [--limit N]      two recordings of the same vectors
//! ritsu-dbt-ref promote --new x.golden --into tests/dbt-ref/golden/m0.golden [--force]
//! ritsu-dbt-ref info  x.golden
//! ```
//!
//! The seed defaults to `RITSU_TEST_SEED` if set, otherwise to the fixed default.

use ritsu_dbt_ref::compare::{self, hex_words};
use ritsu_dbt_ref::format::{self, Config, Outcome, ResHeader};
use ritsu_dbt_ref::generator::{self, CLASSES, DEFAULT_UNITS};
use ritsu_dbt_ref::rng::{DEFAULT_SEED, parse_seed, seed_from_env};
use ritsu_dbt_ref::set::VectorSet;
use ritsu_dbt_ref::{golden, store, t0};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;

type Res<T> = Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

struct Opts {
    args: Vec<String>,
}

impl Opts {
    fn value(&self, name: &str) -> Res<Option<&str>> {
        let mut found = None;
        let mut i = 0;
        while i < self.args.len() {
            if self.args[i] == name {
                let v = self
                    .args
                    .get(i + 1)
                    .ok_or_else(|| format!("{name} needs a value"))?;
                found = Some(v.as_str());
                i += 1;
            }
            i += 1;
        }
        Ok(found)
    }

    fn values(&self, name: &str) -> Vec<&str> {
        let mut out = Vec::new();
        for (i, a) in self.args.iter().enumerate() {
            if a == name
                && let Some(v) = self.args.get(i + 1)
            {
                out.push(v.as_str());
            }
        }
        out
    }

    fn required(&self, name: &str) -> Res<&str> {
        self.value(name)?.ok_or_else(|| format!("missing {name}"))
    }

    fn number(&self, name: &str) -> Res<Option<u64>> {
        self.value(name)?
            .map(|v| parse_seed(v).ok_or_else(|| format!("{name}: {v:?} is not a number")))
            .transpose()
    }

    fn positional(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.args.len() {
            if self.args[i].starts_with("--") {
                i += 2;
            } else {
                out.push(self.args[i].as_str());
                i += 1;
            }
        }
        out
    }
}

fn config(o: &Opts) -> Res<Config> {
    let seed = match o.number("--seed")? {
        Some(s) => s,
        None => seed_from_env(DEFAULT_SEED),
    };
    let mut cfg = generator::default_config(seed);
    if let Some(u) = o.number("--units")? {
        cfg.units = u32::try_from(u).map_err(|_| "--units out of range".to_string())?;
    }
    Ok(cfg)
}

fn io<T>(r: std::io::Result<T>, what: &str) -> Res<T> {
    r.map_err(|e| format!("{what}: {e}"))
}

fn run(args: &[String]) -> Res<ExitCode> {
    let Some(cmd) = args.first() else {
        return Err("usage: ritsu-dbt-ref <classes|gen|show|t0|pack|check|check-raw|golden-diff|promote|info> ...".into());
    };
    let o = Opts {
        args: args[1..].to_vec(),
    };
    match cmd.as_str() {
        "classes" => {
            println!(
                "{:>3}  {:<16} vectors at units={DEFAULT_UNITS}",
                "id", "name"
            );
            let mut total = 0;
            for c in CLASSES {
                let n = generator::class_count(c, DEFAULT_UNITS);
                total += n;
                println!("{:>3}  {:<16} {n}", c.id, c.name);
            }
            println!("total {total}");
            Ok(ExitCode::SUCCESS)
        }
        "gen" => {
            let cfg = config(&o)?;
            let only: Vec<String> = o
                .values("--class")
                .iter()
                .map(ToString::to_string)
                .collect();
            let set = VectorSet::generate(cfg, (!only.is_empty()).then_some(&only[..]))?;
            let out = o.required("--out")?;
            let mut w = BufWriter::new(io(File::create(out), out)?);
            io(format::write_vectors(&mut w, &cfg, &set.vectors), out)?;
            io(w.flush(), out)?;
            eprintln!(
                "{} vectors, seed {:#x}, units {}, generator version {} -> {out}",
                set.vectors.len(),
                cfg.seed,
                cfg.units,
                cfg.gen_version
            );
            Ok(ExitCode::SUCCESS)
        }
        "show" => {
            let cfg = config(&o)?;
            let name = o.required("--class")?;
            let class =
                generator::class_by_name(name).ok_or_else(|| format!("unknown class {name:?}"))?;
            let index = o.number("--index")?.ok_or("missing --index")? as u32;
            if index >= generator::class_count(class, cfg.units) {
                return Err(format!("--index out of range for --units {}", cfg.units));
            }
            let v = generator::vector_at(&cfg, class, index);
            println!("class {} #{index}, seed {:#x}", class.name, cfg.seed);
            println!("words: {}", hex_words(&v));
            disassemble(&v.words);
            println!("initial: sp={:#x} nzcv={:#x}", v.sp, v.nzcv);
            for (i, x) in v.x.iter().enumerate() {
                println!("  x{i:<2} = {x:#018x}");
            }
            let out = t0::run_vector(&cfg, &v);
            println!(
                "T0: {} at pc {:#x}, nzcv {:#x}, sp {:#x}",
                compare::sig_name(out.signal),
                out.state.pc,
                out.state.nzcv,
                out.state.sp
            );
            for i in 0..31 {
                if out.state.x[i] != v.x[i] {
                    println!("  x{i:<2} = {:#018x}", out.state.x[i]);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        "t0" => {
            let (cfg, vectors) = read_vectors(o.required("--vectors")?)?;
            let outs = t0::run_all(&cfg, &vectors);
            let out = o.required("--out")?;
            let mut w = BufWriter::new(io(File::create(out), out)?);
            let h = ResHeader {
                runner_version: 0,
                count: vectors.len() as u32,
                mem_len: cfg.mem_len,
                code_base: cfg.code_base,
                mem_base: cfg.mem_base,
                seed: cfg.seed,
            };
            io(format::write_res_header(&mut w, &h), out)?;
            for r in &outs {
                io(r.write_raw(&mut w), out)?;
            }
            io(w.flush(), out)?;
            Ok(ExitCode::SUCCESS)
        }
        "pack" => {
            let (cfg, vectors) = read_vectors(o.required("--vectors")?)?;
            let set = store::set_for_vectors(cfg, &vectors)?;
            let (rh, outs) = read_raw(o.required("--raw")?)?;
            if outs.len() != vectors.len() || rh.mem_len != cfg.mem_len {
                return Err("the raw result does not belong to the vector file".into());
            }
            let oracle = o.required("--oracle")?;
            if oracle != "hardware" && oracle != "qemu" {
                return Err("--oracle must be hardware or qemu".into());
            }
            let mut meta = vec![
                ("oracle".to_string(), oracle.to_string()),
                ("runner_version".to_string(), rh.runner_version.to_string()),
            ];
            meta.extend(host_info());
            for kv in o.values("--meta") {
                let (k, v) = kv.split_once('=').ok_or("--meta needs key=value")?;
                meta.push((k.to_string(), v.to_string()));
            }
            let bytes = io(store::pack(&set, &outs, meta), "pack")?;
            let out = o.required("--out")?;
            io(std::fs::write(out, &bytes), out)?;
            eprintln!("{} vectors -> {out} ({} bytes)", outs.len(), bytes.len());
            Ok(ExitCode::SUCCESS)
        }
        "check" => {
            let path = o.required("--golden")?;
            let limit = o.number("--limit")?.unwrap_or(10) as usize;
            let bytes = io(std::fs::read(path), path)?;
            let loaded = store::unpack(&bytes).map_err(|e| format!("{path}: {e}"))?;
            Ok(check_golden(path, &loaded, limit))
        }
        "check-raw" => {
            let (cfg, vectors) = read_vectors(o.required("--vectors")?)?;
            let set = store::set_for_vectors(cfg, &vectors)?;
            let (_, reference) = read_raw(o.required("--raw")?)?;
            let limit = o.number("--limit")?.unwrap_or(10) as usize;
            let ours = t0::run_all(&cfg, &vectors);
            let rep = compare::compare(&set, &ours, &reference);
            if rep.mismatches.is_empty() {
                println!(
                    "T0 == reference runner on all {} vectors (seed {:#x})",
                    rep.compared, cfg.seed
                );
                return Ok(ExitCode::SUCCESS);
            }
            print!(
                "{}",
                compare::render(&set, &ours, &reference, "T0", "reference", &rep, limit)
            );
            Ok(ExitCode::FAILURE)
        }
        "golden-diff" => {
            let pos = o.positional();
            let [a, b] = pos[..] else {
                return Err("golden-diff needs two files".into());
            };
            let limit = o.number("--limit")?.unwrap_or(10) as usize;
            let la = store::unpack(&io(std::fs::read(a), a)?).map_err(|e| format!("{a}: {e}"))?;
            let lb = store::unpack(&io(std::fs::read(b), b)?).map_err(|e| format!("{b}: {e}"))?;
            if la.set.cfg != lb.set.cfg || la.header.classes != lb.header.classes {
                return Err("the two golden files describe different vector sets".into());
            }
            println!(
                "A: oracle {} ({})",
                la.oracle(),
                la.header.meta("host_cpu").unwrap_or("?")
            );
            println!(
                "B: oracle {} ({})",
                lb.oracle(),
                lb.header.meta("host_cpu").unwrap_or("?")
            );
            let rep = compare::compare(&la.set, &la.outcomes, &lb.outcomes);
            if rep.mismatches.is_empty() {
                println!("identical results on all {} vectors", rep.compared);
                return Ok(ExitCode::SUCCESS);
            }
            print!(
                "{}",
                compare::render(&la.set, &la.outcomes, &lb.outcomes, "A", "B", &rep, limit)
            );
            Ok(ExitCode::FAILURE)
        }
        "promote" => {
            let new = o.required("--new")?;
            let into = o.required("--into")?;
            let force = o.args.iter().any(|a| a == "--force");
            let new_bytes = io(std::fs::read(new), new)?;
            let n = store::unpack(&new_bytes).map_err(|e| format!("{new}: {e}"))?;
            if let Ok(old_bytes) = std::fs::read(into) {
                // The header alone decides the policy; the old file may no longer be loadable
                // (the generator moved on), which is the usual reason to record again.
                let old_header = io(golden::read_header(&mut old_bytes.as_slice()), into)?;
                let old_oracle = old_header.meta("oracle").unwrap_or("unknown").to_string();
                if old_oracle == "hardware" && n.oracle() != "hardware" && !force {
                    return Err(format!(
                        "{into} is a hardware recording; refusing to replace it with a {} recording (--force to override)",
                        n.oracle()
                    ));
                }
                match store::unpack(&old_bytes) {
                    Ok(old)
                        if old.set.cfg == n.set.cfg && old.header.classes == n.header.classes =>
                    {
                        let rep = compare::compare(&old.set, &old.outcomes, &n.outcomes);
                        println!(
                            "old ({old_oracle}) vs new ({}): {} of {} vectors differ",
                            n.oracle(),
                            rep.mismatches.len(),
                            rep.compared
                        );
                    }
                    Ok(_) => println!(
                        "old and new describe different vector sets (classes or seed changed)"
                    ),
                    Err(e) => {
                        println!("old file is not comparable with the current generator: {e}")
                    }
                }
            }
            io(std::fs::write(into, &new_bytes), into)?;
            println!("{new} -> {into} (oracle {})", n.oracle());
            Ok(ExitCode::SUCCESS)
        }
        "info" => {
            let pos = o.positional();
            let path = pos.first().ok_or("info needs a file")?;
            let bytes = io(std::fs::read(path), path)?;
            let loaded = store::unpack(&bytes).map_err(|e| format!("{path}: {e}"))?;
            println!(
                "{path}: {} bytes, {} vectors",
                bytes.len(),
                loaded.outcomes.len()
            );
            for (k, v) in &loaded.header.meta {
                println!("  {k} = {v}");
            }
            let c = &loaded.header.cfg;
            println!(
                "  seed = {:#x}, units = {}, generator version = {}",
                c.seed, c.units, c.gen_version
            );
            for e in &loaded.header.classes {
                println!("  class {:>2} {:<16} {:>6} vectors", e.id, e.name, e.count);
            }
            let missing = loaded.uncovered_classes();
            if !missing.is_empty() {
                println!("  classes without golden data: {missing:?}");
            }
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

/// T0 against one golden file. Prints the coverage report, the oracle and the verdict.
fn check_golden(path: &str, loaded: &store::Loaded, limit: usize) -> ExitCode {
    let set = &loaded.set;
    let t0_out = t0::run_all(&set.cfg, &set.vectors);
    let rep = compare::compare(set, &t0_out, &loaded.outcomes);
    println!(
        "{path}: oracle {}, {} vectors, seed {:#x}, {} classes",
        loaded.oracle(),
        rep.compared,
        set.cfg.seed,
        loaded.header.classes.len()
    );
    let missing = loaded.uncovered_classes();
    if !missing.is_empty() {
        println!("classes of the generator without golden data: {missing:?}");
    }
    if rep.mismatches.is_empty() {
        println!("T0 == golden on all {} vectors", rep.compared);
        ExitCode::SUCCESS
    } else {
        print!(
            "{}",
            compare::render(set, &t0_out, &loaded.outcomes, "T0", "golden", &rep, limit)
        );
        ExitCode::FAILURE
    }
}

fn read_vectors(path: &str) -> Res<(Config, Vec<format::Vector>)> {
    let f = io(File::open(path), path)?;
    io(format::read_vectors(&mut BufReader::new(f)), path)
}

fn read_raw(path: &str) -> Res<(ResHeader, Vec<Outcome>)> {
    let f = io(File::open(path), path)?;
    io(format::read_results(&mut BufReader::new(f)), path)
}

/// Metadata about the machine that produced the recording (the one this command runs on, which
/// is also where the runner ran).
fn host_info() -> Vec<(String, String)> {
    let mut meta = vec![("host_arch".to_string(), std::env::consts::ARCH.to_string())];
    if let Ok(k) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        meta.push(("host_kernel".into(), k.trim().to_string()));
    }
    if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
        let get = |key: &str| {
            info.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        };
        if let Some(model) = get("model name") {
            meta.push(("host_cpu".into(), model));
        } else {
            // arm64 /proc/cpuinfo: implementer 0x41 = Arm; part 0xd4f = Neoverse V2, ...
            let fields: Vec<String> = [
                "CPU implementer",
                "CPU architecture",
                "CPU variant",
                "CPU part",
                "CPU revision",
            ]
            .iter()
            .filter_map(|k| {
                get(k).map(|v| format!("{}={v}", k.trim_start_matches("CPU ").replace(' ', "_")))
            })
            .collect();
            if !fields.is_empty() {
                meta.push(("host_cpu".into(), fields.join(" ")));
            }
        }
    }
    meta
}

/// Best-effort disassembly through `llvm-mc` for `show`.
fn disassemble(words: &[u32]) {
    let tool = ["llvm-mc-18", "llvm-mc"].into_iter().find(|t| {
        std::process::Command::new(t)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    });
    let Some(tool) = tool else { return };
    let text: String = words
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .map(|b| format!("0x{b:02x} "))
        .collect();
    let child = std::process::Command::new(tool)
        .args(["--disassemble", "--triple=aarch64"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if let Ok(out) = child.wait_with_output() {
            print!("{}", String::from_utf8_lossy(&out.stdout));
        }
    }
}
