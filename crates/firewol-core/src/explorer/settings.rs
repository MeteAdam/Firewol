use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, ErrorKind, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::ItemId;
use super::elevation::ElevationPolicy;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSlot {
    Text,
    Muted,
    FolderLabel,
    Danger,
    Background,
    Surface,
    #[serde(rename = "surface_2")]
    Surface2,
    Field,
    Border,
    Titlebar,
    TabActive,
    TabActiveText,
    TabInactive,
    TabInactiveText,
    CaptionFill,
    CaptionText,
    CaptionHover,
    Accent,
    OnAccent,
    Folder,
    Selection,
    SelectionText,
    Hover,
}

pub type Palette = BTreeMap<ColorSlot, Rgb>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub preset: Preset,
    pub dark: bool,
    pub follow_windows: bool,
    pub caption_left: bool,
    pub caption_shape: CaptionShape,
    pub palettes: BTreeMap<Preset, Palette>,
    #[serde(flatten, skip_serializing)]
    legacy: LegacyColors,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyColors {
    background: Option<Rgb>,
    surface: Option<Rgb>,
    text: Option<Rgb>,
    accent: Option<Rgb>,
    folder: Option<Rgb>,
    folder_label: Option<Rgb>,
    caption_fill: Option<Rgb>,
    caption_hover: Option<Rgb>,
}

impl Appearance {
    fn migrate_colors(&mut self) {
        let legacy = std::mem::take(&mut self.legacy);
        let colors: Palette = [
            (ColorSlot::Background, legacy.background),
            (ColorSlot::Surface, legacy.surface),
            (ColorSlot::Text, legacy.text),
            (ColorSlot::Accent, legacy.accent),
            (ColorSlot::Folder, legacy.folder),
            (ColorSlot::FolderLabel, legacy.folder_label),
            (ColorSlot::CaptionFill, legacy.caption_fill),
            (ColorSlot::CaptionHover, legacy.caption_hover),
        ]
        .into_iter()
        .filter_map(|(slot, color)| color.map(|color| (slot, color)))
        .collect();
        if !colors.is_empty() {
            self.palettes.entry(self.preset).or_insert(colors);
        }
    }
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

/// How one folder is shown. A folder without one opens in details.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    pub grid: bool,
    /// Tile edge in pixels: 96, 128 or 160.
    pub tile: u32,
    /// Sections by date modified.
    #[serde(default)]
    pub group: bool,
}

/// Details column widths in pixels, shared by every folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnWidths {
    pub name: u32,
    pub location: u32,
    pub modified: u32,
    pub kind: u32,
    pub size: u32,
    pub tags: u32,
}

