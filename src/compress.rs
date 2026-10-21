//! Logic for compressing the output tarball.
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Archive formats `cargo-tar` can output.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "String")]
pub enum Format {
    Tar,
    #[default]
    TarGz,
    TarBz2,
}

impl Format {
    /// The file extension used for this format minus a leading dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Tar => "tar",
            Format::TarGz => "tar.gz",
            Format::TarBz2 => "tar.bz2",
        }
    }
}

impl FromStr for Format {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let format = match name {
            "tar" => Format::Tar,
            "tar.gz" | "tgz" | "gz" | "gzip" => Format::TarGz,
            "tar.bz2" | "tbz2" | "tbz" | "bz2" | "bzip2" => Format::TarBz2,
            "tar.xz" | "txz" | "xz" | "lzma" => {
                // XXX: implement at some point:
                bail!("archive format `{name}` is not currently supported");
            },
            "tar.zst" | "tzst" | "zst" | "zstd" => {
                // XXX: implement at some point:
                bail!("archive format `{name}` is not currently supported");
            },
            _ => {
                bail!("unknown archive format `{name}` (expected one of: tar, tar.gz, tar.bz2)");
            },
        };

        Ok(format)
    }
}

impl TryFrom<String> for Format {
    type Error = anyhow::Error;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        name.parse()
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

/// Copy a tar file into its final, possibly compressed, location.
pub fn write_archive(tar: &Path, format: Format, level: u32, output: &Path) -> Result<u64> {
    let mut source =
        File::open(tar).with_context(|| format!("failed to read `{}`", tar.display()))?;

    let file =
        File::create(output).with_context(|| format!("failed to create `{}`", output.display()))?;

    let mut writer = Encoder::open(file, format, level)?;

    io::copy(&mut source, &mut writer)
        .with_context(|| format!("failed to write `{}`", output.display()))?;

    // Finish explicitly, so we can provide a meaningful failure message:
    let file = writer
        .finish()
        .with_context(|| format!("failed to finish `{}`", output.display()))?;

    file.sync_all()
        .with_context(|| format!("failed to flush `{}`", output.display()))?;

    let size = file
        .metadata()
        .with_context(|| format!("failed to inspect `{}`", output.display()))?
        .len();

    Ok(size)
}

/// A writer that compresses into `file`, and can report failures when finished.
///
/// Exactly one of these exists per archive, so the size of the largest variant
/// does not matter.
#[allow(clippy::large_enum_variant)]
enum Encoder {
    Plain(File),
    Gzip(flate2::write::GzEncoder<File>),
    Bzip2(bzip2::write::BzEncoder<File>),
}

impl Encoder {
    fn open(file: File, format: Format, level: u32) -> Result<Self> {
        let encoder = match format {
            Format::Tar => Encoder::Plain(file),

            Format::TarGz => {
                Encoder::Gzip(flate2::write::GzEncoder::new(file, flate2::Compression::new(level)))
            },

            Format::TarBz2 => {
                Encoder::Bzip2(bzip2::write::BzEncoder::new(file, bzip2::Compression::new(level)))
            },
        };

        Ok(encoder)
    }
}

impl Encoder {
    fn finish(self) -> io::Result<File> {
        match self {
            Encoder::Plain(mut file) => {
                file.flush()?;
                Ok(file)
            },
            Encoder::Gzip(encoder) => encoder.finish(),
            Encoder::Bzip2(encoder) => encoder.finish(),
        }
    }
}

impl Write for Encoder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Encoder::Plain(file) => file.write(buf),
            Encoder::Gzip(encoder) => encoder.write(buf),
            Encoder::Bzip2(encoder) => encoder.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Encoder::Plain(file) => file.flush(),
            Encoder::Gzip(encoder) => encoder.flush(),
            Encoder::Bzip2(encoder) => encoder.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_spellings_round_trip() {
        for (spelling, expected) in [
            ("tar", Format::Tar),
            ("tgz", Format::TarGz),
            ("tar.gz", Format::TarGz),
            ("bzip2", Format::TarBz2),
            ("tar.bz2", Format::TarBz2),
        ] {
            assert_eq!(Format::from_str(spelling).unwrap(), expected, "{spelling}");
        }

        for format in [Format::Tar, Format::TarGz, Format::TarBz2] {
            assert_eq!(Format::from_str(format.extension()).unwrap(), format);
        }
    }

    #[test]
    fn test_unsupported_formats_explain_themselves() {
        for spelling in ["tar.xz", "txz", "tar.zst", "zstd"] {
            let message = Format::from_str(spelling).unwrap_err().to_string();
            assert!(message.contains("not currently supported"), "{spelling}: {message}");
        }

        let message = Format::from_str("tar.lz4").unwrap_err().to_string();
        assert!(message.contains("unknown archive format"), "{message}");
    }
}
