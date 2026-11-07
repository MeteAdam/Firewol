use anyhow::{Result, ensure};
use firewol_core::explorer::elevation::{ElevationMode, ElevationPolicy, validate_executable};
use firewol_core::explorer::{
    Appearance, FolderStyle, ItemId, MenuAction, MenuEntry, Rgb, Session, Settings,
};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

use crate::contract::{ElevationView, FolderStyleView, SettingsPatch, SettingsView, StyledFolder};

fn mirror<T: Serialize, U: DeserializeOwned>(value: T) -> Result<U> {
    Ok(serde_json::from_value(serde_json::to_value(value)?)?)
}

pub fn view(settings: &Settings) -> Result<SettingsView> {
    Ok(SettingsView {
        version: settings.version,
        theme_migrated: settings.theme_migrated,
        sidebar: settings.sidebar.unwrap_or(true),
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
        assert!(again.appearance.dark && !again.sidebar);
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
