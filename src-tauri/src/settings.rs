use anyhow::{Result, ensure};
use firewol_core::explorer::elevation::{ElevationMode, ElevationPolicy, validate_executable};
use firewol_core::explorer::{
    Appearance, ColumnWidths, FolderStyle, ItemId, MenuAction, MenuEntry, Rgb, Session, Settings,
};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

use crate::contract::{ElevationView, FolderStyleView, SettingsPatch, SettingsView, StyledFolder};

pub const TILES: [u32; 3] = [96, 128, 160];
pub const ROWS: [u32; 4] = [24, 32, 40, 52];
pub const SIDEBAR_WIDTH: std::ops::RangeInclusive<u32> = 180..=360;
const NAME_WIDTH: std::ops::RangeInclusive<u32> = 120..=2400;
const COLUMN_WIDTH: std::ops::RangeInclusive<u32> = 56..=1200;

fn mirror<T: Serialize, U: DeserializeOwned>(value: T) -> Result<U> {
    Ok(serde_json::from_value(serde_json::to_value(value)?)?)
}

pub fn view(settings: &Settings) -> Result<SettingsView> {
    Ok(SettingsView {
        version: settings.version,
        theme_migrated: settings.theme_migrated,
        sidebar: settings.sidebar.unwrap_or(true),
        grid: settings.grid.unwrap_or(false),
        tile: settings
            .tile
            .filter(|tile| TILES.contains(tile))
            .unwrap_or(128),
        sidebar_width: settings
            .sidebar_width
            .filter(|width| SIDEBAR_WIDTH.contains(width))
            .unwrap_or(232),
        row: settings.row.filter(|row| ROWS.contains(row)).unwrap_or(32),
        shell_items: settings.shell_items.unwrap_or(true),
        show_hidden: settings.show_hidden.unwrap_or(true),
        show_extensions: settings.show_extensions.unwrap_or(true),
        columns: mirror(settings.columns.unwrap_or_default())?,
        appearance: mirror(settings.appearance)?,
        menu: mirror(&settings.menu)?,
        folders: settings
            .folders
            .iter()
            .map(|(key, style)| {
                let (volume, index) = key.parts();
                Ok(StyledFolder {
                    volume,
                    index: index.to_string(),
                    style: style_view(style),
                })
            })
            .collect::<Result<_>>()?,
        elevation: ElevationView {
            mode: mirror(settings.elevation.mode)?,
            executables: settings
                .elevation
                .paths()
                .map(crate::folders::text)
                .collect(),
        },
    })
}

/// Direct conversion: listings call this for every styled folder row.
pub fn style_view(style: &FolderStyle) -> FolderStyleView {
    FolderStyleView {
        alias: style.alias.clone(),
        color: style.color.map(|Rgb(red, green, blue)| [red, green, blue]),
        icon: style.icon.clone(),
        tags: style.tags.clone(),
    }
}

/// Saves (or, for `None` or an empty style, removes) a folder's style. The core re-verifies the
/// identity and that it is still a folder.
pub fn set_folder_style(
    session: &Session,
    folder: &ItemId,
    style: Option<FolderStyleView>,
) -> Result<SettingsView> {
    let style = style.map_or_else(FolderStyle::default, |view| FolderStyle {
        alias: view.alias,
        color: view.color.map(|[red, green, blue]| Rgb(red, green, blue)),
        icon: view.icon,
        tags: view.tags,
    });
    view(&session.set_folder_style(folder, style)?)
}

