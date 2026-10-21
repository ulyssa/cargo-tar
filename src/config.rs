//! Logic for parsing `[package.metadata.tar]` and merging in command-line arguments.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::assets::DEFAULT_DIR_MODE;
use crate::cargo::Package;
use crate::cli::TarballArgs;
use crate::compress::Format;

pub const DEFAULT_COMPRESSION_LEVEL: u32 = 6;

/// A file mode, written either as an octal string or as a number.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Mode {
    /// `mode = "755"`, parsed as octal.
    Text(String),
    /// `mode = 0o755`, used as-is.
    Number(u32),
}

impl Mode {
    pub fn bits(&self) -> Result<u32> {
        let bits = match self {
            Mode::Text(text) => u32::from_str_radix(text.trim_start_matches("0o"), 8)
                .with_context(|| {
                    format!("invalid file mode `{text}`, expected octal digits like \"755\"")
                })?,
            Mode::Number(bits) => *bits,
        };

        if bits > 0o7777 {
            bail!("file mode {bits:#o} is out of range");
        }

        Ok(bits)
    }
}

/// An asset as written in the manifest.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawAsset {
    /// Path relative to the package root; may contain glob patterns.
    pub source: String,

    /// Path inside the archive root directory. A trailing `/` means a directory.
    pub dest: String,

    /// Defaults to 755 for executable sources, 644 otherwise.
    pub mode: Option<Mode>,
}

/// The `[package.metadata.tar]` table.
///
/// Keys are snake_case, with kebab-case accepted as an alias since cargo
/// manifests use both conventions.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawConfig {
    pub format: Option<Format>,

    /// Directory in which to place all other contents inside the archive.
    pub root_dir: Option<String>,

    /// Directory in which to place the binary relative to the `root_dir`.
    pub bin_dir: Option<String>,

    /// The name of the user to use for the archive contents.
    pub user: Option<String>,

    /// The name of the group to use for the archive contents.
    pub group: Option<String>,

    pub dir_mode: Option<Mode>,

    /// Compression level for the selected compression algorithm.
    pub compression_level: Option<u32>,

    /// Files to include in the output archive.
    pub assets: Option<Vec<RawAsset>>,
}

impl RawConfig {
    /// Read the table out of `cargo metadata` output.
    pub fn from_package(package: &Package) -> Result<RawConfig> {
        match package.tar_metadata() {
            Some(table) => {
                RawConfig::deserialize(table).context("invalid [package.metadata.tar] section")
            },
            None => Ok(RawConfig::default()),
        }
    }
}

/// Where a single archive is written.
#[derive(Debug)]
pub struct Output {
    pub format: Format,
    pub path: PathBuf,
}

/// The merged, validated configuration for one run.
#[derive(Debug)]
pub struct Config {
    pub formats: Vec<Format>,
    pub root_dir: String,
    pub bin_dir: String,
    pub user: String,
    pub group: String,
    pub dir_mode: u32,
    pub compression_level: u32,
    pub assets: Vec<RawAsset>,
    pub source_date: Option<u64>,
    pub output_dir: PathBuf,
    /// Set when `--output` named a file rather than a directory.
    pub output_file: Option<PathBuf>,
    pub file_stem: String,
}

impl Config {
    pub fn build(
        args: &TarballArgs,
        package: &Package,
        raw: RawConfig,
        target_dir: &Path,
    ) -> Result<Config> {
        let mut formats = args.formats();

        if formats.is_empty() {
            formats = vec![raw.format.unwrap_or_default()];
        }

        formats.sort_unstable();
        formats.dedup();

        let root_dir_template = args
            .root_dir
            .clone()
            .or(raw.root_dir)
            .unwrap_or_else(|| "{name}-{version}".to_string());

        let root_dir = substitute(&root_dir_template, package, args.target.as_deref());
        validate_root_dir(&root_dir)?;

        let bin_dir = normalize_dir(&args.bin_dir.clone().or(raw.bin_dir).unwrap_or_default())?;

        let dir_mode = match &raw.dir_mode {
            Some(mode) => mode.bits()?,
            None => DEFAULT_DIR_MODE,
        };

        let mut file_stem = format!("{}-{}", package.name, package.version);

        if let Some(target) = &args.target {
            file_stem.push('-');
            file_stem.push_str(target);
        }

        let (output_dir, output_file) = resolve_output(args, &formats, target_dir)?;

        Ok(Config {
            formats,
            root_dir,
            bin_dir,
            user: args.user.clone().or(raw.user).unwrap_or_else(|| package.name.clone()),
            group: args.group.clone().or(raw.group).unwrap_or_else(|| package.name.clone()),
            dir_mode,
            compression_level: args
                .compression_level
                .or(raw.compression_level)
                .unwrap_or(DEFAULT_COMPRESSION_LEVEL),
            assets: raw.assets.unwrap_or_default(),
            source_date: args.source_date,
            output_dir,
            output_file,
            file_stem,
        })
    }

