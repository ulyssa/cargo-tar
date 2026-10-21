//! Parse `cargo metadata` output and run `cargo build` as needed.
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::cli::TarballArgs;

#[derive(Debug, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub targets: Vec<Target>,
    /// The raw `[package.metadata]` table, if the manifest has one.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Package {
    /// The directory containing this package's manifest.
    pub fn root(&self) -> &Path {
        self.manifest_path.parent().unwrap_or(Path::new("."))
    }

    /// Names of the package's `[[bin]]` targets, in manifest order.
    pub fn bin_names(&self) -> Vec<&str> {
        self.targets
            .iter()
            .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
            .map(|target| target.name.as_str())
            .collect()
    }

    /// The raw `[package.metadata.tar]` table, if present.
    pub fn tar_metadata(&self) -> Option<&serde_json::Value> {
        self.metadata.get("tar")
    }

    /// Produce the binaries to include, building them unless `--no-build` was given.
    pub fn executables(&self, args: &TarballArgs, target_dir: &Path) -> Result<Vec<PathBuf>> {
        let requested = self.requested_bins(args)?;

        if args.no_build {
            self.prebuilt(args, &requested, target_dir)
        } else {
            self.build(args, &requested)
        }
    }

    /// The `[[bin]]` targets to include, honoring `--bin`.
    fn requested_bins<'a>(&'a self, args: &'a TarballArgs) -> Result<Vec<&'a str>> {
        let available = self.bin_names();

        if args.bin.is_empty() {
            // When nothing is specified, include everything:
            return Ok(available);
        }

        args.bin
            .iter()
            .map(|requested| {
                available.iter().copied().find(|name| *name == requested).ok_or_else(|| {
                    anyhow!(
                        "package `{}` has no binary named `{requested}` (found: {})",
                        self.name,
                        if available.is_empty() {
                            "none".to_string()
                        } else {
                            available.join(", ")
                        }
                    )
                })
            })
            .collect()
    }

    /// Build the package and collect the executables cargo reports.
    fn build(&self, args: &TarballArgs, requested: &[&str]) -> Result<Vec<PathBuf>> {
        let cargo = cargo_bin();

        let mut command = Command::new(&cargo);
        command.arg("build");
        command.args(["--message-format", "json-render-diagnostics"]);
        command.args(["--profile", args.profile.as_str()]);
        command.args(["--package", &self.name]);

        if let Some(manifest_path) = &args.manifest_path {
            command.arg("--manifest-path").arg(manifest_path);
        }

        if let Some(target) = &args.target {
            command.args(["--target", target]);
        }

        if let Some(target_dir) = &args.target_dir {
            command.arg("--target-dir").arg(target_dir);
        }

        for features in &args.features {
            command.args(["--features", features]);
        }

        if args.no_default_features {
            command.arg("--no-default-features");
        }

        if args.all_features {
            command.arg("--all-features");
        }

        for bin in requested {
            command.args(["--bin", bin]);
        }

        if args.quiet {
            command.arg("--quiet");
        }

        command.stdout(Stdio::piped());

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to run `{}` build", cargo.to_string_lossy()))?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut executables = Vec::new();

        for line in BufReader::new(stdout).lines() {
            let line = line.context("failed to read `cargo build` output")?;

            let Ok(message) = serde_json::from_str::<Message>(&line) else {
                // Skip unparsable lines from build scripts, other packages, etc.
                continue;
            };

            if message.reason == "compiler-artifact"
                && message.package_id == self.id
                && let Some(executable) = message.executable
            {
                executables.push(executable);
            }
        }

        let status = child.wait().context("failed to wait for `cargo build`")?;

        if !status.success() {
            bail!("`cargo build` failed");
        }

        if executables.is_empty() {
            bail!(
                "`cargo build` produced no executables for package `{}`; \
                 it has no [[bin]] target to archive",
                self.name
            );
        }

        Ok(executables)
    }

    /// Locate executables from an earlier build, for `--no-build`.
    fn prebuilt(
        &self,
        args: &TarballArgs,
        requested: &[&str],
        target_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        if requested.is_empty() {
            bail!("no [[bin]] targets to archive");
        }

        let mut directory = target_dir.to_path_buf();

        if let Some(target) = &args.target {
            directory.push(target);
        }

        directory.push(profile_dir(&args.profile));

        let mut executables = Vec::new();

        for name in requested {
            let path = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));

            if !path.is_file() {
                bail!(
                    "`--no-build` was given but `{}` does not exist; build it first",
                    path.display()
                );
            }

            executables.push(path);
        }

        Ok(executables)
    }
}

