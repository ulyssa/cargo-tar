mod archive;
mod assets;
mod cargo;
mod cli;
mod compress;
mod config;

use std::process::ExitCode;

use anyhow::{Context, Result};
use humansize::{DECIMAL, format_size};

use crate::cargo::Project;
use crate::cli::TarballArgs;
use crate::config::{Config, RawConfig};

fn main() -> ExitCode {
    let args = TarballArgs::from_env();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        },
    }
}

fn run(args: &TarballArgs) -> Result<()> {
    let project = Project::load(args)?;
    let package = &project.package;

    let raw = RawConfig::from_package(package)?;
    let config = Config::build(args, package, raw, &project.target_directory)?;

    let executables = package.executables(args, &project.target_directory)?;
    let items = config.assets(package.root(), &executables)?;

    std::fs::create_dir_all(&config.output_dir)
        .with_context(|| format!("failed to create `{}`", config.output_dir.display()))?;

    // The tar stream is built once, then compressed once per requested format,
    // so a multi-format run reads every asset only once.
    let mut tar = tempfile::Builder::new()
        .prefix(".cargo-tar-")
        .suffix(".tar")
        .tempfile_in(&config.output_dir)
        .with_context(|| {
            format!("failed to create a temporary file in `{}`", config.output_dir.display())
        })?;

    if args.verbose {
        for item in &items {
            if let assets::Item::File { path, source, mode } = item {
                println!("  {:o} {}/{path} <- {}", mode, config.root_dir, source.display());
            }
        }
    }

    let file = archive::write(tar.as_file_mut(), &items, &config)?;
    file.sync_all().context("failed to flush the tar stream")?;

    for output in config.outputs() {
        let size = compress::write_archive(
            tar.path(),
            output.format,
            config.compression_level,
            &output.path,
        )?;

        if !args.quiet {
            println!("{} ({})", output.path.display(), format_size(size, DECIMAL));
        }
    }

    Ok(())
}
