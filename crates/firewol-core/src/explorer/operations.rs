use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Foundation::{E_ABORT, S_OK};
use windows::Win32::Storage::FileSystem::{
    CopyFileW, FILE_ATTRIBUTE_REPARSE_POINT, MOVE_FILE_FLAGS, MoveFileExW,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{
    COPYENGINE_S_COLLISIONRESOLVED, COPYENGINE_S_DONT_PROCESS_CHILDREN, FOF_NO_CONNECTED_ELEMENTS,
    FOF_NOERRORUI, FOF_RENAMEONCOLLISION, FOF_SILENT, FOFX_EARLYFAILURE, FOFX_NOCOPYHOOKS,
    FOFX_PRESERVEFILEEXTENSIONS, FileOperation, IFileOperation, IFileOperationProgressSink,
    IFileOperationProgressSink_Impl, IShellItem,
};
use windows::core::{HRESULT, PCWSTR, Ref, implement};

use super::{ItemId, attributes, display_path};
use crate::file_identity::information;
use crate::recycle::{
    Apartment, RecycledItem, filesystem_path, recycle, require_vacant, shell_item,
};

#[path = "transfer.rs"]
mod transfer;
pub use transfer::{
    MAX_TRANSFER_ITEMS, TransferControl, TransferProgress, TransferRequest, TransferState,
};

/// `Recycle` moves 1 to `MAX_TRANSFER_ITEMS` items to the Recycle Bin; one undo restores them all.
pub enum Operation {
    Copy { item: ItemId, into: ItemId },
    Move { item: ItemId, into: ItemId },
    Rename { item: ItemId, name: OsString },
    Recycle { items: Vec<ItemId> },
}

pub struct OperationResult {
    pub message: String,
    pub transferred: Vec<ItemId>,
}

pub struct Operations {
    history: VecDeque<Undo>,
    _sta: Apartment,
}

enum Undo {
    Transfer(TransferUndo),
    /// Items recycled together, in recycle order; undo restores them newest first.
    Recycle(Vec<(RecycledItem, ItemId)>),
}

enum TransferUndo {
    Move {
        item: ItemId,
        parent: ItemId,
        name: OsString,
    },
    CrossVolumeMove {
        item: ItemId,
        parent: ItemId,
        name: OsString,
    },
    Copy {
        item: ItemId,
        stamp: Stamp,
    },
    FolderCopy,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: u64,
    directory: bool,
}

impl Stamp {
    fn read(file: &File) -> Result<Self> {
        let info = information(file)?;
        let (directory, reparse, length, _) = attributes(file)?;
        ensure!(
            !reparse,
            "File operations on links and junctions are unavailable"
        );
        let modified = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime);
        Ok(Self {
            length,
            modified,
            directory,
        })
    }
}

impl Operations {
    pub fn open() -> Result<Self> {
        Ok(Self {
            history: VecDeque::new(),
            _sta: Apartment::sta()?,
        })
    }

    pub fn submit_operation(&mut self, operation: Operation) -> Result<OperationResult> {
        let (record, message) = match operation {
            Operation::Copy { item, into } => {
                let (undo, message) =
                    Self::transfer_one(&item, &into, false, &TransferControl::default())?;
                (Undo::Transfer(undo), message)
            }
            Operation::Move { item, into } => {
                let (undo, message) =
                    Self::transfer_one(&item, &into, true, &TransferControl::default())?;
                (Undo::Transfer(undo), message)
            }
            Operation::Rename { item, name } => {
                validate_name(&name)?;
                let old_name = name_of(item.path())?;
                let parent = parent_of(&item)?;
                let placed = move_item(&item, &parent, &name)?;
                (
                    Undo::Transfer(TransferUndo::Move {
                        item: placed,
                        parent,
                        name: old_name,
                    }),
                    "Renamed item",
                )
            }
            Operation::Recycle { items } => return self.recycle(items),
        };
        self.record(record);
        Ok(OperationResult {
            message: message.into(),
            transferred: Vec::new(),
        })
    }

