mod directory;
pub mod elevation;
mod operations;
mod search;
mod session;
mod settings;
pub mod volume;
mod watch;

pub use directory::Directory;
pub use operations::{
    MAX_TRANSFER_ITEMS, Operation, OperationResult, Operations, TransferControl, TransferProgress,
    TransferRequest, TransferState,
};
pub use search::{Filters, SearchIndex, SearchQuery, SearchReader};
pub use session::{Clipboard, Completion, History, Session};
pub use settings::{
    Appearance, CaptionShape, FolderKey, FolderStyle, MenuAction, MenuEntry, Preset, Rgb, Settings,
    default_menu,
};

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT};

use crate::file_identity::{FileIdentity, identity, information, open_item};
use crate::{MAX_PAGE_SIZE, PageRequest};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> Result<()> {
        ensure!(!self.is_cancelled(), "Request cancelled");
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemId {
    path: PathBuf,
    identity: FileIdentity,
}

impl ItemId {
    fn from_file(path: PathBuf, file: &File) -> Result<Self> {
        Ok(Self {
            path,
            identity: identity(file)?,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verify(&self) -> Result<File> {
        let file =
            open_item(&self.path).with_context(|| format!("Open {}", self.path.display()))?;
        ensure!(
            identity(&file)? == self.identity,
            "The item was replaced. Refresh the folder and select it again."
        );
        Ok(file)
    }

    pub fn at(path: PathBuf) -> Result<Self> {
        let file = open_item(&path)?;
        Self::from_file(path, &file)
    }

    /// The volume serial number and file index that identify the item.
    pub fn identity(&self) -> (u32, u64) {
        (self.identity.volume, self.identity.index)
    }

    /// Rebuilds an identity an interface received earlier. Operations still call
    /// [`ItemId::verify`], so a stale or altered identity cannot reach another file.
    pub fn from_parts(path: PathBuf, volume: u32, index: u64) -> Self {
        Self {
            path,
            identity: FileIdentity { volume, index },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub directory: bool,
    /// File length in bytes; `None` for folders.
    pub size: Option<u64>,
    /// Last write time in Unix seconds.
    pub modified: Option<i64>,
    /// Win32 file attributes, when the source records them.
    pub attributes: Option<u32>,
    pub id: Option<ItemId>,
}

impl Item {
    fn listed(
        path: PathBuf,
        directory: bool,
        size: Option<u64>,
        modified: i64,
        attributes: u32,
    ) -> Self {
        let id = ItemId::at(path.clone()).ok();
        Self {
            path,
            directory,
            size,
            modified: Some(modified),
            attributes: Some(attributes),
            id,
        }
    }

    pub fn name(&self) -> &OsStr {
        self.path.file_name().unwrap_or(self.path.as_os_str())
    }
}

pub struct ItemPage {
    pub offset: u32,
    pub items: Vec<Item>,
    pub has_more: bool,
}

pub const PAGE_SIZE: u32 = MAX_PAGE_SIZE;

pub fn page(offset: u32) -> PageRequest {
    PageRequest {
        offset,
        limit: PAGE_SIZE,
    }
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .context("USERPROFILE is unset")
}

pub fn data_dir() -> Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|path| PathBuf::from(path).join("Firewol"))
        .context("LOCALAPPDATA is unset")
}

pub fn resolve_directory(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path).with_context(|| format!("Open {}", path.display()))?;
    ensure!(path.is_dir(), "The address must be a folder");
    Ok(path)
}

pub fn display_path(path: &Path) -> OsString {
    let units: Vec<_> = path.as_os_str().encode_wide().collect();
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(_) => OsString::from_wide(&units[4..]),
            Prefix::VerbatimUNC(..) => OsString::from_wide(&[&[92, 92][..], &units[8..]].concat()),
            _ => path.as_os_str().to_owned(),
        },
        _ => path.as_os_str().to_owned(),
    }
}

fn attributes(file: &File) -> Result<(bool, bool, u64, i64)> {
    let info = information(file)?;
    let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    let modified = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
        | u64::from(info.ftLastWriteTime.dwLowDateTime);
    Ok((
        info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
        info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0,
        size,
        (modified / 10_000_000) as i64 - 11_644_473_600,
    ))
}

/// Converts a FILETIME (100 ns ticks since 1601) to Unix seconds.
fn unix_seconds(filetime: u64) -> i64 {
    (filetime / 10_000_000) as i64 - 11_644_473_600
}

fn encode(value: &OsStr) -> Vec<u8> {
    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}

fn decode(bytes: &[u8]) -> OsString {
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_le_bytes)
        .collect();
    OsString::from_wide(&units)
}
