use std::ffi::OsString;
use std::fs::{self, File};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, BHID_SFUIObject, BHID_Transfer, CMF_NORMAL, CMINVOKECOMMANDINFO,
    FOLDERID_RecycleBinFolder, IContextMenu, IEnumShellItems, IShellItem, ITransferSource,
    KF_FLAG_DEFAULT, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHCreateItemFromParsingName,
    SHGetKnownFolderItem, SICHINT_TEST_FILESYSPATH_IF_NOT_EQUAL, SIGDN_FILESYSPATH, TSF_NORMAL,
};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, HMENU, SW_SHOWNORMAL};
use windows::core::{PCWSTR, s};

use crate::file_identity::{FileIdentity, identity};

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct PopupMenu(HMENU);

impl Drop for PopupMenu {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

fn shell_item(path: &Path) -> Result<IShellItem> {
    let wide: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>) }
        .with_context(|| format!("Open Shell item {}", path.display()))
}

fn filesystem_path(item: &IShellItem) -> Result<PathBuf> {
    let wide = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .context("Resolve the exact recycled item's filesystem path")?;
    let path = unsafe { OsString::from_wide(wide.as_wide()) };
    unsafe { CoTaskMemFree(Some(wide.0.cast())) };
    Ok(path.into())
}

fn require_vacant(original: &Path) -> Result<()> {
    match original.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => anyhow::bail!("Restore destination already exists"),
    }
}

fn namespace_item(bin: &IShellItem, recycled: &IShellItem) -> Result<IShellItem> {
    let entries: IEnumShellItems =
        unsafe { bin.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems)? };
    loop {
        let mut next = [None];
        let mut fetched = 0;
        unsafe { entries.Next(&mut next, Some(&mut fetched))? };
        let [Some(item)] = next else {
            anyhow::bail!("The exact recycled item is absent from the Recycle Bin namespace");
        };
        if unsafe { item.Compare(recycled, SICHINT_TEST_FILESYSPATH_IF_NOT_EQUAL.0 as u32) }
            .is_ok_and(|order| order == 0)
        {
            return Ok(item);
        }
    }
}

fn restore(
    bin: &IShellItem,
    recycled: &IShellItem,
    original: &Path,
    id: FileIdentity,
) -> Result<()> {
    require_vacant(original)?;
    let path = filesystem_path(recycled)?;
    ensure!(
        identity(&File::open(path)?)? == id,
        "Recycled file identity changed"
    );
    let item = namespace_item(bin, recycled)?;
    let menu: IContextMenu = unsafe { item.BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject) }
        .context("Open exact recycled item's restore handler")?;
    let popup = PopupMenu(unsafe { CreatePopupMenu()? });
    unsafe {
        menu.QueryContextMenu(popup.0, 0, 1, 0x7fff, CMF_NORMAL)
            .ok()?
    };
    let command = CMINVOKECOMMANDINFO {
        cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: s!("undelete"),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { menu.InvokeCommand(&command) }.context("Restore the exact recycled item")?;
    ensure!(
        identity(&File::open(original)?)? == id,
        "Restored a different file"
    );
    Ok(())
}

/// A round-trip test scoped to a newly created fixture file.
pub fn probe(root: &Path) -> Result<()> {
    let stage = tempfile::tempdir_in(root)?;
    let original = stage.path().join("geri-al-火.txt");
    let payload = b"Firewol recycle identity probe";
    fs::write(&original, payload)?;
    let file = File::open(&original)?;
    let id = identity(&file)?;
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let _apartment = Apartment;
    let item = shell_item(&original)?;
    let bin: IShellItem =
        unsafe { SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None) }?;
    let parent = unsafe { item.GetParent()? };
    let transfer: ITransferSource =
        unsafe { parent.BindToHandler(None::<&IBindCtx>, &BHID_Transfer) }
            .context("Bind filesystem transfer source")?;
    let recycled = unsafe { transfer.RecycleItem(&item, &bin, TSF_NORMAL.0 as u32) }
        .context("Recycle the fixture through ITransferSource::RecycleItem")?;
    require_vacant(&original)?;

    fs::write(&original, b"Restore conflict fixture")?;
    ensure!(
        restore(&bin, &recycled, &original, id).is_err()
            && fs::read(&original)? == b"Restore conflict fixture",
        "Restore did not preserve the conflicting file"
    );
    fs::remove_file(&original)?;

    restore(&bin, &recycled, &original, id)?;
    ensure!(
        fs::read(&original)? == payload,
        "Restored file contents changed"
    );
    Ok(())
}
