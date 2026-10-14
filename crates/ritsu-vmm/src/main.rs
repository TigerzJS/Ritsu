//! Ritsu host process: wires CPU, platform and GPU together.
//!
//! Milestone M0 scaffold: parses the command line into a validated
//! [`ritsu_platform::machine::MachineConfig`] and then reports that booting is not
//! implemented yet.
//!
//! Exit codes: `0` for `--help`/`--version`, `2` when a valid configuration was given but
//! booting is not implemented, `64` for command-line errors (as in `sysexits.h` `EX_USAGE`).

mod cli;

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

/// Exit code for "valid request, but not implemented yet".
const EXIT_NOT_IMPLEMENTED: u8 = 2;
/// Exit code for command-line usage errors.
const EXIT_USAGE: u8 = 64;

/// Runs the program with explicit arguments and output streams; returns the exit code.
fn run<I>(args: I, out: &mut dyn Write, err: &mut dyn Write) -> u8
where
    I: IntoIterator<Item = OsString>,
{
    // Write errors on stdout/stderr (e.g. closed pipe) are deliberately ignored: there is
    // nowhere left to report them.
    match cli::parse(args) {
        Ok(cli::Command::Help) => {
            let _ = out.write_all(cli::USAGE.as_bytes());
            0
        }
        Ok(cli::Command::Version) => {
            let _ = writeln!(out, "{}", cli::version_text());
            0
        }
        Ok(cli::Command::Run(config)) => {
            let _ = writeln!(
                err,
                "ritsu-vmm: booting is not implemented yet (M0 scaffold). Configuration is valid: {} MiB RAM, {} vCPUs.",
                config.ram_bytes >> 20,
                config.cpus
            );
            EXIT_NOT_IMPLEMENTED
        }
        Err(e) => {
            let _ = writeln!(err, "ritsu-vmm: {e}\n\n{}", cli::USAGE);
            EXIT_USAGE
        }
    }
}

fn main() -> ExitCode {
    let code = run(
        std::env::args_os().skip(1),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_str(args: &[&str]) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(args.iter().map(OsString::from), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn help_exits_zero() {
        let (code, out, err) = run_str(&["--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("Usage: ritsu-vmm"));
        assert!(err.is_empty());
    }

    #[test]
    fn version_exits_zero() {
        let (code, out, _) = run_str(&["--version"]);
        assert_eq!(code, 0);
        assert!(out.starts_with("ritsu-vmm "));
    }

    #[test]
    fn valid_run_says_not_implemented_and_exits_2() {
        let (code, out, err) = run_str(&["--ram", "4096", "--cpus", "2"]);
        assert_eq!(code, 2);
        assert!(out.is_empty());
        assert!(err.contains("not implemented"), "{err}");
    }

    #[test]
    fn usage_error_exits_64() {
        let (code, _, err) = run_str(&["--nope"]);
        assert_eq!(code, 64);
        assert!(err.contains("unknown argument"));
    }
}
