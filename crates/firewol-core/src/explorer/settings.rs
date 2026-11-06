use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::ItemId;

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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderStyle {
    pub alias: String,
    pub color: Option<Rgb>,
    pub icon: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuAction {
    Open,
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
    Custom { executable: String },
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

pub struct Settings {
    pub last_folder: Option<PathBuf>,
    pub pins: Vec<PathBuf>,
    pub sidebar: Option<bool>,
    pub appearance: Appearance,
    pub menu: Vec<MenuEntry>,
    pub folders: BTreeMap<FolderKey, FolderStyle>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_folder: None,
            pins: Vec::new(),
            sidebar: None,
            appearance: Appearance::default(),
            menu: default_menu(),
            folders: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    last_folder: Option<Vec<u16>>,
    #[serde(default)]
    pins: Vec<Vec<u16>>,
    #[serde(default)]
    sidebar: Option<bool>,
    #[serde(default)]
    appearance: Appearance,
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
        let stored: Stored = serde_json::from_reader(file).context("Read Firewol settings")?;
        let path = |units: Vec<u16>| PathBuf::from(std::ffi::OsString::from_wide(&units));
        Ok(Self {
            last_folder: stored.last_folder.map(path),
            pins: stored.pins.into_iter().map(path).collect(),
            sidebar: stored.sidebar,
            appearance: stored.appearance,
            menu: stored.menu,
            folders: stored.folders.into_iter().collect(),
        })
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
            last_folder: self.last_folder.as_ref().map(units),
            pins: self.pins.iter().map(units).collect(),
            sidebar: self.sidebar,
            appearance: self.appearance,
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
