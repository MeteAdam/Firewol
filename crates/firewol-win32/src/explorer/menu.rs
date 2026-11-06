use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{Item, MenuAction, MenuEntry, display_path};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{DT_LEFT, DT_RIGHT, DrawFocusRect, ScreenToClient};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PWSTR;

use super::controls::{Controls, send, wide};
use super::theme::{Theme, draw_text, fill, mix};
use super::{App, CONTAINING, COPY, CUT, OPEN, PASTE, RECYCLE, REFRESH, RENAME, UNDO};

const FIRST: usize = 1000;
pub(super) const MAX_ENTRIES: usize = 32;

pub(super) fn details(action: &MenuAction) -> (&'static str, &'static str, &'static str, u8) {
    use MenuAction::*;
    match action {
        Open => ("Open", "↵", "Enter", 0),
        Containing => ("Open containing folder", "↗", "", 0),
        Copy => ("Copy", "⧉", "Ctrl+C", 1),
        Cut => ("Cut", "✂", "Ctrl+X", 1),
        Paste => ("Paste in current folder", "▣", "Ctrl+V", 1),
        CopyOther => ("Copy to other pane", "⇥", "", 1),
        MoveOther => ("Move to other pane", "⇄", "", 1),
        Rename => ("Rename…", "✎", "F2", 1),
        Recycle => ("Move to Recycle Bin…", "×", "Del", 1),
        FolderStyle => ("Customize folder…", "◈", "", 2),
        Undo => ("Undo", "↶", "Ctrl+Z", 3),
        Refresh => ("Refresh", "⟳", "F5", 3),
        Custom { .. } => ("Run action", "▶", "", 2),
    }
}

pub(super) fn label(entry: &MenuEntry) -> &str {
    if entry.label.is_empty() {
        details(&entry.action).0
    } else {
        &entry.label
    }
}

struct Row {
    entry: MenuEntry,
    enabled: bool,
    divider: bool,
}

struct Menu(HMENU);

impl Drop for Menu {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

pub(super) fn popup(app: &App, window: HWND, controls: Controls, mouse: bool) -> Result<()> {
    let mut point = POINT::default();
    let item = if mouse {
        unsafe { GetCursorPos(&mut point)? };
        let mut hit = LVHITTESTINFO {
            pt: point,
            ..Default::default()
        };
        let _ = unsafe { ScreenToClient(controls.list, &mut hit.pt) };
        let row = send(
            controls.list,
            LVM_SUBITEMHITTEST,
            0,
            std::ptr::from_mut(&mut hit) as isize,
        );
        app.row(row as i32)
    } else {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(controls.list, &mut rect)? };
        point = POINT {
            x: rect.left + 24,
            y: rect.top + 30,
        };
        app.selection(controls)
    };
    let (searching, current) = {
        let model = app.model.borrow();
        (
            model.search.active(),
            model
                .folder
                .as_ref()
                .and_then(|folder| folder.id.as_ref())
                .is_some(),
        )
    };
    let selected = item.as_ref().is_some_and(|item| item.id.is_some());
    let can_style = item
        .as_ref()
        .map_or(current, |item| item.directory && selected);
    let clipboard = app.session.clipboard.borrow().is_some();
    let idle = !app.session.busy();
    let split = super::Frame::at(app.frame)
        .and_then(|frame| frame.other(window))
        .is_some();
    let mut group = None;
    let rows: Vec<_> = app
        .session
        .settings
        .borrow()
        .menu
        .iter()
        .filter(|entry| entry.visible)
        .filter_map(|entry| {
            use MenuAction::*;
            let enabled = match entry.action {
                Containing if !searching => return None,
                CopyOther | MoveOther if !split => return None,
                Open | Containing => selected,
                Copy | Cut | Rename | Recycle | CopyOther | MoveOther => selected && idle,
                Paste => current && clipboard && idle,
                FolderStyle => can_style,
                Undo => idle,
                Refresh => current,
                Custom { .. } => item.as_ref().map_or(current, |_| selected),
            };
            let next_group = details(&entry.action).3;
            let divider = group
                .replace(next_group)
                .is_some_and(|previous| previous != next_group);
            Some(Row {
                entry: entry.clone(),
                enabled,
                divider,
            })
        })
        .take(MAX_ENTRIES)
        .collect();
    ensure!(
        !rows.is_empty(),
        "No actions are visible. Use Edit menu to show them."
    );
    let menu = Menu(unsafe { CreatePopupMenu()? });
    for (position, row) in rows.iter().enumerate() {
        let mut text = wide(OsStr::new(label(&row.entry)));
        let item = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_ID | MIIM_FTYPE | MIIM_STATE | MIIM_DATA | MIIM_STRING,
            fType: MFT_OWNERDRAW,
            fState: if row.enabled {
                MFS_ENABLED
            } else {
                MFS_DISABLED
            },
            wID: (FIRST + position) as u32,
            dwItemData: std::ptr::from_ref(row) as usize,
            dwTypeData: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        unsafe { InsertMenuItemW(menu.0, position as u32, true, &item)? };
    }
    let command = unsafe {
        TrackPopupMenu(
            menu.0,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            window,
            None,
        )
        .0
    } as usize;
    if let Some(row) = command
        .checked_sub(FIRST)
        .and_then(|index| rows.get(index))
        .filter(|row| row.enabled)
    {
        invoke(app, window, controls, &row.entry.action, item)?;
    }
    Ok(())
}

