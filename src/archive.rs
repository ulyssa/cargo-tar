//! Logic for generating the output tar stream.
//!
//! Headers are filled in field by field rather than copied from the filesystem,
//! so that the metadata in the archive is independent of the actual CI environment,
//! and should therefore be easily reproducible.
use std::fs::File;
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tar::{Builder, EntryType, Header};

use crate::assets::Item;
use crate::config::Config;

/// Write every item into a tar stream, under the configured root directory.
pub fn write<W: Write>(writer: W, items: &[Item], config: &Config) -> Result<W> {
    let mut builder = Builder::new(writer);
    let mtime = config.source_date.unwrap_or_else(now);

    let mut template = Header::new_gnu();
    template.set_uid(0);
    template.set_gid(0);
    template
        .set_username(&config.user)
        .with_context(|| format!("invalid user name `{}`", config.user))?;
    template
        .set_groupname(&config.group)
        .with_context(|| format!("invalid group name `{}`", config.group))?;

    let directory = || {
        let mut header = template.clone();
        header.set_entry_type(EntryType::Directory);
        header.set_size(0);
        header.set_mode(config.dir_mode);
        header.set_mtime(mtime);
        header
    };

    // Root directory entry for everything else:
    builder
        .append_data(&mut directory(), &config.root_dir, io::empty())
        .with_context(|| format!("failed to write the `{}` directory entry", config.root_dir))?;

    for item in items {
        let path = format!("{}/{}", config.root_dir, item.path());

        match item {
            Item::Dir { .. } => {
                builder
                    .append_data(&mut directory(), &path, io::empty())
                    .with_context(|| format!("failed to write directory entry `{path}`"))?;
            },
            Item::File { source, mode, .. } => {
                let mut file = File::open(source)
                    .with_context(|| format!("failed to open `{}`", source.display()))?;

                let metadata = file
                    .metadata()
                    .with_context(|| format!("failed to inspect `{}`", source.display()))?;

                let mut header = template.clone();
                header.set_entry_type(EntryType::Regular);
                header.set_size(metadata.len());
                header.set_mode(*mode);
                header.set_mtime(mtime);

                builder.append_data(&mut header, &path, &mut file).with_context(|| {
                    format!("failed to add `{}` to the archive", source.display())
                })?;
            },
        }
    }

    builder.into_inner().context("failed to finish the tar stream")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}
