//! Records the compiler version and build settings so results can state under which
//! configuration they were measured.

use std::process::Command;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |s| s.trim().to_owned());
    println!("cargo:rustc-env=RITSU_BENCH_RUSTC={version}");
    for (var, key) in [
        ("PROFILE", "RITSU_BENCH_PROFILE"),
        ("OPT_LEVEL", "RITSU_BENCH_OPT_LEVEL"),
        ("CARGO_ENCODED_RUSTFLAGS", "RITSU_BENCH_RUSTFLAGS"),
    ] {
        let value = std::env::var(var).unwrap_or_default().replace('\x1f', " ");
        println!("cargo:rustc-env={key}={value}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RUSTFLAGS");
}