    fn recycle(&mut self, items: Vec<ItemId>) -> Result<OperationResult> {
        ensure!(
            (1..=MAX_TRANSFER_ITEMS).contains(&items.len()),
            "Select between 1 and {MAX_TRANSFER_ITEMS} items"
        );
        let names: HashSet<_> = items.iter().map(|item| item.path()).collect();
        ensure!(
            names.len() == items.len(),
            "The selection contains the same path twice"
        );
        let total = items.len();
        let mut recycled = Vec::with_capacity(total);
        let mut first_error = None;
        for item in &items {
            let result = (|| {
                Stamp::read(&item.verify()?)?;
                let parent = parent_of(item)?;
                Ok::<_, anyhow::Error>((recycle(item.path(), item.identity)?, parent))
            })();
            match result {
                Ok(entry) => recycled.push(entry),
                Err(error) => {
                    first_error = Some(error.context(format!("{}", item.path().display())));
                    break;
                }
            }
        }
        let count = recycled.len();
        if count > 0 {
            self.record(Undo::Recycle(recycled));
        }
        let message = match first_error {
            Some(error) if count == 0 => return Err(error),
            Some(error) => format!(
                "Moved {count}/{total} items to Recycle Bin. First unfinished item: {error:#}"
            ),
            None if total == 1 => "Moved to Recycle Bin".into(),
            None => format!("Moved {count} items to Recycle Bin"),
        };
        Ok(OperationResult {
            message,
            transferred: Vec::new(),
        })
    }

    pub fn transfer(
        &mut self,
        request: TransferRequest,
        control: &TransferControl,
    ) -> Result<OperationResult> {
        let TransferRequest {
            items,
            into,
            move_items,
        } = request;
        ensure!(
            (1..=MAX_TRANSFER_ITEMS).contains(&items.len()),
            "Select between 1 and {MAX_TRANSFER_ITEMS} items"
        );
        let names: HashSet<_> = items.iter().map(|item| item.path()).collect();
        ensure!(
            names.len() == items.len(),
            "A transfer contains the same path twice"
        );
        // Only CopyFile2 transfers report bytes: copies and cross-volume moves of regular files.
        let planned_bytes = items
            .iter()
            .filter(|item| !(move_items && item.identity.volume == into.identity.volume))
            .filter(|item| transfer::regular(item, &into).unwrap_or(false))
            .filter_map(|item| std::fs::metadata(item.path()).ok())
            .map(|meta| meta.len())
            .sum();
        control.plan(items.len(), planned_bytes);
        let execute = |item: &ItemId| {
            let result = control
                .checkpoint()
                .and_then(|()| Self::transfer_one(item, &into, move_items, control));
            control.finish_item();
            result
        };
        let parallel = items.len() > 1
            && items.iter().all(|item| {
                (!move_items || item.identity.volume != into.identity.volume)
                    && transfer::regular(item, &into).unwrap_or(false)
            });
        let results = if parallel {
            // Each lane handles at most one CopyFile2 request and one checksum buffer at a time.
            let (left, right) = items.split_at(items.len().div_ceil(2));
            std::thread::scope(|scope| {
                let lane = scope.spawn(|| right.iter().map(execute).collect::<Vec<_>>());
                let mut results = left.iter().map(execute).collect::<Vec<_>>();
                results.extend(lane.join().expect("Transfer lane panicked"));
                results
            })
        } else {
            items.iter().map(execute).collect()
        };
        let mut transferred = Vec::new();
        let mut first_error = None;
        let mut last_message = "";
        let total = items.len();
        for (item, result) in items.into_iter().zip(results) {
            match result {
                Ok((record, message)) => {
                    self.record(Undo::Transfer(record));
                    transferred.push(item);
                    last_message = message;
                }
                Err(error) if first_error.is_none() => {
                    first_error = Some(error.context(format!("{}", item.path().display())));
                }
                Err(_) => {}
            }
        }
        let count = transferred.len();
        let message = match first_error {
            Some(error) if count == 0 => return Err(error),
            Some(error) => {
                format!("Completed {count}/{total} items. First unfinished item: {error:#}")
            }
            None if total == 1 => last_message.into(),
            None => format!(
                "{} {count} items. Undo applies one item at a time.",
                if move_items { "Moved" } else { "Copied" }
            ),
        };
        Ok(OperationResult {
            message,
            transferred,
        })
    }

    fn record(&mut self, record: Undo) {
        self.history.push_back(record);
        if self.history.len() > MAX_TRANSFER_ITEMS {
            self.history.pop_front();
        }
    }

