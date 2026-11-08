use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::ItemId;
use super::elevation::ElevationPolicy;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Neubrutalism,
    Skeuomorphism,
    Neumorphism,
    Glassmorphism,
    Claymorphism,
    #[default]
    LightMorphism,
    MetalMorphism,
}

impl Preset {
    pub const ALL: [Self; 7] = [
        Self::Neubrutalism,
        Self::Skeuomorphism,
        Self::Neumorphism,
        Self::Glassmorphism,
        Self::Claymorphism,
        Self::LightMorphism,
        Self::MetalMorphism,
    ];

    pub const NAMES: [&str; 7] = [
        "Neubrutalism",
        "Skeuomorphism",
        "Neumorphism",
        "Glassmorphism",
        "Claymorphism",
        "Light-morphism",
        "Metal-morphism",
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionShape {
    #[default]
    Rounded,
    Square,
    Circle,
    Minimal,
}

impl CaptionShape {
    pub const ALL: [Self; 4] = [Self::Rounded, Self::Square, Self::Circle, Self::Minimal];
    pub const NAMES: [&str; 4] = ["Rounded", "Square", "Circle", "Minimal"];
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub preset: Preset,
    pub dark: bool,
    pub follow_windows: bool,
    pub background: Option<Rgb>,
    pub surface: Option<Rgb>,
    pub text: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub folder: Option<Rgb>,
    pub folder_label: Option<Rgb>,
    pub caption_left: bool,
    pub caption_shape: CaptionShape,
    pub caption_fill: Option<Rgb>,
    pub caption_hover: Option<Rgb>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FolderKey {
    volume: u32,
    index: u64,
}

impl From<&ItemId> for FolderKey {
    fn from(item: &ItemId) -> Self {
        Self {
            volume: item.identity.volume,
            index: item.identity.index,
        }
    }
}

impl FolderKey {
    pub fn parts(self) -> (u32, u64) {
        (self.volume, self.index)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderStyle {
    pub alias: String,
    pub color: Option<Rgb>,
    pub icon: String,
    pub tags: Vec<String>,
}

impl FolderStyle {
    pub const MAX_ALIAS: usize = 100;
    pub const MAX_ICON: usize = 8;
    pub const MAX_TAG: usize = 32;
    pub const MAX_TAGS: usize = 8;

    /// Trims the text, drops empty and repeated tags and applies the Win32 editor's limits.
    /// A style that comes back equal to the default means "no customization".
    pub fn normalized(self) -> Result<Self> {
        let alias = self.alias.trim().to_owned();
        ensure!(
            alias.chars().count() <= Self::MAX_ALIAS,
            "Keep the display name within {} characters",
            Self::MAX_ALIAS
        );
        let icon = self.icon.trim().to_owned();
        ensure!(
            icon.chars().count() <= Self::MAX_ICON,
            "Keep the icon within {} characters",
            Self::MAX_ICON
        );
        let mut tags: Vec<String> = Vec::new();
        for tag in self.tags.iter().map(|tag| tag.trim()) {
            if tag.is_empty() || tags.iter().any(|existing| existing == tag) {
                continue;
            }
            ensure!(!tag.contains(','), "Tags cannot contain commas");
            ensure!(
                tag.chars().count() <= Self::MAX_TAG,
                "Keep each tag within {} characters",
                Self::MAX_TAG
            );
            tags.push(tag.to_owned());
        }
        ensure!(
            tags.len() <= Self::MAX_TAGS,
            "Use up to eight tags per folder"
        );
        Ok(Self {
            alias,
            color: self.color,
            icon,
            tags,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuAction {
    Open,
    RunAsAdministrator,
    Containing,
    Copy,
    Cut,
    Paste,
    CopyOther,
    MoveOther,
    Rename,
    Recycle,
    FolderStyle,
    Undo,
    Refresh,
    Custom {
        executable: String,
        #[serde(default)]
        administrator: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuEntry {
    pub action: MenuAction,
    pub visible: bool,
    pub label: String,
    pub icon: String,
    pub color: Option<Rgb>,
}

impl MenuEntry {
    pub fn new(action: MenuAction) -> Self {
        Self {
            action,
            visible: true,
            label: String::new(),
            icon: String::new(),
            color: None,
        }
    }
}

impl Default for MenuEntry {
    fn default() -> Self {
        Self::new(MenuAction::Open)
    }
}

pub fn default_menu() -> Vec<MenuEntry> {
    use MenuAction::*;
    [
        Open,
        RunAsAdministrator,
        Containing,
        Copy,
        Cut,
        Paste,
        CopyOther,
        MoveOther,
        Rename,
        Recycle,
        FolderStyle,
        Undo,
        Refresh,
    ]
    .into_iter()
    .map(MenuEntry::new)
    .collect()
}

#[derive(Clone)]
pub struct Settings {
    pub version: u32,
    pub theme_migrated: bool,
    pub last_folder: Option<PathBuf>,
    pub pins: Vec<PathBuf>,
    /// The Windows folders were added to the pins once; removing one later keeps it removed.
    pub pins_seeded: bool,
    pub sidebar: Option<bool>,
    /// Folder tabs show tiles instead of details.
    pub grid: Option<bool>,
    /// Grid tile edge in pixels: 96, 128 or 160.
    pub tile: Option<u32>,
    /// Sidebar width in pixels, 180 to 360.
    pub sidebar_width: Option<u32>,
    /// Details row height in pixels: 24, 32, 40 or 52.
    pub row: Option<u32>,
    /// The context menu lists commands from Windows and installed apps (default on).
    pub shell_items: Option<bool>,
    pub appearance: Appearance,
    pub elevation: ElevationPolicy,
    pub menu: Vec<MenuEntry>,
    pub folders: BTreeMap<FolderKey, FolderStyle>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 2,
            theme_migrated: false,
            last_folder: None,
            pins: Vec::new(),
            pins_seeded: false,
            sidebar: None,
            grid: None,
            tile: None,
            sidebar_width: None,
            row: None,
            shell_items: None,
            appearance: Appearance::default(),
            elevation: ElevationPolicy::default(),
            menu: default_menu(),
            folders: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    theme_migrated: bool,
    last_folder: Option<Vec<u16>>,
    #[serde(default)]
    pins: Vec<Vec<u16>>,
    #[serde(default)]
    pins_seeded: bool,
    #[serde(default)]
    sidebar: Option<bool>,
    #[serde(default)]
    grid: Option<bool>,
    #[serde(default)]
    tile: Option<u32>,
    #[serde(default)]
    sidebar_width: Option<u32>,
    #[serde(default)]
    row: Option<u32>,
    #[serde(default)]
    shell_items: Option<bool>,
    #[serde(default)]
    appearance: Appearance,
    #[serde(default)]
    elevation: ElevationPolicy,
    #[serde(default = "default_menu")]
    menu: Vec<MenuEntry>,
    #[serde(default)]
    folders: Vec<(FolderKey, FolderStyle)>,
}

impl Settings {
    pub fn load(dir: &Path) -> Result<Self> {
        let file = match File::open(dir.join("explorer.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error.into()),
        };
        let mut stored: Stored = serde_json::from_reader(file).context("Read Firewol settings")?;
        ensure!(
            stored.version <= 2,
            "These settings were saved by a newer Firewol version"
        );
        if !stored
            .menu
            .iter()
            .any(|entry| entry.action == MenuAction::RunAsAdministrator)
        {
            let index = stored
                .menu
                .iter()
                .position(|entry| entry.action == MenuAction::Open)
                .map_or(0, |index| index + 1);
            stored
                .menu
                .insert(index, MenuEntry::new(MenuAction::RunAsAdministrator));
        }
        let path = |units: Vec<u16>| PathBuf::from(std::ffi::OsString::from_wide(&units));
        Ok(Self {
            version: 2,
            theme_migrated: stored.theme_migrated,
            last_folder: stored.last_folder.map(path),
            pins: stored.pins.into_iter().map(path).collect(),
            pins_seeded: stored.pins_seeded,
            sidebar: stored.sidebar,
            grid: stored.grid,
            tile: stored.tile,
            sidebar_width: stored.sidebar_width,
            row: stored.row,
            shell_items: stored.shell_items,
            appearance: stored.appearance,
            elevation: stored.elevation,
            menu: stored.menu,
            folders: stored.folders.into_iter().collect(),
        })
    }

    /// Puts `folders` first, in order, once per profile. The user's other pins stay after them
    /// and a folder that is already pinned moves into place instead of repeating.
    pub fn seed_pins(&mut self, folders: &[PathBuf]) {
        if self.pins_seeded {
            return;
        }
        let same = |left: &Path, right: &Path| {
            left.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
        };
        let mut rest = std::mem::take(&mut self.pins);
        let mut pins = Vec::with_capacity(folders.len() + rest.len());
        for folder in folders {
            match rest.iter().position(|pin| same(pin, folder)) {
                Some(index) => pins.push(rest.remove(index)),
                None => pins.push(folder.clone()),
            }
        }
        pins.append(&mut rest);
        self.pins = pins;
        self.pins_seeded = true;
    }

    pub fn update(dir: &Path, apply: impl FnOnce(&mut Self)) -> Result<Self> {
        fs::create_dir_all(dir)?;
        // Merge independent changes made by separately launched Firewol processes.
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("explorer.lock"))?;
        lock.lock().context("Lock Firewol settings")?;
        let mut settings = Self::load(dir)?;
        apply(&mut settings);
        settings.save(dir)?;
        Ok(settings)
    }

    fn save(&self, dir: &Path) -> Result<()> {
        let units = |path: &PathBuf| path.as_os_str().encode_wide().collect();
        let stored = Stored {
            version: 2,
            theme_migrated: self.theme_migrated,
            last_folder: self.last_folder.as_ref().map(units),
            pins: self.pins.iter().map(units).collect(),
            pins_seeded: self.pins_seeded,
            sidebar: self.sidebar,
            grid: self.grid,
            tile: self.tile,
            sidebar_width: self.sidebar_width,
            row: self.row,
            shell_items: self.shell_items,
            appearance: self.appearance,
            elevation: self.elevation.clone(),
            menu: self.menu.clone(),
            folders: self
                .folders
                .iter()
                .map(|(key, style)| (*key, style.clone()))
                .collect(),
        };
        let file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(file.as_file(), &stored)?;
        file.persist(dir.join("explorer.json"))
            .context("Save Firewol settings")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pins_are_added_once_in_order_without_repeats() {
        let known: Vec<PathBuf> = ["D:\\Desktop", "D:\\Documents", "D:\\Music"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let mut settings = Settings {
            pins: vec![PathBuf::from("C:\\"), PathBuf::from("d:\\documents")],
            ..Settings::default()
        };
        settings.seed_pins(&known);
        let expected = ["D:\\Desktop", "d:\\documents", "D:\\Music", "C:\\"];
        assert_eq!(settings.pins, expected.map(PathBuf::from));
        assert!(settings.pins_seeded);
        // A pin the user removes afterwards does not come back.
        settings.pins.remove(0);
        settings.seed_pins(&known);
        assert_eq!(settings.pins.len(), 3);
        assert_eq!(settings.pins[0], PathBuf::from("d:\\documents"));
    }
}