pub fn update(session: &Session, patch: SettingsPatch) -> Result<SettingsView> {
    let appearance = patch.appearance.map(mirror::<_, Appearance>).transpose()?;
    let menu = patch.menu.map(mirror::<_, Vec<MenuEntry>>).transpose()?;
    let columns = patch.columns.map(mirror::<_, ColumnWidths>).transpose()?;
    if let Some(columns) = columns {
        ensure!(
            [
                columns.location,
                columns.modified,
                columns.kind,
                columns.size,
                columns.tags
            ]
            .iter()
            .all(|width| COLUMN_WIDTH.contains(width))
                && NAME_WIDTH.contains(&columns.name),
            "Column widths are out of range"
        );
    }
    ensure!(
        patch.tile.is_none_or(|tile| TILES.contains(&tile)),
        "Grid tiles are 96, 128 or 160 pixels"
    );
    ensure!(
        patch.row.is_none_or(|row| ROWS.contains(&row)),
        "Details rows are 24, 32, 40 or 52 pixels high"
    );
    ensure!(
        patch
            .sidebar_width
            .is_none_or(|width| SIDEBAR_WIDTH.contains(&width)),
        "The sidebar is 180 to 360 pixels wide"
    );
    if let Some(menu) = &menu {
        ensure!(menu.len() <= 64, "Use up to 64 menu entries");
        for entry in menu {
            ensure!(
                entry.label.len() <= 200 && entry.icon.len() <= 64,
                "Menu text is too long"
            );
            if let MenuAction::Custom { executable, .. } = &entry.action {
                validate_executable(Path::new(executable))?;
            }
        }
    }
    let elevation = patch
        .elevation
        .map(|view| -> Result<_> {
            let mode: ElevationMode = mirror(view.mode)?;
            ensure!(
                mode != ElevationMode::AllExecutables || patch.allow_all_confirmed == Some(true),
                "Confirm Any executable in this window before enabling it"
            );
            ElevationPolicy::new(
                mode,
                &view
                    .executables
                    .into_iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>(),
            )
        })
        .transpose()?;
    let updated = session.update_settings(move |settings| {
        if let Some(appearance) = appearance {
            if patch.migrate_appearance != Some(true) || !settings.theme_migrated {
                settings.appearance = appearance;
            }
            settings.theme_migrated = true;
        }
        if let Some(sidebar) = patch.sidebar {
            settings.sidebar = Some(sidebar);
        }
        if let Some(grid) = patch.grid {
            settings.grid = Some(grid);
        }
        if let Some(tile) = patch.tile {
            settings.tile = Some(tile);
        }
        if let Some(width) = patch.sidebar_width {
            settings.sidebar_width = Some(width);
        }
        if let Some(row) = patch.row {
            settings.row = Some(row);
        }
        if let Some(shell_items) = patch.shell_items {
            settings.shell_items = Some(shell_items);
        }
        if let Some(show_hidden) = patch.show_hidden {
            settings.show_hidden = Some(show_hidden);
        }
        if let Some(show_extensions) = patch.show_extensions {
            settings.show_extensions = Some(show_extensions);
        }
        if let Some(columns) = columns {
            settings.columns = Some(columns);
        }
        if let Some(menu) = menu {
            settings.menu = menu;
        }
        if let Some(elevation) = elevation {
            settings.elevation = elevation;
        }
    })?;
    view(&updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use firewol_core::explorer::{FolderKey, FolderStyle, ItemId};

    #[test]
    fn settings_keep_large_identities_and_migrate_only_once() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let session = Session::open(fixture.path().to_owned(), |_| {})?;
        let item = ItemId::from_parts(fixture.path().to_owned(), 23, u64::MAX - 1);
        session.update_settings(|settings| {
            settings
                .folders
                .insert(FolderKey::from(&item), FolderStyle::default());
        })?;
        let before = view(&session.settings())?;
        assert_eq!(before.folders[0].index, "18446744073709551614");
        let mut appearance = before.appearance;
        appearance.dark = true;
        let migrated = update(
            &session,
            SettingsPatch {
                appearance: Some(appearance.clone()),
                migrate_appearance: Some(true),
                ..Default::default()
            },
        )?;
        assert!(migrated.theme_migrated && migrated.appearance.dark);
        appearance.dark = false;
        let again = update(
            &session,
            SettingsPatch {
                appearance: Some(appearance),
                migrate_appearance: Some(true),
                sidebar: Some(false),
                ..Default::default()
            },
        )?;
        assert!(again.appearance.dark && !again.sidebar && again.sidebar_width == 232);
        let wide = update(
            &session,
            SettingsPatch {
                sidebar_width: Some(300),
                ..Default::default()
            },
        )?;
        assert_eq!(wide.sidebar_width, 300);
        let tall = update(
            &session,
            SettingsPatch {
                row: Some(52),
                ..Default::default()
            },
        )?;
        assert_eq!(tall.row, 52);
        assert_eq!(Settings::load(fixture.path())?.row, Some(52));
        assert!(
            update(
                &session,
                SettingsPatch {
                    row: Some(33),
                    ..Default::default()
                },
            )
            .is_err()
        );
        assert!(
            update(
                &session,
                SettingsPatch {
                    sidebar_width: Some(500),
                    ..Default::default()
                },
            )
            .is_err()
        );
        let reload = Settings::load(fixture.path())?;
        assert!(reload.appearance.dark && reload.theme_migrated);
        assert_eq!(reload.sidebar, Some(false));
        let denied = update(
            &session,
            SettingsPatch {
                elevation: Some(ElevationView {
                    mode: "all_executables".into(),
                    executables: vec![],
                }),
                ..Default::default()
            },
        );
        assert!(denied.is_err());
        Ok(())
    }
}
