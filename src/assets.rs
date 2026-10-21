//! Turning asset declarations into the exact list of archive entries.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{Config, Dest, Mode};

pub const DEFAULT_DIR_MODE: u32 = 0o755;
pub const DEFAULT_BIN_MODE: u32 = 0o755;
pub const DEFAULT_FILE_MODE: u32 = 0o644;

/// An entry in the archive, with a path relative to the root directory of the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Dir { path: String },
    File { path: String, source: PathBuf, mode: u32 },
}

impl Item {
    pub fn path(&self) -> &str {
        match self {
            Item::Dir { path } | Item::File { path, .. } => path,
        }
    }
}

impl Config {
    /// Resolve the configured assets and binaries into a sorted list of entries.
    pub fn assets(&self, package_root: &Path, executables: &[PathBuf]) -> Result<Vec<Item>> {
        let mut files: BTreeMap<String, Item> = BTreeMap::new();

        for asset in &self.assets {
            let dest =
                Dest::parse(&asset.dest).with_context(|| format!("asset `{}`", asset.source))?;

            let mode = asset
                .mode
                .as_ref()
                .map(Mode::bits)
                .transpose()
                .with_context(|| format!("asset `{}`", asset.source))?;

            for source in expand(package_root, &asset.source, &dest)? {
                let name = file_name(&source)?;
                let path = dest.resolve(&name);
                let mode = mode.unwrap_or_else(|| default_mode(&source));

                if let Some(Item::File { source: existing, .. }) = files.get(&path) {
                    bail!(
                        "two assets both land at `{path}` in the archive: `{}` and `{}`",
                        existing.display(),
                        source.display()
                    );
                }

                files.insert(path.clone(), Item::File { path, source, mode });
            }
        }

        for executable in executables {
            let name = file_name(executable)?;
            let path = match self.bin_dir.is_empty() {
                true => name,
                false => format!("{}/{name}", self.bin_dir),
            };

            // Don't overwrite any explicit paths from the `Config.toml`:
            files.entry(path.clone()).or_insert(Item::File {
                path,
                source: executable.clone(),
                mode: DEFAULT_BIN_MODE,
            });
        }

        if files.is_empty() {
            bail!("nothing to archive: no binaries and no assets");
        }

        Ok(with_directories(files))
    }
}

/// Add an entry for every directory an entry lives in.
///
/// Prepare items with the path name and order that tar readers expect.
fn with_directories(files: BTreeMap<String, Item>) -> Vec<Item> {
    let mut items: BTreeMap<String, Item> = BTreeMap::new();

    for (path, item) in files {
        // Every `/` in the path marks one directory that has to exist.
        for (end, _) in path.match_indices('/') {
            let prefix = &path[..end];

            items
                .entry(prefix.to_string())
                .or_insert_with(|| Item::Dir { path: prefix.to_string() });
        }

        items.insert(path, item);
    }

    items.into_values().collect()
}

/// Expand one source path into the files it names.
fn expand(package_root: &Path, source: &str, dest: &Dest) -> Result<Vec<PathBuf>> {
    let pattern = package_root.join(source);

    if !is_glob(source) {
        if pattern.is_dir() {
            bail!(
                "asset source `{source}` is a directory; \
                 use a glob such as `{source}/*` to include its files"
            );
        }

        if !pattern.exists() {
            bail!("asset source `{source}` does not exist (looked in {})", package_root.display());
        }

        return Ok(vec![pattern]);
    }

    if !dest.is_dir {
        bail!(
            "asset source `{source}` is a glob, so its dest must end in `/` \
             to name a directory"
        );
    }

    let pattern = pattern
        .to_str()
        .ok_or_else(|| anyhow!("asset source `{source}` is not valid UTF-8"))?;

    let mut matches = Vec::new();

    for entry in glob::glob(pattern).with_context(|| format!("invalid glob `{source}`"))? {
        let entry = entry.with_context(|| format!("failed to read a match for `{source}`"))?;

        // Directories matched by a glob are skipped; their files can be matched
        // with a deeper pattern.
        if entry.is_file() {
            matches.push(entry);
        }
    }

    if matches.is_empty() {
        bail!("asset source `{source}` matched no files");
    }

    matches.sort();

    Ok(matches)
}

fn is_glob(source: &str) -> bool {
    source.contains(['*', '?', '['])
}

fn file_name(path: &Path) -> Result<String> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => Ok(name.to_string()),
        None => bail!("`{}` has no usable file name", path.display()),
    }
}

/// Mirror the source's executable bit when the manifest gives no mode.
fn default_mode(source: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Ok(metadata) = source.metadata()
            && metadata.permissions().mode() & 0o111 != 0
        {
            return DEFAULT_BIN_MODE;
        }
    }

    let _ = source;

    DEFAULT_FILE_MODE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(items: &[Item]) -> Vec<&str> {
        items
            .iter()
            .filter(|item| matches!(item, Item::Dir { .. }))
            .map(Item::path)
            .collect()
    }

    fn file(path: &str) -> (String, Item) {
        (
            path.to_string(),
            Item::File {
                path: path.to_string(),
                source: PathBuf::from(path),
                mode: 0o644,
            },
        )
    }

    #[test]
    fn test_directories_are_synthesized_once_and_precede_their_files() {
        let files = BTreeMap::from([
            file("share/man/man1/iamb.1"),
            file("share/man/man5/iamb.5"),
            file("iamb"),
        ]);

        let items = with_directories(files);
        let paths: Vec<&str> = items.iter().map(Item::path).collect();

        assert_eq!(
            paths,
            [
                "iamb",
                "share",
                "share/man",
                "share/man/man1",
                "share/man/man1/iamb.1",
                "share/man/man5",
                "share/man/man5/iamb.5",
            ]
        );
        assert_eq!(dirs(&items), ["share", "share/man", "share/man/man1", "share/man/man5"]);
    }

    #[test]
    fn test_sibling_names_still_nest_correctly() {
        // `-` sorts before `/`, so a directory's siblings can interleave; each
        // directory must still come before its own children.
        let items = with_directories(BTreeMap::from([file("share-extra/a"), file("share/b")]));
        let paths: Vec<&str> = items.iter().map(Item::path).collect();

        assert_eq!(paths, ["share", "share-extra", "share-extra/a", "share/b"]);
    }

    #[test]
    fn test_globs_require_a_directory_dest() {
        let dest = Dest::parse("themes/dark.toml").unwrap();
        let message = expand(Path::new("."), "themes/*.toml", &dest).unwrap_err().to_string();

        assert!(message.contains("must end in `/`"), "{message}");
    }

    #[test]
    fn test_missing_sources_and_empty_globs_are_errors() {
        let dir = Dest::parse("share/").unwrap();
        let file = Dest::parse("share/thing").unwrap();

        let missing = expand(Path::new("/nonexistent"), "thing", &file).unwrap_err().to_string();
        assert!(missing.contains("does not exist"), "{missing}");

        let empty = expand(Path::new("/nonexistent"), "*.toml", &dir).unwrap_err().to_string();
        assert!(empty.contains("matched no files"), "{empty}");
    }

    #[test]
    fn test_glob_detection() {
        assert!(is_glob("themes/*.toml"));
        assert!(is_glob("docs/iamb.?"));
        assert!(is_glob("docs/[ab].1"));
        assert!(!is_glob("docs/iamb.1"));
    }
}