impl Default for ColumnWidths {
    fn default() -> Self {
        Self {
            name: 420,
            location: 320,
            modified: 150,
            kind: 128,
            size: 84,
            tags: 168,
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
        Cut,
        Copy,
        Paste,
        CopyOther,
        MoveOther,
        Recycle,
        Rename,
        FolderStyle,
        Undo,
        Refresh,
    ]
    .into_iter()
    .map(MenuEntry::new)
    .collect()
}

/// A menu still in the earlier default order takes the new one; a reordered menu is left alone.
fn adopt_default_order(menu: &mut [MenuEntry]) {
    use MenuAction::*;
    let earlier = [Copy, Cut, Paste, CopyOther, MoveOther, Rename, Recycle];
    let current = [Cut, Copy, Paste, CopyOther, MoveOther, Recycle, Rename];
    let positions: Vec<usize> = (0..menu.len())
        .filter(|&index| earlier.contains(&menu[index].action))
        .collect();
    let unchanged = positions.len() == earlier.len()
        && positions
            .iter()
            .zip(&earlier)
            .all(|(&index, action)| menu[index].action == *action);
    if !unchanged {
        return;
    }
    let entry = |action: &MenuAction| {
        menu.iter()
            .find(|entry| entry.action == *action)
            .cloned()
            .unwrap_or_default()
    };
    let reordered: Vec<MenuEntry> = current.iter().map(entry).collect();
    for (index, entry) in positions.into_iter().zip(reordered) {
        menu[index] = entry;
    }
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
    /// Folders list items marked hidden or system (default on). Firewol's own view only.
    pub show_hidden: Option<bool>,
    /// File names show their extension (default on). Firewol's own view only.
    pub show_extensions: Option<bool>,
    pub columns: Option<ColumnWidths>,
    pub appearance: Appearance,
    pub elevation: ElevationPolicy,
    pub menu: Vec<MenuEntry>,
    pub folders: BTreeMap<FolderKey, FolderStyle>,
    /// Each folder's own view, by the same identity as its style.
    pub views: BTreeMap<FolderKey, SavedView>,
    /// Pictures and Videos were set to tiles once; a later choice stays.
    pub views_seeded: bool,
    /// Downloads was grouped by date once; a later choice stays.
    pub groups_seeded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 3,
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
            show_hidden: None,
            show_extensions: None,
            columns: None,
            appearance: Appearance::default(),
            elevation: ElevationPolicy::default(),
            menu: default_menu(),
            folders: BTreeMap::new(),
            views: BTreeMap::new(),
            views_seeded: false,
            groups_seeded: false,
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
    show_hidden: Option<bool>,
    #[serde(default)]
    show_extensions: Option<bool>,
    #[serde(default)]
    columns: Option<ColumnWidths>,
    #[serde(default)]
    appearance: Appearance,
    #[serde(default)]
    elevation: ElevationPolicy,
    #[serde(default = "default_menu")]
    menu: Vec<MenuEntry>,
    #[serde(default)]
    folders: Vec<(FolderKey, FolderStyle)>,
    #[serde(default)]
    views: Vec<(FolderKey, SavedView)>,
    #[serde(default)]
    views_seeded: bool,
    #[serde(default)]
    groups_seeded: bool,
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
            stored.version <= 3,
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
        adopt_default_order(&mut stored.menu);
        if stored.version < 3 {
            stored.appearance.migrate_colors();
        }
        let path = |units: Vec<u16>| PathBuf::from(std::ffi::OsString::from_wide(&units));
        Ok(Self {
            version: 3,
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
            show_hidden: stored.show_hidden,
            show_extensions: stored.show_extensions,
            columns: stored.columns,
            appearance: stored.appearance,
            elevation: stored.elevation,
            menu: stored.menu,
            folders: stored.folders.into_iter().collect(),
            views: stored.views.into_iter().collect(),
            views_seeded: stored.views_seeded,
            groups_seeded: stored.groups_seeded,
        })
    }

    /// For startup. A file that cannot be parsed, as a crash can leave one, is set aside so
    /// Firewol still opens; `true` reports that. Settings from a newer version stay an error.
    pub fn load_or_recover(dir: &Path) -> Result<(Self, bool)> {
        match Self::load(dir) {
            Err(error)
                if error
                    .downcast_ref::<serde_json::Error>()
                    .is_some_and(|parse| !parse.is_io()) =>
            {
                fs::rename(dir.join("explorer.json"), dir.join("explorer.json.corrupt"))
                    .context("Set the unreadable settings aside")?;
                Ok((Self::default(), true))
            }
            loaded => loaded.map(|settings| (settings, false)),
        }
    }

