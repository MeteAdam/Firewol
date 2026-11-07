use firewol_core::explorer::display_path;
use firewol_core::explorer::elevation::{ElevationMode, ElevationPolicy, validate_executable};
use std::cell::RefCell;
use std::ffi::OsString;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{
    Appearance, CaptionShape, FolderStyle, MenuAction, MenuEntry, Preset, default_menu,
};
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::color;
use super::controls::{child, place, send, set_text, wide};
use super::form::{Dialog, Field, fields};
use super::menu;

pub(super) fn appearance(owner: HWND, previous: Appearance) -> Result<Option<Appearance>> {
    let preset = Preset::ALL
        .iter()
        .position(|preset| *preset == previous.preset)
        .unwrap_or_default();
    let shape = CaptionShape::ALL
        .iter()
        .position(|shape| *shape == previous.caption_shape)
        .unwrap_or_default();
    fields(
        owner,
        w!("Firewol appearance"),
        &[
            Field::Choice("Theme", &Preset::NAMES, preset),
            Field::Color("Background", previous.background),
            Field::Color("Surfaces", previous.surface),
            Field::Color("Text", previous.text),
            Field::Color("Accent", previous.accent),
            Field::Color("Folder icon", previous.folder),
            Field::Color("Folder label", previous.folder_label),
            Field::Choice(
                "Window buttons",
                &["Right", "Left"],
                usize::from(previous.caption_left),
            ),
            Field::Choice("Button shape", &CaptionShape::NAMES, shape),
            Field::Color("Button color", previous.caption_fill),
            Field::Color("Button hover", previous.caption_hover),
        ],
        "Leave a color blank to use the theme value.",
        |values| {
            let preset = Preset::NAMES
                .iter()
                .position(|name| values[0] == *name)
                .context("Choose a theme")?;
            let shape = CaptionShape::NAMES
                .iter()
                .position(|name| values[8] == *name)
                .context("Choose a button shape")?;
            Ok(Appearance {
                preset: Preset::ALL[preset],
                background: color::parse(&values[1])?,
                surface: color::parse(&values[2])?,
                text: color::parse(&values[3])?,
                accent: color::parse(&values[4])?,
                folder: color::parse(&values[5])?,
                folder_label: color::parse(&values[6])?,
                caption_left: values[7] == "Left",
                caption_shape: CaptionShape::ALL[shape],
                caption_fill: color::parse(&values[9])?,
                caption_hover: color::parse(&values[10])?,
                ..previous
            })
        },
    )
}

pub(super) fn folder(owner: HWND, previous: &FolderStyle) -> Result<Option<FolderStyle>> {
    fields(
        owner,
        w!("Customize folder"),
        &[
            Field::Text("Display name", previous.alias.clone().into(), 100),
            Field::Color("Icon color", previous.color),
            Field::Text("Icon glyph", previous.icon.clone().into(), 8),
            Field::Text(
                "Tags (comma separated)",
                previous.tags.join(", ").into(),
                255,
            ),
        ],
        "Leave fields blank to use the folder's default appearance.",
        |values| {
            let mut tags = Vec::new();
            for tag in value(&values[3])?
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
            {
                ensure!(
                    tag.chars().count() <= 32,
                    "Keep each tag within 32 characters"
                );
                if !tags.iter().any(|existing| existing == tag) {
                    tags.push(tag.to_owned());
                }
            }
            ensure!(tags.len() <= 8, "Use up to eight tags per folder");
            Ok(FolderStyle {
                alias: value(&values[0])?.to_owned(),
                color: color::parse(&values[1])?,
                icon: value(&values[2])?.to_owned(),
                tags,
            })
        },
    )
}

fn value(value: &OsString) -> Result<&str> {
    value
        .to_str()
        .map(str::trim)
        .context("Enter valid Unicode text")
}

fn menu_entry(owner: HWND, mut previous: MenuEntry) -> Result<Option<MenuEntry>> {
    let mut rows = vec![
        Field::Text("Label", previous.label.clone().into(), 64),
        Field::Text("Icon glyph", previous.icon.clone().into(), 8),
        Field::Color("Icon color", previous.color),
        Field::Choice(
            "Visibility",
            &["Show", "Hide"],
            usize::from(!previous.visible),
        ),
    ];
    let custom = if let MenuAction::Custom {
        executable,
        administrator,
    } = &previous.action
    {
        rows.push(Field::Text(
            "Executable path",
            executable.clone().into(),
            32767,
        ));
        rows.push(Field::Choice(
            "Run with",
            &["Standard permissions", "Administrator consent"],
            usize::from(*administrator),
        ));
        true
    } else {
        false
    };
    let changes = fields(
        owner,
        w!("Edit menu action"),
        &rows,
        if custom {
            "Firewol passes the selected path as a single argument."
        } else {
            "Blank fields use the action defaults."
        },
        |values| {
            let label = value(&values[0])?.to_owned();
            let icon = value(&values[1])?.to_owned();
            let color = color::parse(&values[2])?;
            let visible = values[3] == "Show";
            let executable = if custom {
                ensure!(!label.is_empty(), "Give the action a label");
                let executable = value(&values[4])?;
                validate_executable(Path::new(executable))?;
                Some((executable.to_owned(), values[5] == "Administrator consent"))
            } else {
                None
            };
            Ok((label, icon, color, visible, executable))
        },
    )?;
    let Some((label, icon, color, visible, executable)) = changes else {
        return Ok(None);
    };
    previous.label = label;
    previous.icon = icon;
    previous.color = color;
    previous.visible = visible;
    if let Some((executable, administrator)) = executable {
        previous.action = MenuAction::Custom {
            executable,
            administrator,
        };
    }
    Ok(Some(previous))
}

