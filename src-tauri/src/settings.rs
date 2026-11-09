use anyhow::{Result, ensure};
use firewol_core::explorer::elevation::{
    ElevationMode, ElevationPolicy, is_network_path, validate_executable,
};
use firewol_core::explorer::{
    CaptionShape, ColorSlot, ColumnWidths, FolderStyle, ItemId, MenuAction, MenuEntry, Preset, Rgb,
    Session, Settings,
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::contract::{ElevationView, FolderStyleView, SettingsPatch, SettingsView, StyledFolder};

pub const TILES: [u32; 3] = [96, 128, 160];
pub const ROWS: [u32; 4] = [24, 32, 40, 52];
pub const SIDEBAR_WIDTH: std::ops::RangeInclusive<u32> = 180..=360;
const NAME_WIDTH: std::ops::RangeInclusive<u32> = 120..=2400;
const COLUMN_WIDTH: std::ops::RangeInclusive<u32> = 56..=1200;
const EXECUTABLE_LENGTH: usize = 4096;

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
        appearance: mirror(&settings.appearance)?,
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

/// What a patch would newly allow to run, or to run elevated. Saving the same values again, and
/// anything that only narrows the policy, needs no approval.
fn escalations(
    current: &Settings,
    menu: Option<&[MenuEntry]>,
    elevation: Option<&ElevationPolicy>,
) -> Vec<String> {
    let mut changes = Vec::new();
    for entry in menu.unwrap_or_default() {
        let MenuAction::Custom {
            executable,
            administrator,
        } = &entry.action
        else {
            continue;
        };
        let known = current.menu.iter().any(|old| {
            matches!(&old.action, MenuAction::Custom { executable: path, administrator: elevated }
                if path == executable && (*elevated || !*administrator))
        });
        if !known {
            let verb = if *administrator {
                "Run as administrator"
            } else {
                "Run from the menu"
            };
            changes.push(format!("{verb}: {executable}"));
        }
    }
    if let Some(policy) = elevation {
        if policy.mode > current.elevation.mode {
            changes.push(match policy.mode {
                ElevationMode::AllExecutables => "Any executable can run as administrator".into(),
                _ => "Allowlisted executables can run as administrator".into(),
            });
        }
        for path in policy.paths() {
            if !current.elevation.paths().any(|known| known == path) {
                changes.push(format!("Allow as administrator: {}", path.display()));
            }
        }
    }
    changes
}

/// `confirm` is asked before anything that widens what can run is saved.
pub fn update(
    session: &Session,
    patch: SettingsPatch,
    confirm: impl FnOnce(&[String]) -> bool,
) -> Result<SettingsView> {
    let appearance = patch.appearance;
    let preset = appearance
        .as_ref()
        .and_then(|patch| patch.preset.as_ref())
        .map(mirror::<_, Preset>)
        .transpose()?;
    let shape = appearance
        .as_ref()
        .and_then(|patch| patch.caption_shape.as_ref())
        .map(mirror::<_, CaptionShape>)
        .transpose()?;
    let palettes = appearance
        .as_ref()
        .and_then(|patch| patch.palettes.as_ref())
        .map(mirror::<_, BTreeMap<Preset, BTreeMap<ColorSlot, Option<Rgb>>>>)
        .transpose()?;
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
            if let MenuAction::Custom {
                executable,
                administrator,
            } = &entry.action
            {
                ensure!(
                    executable.len() <= EXECUTABLE_LENGTH,
                    "The executable path is too long"
                );
                validate_executable(Path::new(executable))?;
                ensure!(
                    !administrator || !is_network_path(Path::new(executable)),
                    "Only a program on this PC can run as administrator"
                );
            }
        }
    }
    let elevation = patch
        .elevation
        .map(|view| -> Result<_> {
            let mode: ElevationMode = mirror(view.mode)?;
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
    let changes = escalations(&session.settings(), menu.as_deref(), elevation.as_ref());
    ensure!(
        changes.is_empty() || confirm(&changes),
        "The change was not confirmed"
    );
    let updated = session.update_settings(move |settings| {
        if let Some(appearance) = appearance {
            if patch.migrate_appearance != Some(true) || !settings.theme_migrated {
                let current = &mut settings.appearance;
                if let Some(preset) = preset {
                    current.preset = preset;
                }
                if let Some(dark) = appearance.dark {
                    current.dark = dark;
                }
                if let Some(follow) = appearance.follow_windows {
                    current.follow_windows = follow;
                }
                if let Some(left) = appearance.caption_left {
                    current.caption_left = left;
                }
                if let Some(shape) = shape {
                    current.caption_shape = shape;
                }
                for (preset, changes) in palettes.into_iter().flatten() {
                    let palette = current.palettes.entry(preset).or_default();
                    for (slot, color) in changes {
                        if let Some(color) = color {
                            palette.insert(slot, color);
                        } else {
                            palette.remove(&slot);
                        }
                    }
                    if palette.is_empty() {
                        current.palettes.remove(&preset);
                    }
                }
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
    use anyhow::Context;
    use firewol_core::explorer::{FolderKey, FolderStyle, ItemId};

    #[test]
    fn palette_patches_migrate_merge_and_reset_independently() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        std::fs::write(
            fixture.path().join("explorer.json"),
            r#"{
            "version": 2, "theme_migrated": true,
            "appearance": {"preset":"glassmorphism","accent":[18,52,86],"caption_fill":[1,2,3]}
        }"#,
        )?;
        let session = Session::open(fixture.path().to_owned(), |_| {})?;
        let patch = |value| update(&session, serde_json::from_value(value)?, |_| true);
        let migrated = view(&session.settings())?;
        assert_eq!(migrated.version, 3);
        assert_eq!(migrated.appearance.palettes.len(), 1);
        assert_eq!(
            migrated.appearance.palettes["glassmorphism"]["accent"],
            [18, 52, 86]
        );
        assert_eq!(
            migrated.appearance.palettes["glassmorphism"]["caption_fill"],
            [1, 2, 3]
        );
        patch(
            serde_json::json!({"appearance": {"preset": "light_morphism", "palettes": {
                "light_morphism": {"accent": [4, 5, 6]}, "glassmorphism": {"muted": [7, 8, 9]}
            }}}),
        )?;
        let second = Session::open(fixture.path().to_owned(), |_| {})?;
        update(
            &second,
            serde_json::from_value(serde_json::json!({"appearance": {
                "palettes": {"light_morphism": {"folder_label": [10, 11, 12]}}
            }}))?,
            |_| true,
        )?;
        let saved = patch(serde_json::json!({"appearance": {"palettes": {
            "glassmorphism": {"accent": null, "caption_fill": null, "muted": null, "surface_2": null}
        }}}))?;
        assert_eq!(saved.appearance.preset, "light_morphism");
        assert!(!saved.appearance.palettes.contains_key("glassmorphism"));
        let reloaded = view(&Settings::load(fixture.path())?)?;
        assert_eq!(
            reloaded.appearance.palettes["light_morphism"]["accent"],
            [4, 5, 6]
        );
        assert_eq!(
            reloaded.appearance.palettes["light_morphism"]["folder_label"],
            [10, 11, 12]
        );
        Ok(())
    }

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
        let migrated = update(
            &session,
            SettingsPatch {
                appearance: Some(crate::contract::AppearancePatch {
                    dark: Some(true),
                    ..Default::default()
                }),
                migrate_appearance: Some(true),
                ..Default::default()
            },
            |_| true,
        )?;
        assert!(migrated.theme_migrated && migrated.appearance.dark);
        let again = update(
            &session,
            SettingsPatch {
                appearance: Some(crate::contract::AppearancePatch {
                    dark: Some(false),
                    ..Default::default()
                }),
                migrate_appearance: Some(true),
                sidebar: Some(false),
                ..Default::default()
            },
            |_| true,
        )?;
        assert!(again.appearance.dark && !again.sidebar && again.sidebar_width == 232);
        let wide = update(
            &session,
            SettingsPatch {
                sidebar_width: Some(300),
                ..Default::default()
            },
            |_| true,
        )?;
        assert_eq!(wide.sidebar_width, 300);
        let tall = update(
            &session,
            SettingsPatch {
                row: Some(52),
                ..Default::default()
            },
            |_| true,
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
                |_| true,
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
                |_| true,
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
            |_| false,
        );
        assert!(denied.is_err());
        Ok(())
    }

    fn allowlist(executables: Vec<String>) -> SettingsPatch {
        SettingsPatch {
            elevation: Some(ElevationView {
                mode: "allowlist".into(),
                executables,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn widening_the_policy_needs_the_person_at_the_pc() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let session = Session::open(fixture.path().to_owned(), |_| {})?;
        let root = std::env::var("SystemRoot")?;
        let tool = format!(r"{root}\System32\whoami.exe");
        let refused = update(&session, allowlist(vec![tool.clone()]), |changes| {
            assert!(changes.iter().any(|change| change.contains("whoami")));
            false
        })
        .err()
        .context("The refused change was saved")?;
        assert!(refused.to_string().contains("not confirmed"));
        let saved = Settings::load(fixture.path())?;
        assert_eq!(saved.elevation.mode, ElevationMode::Standard);
        assert_eq!(saved.elevation.paths().count(), 0);

        let allowed = update(&session, allowlist(vec![tool.clone()]), |_| true)?;
        assert_eq!(allowed.elevation.executables.len(), 1);

        // Saving the same policy, or narrowing it, never asks.
        update(&session, allowlist(vec![tool]), |_| panic!("no new access"))?;
        update(&session, allowlist(vec![]), |_| panic!("narrower"))?;
        let standard = SettingsPatch {
            elevation: Some(ElevationView {
                mode: "standard".into(),
                executables: vec![],
            }),
            ..Default::default()
        };
        update(&session, standard, |_| panic!("narrower"))?;
        Ok(())
    }

    #[test]
    fn only_new_or_raised_custom_actions_count_as_escalations() {
        let custom = |executable: &str, administrator| {
            MenuEntry::new(MenuAction::Custom {
                executable: executable.into(),
                administrator,
            })
        };
        let current = Settings {
            menu: vec![custom(r"C:\a.exe", false), custom(r"C:\b.exe", true)],
            ..Default::default()
        };
        let check = |menu: Vec<MenuEntry>| escalations(&current, Some(&menu), None).len();
        assert_eq!(check(current.menu.clone()), 0);
        assert_eq!(check(vec![custom(r"C:\b.exe", false)]), 0);
        assert_eq!(check(vec![custom(r"C:\a.exe", true)]), 1);
        assert_eq!(check(vec![custom(r"C:\new.exe", false)]), 1);
    }
}