    /// Moves a pin so it sits before `before`, or last when `before` is `None`. Unknown paths
    /// and a move onto itself change nothing.
    pub fn move_pin(&mut self, folder: &Path, before: Option<&Path>) {
        let Some(from) = self.pins.iter().position(|pin| pin == folder) else {
            return;
        };
        if before == Some(folder)
            || before.is_some_and(|path| !self.pins.iter().any(|pin| pin == path))
        {
            return;
        }
        let pin = self.pins.remove(from);
        let to = before
            .and_then(|path| self.pins.iter().position(|pin| pin == path))
            .unwrap_or(self.pins.len());
        self.pins.insert(to, pin);
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
            version: 3,
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
            show_hidden: self.show_hidden,
            show_extensions: self.show_extensions,
            columns: self.columns,
            appearance: self.appearance.clone(),
            elevation: self.elevation.clone(),
            menu: self.menu.clone(),
            folders: self
                .folders
                .iter()
                .map(|(key, style)| (*key, style.clone()))
                .collect(),
            views: self.views.iter().map(|(key, view)| (*key, *view)).collect(),
            views_seeded: self.views_seeded,
            groups_seeded: self.groups_seeded,
        };
        let file = tempfile::NamedTempFile::new_in(dir)?;
        let mut writer = BufWriter::new(file.as_file());
        serde_json::to_writer(&mut writer, &stored)?;
        writer.flush()?;
        drop(writer);
        // Without this the rename can reach the disk before the data, and a crash leaves a hole.
        file.as_file().sync_all()?;
        file.persist(dir.join("explorer.json"))
            .context("Save Firewol settings")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_folder_keeps_its_own_view_across_a_reload() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let folder = |name: &str| -> Result<ItemId> {
            let path = fixture.path().join(name);
            fs::create_dir(&path)?;
            ItemId::at(path)
        };
        let (photos, code) = (folder("photos")?, folder("code")?);
        let tiles = SavedView {
            grid: true,
            tile: 96,
            group: false,
        };
        let data = fixture.path().join("data");
        Settings::update(&data, |settings| {
            settings.views.insert(FolderKey::from(&photos), tiles);
        })?;
        let loaded = Settings::load(&data)?;
        assert_eq!(loaded.views.get(&FolderKey::from(&photos)), Some(&tiles));
        assert_eq!(loaded.views.get(&FolderKey::from(&code)), None);
        Ok(())
    }

    #[test]
    fn only_a_menu_in_the_earlier_default_order_is_reordered() {
        use MenuAction::*;
        let menu = |actions: Vec<MenuAction>| -> Vec<MenuEntry> {
            actions.into_iter().map(MenuEntry::new).collect()
        };
        let actions = |menu: &[MenuEntry]| -> Vec<MenuAction> {
            menu.iter().map(|entry| entry.action.clone()).collect()
        };
        let mut earlier = menu(vec![
            Open, Copy, Cut, Paste, CopyOther, MoveOther, Rename, Recycle, Undo,
        ]);
        earlier[1].label = "Duplicate".into();
        adopt_default_order(&mut earlier);
        assert_eq!(
            actions(&earlier),
            [
                Open, Cut, Copy, Paste, CopyOther, MoveOther, Recycle, Rename, Undo
            ]
        );
        let copy = earlier.iter().find(|entry| entry.action == Copy);
        assert_eq!(copy.map(|entry| entry.label.as_str()), Some("Duplicate"));
        let chosen = vec![Rename, Copy, Cut, Paste, CopyOther, MoveOther, Recycle];
        let mut reordered = menu(chosen.clone());
        adopt_default_order(&mut reordered);
        assert_eq!(actions(&reordered), chosen);
    }

    #[test]
    fn a_pin_moves_before_another_or_to_the_end() {
        let path = |name: &str| PathBuf::from(format!("D:\\{name}"));
        let names = |settings: &Settings| {
            settings
                .pins
                .iter()
                .map(|pin| pin.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("")
        };
        let mut settings = Settings {
            pins: ["a", "b", "c", "d"].map(path).to_vec(),
            ..Settings::default()
        };
        settings.move_pin(&path("a"), Some(&path("d")));
        assert_eq!(names(&settings), "bcad");
        settings.move_pin(&path("d"), Some(&path("b")));
        assert_eq!(names(&settings), "dbca");
        settings.move_pin(&path("d"), None);
        assert_eq!(names(&settings), "bcad");
        settings.move_pin(&path("c"), Some(&path("c")));
        settings.move_pin(&path("c"), Some(&path("missing")));
        settings.move_pin(&path("missing"), None);
        assert_eq!(names(&settings), "bcad");
    }

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
