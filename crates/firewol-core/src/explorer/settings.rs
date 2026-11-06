use std::fs::{self, File};
use std::io::ErrorKind;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Default)]
pub struct Settings {
    pub last_folder: Option<PathBuf>,
    pub pins: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    last_folder: Option<Vec<u16>>,
    pins: Vec<Vec<u16>>,
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
        })
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;
        let units = |path: &PathBuf| path.as_os_str().encode_wide().collect();
        let stored = Stored {
            last_folder: self.last_folder.as_ref().map(units),
            pins: self.pins.iter().map(units).collect(),
        };
        let file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(file.as_file(), &stored)?;
        file.persist(dir.join("explorer.json"))
            .context("Save Firewol settings")?;
        Ok(())
    }
}
