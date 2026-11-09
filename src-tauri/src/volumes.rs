//! NTFS volume indexes shared by every window. Each drive has at most one index and one
//! elevated read-only helper; it runs until the user stops it or Firewol exits.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use firewol_core::explorer::{Cancellation, SearchIndex, SearchReader, data_dir};

use crate::contract::VolumeState;
use crate::folders::lock;

pub type VolumeNotify = Arc<dyn Fn(VolumeState) + Send + Sync>;

const LAUNCH_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Volumes {
    drives: Mutex<HashMap<char, Arc<Volume>>>,
    launches: Mutex<LaunchGate>,
}

/// A stopped index can still be waiting on its UAC prompt, so stopping and starting again must not
/// stack another prompt. One launch per drive at a time, and never two in quick succession.
#[derive(Default)]
struct LaunchGate {
    active: HashSet<char>,
    last: HashMap<char, Instant>,
}

impl LaunchGate {
    fn begin(&mut self, drive: char, now: Instant) -> Result<()> {
        anyhow::ensure!(
            !self.active.contains(&drive),
            "The {drive}: index is still stopping. Try again in a moment."
        );
        anyhow::ensure!(
            self.last
                .get(&drive)
                .is_none_or(|last| now.duration_since(*last) >= LAUNCH_INTERVAL),
            "Wait a moment before starting the {drive}: index again."
        );
        self.active.insert(drive);
        self.last.insert(drive, now);
        Ok(())
    }

    fn finish(&mut self, drive: char) {
        self.active.remove(&drive);
    }
}

struct Volume {
    drive: char,
    index: SearchIndex,
    cancel: Cancellation,
    ready: Mutex<Option<u64>>,
    reader: Mutex<Option<SearchReader>>,
}

impl Volume {
    fn state(&self) -> VolumeState {
        let ready = *lock(&self.ready);
        VolumeState {
            drive: self.drive.to_string(),
            ready: ready.is_some(),
            running: true,
            count: ready.unwrap_or(0),
            error: None,
        }
    }
}

/// Parses the single drive letter the interface sends.
pub fn drive(text: &str) -> Result<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), None) if drive.is_ascii_alphabetic() => Ok(drive.to_ascii_uppercase()),
        _ => anyhow::bail!("Enter a drive letter"),
    }
}

impl Volumes {
    pub fn states(&self) -> Vec<VolumeState> {
        lock(&self.drives)
            .values()
            .map(|volume| volume.state())
            .collect()
    }

    /// Starts indexing `drive` (Windows asks for administrator approval) unless it already runs.
    pub fn start(self: &Arc<Self>, drive: char, notify: VolumeNotify) -> Result<VolumeState> {
        let mut drives = lock(&self.drives);
        if let Some(volume) = drives.get(&drive) {
            return Ok(volume.state());
        }
        lock(&self.launches).begin(drive, Instant::now())?;
        let created = data_dir().and_then(|data| Ok((SearchIndex::create(&data)?, data)));
        let (index, data) = match created {
            Ok(created) => created,
            Err(error) => {
                lock(&self.launches).finish(drive);
                return Err(error);
            }
        };
        let volume = Arc::new(Volume {
            drive,
            index,
            cancel: Cancellation::default(),
            ready: Mutex::default(),
            reader: Mutex::default(),
        });
        drives.insert(drive, volume.clone());
        drop(drives);
        let state = volume.state();
        let volumes = self.clone();
        thread::spawn(move || {
            let result = volume
                .index
                .watch_volume(drive, &data, &volume.cancel, |count| {
                    *lock(&volume.ready) = Some(count);
                    notify(volume.state());
                });
            let stopped = volume.cancel.is_cancelled();
            volumes.forget(&volume);
            lock(&volumes.launches).finish(drive);
            notify(VolumeState {
                ready: false,
                running: false,
                count: 0,
                error: result
                    .err()
                    .filter(|_| !stopped)
                    .map(|error| format!("{error:#}")),
                ..volume.state()
            });
        });
        Ok(state)
    }

    pub fn stop(&self, drive: char) {
        if let Some(volume) = lock(&self.drives).remove(&drive) {
            volume.cancel.cancel();
        }
    }

    /// Removes `volume` unless a newer index for the same drive replaced it.
    fn forget(&self, volume: &Arc<Volume>) {
        let mut drives = lock(&self.drives);
        if drives
            .get(&volume.drive)
            .is_some_and(|current| Arc::ptr_eq(current, volume))
        {
            drives.remove(&volume.drive);
        }
    }

    /// Runs `read` with the drive's reader once its first index pass has finished.
    pub fn read<T>(
        &self,
        drive: char,
        read: impl FnOnce(&mut SearchReader) -> Result<T>,
    ) -> Result<T> {
        let volume = lock(&self.drives)
            .get(&drive)
            .cloned()
            .with_context(|| format!("Volume search for {drive}: is not running"))?;
        anyhow::ensure!(
            lock(&volume.ready).is_some(),
            "The {drive}: index is not ready"
        );
        let mut reader = lock(&volume.reader);
        if reader.is_none() {
            *reader = Some(volume.index.reader()?);
        }
        read(reader.as_mut().context("Open the volume index")?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launches_never_stack_for_a_drive() -> Result<()> {
        let mut gate = LaunchGate::default();
        let start = Instant::now();
        gate.begin('C', start)?;
        assert!(gate.begin('C', start + LAUNCH_INTERVAL * 2).is_err());
        gate.begin('D', start)?;
        gate.finish('C');
        assert!(gate.begin('C', start + LAUNCH_INTERVAL / 2).is_err());
        gate.begin('C', start + LAUNCH_INTERVAL)?;
        Ok(())
    }
}