    /// The archive to write for each requested format.
    pub fn outputs(&self) -> Vec<Output> {
        self.formats
            .iter()
            .map(|&format| {
                let path = match &self.output_file {
                    Some(path) => path.clone(),
                    None => {
                        self.output_dir.join(format!("{}.{}", self.file_stem, format.extension()))
                    },
                };

                Output { format, path }
            })
            .collect()
    }
}

/// Expand `{name}`, `{version}`, and `{target}` in a template.
fn substitute(template: &str, package: &Package, target: Option<&str>) -> String {
    template
        .replace("{name}", &package.name)
        .replace("{version}", &package.version)
        .replace("{target}", target.unwrap_or_default())
}

fn validate_root_dir(root_dir: &str) -> Result<()> {
    if root_dir.is_empty() {
        bail!("the archive root directory cannot be empty");
    }

    if root_dir.starts_with('/') || root_dir.contains("..") {
        bail!("the archive root directory `{root_dir}` must be a plain relative name");
    }

    Ok(())
}

/// Clean up a directory path written in configuration: no leading or trailing
/// slashes, no `.` or `..` components.
fn normalize_dir(dir: &str) -> Result<String> {
    let mut parts = Vec::new();

    for part in dir.split('/') {
        match part {
            "" | "." => continue,
            ".." => bail!("`{dir}` must not contain `..`"),
            part => parts.push(part),
        }
    }

    Ok(parts.join("/"))
}

/// Split `--output` into the directory to write into and, when it named a single
/// archive file rather than a directory, that file's path.
fn resolve_output(
    args: &TarballArgs,
    formats: &[Format],
    target_dir: &Path,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let Some(path) = &args.output else {
        return Ok((target_dir.join("tar"), None));
    };

    if !names_a_file(path, formats) {
        return Ok((path.clone(), None));
    }

    if formats.len() > 1 {
        bail!(
            "`--output {}` names a single file, but {} formats were requested; \
             pass a directory instead",
            path.display(),
            formats.len()
        );
    }

    let dir = path.parent().filter(|parent| !parent.as_os_str().is_empty());

    Ok((dir.unwrap_or(Path::new(".")).to_path_buf(), Some(path.clone())))
}

/// Whether a `--output` path names an archive file rather than a directory.
fn names_a_file(path: &Path, formats: &[Format]) -> bool {
    if path.is_dir() {
        return false;
    }

    let name = match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name.to_ascii_lowercase(),
        None => return false,
    };

    formats.iter().any(|format| name.ends_with(format.extension())) || name.ends_with(".tar")
}

/// A destination path inside the archive root directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dest {
    /// Cleaned path, relative to the archive root, without a trailing slash.
    pub path: String,
    /// Whether the manifest wrote this as a directory (trailing `/`).
    pub is_dir: bool,
}

impl Dest {
    /// Parse an asset destination.
    ///
    /// Leading `/` is stripped, so dests pasted from a cargo-deb or
    /// cargo-generate-rpm manifest still land inside the archive root.
    pub fn parse(dest: &str) -> Result<Dest> {
        let is_dir = dest.is_empty() || dest.ends_with('/') || dest.ends_with("/.") || dest == ".";
        let path = normalize_dir(dest)?;

        if !is_dir && path.is_empty() {
            bail!("asset dest `{dest}` is not a usable path");
        }

        Ok(Dest { path, is_dir })
    }

