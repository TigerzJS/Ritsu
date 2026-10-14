//! Command-line parsing for `ritsu-vmm` (std only, no external crates).
//!
//! Accepted forms: `--flag value` and `--flag=value`. Later occurrences of a flag override
//! earlier ones. Paths are kept as `OsString`/`PathBuf` so non-UTF-8 paths work.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use ritsu_platform::machine::{ConfigError, MachineConfig};

/// What the user asked `ritsu-vmm` to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Build a machine from the configuration and run it.
    Run(MachineConfig),
    /// Print usage and exit successfully.
    Help,
    /// Print the version and exit successfully.
    Version,
}

/// A command-line error.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliError {
    /// A flag that needs a value was the last argument.
    MissingValue(String),
    /// An argument that is not a known flag.
    UnknownArgument(String),
    /// A numeric flag got a value that is not a valid unsigned number.
    InvalidNumber {
        /// The flag name.
        flag: String,
        /// The offending value.
        value: String,
    },
    /// The assembled configuration violates a machine limit.
    Config(ConfigError),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue(flag) => write!(f, "option {flag} requires a value"),
            Self::UnknownArgument(arg) => write!(f, "unknown argument: {arg}"),
            Self::InvalidNumber { flag, value } => {
                write!(f, "invalid number for {flag}: {value:?}")
            }
            Self::Config(e) => write!(f, "invalid configuration: {e}"),
        }
    }
}

impl std::error::Error for CliError {}

/// Usage text printed for `--help` and after usage errors.
pub(crate) const USAGE: &str = "\
Usage: ritsu-vmm [OPTIONS]

Options:
  --ram <MiB>          Guest RAM in MiB (default 8192, min 2048, multiple of 16 KiB)
  --cpus <n>           Number of vCPUs (default 4, 1..=8)
  --disk <path>        Root disk image
  --aux <path>         Auxiliary storage image (NVRAM / boot state)
  --boot-rom <path>    User-supplied boot ROM (never shipped with Ritsu)
  --help               Print this help and exit
  --version            Print version and exit

Booting is not implemented yet (milestone M0 scaffold).
";

/// The version line printed for `--version`.
pub(crate) fn version_text() -> String {
    format!("ritsu-vmm {}", env!("CARGO_PKG_VERSION"))
}

fn text(arg: &OsString) -> String {
    arg.to_string_lossy().into_owned()
}

fn parse_number(flag: &str, value: &OsString) -> Result<u64, CliError> {
    value
        .to_str()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| CliError::InvalidNumber {
            flag: flag.to_owned(),
            value: text(value),
        })
}

