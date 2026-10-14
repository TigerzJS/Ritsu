//! Integration tests: T0 against the checked-in golden files, the golden files against the
//! hand-computed expectations, and the failure report itself.

use ritsu_dbt_ref::compare::{self, diff_outcomes};
use ritsu_dbt_ref::format::{Outcome, sig};
use ritsu_dbt_ref::generator::{self, CLASSES};
use ritsu_dbt_ref::rng::{DEFAULT_SEED, seed_from_env};
use ritsu_dbt_ref::set::VectorSet;
use ritsu_dbt_ref::{handmade, store, t0};
use std::path::{Path, PathBuf};
use std::process::Command;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("golden")
}

fn golden_files() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(golden_dir())
        .expect("tests/dbt-ref/golden/ must exist")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "golden"))
        .collect();
    v.sort();
    v
}

fn load(path: &Path) -> store::Loaded {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    store::unpack(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn loaded_goldens() -> Vec<(PathBuf, store::Loaded)> {
    let files = golden_files();
    assert!(
        !files.is_empty(),
        "no golden file in tests/dbt-ref/golden/: record one with `make dbt-ref-record dbt-ref-update`"
    );
    files.into_iter().map(|p| (p.clone(), load(&p))).collect()
}

/// The reference comparison (testing.md section 4, "Referenzvergleich"): T0 must reproduce every
/// recorded result exactly. The vectors are regenerated from the seed in the file header.
#[test]
fn t0_matches_the_checked_in_golden_files() {
    for (path, g) in loaded_goldens() {
        let set = &g.set;
        let ours = t0::run_all(&set.cfg, &set.vectors);
        let rep = compare::compare(set, &ours, &g.outcomes);
        eprintln!(
            "{}: oracle {}, {} vectors, seed {:#x}",
            path.display(),
            g.oracle(),
            rep.compared,
            set.cfg.seed
        );
        assert!(
            rep.mismatches.is_empty(),
            "T0 deviates from {} (oracle {}):\n{}",
            path.display(),
            g.oracle(),
            compare::render(set, &ours, &g.outcomes, "T0", "golden", &rep, 8)
        );
    }
}

/// The generator has no class without golden data (coverage, testing.md section 4.2 item 5).
#[test]
fn every_generator_class_has_golden_data() {
    let goldens = loaded_goldens();
    for class in CLASSES {
        assert!(
            goldens
                .iter()
                .any(|(_, g)| g.header.classes.iter().any(|e| e.id == class.id)),
            "class {:?} has no golden data; record it: `make dbt-ref-record dbt-ref-update`",
            class.name
        );
    }
}

#[test]
fn golden_files_declare_their_oracle() {
    for (path, g) in loaded_goldens() {
        let oracle = g.oracle();
        assert!(
            oracle == "hardware" || oracle == "qemu",
            "{}: oracle must be `hardware` or `qemu`, got {oracle:?}",
            path.display()
        );
        if oracle == "qemu" {
            eprintln!(
                "NOTE {}: provisional golden file recorded with qemu (secondary oracle); replace it with a hardware recording",
                path.display()
            );
        }
    }
}

/// The recorded results of the hand-written vectors equal the hand-computed expectation. This
/// checks the recording pipeline (runner, file format) independently of T0.
#[test]
fn golden_reproduces_the_hand_computed_expectations() {
    for (path, g) in loaded_goldens() {
        let Some(span) = g.set.spans.iter().find(|s| s.class.name == "handmade") else {
            continue;
        };
        let cases = handmade::cases();
        assert_eq!(span.count, cases.len());
        for (i, case) in cases.iter().enumerate() {
            let want = case.expected(&g.set.cfg);
            let got = &g.outcomes[span.start + i];
            let d = diff_outcomes(&want, got);
            assert!(
                d.is_empty(),
                "{}: {}: hand-computed vs recorded: {d:?}",
                path.display(),
                case.name
            );
        }
    }
}

/// Sanity of the corpus itself: the vectors exercise what they claim to (every condition code
/// both taken and not taken, every NZCV combination that arithmetic can produce, all signals).
#[test]
fn golden_corpus_reaches_the_interesting_outcomes() {
    for (path, g) in loaded_goldens() {
        let by_class = |name: &str| g.set.spans.iter().find(|s| s.class.name == name);

        // With a non-negative immediate, SUBS can end in 5 flag combinations (C, CV, ZC, N, NC) and
        // ADDS in 5 (none, N, NV, ZC, C); the operand relations of the generator must reach all.
        for name in ["subs_imm", "adds_imm"] {
            if let Some(s) = by_class(name) {
                let mut seen = [false; 16];
                for o in &g.outcomes[s.start..s.start + s.count] {
                    seen[(o.state.nzcv >> 28) as usize] = true;
                }
                let n = seen.iter().filter(|b| **b).count();
                assert!(
                    n >= 5,
                    "{}: {name} reaches only {n} flag combinations: {seen:?}",
                    path.display()
                );
            }
        }

        if let Some(s) = by_class("cmp_b_cond") {
            // `cmp_b_cond` = [one flag-setting compare][B.cond][markers]; the first marker writes
            // X20. If X20 is unchanged the first marker was skipped, so the branch was taken.
            let mut skipped = [0u32; 16];
            let mut ran = [0u32; 16];
            for i in s.start..s.start + s.count {
                let v = &g.set.vectors[i];
                let cond = (v.words[1] & 0xf) as usize;
                if g.outcomes[i].state.x[20] == v.x[20] {
                    skipped[cond] += 1;
                } else {
                    ran[cond] += 1;
                }
            }
            for c in 0..16 {
                assert!(
                    skipped[c] > 0 && ran[c] > 0,
                    "{}: cond {c}: first marker skipped {skipped:?}, executed {ran:?}",
                    path.display()
                );
            }
        }

        let mut signals = std::collections::BTreeSet::new();
        for o in &g.outcomes {
            signals.insert(o.signal);
        }
        assert!(
            signals.contains(&sig::TRAP) && signals.contains(&sig::ILL),
            "{}: signals seen: {signals:?}",
            path.display()
        );
    }
}

/// The comparison must notice a deviation and report it reproducibly (a green run means something).
#[test]
fn a_seeded_deviation_is_detected_and_reported() {
    let (_, g) = loaded_goldens().into_iter().next().unwrap();
    let ours = t0::run_all(&g.set.cfg, &g.set.vectors);
    let mut bad: Vec<Outcome> = g.outcomes.clone();
    let target = g
        .set
        .spans
        .iter()
        .find(|s| s.class.name == "add_imm")
        .unwrap()
        .start
        + 7;
    bad[target].state.x[3] ^= 0x10;
    bad[target + 1].state.nzcv ^= 1 << 31;
    let rep = compare::compare(&g.set, &ours, &bad);
    assert_eq!(rep.mismatches.len(), 2, "{rep:?}");
    let text = compare::render(&g.set, &ours, &bad, "T0", "golden", &rep, 10);
    assert!(text.contains("add_imm #7"), "{text}");
    assert!(text.contains("x3: "), "{text}");
    assert!(
        text.contains(&format!("RITSU_TEST_SEED={:#x}", g.set.cfg.seed)),
        "{text}"
    );
    assert!(text.contains("words    :"), "{text}");
}

/// Fresh seed against a live reference: the runner built with clang-18 under `qemu-aarch64`, or
/// natively on an arm64 host (then the reference is the real CPU). `RITSU_TEST_SEED` selects the
/// seed; without it the default seed is used.
#[test]
#[ignore = "needs clang + ld.lld and qemu-aarch64 (or an arm64 host): make dbt-ref-fresh"]
fn t0_matches_the_live_reference_for_a_fresh_seed() {
    let seed = seed_from_env(DEFAULT_SEED ^ 0x1234_5678);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = std::env::temp_dir().join(format!("ritsu-dbt-ref-{}-{seed:x}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let runner = tmp.join("ritsu-ref-runner");
    let status = Command::new("sh")
        .arg(manifest.join("runner/build.sh"))
        .arg(&runner)
        .status()
        .expect("sh");
    assert!(status.success(), "building the runner failed");

    let cfg = generator::default_config(seed);
    let cfg = ritsu_dbt_ref::format::Config { units: 100, ..cfg };
    let set = VectorSet::generate(cfg, None).unwrap();
    let vectors = tmp.join("vectors.bin");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&vectors).unwrap());
    ritsu_dbt_ref::format::write_vectors(&mut f, &cfg, &set.vectors).unwrap();
    drop(f);

    let raw = tmp.join("results.raw");
    let native = std::env::consts::ARCH == "aarch64";
    let mut cmd = if native {
        Command::new(&runner)
    } else {
        Command::new("qemu-aarch64")
    };
    if !native {
        cmd.arg(&runner);
    }
    let status = cmd
        .arg("-i")
        .arg(&vectors)
        .arg("-o")
        .arg(&raw)
        .arg("-t")
        .arg("300")
        .status()
        .expect("runner");
    assert!(status.success(), "the reference runner failed");

    let (_, reference) = ritsu_dbt_ref::format::read_results(&mut std::io::BufReader::new(
        std::fs::File::open(&raw).unwrap(),
    ))
    .unwrap();
    let ours = t0::run_all(&cfg, &set.vectors);
    let rep = compare::compare(&set, &ours, &reference);
    assert!(
        rep.mismatches.is_empty(),
        "RITSU_TEST_SEED={seed:#x}\n{}",
        compare::render(
            &set,
            &ours,
            &reference,
            "T0",
            if native { "hardware" } else { "qemu" },
            &rep,
            8
        )
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