    fn transfer_one(
        item: &ItemId,
        into: &ItemId,
        moving: bool,
        control: &TransferControl,
    ) -> Result<(TransferUndo, &'static str)> {
        if moving {
            let name = name_of(item.path())?;
            let parent = parent_of(item)?;
            if item.identity.volume == into.identity.volume {
                let placed = move_item(item, into, &name)?;
                return Ok((
                    TransferUndo::Move {
                        item: placed,
                        parent,
                        name,
                    },
                    "Moved item",
                ));
            }
            ensure!(
                transfer::regular(item, into)?,
                "Cross-volume moves require a regular file on fixed local drives"
            );
            let (placed, _) = transfer::place(item, into, &name, true, control)?;
            return Ok((
                TransferUndo::CrossVolumeMove {
                    item: placed,
                    parent,
                    name,
                },
                "Verified and moved file",
            ));
        }
        if transfer::regular(item, into)? {
            let (item, stamp) =
                transfer::place(item, into, &name_of(item.path())?, false, control)?;
            return Ok((
                TransferUndo::Copy { item, stamp },
                "Copied and verified file",
            ));
        }
        Self::copy(item, into)
    }

    pub fn undo(&mut self) -> Result<OperationResult> {
        self.undo_with_control(&TransferControl::default())
    }

    pub fn undo_with_control(&mut self, control: &TransferControl) -> Result<OperationResult> {
        control.checkpoint()?;
        let record = self
            .history
            .back()
            .context("There is no operation to undo")?;
        let message = match record {
            Undo::Transfer(TransferUndo::Move { item, parent, name }) => {
                move_item(item, parent, name)?;
                "Restored the original location"
            }
            Undo::Transfer(TransferUndo::CrossVolumeMove { item, parent, name }) => {
                transfer::place(item, parent, name, true, control)?;
                "Verified and restored the original location"
            }
            Undo::Transfer(TransferUndo::Copy { item, stamp }) => {
                ensure!(
                    &Stamp::read(&item.verify()?)? == stamp,
                    "The copied item has changed"
                );
                if stamp.directory {
                    ensure!(
                        fs::read_dir(item.path())?.next().is_none(),
                        "The copied folder now contains files"
                    );
                }
                recycle(item.path(), item.identity)?;
                "Moved the copy to Recycle Bin"
            }
            Undo::Recycle(_) => return self.restore_recycled(),
            Undo::Transfer(TransferUndo::FolderCopy) => {
                bail!("A copy of a nonempty folder cannot be safely undone")
            }
        };
        self.history.pop_back();
        Ok(OperationResult {
            message: message.into(),
            transferred: Vec::new(),
        })
    }

    /// Restores a recycled batch newest first. Items restored before a failure leave the record,
    /// so the next undo retries only what is still in the Recycle Bin.
    fn restore_recycled(&mut self) -> Result<OperationResult> {
        let Some(Undo::Recycle(items)) = self.history.back_mut() else {
            bail!("The latest operation is not a recycle");
        };
        let total = items.len();
        let mut restored = 0;
        while let Some((item, parent)) = items.last() {
            let result = parent.verify().and_then(|_| item.restore());
            if let Err(error) = result {
                if restored == 0 {
                    return Err(error);
                }
                let left = items.len();
                return Ok(OperationResult {
                    message: format!(
                        "Restored {restored}/{total} items. {left} stay in the Recycle Bin: {error:#}"
                    ),
                    transferred: Vec::new(),
                });
            }
            items.pop();
            restored += 1;
        }
        self.history.pop_back();
        Ok(OperationResult {
            message: if total == 1 {
                "Restored the exact item from Recycle Bin".into()
            } else {
                format!("Restored {total} items from Recycle Bin")
            },
            transferred: Vec::new(),
        })
    }

    fn copy(item: &ItemId, into: &ItemId) -> Result<(TransferUndo, &'static str)> {
        let name = name_of(item.path())?;
        let destination = destination(item, into, &name)?;
        // A partial Shell copy lives in a private staging directory until the final name can be claimed.
        let stage = tempfile::Builder::new()
            .prefix(".firewol-")
            .tempdir_in(into.path())?;
        let staging_parent = ItemId::at(stage.path().to_path_buf())?;
        let staged_path = stage.path().join(&name);
        if extended(item.path()) || extended(&staged_path) {
            let directory = Stamp::read(&item.verify()?)?.directory;
            staging_parent.verify()?;
            copy_extended(item.path(), &staged_path, directory)?;
        } else {
            let actual = shell_transfer(item, &staging_parent, &staged_path, Transfer::Copy)?;
            ensure!(
                display_path(&actual) == display_path(&staged_path),
                "The copy did not retain its original name"
            );
        }
        let staged = ItemId::at(staged_path.clone())?;
        let stamp = Stamp::read(&staged.verify()?)?;
        let nonempty = stamp.directory && fs::read_dir(&staged_path)?.next().is_some();
        into.verify()?;
        move_without_replace(&staged_path, &destination).context("Publish the completed copy")?;
        let copied = ItemId::at(destination)?;
        ensure!(
            copied.identity == staged.identity,
            "The published copy could not be verified"
        );
        if nonempty {
            Ok((
                TransferUndo::FolderCopy,
                "Copied folder. Undo is available for files and empty folders.",
            ))
        } else {
            Ok((
                TransferUndo::Copy {
                    item: copied,
                    stamp,
                },
                "Copied item",
            ))
        }
    }
}

