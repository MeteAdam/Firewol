//! Starts a Shell drag of files so Explorer, the desktop and other programs can take them.
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use anyhow::{Result, ensure};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, IDropSource};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName,
};
use windows_core::PCWSTR;

/// Frees the Shell's item ID lists when the drag ends.
struct Pidls(Vec<*mut ITEMIDLIST>);

impl Drop for Pidls {
    fn drop(&mut self) {
        for pidl in &self.0 {
            unsafe { CoTaskMemFree(Some(pidl.cast_const().cast())) };
        }
    }
}

/// Runs the Shell's drag loop (it returns when the button is released) with the default drop
/// source and drag image. The target program copies or moves; the folder watcher refreshes the
/// listing afterwards. Call on a thread that has OLE initialized, with the window handle.
pub fn drag_out(hwnd: HWND, paths: &[PathBuf]) -> Result<()> {
    ensure!(!paths.is_empty(), "Nothing to drag");
    let mut pidls = Pidls(Vec::new());
    for path in paths {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut pidl = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None)? };
        pidls.0.push(pidl);
    }
    let absolute: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|pidl| pidl.cast_const()).collect();
    unsafe {
        let items = SHCreateShellItemArrayFromIDLists(&absolute)?;
        let data: IDataObject = items.BindToHandler(None, &BHID_DataObject)?;
        SHDoDragDrop(
            Some(hwnd),
            &data,
            None::<&IDropSource>,
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
        )?;
    }
    Ok(())
}
