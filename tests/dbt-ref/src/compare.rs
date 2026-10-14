//! Comparison of two result sets and the failure report (testing.md section 4.5).

use crate::format::{Config, Outcome, Vector, sig};
use crate::set::VectorSet;
use std::fmt::Write as _;

/// Differences between two outcomes, one line per differing field; empty if equal.
#[must_use]
pub fn diff_outcomes(a: &Outcome, b: &Outcome) -> Vec<String> {
    let mut d = Vec::new();
    if a.signal != b.signal {
        d.push(format!(
            "signal: {} vs {}",
            sig_name(a.signal),
            sig_name(b.signal)
        ));
    }
    if a.state.pc != b.state.pc {
        d.push(format!("pc: {:#018x} vs {:#018x}", a.state.pc, b.state.pc));
    }
    for i in 0..31 {
        if a.state.x[i] != b.state.x[i] {
            d.push(format!(
                "x{i}: {:#018x} vs {:#018x}",
                a.state.x[i], b.state.x[i]
            ));
        }
    }
    if a.state.sp != b.state.sp {
        d.push(format!("sp: {:#018x} vs {:#018x}", a.state.sp, b.state.sp));
    }
    if a.state.nzcv != b.state.nzcv {
        d.push(format!(
            "nzcv: {} vs {}",
            nzcv_str(a.state.nzcv),
            nzcv_str(b.state.nzcv)
        ));
    }
    if a.mem != b.mem {
        d.push("data window differs".to_string());
    }
    d
}

/// Printable signal name.
#[must_use]
pub fn sig_name(s: u16) -> String {
    match s {
        sig::ILL => "SIGILL".into(),
        sig::TRAP => "SIGTRAP".into(),
        sig::BUS => "SIGBUS".into(),
        sig::SEGV => "SIGSEGV".into(),
        sig::T0_UNIMPLEMENTED => "T0-UNIMPLEMENTED".into(),
        sig::T0_STEP_LIMIT => "T0-STEP-LIMIT".into(),
        n => format!("signal {n}"),
    }
}

fn nzcv_str(v: u64) -> String {
    let b = |bit: u32, c: char| if v >> bit & 1 != 0 { c } else { '-' };
    format!("{}{}{}{}", b(31, 'N'), b(30, 'Z'), b(29, 'C'), b(28, 'V'))
}

/// One failing vector.
#[derive(Debug)]
pub struct Mismatch {
    /// Global vector index.
    pub index: usize,
    /// Class name.
    pub class: &'static str,
    /// Index inside the class.
    pub class_index: usize,
    /// Field differences.
    pub diffs: Vec<String>,
}

/// Result of comparing two result sets.
#[derive(Debug, Default)]
pub struct Report {
    /// Vectors compared.
    pub compared: usize,
    /// Failing vectors.
    pub mismatches: Vec<Mismatch>,
}

/// Compares `a` (named `a_name`) with `b` (named `b_name`) vector by vector.
///
/// # Panics
///
/// If the lengths do not match the vector set.
#[must_use]
pub fn compare(set: &VectorSet, a: &[Outcome], b: &[Outcome]) -> Report {
    assert_eq!(a.len(), set.vectors.len());
    assert_eq!(b.len(), set.vectors.len());
    let mut report = Report {
        compared: a.len(),
        ..Report::default()
    };
    for i in 0..a.len() {
        let diffs = diff_outcomes(&a[i], &b[i]);
        if !diffs.is_empty() {
            let span = set.span_of(i).expect("every vector belongs to a class");
            report.mismatches.push(Mismatch {
                index: i,
                class: span.class.name,
                class_index: i - span.start,
                diffs,
            });
        }
    }
    report
}

/// Hex dump of the words of a vector, `llvm-mc`-style bytes.
#[must_use]
pub fn hex_words(v: &Vector) -> String {
    v.words
        .iter()
        .map(|w| format!("{w:08x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Human-readable report of at most `limit` mismatches, ending with the repeat commands.
#[must_use]
pub fn render(
    set: &VectorSet,
    a: &[Outcome],
    b: &[Outcome],
    a_name: &str,
    b_name: &str,
    report: &Report,
    limit: usize,
) -> String {
    let mut s = String::new();
    let cfg: &Config = &set.cfg;
    let _ = writeln!(
        s,
        "{} of {} vectors differ ({a_name} vs {b_name}); showing {}",
        report.mismatches.len(),
        report.compared,
        report.mismatches.len().min(limit)
    );
    let mut per_class: Vec<(&str, usize)> = Vec::new();
    for m in &report.mismatches {
        match per_class.iter_mut().find(|(c, _)| *c == m.class) {
            Some((_, n)) => *n += 1,
            None => per_class.push((m.class, 1)),
        }
    }
    let _ = writeln!(s, "per class: {per_class:?}");
    for m in report.mismatches.iter().take(limit) {
        let v = &set.vectors[m.index];
        let _ = writeln!(
            s,
            "\n--- {} #{} (global {}) seed {:#x}",
            m.class, m.class_index, m.index, cfg.seed
        );
        let _ = writeln!(s, "words    : {}", hex_words(v));
        let _ = writeln!(s, "initial  : sp={:#x} nzcv={}", v.sp, nzcv_str(v.nzcv));
        for d in &m.diffs {
            let _ = writeln!(s, "  {d}");
            // Initial value of the differing register, for orientation.
            if let Some(reg) = d
                .strip_prefix('x')
                .and_then(|r| r.split(':').next())
                .and_then(|r| r.parse::<usize>().ok())
            {
                let _ = writeln!(s, "      (initial x{reg} = {:#018x})", v.x[reg]);
            }
        }
        let _ = writeln!(s, "  {a_name}: {}", describe(&a[m.index]));
        let _ = writeln!(s, "  {b_name}: {}", describe(&b[m.index]));
    }
    if let Some(m) = report.mismatches.first() {
        let _ = writeln!(
            s,
            "\nRepeat the first failure:\n  RITSU_TEST_SEED={:#x} cargo run --manifest-path tests/dbt-ref/Cargo.toml -- show --units {} --class {} --index {}\n\
             Run the whole set again against a live reference:\n  RITSU_TEST_SEED={:#x} DBT_REF_UNITS={} make dbt-ref-fresh",
            cfg.seed, cfg.units, m.class, m.class_index, cfg.seed, cfg.units
        );
    }
    s
}

fn describe(o: &Outcome) -> String {
    format!("{} at pc {:#x}", sig_name(o.signal), o.state.pc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::State;

    fn outcome() -> Outcome {
        Outcome {
            signal: sig::TRAP,
            si_code: 0,
            state: State {
                x: [0; 31],
                sp: 0,
                nzcv: 0,
                pc: 0x100,
            },
            mem: None,
        }
    }

    #[test]
    fn equal_outcomes_have_no_diff() {
        let mut b = outcome();
        b.si_code = 99; // informational only
        assert!(diff_outcomes(&outcome(), &b).is_empty());
    }

    #[test]
    fn every_field_is_compared() {
        let a = outcome();
        let mut b = outcome();
        b.signal = sig::ILL;
        b.state.pc += 4;
        b.state.x[7] = 1;
        b.state.sp = 2;
        b.state.nzcv = 0xf000_0000;
        b.mem = Some(vec![1]);
        let d = diff_outcomes(&a, &b);
        assert_eq!(d.len(), 6, "{d:?}");
        assert!(d.iter().any(|l| l.starts_with("x7: ")));
        assert!(d.iter().any(|l| l.contains("NZCV")));
    }
}