fn invoke(
    app: &App,
    window: HWND,
    controls: Controls,
    action: &MenuAction,
    item: Option<Item>,
) -> Result<()> {
    use MenuAction::*;
    let command = match action {
        Open => OPEN,
        Containing => CONTAINING,
        Copy => COPY,
        Cut => CUT,
        Paste => PASTE,
        Rename => RENAME,
        Recycle => RECYCLE,
        Undo => UNDO,
        Refresh => REFRESH,
        FolderStyle => return app.customize_folder(item),
        CopyOther | MoveOther => {
            let item = item
                .and_then(|item| item.id)
                .context("Select an item with a verified identity")?;
            return app.transfer(item, *action == MoveOther);
        }
        Custom { executable } => {
            let (id, directory) = if let Some(item) = item {
                (item.id, item.directory)
            } else {
                (
                    app.model
                        .borrow()
                        .folder
                        .as_ref()
                        .and_then(|folder| folder.id.clone()),
                    true,
                )
            };
            let id = id.context("The target identity is unavailable")?;
            id.verify()?;
            let executable = Path::new(executable);
            validate_executable(executable)?;
            let folder = if directory {
                id.path()
            } else {
                id.path().parent().context("Target has no folder")?
            };
            Command::new(executable)
                .arg(display_path(id.path()))
                .current_dir(folder)
                .spawn()
                .with_context(|| format!("Launch {}", executable.display()))?;
            return Ok(());
        }
    };
    app.action(window, controls, command, item)
}

pub(super) fn validate_executable(executable: &Path) -> Result<()> {
    ensure!(
        executable.is_absolute()
            && executable
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")),
        "Enter an absolute path to an .exe file"
    );
    Ok(())
}

fn row_data(item_type: DRAWITEMSTRUCT_CTL_TYPE, id: u32, data: usize) -> Option<*const Row> {
    (item_type == ODT_MENU && (FIRST..FIRST + MAX_ENTRIES).contains(&(id as usize)) && data != 0)
        .then_some(data as *const Row)
}

pub(super) fn measure(info: &mut MEASUREITEMSTRUCT) -> bool {
    let Some(row) = row_data(info.CtlType, info.itemID, info.itemData) else {
        return false;
    };
    let row = unsafe { &*row };
    info.itemWidth = 292;
    info.itemHeight = if row.divider { 28 } else { 25 };
    true
}

pub(super) fn draw(info: &DRAWITEMSTRUCT, theme: &Theme) -> bool {
    let Some(row) = row_data(info.CtlType, info.itemID, info.itemData) else {
        return false;
    };
    let row = unsafe { &*row };
    let dc = info.hDC;
    let mut rect = info.rcItem;
    fill(dc, rect, theme.surface);
    if row.divider {
        fill(
            dc,
            RECT {
                left: rect.left + 7,
                right: rect.right - 7,
                top: rect.top + 1,
                bottom: rect.top + 2,
            },
            theme.border,
        );
        rect.top += 3;
    }
    if row.enabled && info.itemState.0 & ODS_SELECTED.0 != 0 {
        fill(
            dc,
            RECT {
                left: rect.left + 2,
                right: rect.right - 2,
                ..rect
            },
            mix(theme.surface, theme.accent, 20),
        );
    }
    let (_, icon, shortcut, _) = details(&row.entry.action);
    let icon = if row.entry.icon.is_empty() {
        icon
    } else {
        &row.entry.icon
    };
    let color = if row.enabled {
        row.entry.color.unwrap_or(theme.accent)
    } else {
        theme.muted
    };
    let text = if row.enabled { theme.text } else { theme.muted };
    draw_text(
        dc,
        RECT {
            left: rect.left + 9,
            right: rect.left + 30,
            ..rect
        },
        OsStr::new(icon),
        color,
        DT_LEFT,
    );
    draw_text(
        dc,
        RECT {
            left: rect.left + 34,
            right: rect.right - 62,
            ..rect
        },
        OsStr::new(label(&row.entry)),
        text,
        DT_LEFT,
    );
    draw_text(
        dc,
        RECT {
            left: rect.right - 60,
            right: rect.right - 10,
            ..rect
        },
        OsStr::new(shortcut),
        theme.muted,
        DT_RIGHT,
    );
    if info.itemState.0 & ODS_FOCUS.0 != 0 {
        let _ = unsafe { DrawFocusRect(dc, &rect) };
    }
    true
}
