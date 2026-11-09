//! State shared by every Firewol window, independent of the interface toolkit:
//! navigation history, the in-app clipboard, one file operation at a time and settings.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, ensure};

use super::{
    FolderKey, FolderStyle, ItemId, MAX_TRANSFER_ITEMS, Operation, OperationResult, Operations,
    SavedView, Settings, TransferControl, TransferProgress, TransferRequest,
};

const HISTORY_LIMIT: usize = 64;
const MAX_PINS: usize = 256;
const MAX_FOLDER_ENTRIES: usize = 4096;
const FOLDER_SAVE_DELAY: Duration = Duration::from_secs(2);
const MEDIA_VIEW: SavedView = SavedView {
    grid: true,
    tile: 128,
    group: false,
};
/// A folder that has only been grouped opens in details.
const DOWNLOADS_VIEW: SavedView = SavedView {
    grid: false,
    tile: 128,
    group: true,
};

/// Back and forward history of one pane.
#[derive(Clone, Debug, Default)]
pub struct History {
    folders: Vec<PathBuf>,
    position: usize,
}

impl History {
    pub fn current(&self) -> Option<&Path> {
        self.folders.get(self.position).map(PathBuf::as_path)
    }

    /// Records a folder that opened from a new navigation and drops the forward entries.
    pub fn visit(&mut self, path: PathBuf) {
        if self.current() == Some(path.as_path()) {
            return;
        }
        if !self.folders.is_empty() {
            self.folders.truncate(self.position + 1);
        }
        self.folders.push(path);
        if self.folders.len() > HISTORY_LIMIT {
            self.folders.remove(0);
        }
        self.position = self.folders.len() - 1;
    }

    /// The entry one step back or forward, without moving there.
    pub fn peek(&self, forward: bool) -> Option<(usize, &Path)> {
        let position = if forward {
            self.position.checked_add(1)?
        } else {
            self.position.checked_sub(1)?
        };
        self.folders
            .get(position)
            .map(|path| (position, path.as_path()))
    }

    /// Moves to an entry from [`History::peek`] once its folder has opened.
    pub fn go(&mut self, position: usize) {
        if position < self.folders.len() {
            self.position = position;
        }
    }
}

/// Items copied or cut inside Firewol; they are pasted by identity, not by name.
#[derive(Clone, Debug)]
pub struct Clipboard {
    pub items: Vec<ItemId>,
    pub cut: bool,
}

/// The outcome of a file operation, delivered to the notifier on the worker thread.
#[derive(Clone, Debug)]
pub struct Completion {
    pub message: String,
    pub succeeded: bool,
    /// Folder views should reload: the operation succeeded or a transfer ran at least partly.
    pub refresh: bool,
}

enum Job {
    Run(Operation),
    Transfer(TransferRequest),
    Undo,
}

struct Request {
    job: Job,
    control: TransferControl,
    consume_cut: bool,
}

struct Running {
    control: TransferControl,
    transfer: bool,
}

pub struct Session {
    data: PathBuf,
    recovered: bool,
    pending_folder: Mutex<Option<PathBuf>>,
    saving_folder: AtomicBool,
    settings: Mutex<Settings>,
    settings_write: Mutex<()>,
    clipboard: Arc<Mutex<Option<Clipboard>>>,
    running: Arc<Mutex<Option<Running>>>,
    jobs: Sender<Request>,
}

impl Session {
    /// Loads settings from `data` and starts the file worker, which reports to `notify`.
    pub fn open(data: PathBuf, notify: impl Fn(Completion) + Send + 'static) -> Result<Self> {
        let (settings, recovered) = Settings::load_or_recover(&data)?;
        let clipboard = Arc::new(Mutex::new(None::<Clipboard>));
        let running = Arc::new(Mutex::new(None::<Running>));
        let (jobs, requests) = mpsc::channel::<Request>();
        let worker_clipboard = Arc::clone(&clipboard);
        let worker_running = Arc::clone(&running);
        thread::Builder::new()
            .name("firewol-files".into())
            .spawn(move || {
                // The Shell operations need this thread's COM apartment.
                let mut operations = None;
                while let Ok(request) = requests.recv() {
                    let transfer = !matches!(request.job, Job::Run(_));
                    let result = match operations.get_or_insert_with(Operations::open) {
                        Ok(operations) => run(operations, request.job, &request.control),
                        Err(error) => {
                            Err(anyhow::anyhow!("Start the Windows Shell worker: {error:#}"))
                        }
                    };
                    if request.consume_cut
                        && let Ok(result) = &result
                    {
                        let mut clipboard = lock(&worker_clipboard);
                        if let Some(cut) = clipboard.as_mut() {
                            cut.items.retain(|item| !result.transferred.contains(item));
                            if cut.items.is_empty() {
                                *clipboard = None;
                            }
                        }
                    }
                    lock(&worker_running).take();
                    notify(match result {
                        Ok(result) => Completion {
                            message: result.message,
                            succeeded: true,
                            refresh: true,
                        },
                        Err(error) => Completion {
                            message: format!("{error:#}"),
                            succeeded: false,
                            refresh: transfer,
                        },
                    });
                }
            })
            .context("Start the file operation worker")?;
        Ok(Self {
            data,
            recovered,
            pending_folder: Mutex::default(),
            saving_folder: AtomicBool::default(),
            settings: Mutex::new(settings),
            settings_write: Mutex::new(()),
            clipboard,
            running,
            jobs,
        })
    }