/// Prioritize the `cargo` binary specified in environment over `PATH`.
fn cargo_bin() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// One line of `cargo build --message-format=json` output that we care about.
#[derive(Debug, Deserialize)]
struct Message {
    reason: String,
    #[serde(default)]
    package_id: String,
    #[serde(default)]
    executable: Option<PathBuf>,
}

/// The subdirectory of the target directory that a profile's output lands in.
fn profile_dir(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "bench" => "release",
        other => other,
    }
}

#[derive(Debug, Deserialize)]
struct MetadataOutput {
    packages: Vec<Package>,
    target_directory: PathBuf,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
}

#[derive(Debug)]
pub struct Project {
    pub package: Package,
    pub target_directory: PathBuf,
}

impl Project {
    /// Run `cargo metadata` and pick out the package to archive.
    pub fn load(args: &TarballArgs) -> Result<Self> {
        let cargo = cargo_bin();

        let mut command = Command::new(&cargo);
        command.args(["metadata", "--format-version", "1", "--no-deps"]);

        if let Some(manifest_path) = &args.manifest_path {
            command.arg("--manifest-path").arg(manifest_path);
        }

        let output = command
            .output()
            .with_context(|| format!("failed to run `{}` metadata", cargo.to_string_lossy()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("`cargo metadata` failed:\n{}", stderr.trim_end());
        }

        let metadata: MetadataOutput = serde_json::from_slice(&output.stdout)
            .context("failed to parse `cargo metadata` output")?;

        let target_directory = args.target_dir.clone().unwrap_or(metadata.target_directory);

        let package = select_package(
            metadata.packages,
            &metadata.workspace_members,
            args.package.as_deref(),
        )?;

        Ok(Self { package, target_directory })
    }
}

/// Grab the requested package from the workspace, or just the one if there's only one package.
fn select_package(
    packages: Vec<Package>,
    workspace_members: &[String],
    requested: Option<&str>,
) -> Result<Package> {
    let mut members: Vec<Package> = packages
        .into_iter()
        .filter(|package| workspace_members.contains(&package.id))
        .collect();

    if let Some(requested) = requested {
        // Accept both `name` and cargo's `name@version` spec form:
        let name = requested.split('@').next().unwrap_or(requested);

        let position =
            members.iter().position(|package| package.name == name).ok_or_else(|| {
                anyhow!("no package named `{name}` in this workspace (found: {})", names(&members))
            })?;

        return Ok(members.swap_remove(position));
    }

    match members.len() {
        0 => bail!("`cargo metadata` reported no workspace packages"),
        1 => Ok(members.remove(0)),
        _ => {
            bail!(
                "Please specify with `--package` which workspace package you want: {}",
                names(&members)
            )
        },
    }
}

fn names(packages: &[Package]) -> String {
    let mut names: Vec<&str> = packages.iter().map(|package| package.name.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR};

    use clap::Parser;

    fn args(argv: &[&str]) -> TarballArgs {
        TarballArgs::parse_from(argv)
    }

    fn package(name: &str) -> Package {
        Package {
            id: format!("path+file:///{name}#1.0.0"),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from(format!("/{name}/Cargo.toml")),
            targets: vec![
                Target {
                    name: name.to_string(),
                    kind: vec!["bin".to_string()],
                },
                Target {
                    name: format!("{name}-lib"),
                    kind: vec!["lib".to_string()],
                },
            ],
            metadata: serde_json::Value::Null,
        }
    }

    fn multibin() -> Package {
        let mut package = package("tool");
        package.targets.push(Target {
            name: "helper".to_string(),
            kind: vec!["bin".to_string()],
        });
        package
    }

    #[test]
    fn test_all_bins_are_included_by_default() {
        let args = args(&["cargo-tar"]);
        let package = multibin();

        assert_eq!(package.requested_bins(&args).unwrap(), ["tool", "helper"]);
    }

    #[test]
    fn test_bin_flag_restricts_and_orders_selection() {
        let args = args(&["cargo-tar", "--bin", "helper"]);
        let package = multibin();
        assert_eq!(package.requested_bins(&args).unwrap(), ["helper"]);
    }

    #[test]
    fn test_unknown_bin_lists_what_exists() {
        let args = args(&["cargo-tar", "--bin", "nope"]);
        let package = multibin();
        let message = package.requested_bins(&args).unwrap_err().to_string();

        assert!(message.contains("no binary named `nope`"), "{message}");
        assert!(message.contains("tool, helper"), "{message}");
    }

    #[test]
    fn test_prebuilt_lookup_reports_the_path_it_wanted() {
        let args = args(&[
            "cargo-tar",
            "--no-build",
            "--target",
            "x86_64-unknown-linux-gnu",
        ]);
        let package = multibin();
        let target_dir = PathBuf::from(MAIN_SEPARATOR_STR).join("nonexistent");
        let message = package.prebuilt(&args, &["tool"], &target_dir).unwrap_err().to_string();

        let expected =
            format!("x86_64-unknown-linux-gnu{MAIN_SEPARATOR}release{MAIN_SEPARATOR}tool");
        assert!(message.contains(&expected), "{message}");
    }

    #[test]
    fn test_profile_dirs_follow_cargo() {
        assert_eq!(profile_dir("dev"), "debug");
        assert_eq!(profile_dir("test"), "debug");
        assert_eq!(profile_dir("bench"), "release");
        assert_eq!(profile_dir("release"), "release");
        assert_eq!(profile_dir("fast"), "fast");
    }

    fn members(packages: &[Package]) -> Vec<String> {
        packages.iter().map(|package| package.id.clone()).collect()
    }

    #[test]
    fn test_single_package_needs_no_selection() {
        let packages = vec![package("solo")];
        let ids = members(&packages);

        let selected = select_package(packages, &ids, None).unwrap();
        assert_eq!(selected.name, "solo");
    }

    #[test]
    fn test_ambiguous_workspace_lists_candidates() {
        let packages = vec![package("beta"), package("alpha")];
        let ids = members(&packages);

        let message = select_package(packages, &ids, None).unwrap_err().to_string();
        assert!(message.contains("--package"), "{message}");
        assert!(message.contains("alpha, beta"), "{message}");
    }

    #[test]
    fn test_selects_requested_package_by_name_or_spec() {
        for requested in ["beta", "beta@1.0.0"] {
            let packages = vec![package("alpha"), package("beta")];
            let ids = members(&packages);

            let selected = select_package(packages, &ids, Some(requested)).unwrap();
            assert_eq!(selected.name, "beta", "{requested}");
        }
    }

    #[test]
    fn test_unknown_package_is_an_error() {
        let packages = vec![package("alpha")];
        let ids = members(&packages);

        let message = select_package(packages, &ids, Some("nope")).unwrap_err().to_string();
        assert!(message.contains("no package named `nope`"), "{message}");
    }

    #[test]
    fn test_bin_names_skip_other_target_kinds() {
        assert_eq!(package("tool").bin_names(), ["tool"]);
    }
}
