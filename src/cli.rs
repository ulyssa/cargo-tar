//! Parse command-line arguments.
use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;

use crate::compress::Format;

/// Wrapper matching how cargo invokes subcommands: `cargo tar <args>`.
#[derive(Debug, Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum Cargo {
    Tarball(TarballArgs),
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Build compressed release archives from a Cargo project",
    long_about = None,
)]
pub struct TarballArgs {
    /// Package to archive when within a workspace.
    #[arg(short, long, value_name = "SPEC")]
    pub package: Option<String>,

    /// Path to Cargo.toml.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Build for the given target triple.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,

    /// Directory for all generated artifacts.
    #[arg(long, value_name = "DIR")]
    pub target_dir: Option<PathBuf>,

    /// Cargo profile to build.
    #[arg(long, value_name = "NAME", default_value = "release")]
    pub profile: String,

    /// Space or comma separated list of features to activate; repeatable.
    #[arg(short = 'F', long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Do not activate the `default` feature.
    #[arg(long)]
    pub no_default_features: bool,

    /// Activate all available features.
    #[arg(long, conflicts_with = "no_default_features")]
    pub all_features: bool,

    /// Only include the named binary; repeatable.
    #[arg(long, value_name = "NAME")]
    pub bin: Vec<String>,

    /// Package already-built binaries instead of running `cargo build`.
    #[arg(long)]
    pub no_build: bool,

    /// Output directory, or output file when a single format is requested.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Name of the directory everything is placed under inside the archive.
    #[arg(long, value_name = "NAME")]
    pub root_dir: Option<String>,

    /// Directory inside the archive that binaries are placed in.
    #[arg(long, value_name = "DIR")]
    pub bin_dir: Option<String>,

    /// Override archive format from Cargo.toml.
    #[arg(long, value_name = "FORMAT")]
    pub format: Vec<Format>,

    /// Override the user name used in the archive as the owning user.
    #[arg(long, value_name = "NAME")]
    pub user: Option<String>,

    /// Override the group name used in the archive as the owning group.
    #[arg(long, value_name = "NAME")]
    pub group: Option<String>,

    /// Override the default compression level (1-9).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=9))]
    pub compression_level: Option<u32>,

    /// Timestamp for every archive entry in Unix epoch seconds.
    #[arg(long, value_name = "EPOCH", env = "SOURCE_DATE_EPOCH")]
    pub source_date: Option<u64>,

    /// Print more detail about what is being archived.
    #[arg(short, long)]
    pub verbose: bool,

    /// Only print errors.
    #[arg(short, long, conflicts_with = "verbose")]
    pub quiet: bool,
}

impl TarballArgs {
    /// Parse arguments, accepting both `cargo tar ...` and `cargo-tar ...`.
    pub fn from_env() -> Self {
        TarballArgs::from_iter(std::env::args_os())
    }

    fn from_iter(argv: impl IntoIterator<Item = OsString>) -> Self {
        let argv: Vec<OsString> = argv.into_iter().collect();

        if argv.get(1).is_some_and(|arg| arg == "tar") {
            let Cargo::Tarball(args) = Cargo::parse_from(argv);
            args
        } else {
            Parser::parse_from(argv)
        }
    }

    /// The formats requested on the command-line, if any.
    pub fn formats(&self) -> Vec<Format> {
        self.format.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(argv: &[&str]) -> TarballArgs {
        TarballArgs::from_iter(argv.iter().map(OsString::from))
    }

    #[test]
    fn test_parses_both_invocation_forms() {
        let direct = args(&["cargo-tar", "--format", "tar"]);
        let via_cargo = args(&["cargo-tar", "tar", "--format", "tar"]);

        assert_eq!(direct.format, [Format::Tar]);
        assert_eq!(via_cargo.format, [Format::Tar]);
    }

    #[test]
    fn test_profile_defaults_to_release() {
        assert_eq!(args(&["cargo-tar"]).profile, "release");
        assert_eq!(args(&["cargo-tar", "--profile", "release"]).profile, "release");
        assert_eq!(args(&["cargo-tar", "--profile", "dev"]).profile, "dev");
        assert_eq!(args(&["cargo-tar", "--profile", "fast"]).profile, "fast");
    }
}