pub(super) fn menu(owner: HWND, previous: Vec<MenuEntry>) -> Result<Option<Vec<MenuEntry>>> {
    let dialog = Dialog::create(owner, w!("Edit Firewol menu"), 512, 465)?;
    let window = dialog.window;
    let list = child(
        window,
        dialog.instance,
        20,
        w!("LISTBOX"),
        w!("Menu actions"),
        WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
        WS_EX_CLIENTEDGE,
    )?;
    let entries = RefCell::new(previous);
    dialog.text(21, "Menu actions · top to bottom", 14, 12, 360)?;
    place(list, 14, 40, 362, 307);
    for (i, label) in [
        w!("Up"),
        w!("Down"),
        w!("Edit…"),
        w!("Add…"),
        w!("Remove"),
        w!("Reset"),
    ]
    .into_iter()
    .enumerate()
    {
        dialog.button(10 + i, label, 387, 40 + 34 * i as i32, 95)?;
    }
    let status = dialog.text(22, "", 14, 354, 468)?;
    dialog.button(1, w!("Apply"), 310, 387, 80)?;
    dialog.button(2, w!("Cancel"), 402, 387, 80)?;
    let refresh = |select: usize| {
        send(list, LB_RESETCONTENT, 0, 0);
        for entry in entries.borrow().iter() {
            let label = if entry.visible {
                menu::label(entry).to_owned()
            } else {
                format!("[hidden] {}", menu::label(entry))
            };
            let label = wide(label.as_ref());
            send(list, LB_ADDSTRING, 0, label.as_ptr() as isize);
        }
        send(list, LB_SETCURSEL, select, 0);
    };
    refresh(0);
    dialog.run(list, &[], |reply, message, wparam, _| {
        if message != WM_COMMAND {
            return None;
        }
        let id = wparam.0 & 0xffff;
        let id = if id == 20 && (wparam.0 >> 16) as u32 == LBN_DBLCLK {
            12
        } else {
            id
        };
        let action = || -> Result<()> {
            let selection = send(list, LB_GETCURSEL, 0, 0) as usize;
            let mut select = selection;
            let count = entries.borrow().len();
            match id {
                1 => {
                    reply.accept(entries.borrow().clone());
                    return Ok(());
                }
                10 if selection < count && selection > 0 => {
                    select = selection - 1;
                    entries.borrow_mut().swap(selection, select);
                }
                11 if selection < count.saturating_sub(1) => {
                    select = selection + 1;
                    entries.borrow_mut().swap(selection, select);
                }
                12 if selection < count => {
                    let entry = entries.borrow()[selection].clone();
                    if let Some(edited) = menu_entry(window, entry)? {
                        entries.borrow_mut()[selection] = edited;
                    }
                }
                13 => {
                    ensure!(
                        count < menu::MAX_ENTRIES,
                        "Remove an action before adding another"
                    );
                    let entry = MenuEntry::new(MenuAction::Custom {
                        executable: String::new(),
                        administrator: false,
                    });
                    if let Some(entry) = menu_entry(window, entry)? {
                        select = count;
                        entries.borrow_mut().push(entry);
                    }
                }
                14 if selection < count => {
                    let mut entries = entries.borrow_mut();
                    if matches!(entries[selection].action, MenuAction::Custom { .. }) {
                        entries.remove(selection);
                        select = selection.min(entries.len().saturating_sub(1));
                    } else {
                        entries[selection].visible = false;
                    }
                }
                15 => {
                    entries.replace(default_menu());
                    select = 0;
                }
                _ => return Ok(()),
            }
            set_text(status, "");
            refresh(select);
            Ok(())
        };
        if let Err(error) = action() {
            set_text(status, format!("{error}"));
        }
        Some(LRESULT(0))
    })
}

pub(super) fn elevation(
    owner: HWND,
    previous: &ElevationPolicy,
) -> Result<Option<ElevationPolicy>> {
    let choices = [
        "Standard permissions",
        "Allowlisted executables",
        "Any executable (higher risk)",
    ];
    let mode = match previous.mode {
        ElevationMode::Standard => 0,
        ElevationMode::Allowlist => 1,
        ElevationMode::AllExecutables => 2,
    };
    let paths = previous
        .paths()
        .map(|path| display_path(path).to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" | ");
    let changed = fields(
        owner,
        w!("Administrator actions"),
        &[
            Field::Choice("Elevation policy", &choices, mode),
            Field::Text("Allowlisted .exe paths", paths.into(), 32_767),
        ],
        "Separate absolute .exe paths with |. Windows asks for consent.",
        |values| {
            let mode = match value(&values[0])? {
                "Allowlisted executables" => ElevationMode::Allowlist,
                "Any executable (higher risk)" => ElevationMode::AllExecutables,
                _ => ElevationMode::Standard,
            };
            let paths = value(&values[1])?
                .split('|')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from)
                .collect::<Vec<_>>();
            ElevationPolicy::new(mode, &paths)
        },
    )?;
    if changed
        .as_ref()
        .is_some_and(|policy| policy.mode == ElevationMode::AllExecutables)
        && previous.mode != ElevationMode::AllExecutables
        && unsafe {
            MessageBoxW(
                Some(owner),
                w!(
                    "Any executable or configured administrator action can request elevation. An approved program can change system settings and access protected data. Enable this policy?"
                ),
                w!("Allow any executable to request elevation"),
                MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        } != IDOK
    {
        return Ok(None);
    }
    Ok(changed)
}