fn move_item(item: &ItemId, into: &ItemId, name: &OsStr) -> Result<ItemId> {
    ensure!(
        item.identity.volume == into.identity.volume,
        "This move must stay on the same volume"
    );
    let same_folder = item.path().parent() == Some(into.path());
    let proposed = into.path().join(name);
    let case_change = same_folder
        && proposed != item.path()
        && proposed
            .canonicalize()
            .is_ok_and(|path| path == item.path())
        && ItemId::at(proposed.clone()).is_ok_and(|target| target.identity == item.identity);
    let destination = if case_change {
        proposed
    } else {
        destination(item, into, name)?
    };
    let actual = if case_change || extended(item.path()) || extended(&destination) {
        Stamp::read(&item.verify()?)?;
        into.verify()?;
        move_without_replace(item.path(), &destination)
            .context("Move the item without replacing the destination")?;
        destination.clone()
    } else {
        let transfer = if same_folder {
            Transfer::Rename
        } else {
            Transfer::Move
        };
        shell_transfer(item, into, &destination, transfer)?
    };
    let moved = ItemId::at(actual.clone())?;
    ensure!(
        moved.identity == item.identity,
        "The moved item could not be verified"
    );
    if display_path(&actual) != display_path(&destination) {
        move_without_replace(&actual, item.path()).with_context(|| {
            format!(
                "A conflicting name appeared. The item is at {}",
                actual.display()
            )
        })?;
        bail!("A conflicting name appeared. The item was returned to its original location.");
    }
    Ok(ItemId {
        path: destination,
        identity: moved.identity,
    })
}

fn extended(path: &Path) -> bool {
    display_path(path).encode_wide().count() >= 260
}

fn copy_extended(from: &Path, to: &Path, directory: bool) -> Result<()> {
    if !directory {
        let from = wide(from.as_os_str());
        let to = wide(to.as_os_str());
        return unsafe { CopyFileW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), true) }
            .context("Copy an extended-length file path");
    }
    fs::create_dir(to)?;
    let mut pending = vec![(fs::read_dir(from)?, to.to_owned())];
    while let Some((entries, target)) = pending.last_mut() {
        let Some(entry) = entries.next().transpose()? else {
            pending.pop();
            continue;
        };
        let metadata = entry.metadata()?;
        ensure!(
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
            "The folder contains a link or junction that cannot be copied safely"
        );
        let target = target.join(entry.file_name());
        if metadata.is_dir() {
            fs::create_dir(&target)?;
            pending.push((fs::read_dir(entry.path())?, target));
        } else {
            copy_extended(&entry.path(), &target, false)?;
        }
    }
    Ok(())
}

fn move_without_replace(from: &Path, to: &Path) -> windows::core::Result<()> {
    let from = wide(from.as_os_str());
    let to = wide(to.as_os_str());
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVE_FILE_FLAGS(0),
        )
    }
}

fn destination(item: &ItemId, into: &ItemId, name: &OsStr) -> Result<PathBuf> {
    let (directory, reparse, _, _) = attributes(&into.verify()?)?;
    ensure!(
        directory && !reparse,
        "The destination must be a regular folder"
    );
    let stamp = Stamp::read(&item.verify()?)?;
    ensure!(
        !stamp.directory || !into.path().starts_with(item.path()),
        "A folder cannot be transferred into itself"
    );
    let target = into.path().join(name);
    require_vacant(&target)?;
    Ok(target)
}

fn name_of(path: &Path) -> Result<OsString> {
    Ok(path
        .file_name()
        .context("A drive root cannot be changed")?
        .to_owned())
}

fn parent_of(item: &ItemId) -> Result<ItemId> {
    ItemId::at(
        item.path()
            .parent()
            .context("A drive root cannot be changed")?
            .to_owned(),
    )
}

