use std::fs::{self, File};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use firewol_core::{FixtureInfo, fixture_root, index_workload, ntfs, prepare, recycle};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    if let ["ntfs-helper", drive, volume, index] = args.as_slice() {
        return ntfs::find_reference(drive.parse()?, volume.parse()?, index.parse()?);
    }
    let root = fixture_root()?;
    match args.as_slice() {
        ["prepare", entries, index_rows] => {
            prepare(
                &root,
                FixtureInfo {
                    entries: entries.parse()?,
                    index_rows: index_rows.parse()?,
                },
            )?;
            println!("Prepared {}", root.display());
        }
        ["index-load", count] => {
            index_workload(&root, count.parse()?)?;
            println!("Built the temporary FTS5 workload");
        }
        ["copy-load"] => {
            copy_workload(&root)?;
            println!("Copied the 256 MiB synthetic file");
        }
        ["recycle-probe"] => {
            recycle::probe(&root)?;
            println!("Recycle, conflict handling and exact NTFS file restoration verified");
        }
        ["ntfs-probe"] => {
            ntfs::probe(&root)?;
            println!("Read-only elevated MFT probe found the exact fixture file reference");
        }
        _ => bail!(
            "Usage: firewol-m0 prepare FILES ROWS | index-load ROWS | copy-load | recycle-probe | ntfs-probe"
        ),
    }
    Ok(())
}

fn copy_workload(root: &Path) -> Result<()> {
    let stage = tempfile::tempdir_in(root)?;
    let source = stage.path().join("source.bin");
    let destination = stage.path().join("copy.bin");
    let length = 256 * 1024 * 1024;
    File::create_new(&source)?.set_len(length)?;
    let copied = fs::copy(source, destination).context("Copy synthetic transfer file")?;
    ensure!(copied == length, "Synthetic transfer size did not match");
    Ok(())
}