/// Parses the arguments (without the program name).
///
/// `--help` and `--version` take effect as soon as they are seen, before the rest is
/// validated. For [`Command::Run`] the configuration is validated with
/// [`MachineConfig::validate`].
pub(crate) fn parse<I>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut config = MachineConfig::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(s) = arg.to_str() else {
            return Err(CliError::UnknownArgument(text(&arg)));
        };
        // Split `--flag=value`.
        let (flag, inline) = match s.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(OsString::from(v))),
            _ => (s.to_owned(), None),
        };
        match flag.as_str() {
            "--help" | "-h" if inline.is_none() => return Ok(Command::Help),
            "--version" | "-V" if inline.is_none() => return Ok(Command::Version),
            "--ram" | "--cpus" | "--disk" | "--aux" | "--boot-rom" => {
                let value = match inline {
                    Some(v) => v,
                    None => args
                        .next()
                        .ok_or_else(|| CliError::MissingValue(flag.clone()))?,
                };
                match flag.as_str() {
                    "--ram" => {
                        let mib = parse_number(&flag, &value)?;
                        config.ram_bytes =
                            mib.checked_mul(1 << 20)
                                .ok_or_else(|| CliError::InvalidNumber {
                                    flag: flag.clone(),
                                    value: text(&value),
                                })?;
                    }
                    "--cpus" => {
                        let n = parse_number(&flag, &value)?;
                        // Out-of-range values are rejected by validate(); saturate so that
                        // huge numbers still produce a readable range error.
                        config.cpus = u32::try_from(n).unwrap_or(u32::MAX);
                    }
                    "--disk" => config.disk = Some(PathBuf::from(value)),
                    "--aux" => config.aux = Some(PathBuf::from(value)),
                    _ => config.boot_rom = Some(PathBuf::from(value)),
                }
            }
            _ => return Err(CliError::UnknownArgument(s.to_owned())),
        }
    }
    config.validate().map_err(CliError::Config)?;
    Ok(Command::Run(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn run_config(list: &[&str]) -> MachineConfig {
        match parse(args(list)) {
            Ok(Command::Run(c)) => c,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn defaults() {
        assert_eq!(run_config(&[]), MachineConfig::default());
    }

    #[test]
    fn all_options() {
        let c = run_config(&[
            "--ram",
            "4096",
            "--cpus",
            "6",
            "--disk",
            "/d.img",
            "--aux=/a.img",
            "--boot-rom",
            "/rom.bin",
        ]);
        assert_eq!(c.ram_bytes, 4096 << 20);
        assert_eq!(c.cpus, 6);
        assert_eq!(c.disk, Some(PathBuf::from("/d.img")));
        assert_eq!(c.aux, Some(PathBuf::from("/a.img")));
        assert_eq!(c.boot_rom, Some(PathBuf::from("/rom.bin")));
    }

    #[test]
    fn later_flag_wins() {
        assert_eq!(run_config(&["--cpus", "2", "--cpus=3"]).cpus, 3);
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse(args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse(args(&["-h"])), Ok(Command::Help));
        assert_eq!(parse(args(&["--version"])), Ok(Command::Version));
        // help wins even if the other arguments would fail validation
        assert_eq!(parse(args(&["--ram", "1", "--help"])), Ok(Command::Help));
    }

    #[test]
    fn missing_value() {
        assert_eq!(
            parse(args(&["--ram"])),
            Err(CliError::MissingValue("--ram".into()))
        );
        assert_eq!(
            parse(args(&["--cpus", "2", "--disk"])),
            Err(CliError::MissingValue("--disk".into()))
        );
    }

    #[test]
    fn unknown_flag() {
        assert_eq!(
            parse(args(&["--bogus"])),
            Err(CliError::UnknownArgument("--bogus".into()))
        );
        assert_eq!(
            parse(args(&["positional"])),
            Err(CliError::UnknownArgument("positional".into()))
        );
        assert_eq!(
            parse(args(&["--help=1"])),
            Err(CliError::UnknownArgument("--help=1".into()))
        );
    }

    #[test]
    fn invalid_numbers() {
        for bad in ["abc", "-1", "", "1.5", "0x10", "99999999999999999999999"] {
            assert!(
                matches!(
                    parse(args(&["--ram", bad])),
                    Err(CliError::InvalidNumber { .. })
                ),
                "ram {bad:?}"
            );
            assert!(
                matches!(
                    parse(args(&["--cpus", bad])),
                    Err(CliError::InvalidNumber { .. })
                ),
                "cpus {bad:?}"
            );
        }
        // MiB -> bytes overflow
        assert!(matches!(
            parse(args(&["--ram", "18446744073709551615"])),
            Err(CliError::InvalidNumber { .. })
        ));
    }

    #[test]
    fn config_limits_are_enforced() {
        assert!(matches!(
            parse(args(&["--ram", "16"])),
            Err(CliError::Config(ConfigError::RamTooSmall { .. }))
        ));
        assert!(matches!(
            parse(args(&["--cpus", "0"])),
            Err(CliError::Config(ConfigError::CpuCount { cpus: 0 }))
        ));
        assert!(matches!(
            parse(args(&["--cpus", "99999999999"])),
            Err(CliError::Config(ConfigError::CpuCount { .. }))
        ));
    }

    #[test]
    fn error_messages() {
        assert!(
            CliError::MissingValue("--ram".into())
                .to_string()
                .contains("--ram")
        );
        assert!(
            CliError::UnknownArgument("x".into())
                .to_string()
                .contains("unknown")
        );
    }
}