fn validate_name(name: &OsStr) -> Result<()> {
    let text = name.to_string_lossy();
    let stem = text
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let reserved_port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|port| {
            ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&port)
        });
    ensure!(
        !text.is_empty()
            && name.encode_wide().count() <= 255
            && !text.ends_with(['.', ' '])
            && !text
                .chars()
                .any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch))
            && !["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
            && !reserved_port,
        "Enter a valid Windows file or folder name"
    );
    Ok(())
}

enum Transfer {
    Copy,
    Move,
    Rename,
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

fn shell_transfer(
    source: &ItemId,
    parent: &ItemId,
    target: &Path,
    transfer: Transfer,
) -> Result<PathBuf> {
    let item = shell_item(source.path())?;
    let into = shell_item(parent.path())?;
    let new_name = wide(&name_of(target)?);
    let result = Rc::new(RefCell::new(None));
    let sink: IFileOperationProgressSink = TransferSink {
        source: source.clone(),
        parent: parent.clone(),
        target: target.to_owned(),
        result: Rc::clone(&result),
    }
    .into();
    let operation: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)? };
    let performed = unsafe {
        operation.SetOperationFlags(
            FOF_RENAMEONCOLLISION
                | FOFX_PRESERVEFILEEXTENSIONS
                | FOFX_EARLYFAILURE
                | FOF_NOERRORUI
                | FOF_SILENT
                | FOF_NO_CONNECTED_ELEMENTS
                | FOFX_NOCOPYHOOKS,
        )?;
        let name = PCWSTR(new_name.as_ptr());
        match transfer {
            Transfer::Copy => operation.CopyItem(&item, &into, name, &sink)?,
            Transfer::Move => operation.MoveItem(&item, &into, name, &sink)?,
            Transfer::Rename => operation.RenameItem(&item, name, &sink)?,
        }
        operation.PerformOperations()
    };
    let reported = result.borrow_mut().take();
    if let Some(Err(error)) = reported {
        return Err(error);
    }
    performed.context("Windows Shell file operation")?;
    ensure!(
        !unsafe { operation.GetAnyOperationsAborted()? }.as_bool(),
        "Windows cancelled the file operation"
    );
    reported.context("Windows did not report a completed file operation")?
}

type TransferResult = Rc<RefCell<Option<Result<PathBuf>>>>;

#[implement(IFileOperationProgressSink)]
struct TransferSink {
    source: ItemId,
    parent: ItemId,
    target: PathBuf,
    result: TransferResult,
}

impl TransferSink {
    fn before(&self, item: Ref<IShellItem>) -> windows::core::Result<()> {
        let checked = (|| -> Result<()> {
            let path = filesystem_path(item.ok()?)?;
            if display_path(&path) == display_path(self.source.path()) {
                self.source.verify()?;
                self.parent.verify()?;
                require_vacant(&self.target)?;
            }
            Ok(())
        })();
        if let Err(error) = checked {
            *self.result.borrow_mut() = Some(Err(error));
            return Err(E_ABORT.into());
        }
        Ok(())
    }

    fn after(
        &self,
        item: Ref<IShellItem>,
        status: HRESULT,
        output: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        let path = filesystem_path(item.ok()?).map_err(|_| windows::core::Error::from(E_ABORT))?;
        if display_path(&path) == display_path(self.source.path()) {
            let finished = (|| -> Result<PathBuf> {
                status.ok()?;
                ensure!(
                    [
                        S_OK,
                        COPYENGINE_S_COLLISIONRESOLVED,
                        COPYENGINE_S_DONT_PROCESS_CHILDREN
                    ]
                    .contains(&status),
                    "Windows reported an incomplete transfer ({status:?})"
                );
                match output.as_ref() {
                    Some(output) => filesystem_path(output),
                    None if status != COPYENGINE_S_COLLISIONRESOLVED => Ok(self.target.clone()),
                    None => bail!("Windows did not report where the item was placed"),
                }
            })();
            *self.result.borrow_mut() = Some(finished);
        }
        Ok(())
    }
}

impl IFileOperationProgressSink_Impl for TransferSink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.before(item)
    }
    fn PostRenameItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: &PCWSTR,
        status: HRESULT,
        output: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.after(item, status, output)
    }
    fn PreMoveItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.before(item)
    }
    fn PostMoveItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        status: HRESULT,
        output: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.after(item, status, output)
    }
    fn PreCopyItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.before(item)
    }
    fn PostCopyItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        status: HRESULT,
        output: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.after(item, status, output)
    }
    fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> windows::core::Result<()> {
        Err(E_ABORT.into())
    }
    fn PostDeleteItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Err(E_ABORT.into())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}
