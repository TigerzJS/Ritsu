//! Run metadata: commit, host CPU, toolchain, configuration. Everything a number needs to be
//! interpreted later (docs/process/testing.md section 8).

use crate::harness::Config;
use crate::json::Json;
use std::fs;
use std::process::Command;

/// Value of the `RITSU_TEST_SEED` environment variable (decimal or `0x` hex) or `default`.
#[must_use]
pub fn seed_from_env(default: u64) -> u64 {
    match std::env::var("RITSU_TEST_SEED") {
        Ok(v) => {
            let v = v.trim();
            let parsed = v
                .strip_prefix("0x")
                .map_or_else(|| v.parse::<u64>(), |h| u64::from_str_radix(h, 16));
            parsed.unwrap_or_else(|_| panic!("RITSU_TEST_SEED={v:?} is not a u64"))
        }
        Err(_) => default,
    }
}

/// Fields of the first CPU entry in `/proc/cpuinfo`, plus the logical CPU count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuInfo {
    /// `model name` (x86) or `Hardware`/`CPU part` fallback; `unknown` if unreadable.
    pub model: String,
    /// Number of `processor` entries.
    pub logical_cpus: usize,
    /// Whether the `avx2` flag is present.
    pub avx2: bool,
    /// Whether the `avx512f` flag is present (the G5 MF has none, CLAUDE.md section 3 rule 4).
    pub avx512f: bool,
}

/// Parses the text of `/proc/cpuinfo`.
#[must_use]
pub fn parse_cpuinfo(text: &str) -> CpuInfo {
    let mut model = None;
    let mut logical = 0usize;
    let mut flags = "";
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "processor" => logical += 1,
            "model name" | "Model" | "Hardware" if model.is_none() => {
                model = Some(value.to_owned());
            }
            "flags" | "Features" if flags.is_empty() => flags = value,
            _ => {}
        }
    }
    let has = |f: &str| flags.split_whitespace().any(|x| x == f);
    CpuInfo {
        model: model.unwrap_or_else(|| "unknown".to_owned()),
        logical_cpus: logical,
        avx2: has("avx2"),
        avx512f: has("avx512f"),
    }
}

fn read_trim(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR")])
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix timestamp.
#[must_use]
pub fn iso8601_utc(unix_secs: i64) -> String {
    let (y, m, d) = civil_from_days(unix_secs.div_euclid(86_400));
    let s = unix_secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

/// Everything that describes a run, as JSON.
#[must_use]
pub fn collect(host_id: &str, cfg: &Config, quick: bool, seed: u64, filter: Option<&str>) -> Json {
    let cpuinfo = parse_cpuinfo(&fs::read_to_string("/proc/cpuinfo").unwrap_or_default());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let opt_or_null = |o: Option<String>| o.map_or(Json::Null, Json::Str);
    Json::obj([
        ("created_utc", Json::str(iso8601_utc(now))),
        ("host_id", Json::str(host_id)),
        ("commit", opt_or_null(git(&["rev-parse", "HEAD"]))),
        (
            // Tracked files only: results/ and build output are ignored, other agents' new files
            // do not count. A dirty tree means the numbers are not reproducible from the commit.
            "dirty",
            git(&["status", "--porcelain", "--untracked-files=no"])
                .map_or(Json::Null, |s| Json::Bool(!s.is_empty())),
        ),
        ("rustc", Json::str(env!("RITSU_BENCH_RUSTC"))),
        (
            "profile",
            Json::obj([
                ("name", Json::str(env!("RITSU_BENCH_PROFILE"))),
                ("opt_level", Json::str(env!("RITSU_BENCH_OPT_LEVEL"))),
                ("debug_assertions", Json::Bool(cfg!(debug_assertions))),
                (
                    "panic",
                    Json::str(if cfg!(panic = "abort") {
                        "abort"
                    } else {
                        "unwind"
                    }),
                ),
                ("rustflags", Json::str(env!("RITSU_BENCH_RUSTFLAGS"))),
            ]),
        ),
        (
            "cpu",
            Json::obj([
                ("model", Json::str(&cpuinfo.model)),
                (
                    "logical_cpus",
                    Json::Int(i64::try_from(cpuinfo.logical_cpus).unwrap_or(-1)),
                ),
                ("avx2", Json::Bool(cpuinfo.avx2)),
                ("avx512f", Json::Bool(cpuinfo.avx512f)),
                (
                    "governor",
                    opt_or_null(read_trim(
                        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
                    )),
                ),
            ]),
        ),
        (
            "os",
            Json::obj([
                (
                    "kernel",
                    opt_or_null(read_trim("/proc/sys/kernel/osrelease")),
                ),
                ("arch", Json::str(std::env::consts::ARCH)),
            ]),
        ),
        (
            "config",
            Json::obj([
                ("quick", Json::Bool(quick)),
                (
                    "warmup_ms",
                    Json::Int(i64::try_from(cfg.warmup.as_millis()).unwrap_or(-1)),
                ),
                (
                    "sample_ms",
                    Json::Int(i64::try_from(cfg.sample_time.as_millis()).unwrap_or(-1)),
                ),
                (
                    "samples",
                    Json::Int(i64::try_from(cfg.samples).unwrap_or(-1)),
                ),
                ("filter", opt_or_null(filter.map(str::to_owned))),
                // Only payload bytes depend on the seed; work counts do not.
                ("seed", Json::Int(i64::try_from(seed).unwrap_or(-1))),
            ]),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpuinfo_parsing() {
        let text = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i5-12500H\nflags\t\t: fpu avx2 bmi2\n\nprocessor\t: 1\nmodel name\t: other\nflags\t\t: avx512f\n";
        let c = parse_cpuinfo(text);
        assert_eq!(c.model, "Intel(R) Core(TM) i5-12500H");
        assert_eq!(c.logical_cpus, 2);
        assert!(c.avx2);
        assert!(!c.avx512f, "only the first entry's flags count");
        assert_eq!(parse_cpuinfo("").model, "unknown");
    }

    #[test]
    fn dates_match_known_timestamps() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso8601_utc(1_791_936_000), "2026-10-14T00:00:00Z");
    }
}