    /// Resolve against a source file name, appending it for directory dests.
    pub fn resolve(&self, source_name: &str) -> String {
        match self.is_dir {
            true if self.path.is_empty() => source_name.to_string(),
            true => format!("{}/{source_name}", self.path),
            false => self.path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modes_accept_octal_text_and_numbers() {
        assert_eq!(Mode::Text("755".into()).bits().unwrap(), 0o755);
        assert_eq!(Mode::Text("0755".into()).bits().unwrap(), 0o755);
        assert_eq!(Mode::Text("0o644".into()).bits().unwrap(), 0o644);
        assert_eq!(Mode::Number(0o755).bits().unwrap(), 0o755);

        assert!(Mode::Text("not-a-mode".into()).bits().is_err());
        assert!(Mode::Text("888".into()).bits().is_err());
        assert!(Mode::Number(0o100755).bits().is_err());
    }

    #[test]
    fn test_dests_are_relative_to_the_archive_root() {
        let dest = Dest::parse("/usr/bin/iamb").unwrap();
        assert_eq!(dest.path, "usr/bin/iamb");
        assert!(!dest.is_dir);
        assert_eq!(dest.resolve("iamb"), "usr/bin/iamb");
    }

    #[test]
    fn test_directory_dests_take_the_source_name() {
        let dest = Dest::parse("share/themes/").unwrap();
        assert!(dest.is_dir);
        assert_eq!(dest.resolve("dark.toml"), "share/themes/dark.toml");

        let root = Dest::parse("").unwrap();
        assert!(root.is_dir);
        assert_eq!(root.resolve("README.md"), "README.md");
    }

    #[test]
    fn test_dests_reject_parent_components() {
        let message = Dest::parse("../escape").unwrap_err().to_string();
        assert!(message.contains("`..`"), "{message}");
    }

    #[test]
    fn test_bin_dirs_are_normalized() {
        assert_eq!(normalize_dir("").unwrap(), "");
        assert_eq!(normalize_dir("/bin/").unwrap(), "bin");
        assert_eq!(normalize_dir("./usr/bin").unwrap(), "usr/bin");
        assert!(normalize_dir("bin/../..").is_err());
    }

    #[test]
    fn test_root_dirs_must_be_relative() {
        assert!(validate_root_dir("iamb-1.0.0").is_ok());
        assert!(validate_root_dir("").is_err());
        assert!(validate_root_dir("/iamb").is_err());
        assert!(validate_root_dir("../iamb").is_err());
    }

    #[test]
    fn test_output_paths_are_recognized_as_files() {
        let formats = [Format::Tar];

        assert!(names_a_file(Path::new("dist/thing.tar"), &formats));
        assert!(names_a_file(Path::new("dist/thing.tar.gz"), &[Format::TarGz]));
        assert!(!names_a_file(Path::new("dist"), &formats));
    }

    #[test]
    fn test_output_splits_into_a_directory_and_an_optional_file() {
        use clap::Parser;

        let args = |argv: &[&str]| TarballArgs::parse_from(argv);
        let target = Path::new("/target");
        let tar = [Format::Tar];

        // With no `--output` the archive lands under the target directory.
        let (dir, file) = resolve_output(&args(&["cargo-tar"]), &tar, target).unwrap();
        assert_eq!(dir, Path::new("/target/tar"));
        assert_eq!(file, None);

        // A path that does not look like an archive stays a directory.
        let argv = args(&["cargo-tar", "--output", "dist"]);
        let (dir, file) = resolve_output(&argv, &tar, target).unwrap();
        assert_eq!(dir, Path::new("dist"));
        assert_eq!(file, None);

        // An archive name is split into its parent and itself.
        let argv = args(&["cargo-tar", "--output", "dist/thing.tar"]);
        let (dir, file) = resolve_output(&argv, &tar, target).unwrap();
        assert_eq!(dir, Path::new("dist"));
        assert_eq!(file.unwrap(), Path::new("dist/thing.tar"));

        // A bare archive name has no parent, so it lands in the current directory.
        let argv = args(&["cargo-tar", "--output", "thing.tar"]);
        let (dir, file) = resolve_output(&argv, &tar, target).unwrap();
        assert_eq!(dir, Path::new("."));
        assert_eq!(file.unwrap(), Path::new("thing.tar"));

        // One file cannot hold several formats.
        let argv = args(&["cargo-tar", "--output", "thing.tar"]);
        let message = resolve_output(&argv, &[Format::Tar, Format::TarGz], target)
            .unwrap_err()
            .to_string();
        assert!(message.contains("names a single file"), "{message}");
    }
}