    /// True when unreadable settings were set aside at startup and defaults are in use.
    pub fn settings_recovered(&self) -> bool {
        self.recovered
    }

    /// Notes the folder to reopen next time. Saved a moment later, so browsing quickly writes
    /// once, and best effort, because a settings file in use must not fail a navigation.
    pub fn remember_folder(self: &Arc<Self>, folder: PathBuf) {
        *lock(&self.pending_folder) = Some(folder);
        if !self.saving_folder.swap(true, Ordering::AcqRel) {
            let session = Arc::clone(self);
            thread::spawn(move || {
                thread::sleep(FOLDER_SAVE_DELAY);
                session.saving_folder.store(false, Ordering::Release);
                session.flush_folder();
            });
        }
    }

    /// Writes a remembered folder now; called when the last window closes.
    pub fn flush_folder(&self) {
        let Some(folder) = lock(&self.pending_folder).take() else {
            return;
        };
        if self.settings().last_folder.as_ref() != Some(&folder) {
            let _ = self.update_settings(|settings| settings.last_folder = Some(folder));
        }
    }

    /// Settings as they are on disk now. For decisions that another running Firewol process may
    /// have just tightened, such as who can run elevated.
    pub fn fresh_settings(&self) -> Result<Settings> {
        Settings::load(&self.data)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    /// Applies a change and saves it, merging edits made by other Firewol processes.
    pub fn update_settings(&self, apply: impl FnOnce(&mut Settings)) -> Result<Settings> {
        // Preserve write order in the in-memory snapshot as well as on disk.
        let _write = lock(&self.settings_write);
        let settings = Settings::update(&self.data, apply)?;
        *lock(&self.settings) = settings.clone();
        Ok(settings)
    }

    /// Saves the alias, colour, icon and tags of a folder, keyed by its identity so they follow
    /// renames and moves on the same volume. A default (empty) style removes the customization.
    pub fn set_folder_style(&self, folder: &ItemId, style: FolderStyle) -> Result<Settings> {
        let style = style.normalized()?;
        let file = folder.verify()?;
        ensure!(file.metadata()?.is_dir(), "Select a folder to customize it");
        let key = FolderKey::from(folder);
        let current = self.settings();
        ensure!(
            current.folders.len() < MAX_FOLDER_ENTRIES
                || current.folders.contains_key(&key)
                || style == FolderStyle::default(),
            "Use up to {MAX_FOLDER_ENTRIES} customized folders"
        );
        self.update_settings(|settings| {
            if style == FolderStyle::default() {
                settings.folders.remove(&key);
            } else {
                settings.folders.insert(key, style);
            }
        })
    }

    /// Adds the Windows user folders to the pins the first time this profile runs.
    pub fn seed_pins(&self) -> Result<()> {
        if self.settings().pins_seeded {
            return Ok(());
        }
        let known = super::drives::known_folders();
        self.update_settings(|settings| settings.seed_pins(&known))
            .map(drop)
    }

    /// Remembers how this folder is shown. The identity keeps it through renames and moves.
    pub fn set_folder_view(&self, folder: &ItemId, view: SavedView) -> Result<()> {
        ensure!(
            folder.verify()?.metadata()?.is_dir(),
            "Only folders have a view"
        );
        let key = FolderKey::from(folder);
        self.update_settings(|settings| {
            // A remembered view is a convenience, so the oldest key simply makes room.
            if settings.views.len() >= MAX_FOLDER_ENTRIES && !settings.views.contains_key(&key) {
                settings.views.pop_first();
            }
            settings.views.insert(key, view);
        })
        .map(drop)
    }

    /// Starts Pictures and Videos as tiles, once per profile.
    pub fn seed_views(&self) -> Result<()> {
        if self.settings().views_seeded {
            return Ok(());
        }
        let keys: Vec<FolderKey> = super::drives::media_folders()
            .into_iter()
            .filter_map(|folder| ItemId::at(folder).ok())
            .map(|folder| FolderKey::from(&folder))
            .collect();
        self.update_settings(|settings| {
            for key in keys {
                settings.views.entry(key).or_insert(MEDIA_VIEW);
            }
            settings.views_seeded = true;
        })
        .map(drop)
    }

    /// Groups Downloads by date once, as Explorer shows it; a later choice stays.
    pub fn seed_groups(&self) -> Result<()> {
        if self.settings().groups_seeded {
            return Ok(());
        }
        let key = super::drives::downloads_folder()
            .and_then(|folder| ItemId::at(folder).ok())
            .map(|folder| FolderKey::from(&folder));
        self.update_settings(|settings| {
            if let Some(key) = key {
                settings.views.entry(key).or_insert(DOWNLOADS_VIEW).group = true;
            }
            settings.groups_seeded = true;
        })
        .map(drop)
    }

    /// Moves a pinned folder before `before`, or to the end of the pins when that is `None`.
    pub fn move_pin(&self, folder: &Path, before: Option<&Path>) -> Result<()> {
        self.update_settings(|settings| settings.move_pin(folder, before))
            .map(drop)
    }

    /// Pins the folder, or unpins it when it is already pinned. Returns whether it is pinned.
    pub fn toggle_pin(&self, folder: &Path) -> Result<bool> {
        let settings = self.settings();
        if !settings.pins.iter().any(|pin| pin == folder) {
            ensure!(settings.pins.len() < MAX_PINS, "Use up to {MAX_PINS} pins");
            ensure!(
                std::fs::metadata(folder).is_ok_and(|metadata| metadata.is_dir()),
                "Only an existing folder can be pinned"
            );
        }
        let mut pinned = false;
        self.update_settings(|settings| {
            if let Some(index) = settings.pins.iter().position(|pin| pin == folder) {
                settings.pins.remove(index);
            } else {
                settings.pins.push(folder.to_owned());
                pinned = true;
            }
        })?;
        Ok(pinned)
    }

    pub fn set_clipboard(&self, items: Vec<ItemId>, cut: bool) -> Result<()> {
        ensure!(!self.busy(), "Wait for the current operation to finish");
        ensure!(
            (1..=MAX_TRANSFER_ITEMS).contains(&items.len()),
            "Select between 1 and {MAX_TRANSFER_ITEMS} items"
        );
        *lock(&self.clipboard) = Some(Clipboard { items, cut });
        Ok(())
    }

    pub fn clipboard(&self) -> Option<Clipboard> {
        lock(&self.clipboard).clone()
    }

    /// Copies or moves the clipboard items into `into`. A move empties the clipboard
    /// of every item that arrived.
    pub fn paste(&self, into: ItemId) -> Result<TransferControl> {
        let clipboard = self
            .clipboard()
            .context("Copy or cut an item in Firewol first")?;
        let cut = clipboard.cut;
        self.submit(
            Job::Transfer(TransferRequest {
                items: clipboard.items,
                into,
                move_items: cut,
            }),
            cut,
        )
    }

    pub fn transfer(
        &self,
        items: Vec<ItemId>,
        into: ItemId,
        move_items: bool,
    ) -> Result<TransferControl> {
        ensure!(
            (1..=MAX_TRANSFER_ITEMS).contains(&items.len()),
            "Select between 1 and {MAX_TRANSFER_ITEMS} items to transfer"
        );
        self.submit(
            Job::Transfer(TransferRequest {
                items,
                into,
                move_items,
            }),
            false,
        )
    }

    pub fn run(&self, operation: Operation) -> Result<()> {
        self.submit(Job::Run(operation), false).map(drop)
    }

    /// Reverses the most recent reversible operation of this session.
    pub fn undo(&self) -> Result<TransferControl> {
        self.submit(Job::Undo, false)
    }

    pub fn busy(&self) -> bool {
        lock(&self.running).is_some()
    }

    /// Progress of the running transfer or undo, if one is running.
    pub fn progress(&self) -> Option<TransferProgress> {
        lock(&self.running)
            .as_ref()
            .filter(|running| running.transfer)
            .map(|running| running.control.progress())
    }

    /// The pause and cancel handle of the running transfer or undo.
    pub fn control(&self) -> Option<TransferControl> {
        lock(&self.running)
            .as_ref()
            .filter(|running| running.transfer)
            .map(|running| running.control.clone())
    }

    fn submit(&self, job: Job, consume_cut: bool) -> Result<TransferControl> {
        let mut running = lock(&self.running);
        ensure!(running.is_none(), "A file operation is already running");
        let control = TransferControl::default();
        *running = Some(Running {
            control: control.clone(),
            transfer: !matches!(job, Job::Run(_)),
        });
        if self
            .jobs
            .send(Request {
                job,
                control: control.clone(),
                consume_cut,
            })
            .is_err()
        {
            *running = None;
            anyhow::bail!("The file operation worker stopped");
        }
        Ok(control)
    }
}

fn run(
    operations: &mut Operations,
    job: Job,
    control: &TransferControl,
) -> Result<OperationResult> {
    match job {
        Job::Run(operation) => operations.submit_operation(operation),
        Job::Transfer(request) => operations.transfer(request, control),
        Job::Undo => operations.undo_with_control(control),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
