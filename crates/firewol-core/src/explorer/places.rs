//! Breadcrumb and status-bar helpers: sibling folders for a chevron and drive free space.

use std::cmp::Ordering;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use windows::Win32::Globalization::{CompareStringEx, NORM_IGNORECASE, SORT_DIGITSASNUMBERS};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::core::PCWSTR;

/// Windows Explorer's name order: case-insensitive, with digit runs compared as numbers.
pub fn natural_order(left: &str, right: &str) -> Ordering {
    let left_wide: Vec<u16> = left.encode_utf16().collect();
    let right_wide: Vec<u16> = right.encode_utf16().collect();
    let result = unsafe {
        CompareStringEx(
            PCWSTR::null(),
            NORM_IGNORECASE | SORT_DIGITSASNUMBERS,
            &left_wide,
            &right_wide,
            None,
            None,
            None,
        )
    };
    match result.0 {
        1 => Ordering::Less,
        2 => Ordering::Equal,
        3 => Ordering::Greater,
        _ => left.cmp(right),
    }
}

/// The folders directly inside `path` in natural name order, at most `limit` of them, and whether
/// more exist. Links and junctions are listed like folders but never followed.
pub fn subfolders(path: &Path, limit: usize) -> Result<(Vec<PathBuf>, bool)> {
    let entries = fs::read_dir(path).with_context(|| format!("List {}", path.display()))?;
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                || (entry.file_type().is_ok_and(|kind| kind.is_symlink())
                    && fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir()))
        })
        .map(|entry| entry.path())
        .collect();
    folders.sort_by(|left, right| {
        let name = |path: &PathBuf| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        natural_order(&name(left), &name(right))
    });
    let more = folders.len() > limit;
    folders.truncate(limit);
    Ok((folders, more))
}

/// Free bytes available to the user and the total size of the volume holding `path`.
pub fn free_space(path: &Path) -> Result<(u64, u64)> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total) = (0u64, 0u64);
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut free),
            Some(&mut total),
            None,
        )
    }
    .with_context(|| format!("Read the free space of {}", path.display()))?;
    Ok((free, total))
}
