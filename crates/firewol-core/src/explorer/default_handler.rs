//! Sends folder opening and Win+E to Firewol for this user, and undoes it exactly.
//!
//! Windows has no setting for a default file manager. The per-user registration below overrides
//! the machine-wide one; it needs no administrator rights and leaves Explorer itself untouched.
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, HWND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW,
    RegQueryInfoKeyW, RegSetValueExW,
};
use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::Win32::UI::WindowsAndMessaging::{
    IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW,
};
use windows_core::{PCWSTR, w};

use super::picker_pipe::is_pick_pipe;
use super::resolve_directory;

const USER_CLASSES: &str = r"Software\Classes";
const EXECUTABLE_NAME: &str = "firewol-tauri.exe";
const BACKUP_FILE: &str = "default-apps-backup.json";
/// The per-user override of what Win+E opens.
const WIN_E: &str = r"CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command";
const FOLDER_OPEN: &str = r"Folder\shell\open\command";
const FOLDER_EXPLORE: &str = r"Folder\shell\explore\command";
/// A target that returns from Explorer within this time is a loop, not a second request.
const LOOP_WINDOW: Duration = Duration::from_secs(3);

/// What a launch of Firewol was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    Default,
    Folder(PathBuf),
    /// A program's Open window is waiting on this pipe for Firewol's answer.
    Pick(String),
    /// Something only Explorer can show: Control Panel, the Recycle Bin, a library.
    HandOff(OsString),
}

/// Reads the arguments of a launch. Flags are never treated as places to open.
pub fn classify(mut arguments: impl Iterator<Item = OsString>) -> Launch {
    let Some(argument) = arguments.next().filter(|value| !value.is_empty()) else {
        return Launch::Default;
    };
    let text = argument.to_string_lossy();
    if text == "--pick" {
        return arguments
            .next()
            .map(|pipe| pipe.to_string_lossy().into_owned())
            .filter(|pipe| is_pick_pipe(pipe))
            .map_or(Launch::Default, Launch::Pick);
    }
    if text.starts_with('-') {
        return Launch::Default;
    }
    if text.starts_with("::") || text.to_lowercase().starts_with("shell:") {
        return Launch::HandOff(argument);
    }
    match resolve_directory(Path::new(&argument)) {
        Ok(folder) => Launch::Folder(folder),
        Err(_) => Launch::HandOff(argument),
    }
}

/// Opens `target` in Explorer. If the same target comes straight back, something is sending
/// Explorer to Firewol and Firewol to Explorer; the loop stops here.
pub fn hand_off(target: &OsStr, data: &Path) -> Result<()> {
    fs::create_dir_all(data)?;
    let marker = data.join("handoff.last");
    let name = target.to_string_lossy();
    let recent = fs::metadata(&marker)
        .and_then(|item| item.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age < LOOP_WINDOW);
    let same = fs::read_to_string(&marker).is_ok_and(|last| last == name);
    ensure!(!(recent && same), "Explorer sent {name} back to Firewol");
    fs::write(&marker, name.as_bytes())?;
    std::process::Command::new(windows_directory()?.join("explorer.exe"))
        .arg(target)
        .spawn()
        .context("Start Explorer")?;
    Ok(())
}

fn windows_directory() -> Result<PathBuf> {
    let mut buffer = [0u16; 260];
    let length = unsafe { GetSystemWindowsDirectoryW(Some(&mut buffer)) } as usize;
    ensure!(
        (1..buffer.len()).contains(&length),
        "Find the Windows folder"
    );
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

/// Where the copy the registry points at lives. A build folder can vanish; this one stays.
pub fn install_dir() -> Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|path| PathBuf::from(path).join(r"Programs\Firewol"))
        .context("LOCALAPPDATA is unset")
}

pub fn installed_executable() -> Result<PathBuf> {
    Ok(install_dir()?.join(EXECUTABLE_NAME))
}

fn digest(path: &Path) -> Result<[u8; 32]> {
    Ok(Sha256::digest(fs::read(path)?).into())
}

/// The installed copy exists and differs from the program running now.
pub fn installed_copy_outdated(current: &Path, installed: &Path) -> bool {
    current != installed
        && match (digest(current), digest(installed)) {
            (Ok(now), Ok(copy)) => now != copy,
            _ => true,
        }
}

/// Copies this program to `installed`. A running copy is locked but can be renamed aside.
pub fn install_copy(current: &Path, installed: &Path) -> Result<()> {
    if current == installed {
        return Ok(());
    }
    let folder = installed
        .parent()
        .context("The install path has no folder")?;
    fs::create_dir_all(folder)?;
    remove_old_copies(folder);
    let staged = installed.with_extension("exe.new");
    fs::copy(current, &staged).context("Copy Firewol")?;
    if fs::rename(&staged, installed).is_err() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        fs::rename(installed, folder.join(format!("firewol-old-{stamp}.exe")))
            .context("Move the running copy aside")?;
        fs::rename(&staged, installed).context("Put the new copy in place")?;
    }
    Ok(())
}

/// Best effort: a copy still running stays locked until the next time.
fn remove_old_copies(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("firewol-old-")
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Asks at the PC itself, outside the WebView, as every change to how Windows behaves does here.
pub fn confirm_enable(owner: HWND) -> bool {
    let answer = unsafe {
        MessageBoxW(
            Some(owner),
            w!(
                "Windows will open folders and Win+E in Firewol for your account.\n\n\
                 Explorer stays installed. The desktop, the taskbar and virtual folders such as \
                 the Recycle Bin keep using it.\n\n\
                 You can turn this off in Settings, or run firewol-tauri.exe --restore-explorer."
            ),
            w!("Open folders in Firewol?"),
            MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    answer == IDOK
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain([0]).collect()
}

fn checked(status: WIN32_ERROR, what: &str) -> Result<()> {
    status.ok().with_context(|| what.to_owned())
}

fn open_key(path: &str) -> Option<Key> {
    let mut key = HKEY::default();
    let name = wide(path);
    unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            None,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    }
    .is_ok()
    .then_some(Key(key))
}

fn create_key(path: &str) -> Result<Key> {
    let mut key = HKEY::default();
    let name = wide(path);
    checked(
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(name.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut key,
                None,
            )
        },
        "Create a registry key",
    )?;
    Ok(Key(key))
}

fn read_value(key: &Key, name: PCWSTR) -> Option<String> {
    let mut bytes = 0u32;
    unsafe {
        RegGetValueW(
            key.0,
            None,
            name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    }
    .ok()
    .ok()?;
    let mut text = vec![0u16; (bytes as usize).div_ceil(2)];
    unsafe {
        RegGetValueW(
            key.0,
            None,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    }
    .ok()
    .ok()?;
    let end = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    Some(String::from_utf16_lossy(&text[..end]))
}

fn write_value(key: &Key, name: PCWSTR, value: &str) -> Result<()> {
    let text = wide(value);
    let bytes: Vec<u8> = text.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    checked(
        unsafe { RegSetValueExW(key.0, name, None, REG_SZ, Some(&bytes)) },
        "Write a registry value",
    )
}

fn clear_value(key: &Key, name: PCWSTR) -> Result<()> {
    let status = unsafe { RegDeleteValueW(key.0, name) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    checked(status, "Remove a registry value")
}

fn is_empty(key: &Key) -> bool {
    let (mut keys, mut values) = (0u32, 0u32);
    let read = unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            Some(&mut keys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
    };
    read.is_ok() && keys == 0 && values == 0
}

fn delete_key_if_empty(full: &str) -> Result<()> {
    let Some(key) = open_key(full) else {
        return Ok(());
    };
    if !is_empty(&key) {
        return Ok(());
    }
    drop(key);
    let name = wide(full);
    checked(
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr())) },
        "Remove a registry key",
    )
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedCommand {
    /// Relative to the classes root.
    path: String,
    existed: bool,
    default: Option<String>,
    delegate: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Backup {
    commands: Vec<SavedCommand>,
    /// Keys this registration created, parents first, relative to the classes root.
    created: Vec<String>,
}

/// One user's folder-handler registration.
pub struct FolderHandler {
    root: String,
    backup: PathBuf,
    notify_shell: bool,
}

const DELEGATE: PCWSTR = w!("DelegateExecute");

impl FolderHandler {
    pub fn for_user(data: &Path) -> Self {
        Self {
            root: USER_CLASSES.to_owned(),
            backup: data.join(BACKUP_FILE),
            notify_shell: true,
        }
    }

    fn full(&self, path: &str) -> String {
        format!(r"{}\{path}", self.root)
    }

    fn command_for(executable: &Path, passes_folder: bool) -> String {
        if passes_folder {
            format!("\"{}\" \"%1\"", executable.display())
        } else {
            format!("\"{}\"", executable.display())
        }
    }

    fn targets() -> [(&'static str, bool); 3] {
        [(FOLDER_OPEN, true), (FOLDER_EXPLORE, true), (WIN_E, false)]
    }

    /// The command Windows runs to open a folder for this user, if this user has set one.
    pub fn current_command(&self) -> Option<String> {
        read_value(&open_key(&self.full(FOLDER_OPEN))?, PCWSTR::null())
    }

    pub fn is_enabled(&self, executable: &Path) -> bool {
        Self::targets().iter().all(|(path, passes_folder)| {
            open_key(&self.full(path))
                .and_then(|key| read_value(&key, PCWSTR::null()))
                .is_some_and(|value| value == Self::command_for(executable, *passes_folder))
        })
    }

    fn read_backup(&self) -> Option<Backup> {
        serde_json::from_slice(&fs::read(&self.backup).ok()?).ok()
    }

    fn snapshot(&self) -> Backup {
        let mut commands = Vec::new();
        let mut created = Vec::new();
        for (path, _) in Self::targets() {
            let key = open_key(&self.full(path));
            commands.push(SavedCommand {
                path: path.to_owned(),
                existed: key.is_some(),
                default: key.as_ref().and_then(|key| read_value(key, PCWSTR::null())),
                delegate: key.as_ref().and_then(|key| read_value(key, DELEGATE)),
            });
            let mut prefix = String::new();
            for part in path.split('\\') {
                if !prefix.is_empty() {
                    prefix.push('\\');
                }
                prefix.push_str(part);
                if open_key(&self.full(&prefix)).is_none() && !created.contains(&prefix) {
                    created.push(prefix.clone());
                }
            }
        }
        Backup { commands, created }
    }

    fn write_commands(&self, executable: &Path) -> Result<()> {
        for (path, passes_folder) in Self::targets() {
            let key = create_key(&self.full(path))?;
            write_value(
                &key,
                PCWSTR::null(),
                &Self::command_for(executable, passes_folder),
            )?;
            // An empty value makes Windows run the command instead of its built-in handler.
            write_value(&key, DELEGATE, "")?;
        }
        Ok(())
    }

    /// Points folder opening at `executable`. Enabling again keeps the first backup, so turning
    /// this off always returns to what was there before Firewol.
    pub fn enable(&self, executable: &Path) -> Result<()> {
        if self.read_backup().is_none() {
            if let Some(folder) = self.backup.parent() {
                fs::create_dir_all(folder)?;
            }
            let staged = self.backup.with_extension("json.new");
            fs::write(&staged, serde_json::to_vec_pretty(&self.snapshot())?)?;
            fs::rename(&staged, &self.backup)?;
        }
        if let Err(error) = self.write_commands(executable) {
            let _ = self.disable();
            return Err(error);
        }
        self.tell_shell();
        Ok(())
    }

    /// Restores what was there before. Without a backup it removes only Firewol's own commands.
    pub fn disable(&self) -> Result<()> {
        match self.read_backup() {
            Some(backup) => self.restore(&backup)?,
            None => self.remove_own_commands()?,
        }
        match fs::remove_file(&self.backup) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).context("Remove the backup");
            }
            _ => {}
        }
        self.tell_shell();
        Ok(())
    }

    fn restore(&self, backup: &Backup) -> Result<()> {
        for saved in &backup.commands {
            let Some(key) = open_key(&self.full(&saved.path)) else {
                continue;
            };
            let values = [
                (PCWSTR::null(), &saved.default),
                (DELEGATE, &saved.delegate),
            ];
            for (name, value) in values {
                match value {
                    Some(value) if saved.existed => write_value(&key, name, value)?,
                    _ => clear_value(&key, name)?,
                }
            }
        }
        // Children come after their parents in the list, so walk it backwards.
        for path in backup.created.iter().rev() {
            self.delete_if_empty(path)?;
        }
        Ok(())
    }

    fn remove_own_commands(&self) -> Result<()> {
        for (path, _) in Self::targets() {
            let Some(key) = open_key(&self.full(path)) else {
                continue;
            };
            let ours = read_value(&key, PCWSTR::null())
                .is_some_and(|value| value.to_lowercase().contains(EXECUTABLE_NAME));
            if ours {
                clear_value(&key, PCWSTR::null())?;
                clear_value(&key, DELEGATE)?;
                drop(key);
                self.delete_if_empty(path)?;
            }
        }
        Ok(())
    }

    fn delete_if_empty(&self, path: &str) -> Result<()> {
        delete_key_if_empty(&self.full(path))
    }

    fn tell_shell(&self) {
        if self.notify_shell {
            unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
        }
    }
}

const OPEN_DIALOG_CLASS: &str = "{DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7}";
/// The Save class is only ever removed: the earlier test build registered it too.
const DIALOG_CLASSES: [&str; 2] = [OPEN_DIALOG_CLASS, "{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"];
const PICKER_NAME: &str = "firewol-picker";
const PICKER_BUILD_NAME: &str = "firewol_picker.dll";

fn built_picker(current_executable: &Path) -> Result<PathBuf> {
    Ok(current_executable
        .parent()
        .context("The program has no folder")?
        .join(PICKER_BUILD_NAME))
}

/// Copies the picker DLL beside the installed program, named by its contents: a loaded DLL is
/// locked, so a new build can never replace one that programs still have open.
pub fn install_picker(current_executable: &Path, folder: &Path) -> Result<PathBuf> {
    let built = built_picker(current_executable)?;
    ensure!(
        built.is_file(),
        "{PICKER_BUILD_NAME} was not built next to Firewol"
    );
    let hash: String = digest(&built)?[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fs::create_dir_all(folder)?;
    let installed = folder.join(format!("{PICKER_NAME}-{hash}.dll"));
    if !installed.is_file() {
        fs::copy(&built, &installed).context("Copy the file picker")?;
    }
    remove_old_pickers(folder, &installed);
    Ok(installed)
}

/// Best effort: a DLL still loaded in some program stays until it closes.
fn remove_old_pickers(folder: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(PICKER_NAME) && name.ends_with(".dll") && entry.path() != keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The registered picker is not the one built next to this Firewol.
pub fn picker_outdated(current_executable: &Path, registered: &Path) -> bool {
    let Ok(built) = built_picker(current_executable) else {
        return false;
    };
    // The installed copy has no build beside it; there is nothing newer to offer.
    built.is_file()
        && match (digest(&built), digest(registered)) {
            (Ok(now), Ok(copy)) => now != copy,
            _ => true,
        }
}

/// Asks at the PC itself. The DLL is loaded into every program that opens a file window.
pub fn confirm_picker(owner: HWND) -> bool {
    let answer = unsafe {
        MessageBoxW(
            Some(owner),
            w!(
                "Programs that open an Open window will load a small Firewol DLL and show Firewol \
                 instead.\n\n\
                 If Firewol cannot answer, the normal Windows window opens. Save windows stay as \
                 they are. Windows or antivirus software may flag this technique.\n\n\
                 You can turn this off in Settings, or run firewol-tauri.exe --restore-explorer."
            ),
            w!("Open files through Firewol?"),
            MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    answer == IDOK
}

/// The per-user registration that puts Firewol in front of Windows' Open window.
pub struct FileDialogs {
    root: String,
}

impl FileDialogs {
    pub fn for_user() -> Self {
        Self {
            root: USER_CLASSES.to_owned(),
        }
    }

    fn server(&self, class: &str) -> String {
        format!(r"{}\CLSID\{class}\InprocServer32", self.root)
    }

    fn registered(&self, class: &str) -> Option<String> {
        read_value(&open_key(&self.server(class))?, PCWSTR::null())
    }

    fn is_ours(value: &str) -> bool {
        value.to_lowercase().contains(PICKER_NAME)
    }

    pub fn is_enabled(&self) -> bool {
        self.registered(OPEN_DIALOG_CLASS)
            .is_some_and(|value| Self::is_ours(&value))
    }

    /// The DLL Windows loads for the Open window, when it is Firewol's.
    pub fn registered_picker(&self) -> Option<PathBuf> {
        self.registered(OPEN_DIALOG_CLASS)
            .filter(|value| Self::is_ours(value))
            .map(PathBuf::from)
    }

    /// Refuses to replace another program's registration.
    pub fn enable(&self, dll: &Path) -> Result<()> {
        if let Some(value) = self.registered(OPEN_DIALOG_CLASS) {
            ensure!(
                Self::is_ours(&value),
                "Another program already handles Open windows"
            );
        }
        let written = (|| {
            let key = create_key(&self.server(OPEN_DIALOG_CLASS))?;
            write_value(&key, PCWSTR::null(), &dll.display().to_string())?;
            write_value(&key, w!("ThreadingModel"), "Apartment")
        })();
        if written.is_err() {
            let _ = self.disable();
        }
        written
    }

    pub fn disable(&self) -> Result<()> {
        for class in DIALOG_CLASSES {
            let server = self.server(class);
            let Some(key) = open_key(&server) else {
                continue;
            };
            if !read_value(&key, PCWSTR::null()).is_some_and(|value| Self::is_ours(&value)) {
                continue;
            }
            clear_value(&key, PCWSTR::null())?;
            clear_value(&key, w!("ThreadingModel"))?;
            drop(key);
            delete_key_if_empty(&server)?;
            delete_key_if_empty(&format!(r"{}\CLSID\{class}", self.root))?;
        }
        Ok(())
    }
}

/// Turns everything off from the command line, for when the interface cannot start.
pub fn restore_from_command_line(data: &Path) -> Result<()> {
    FileDialogs::for_user().disable()?;
    FolderHandler::for_user(data).disable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::RegDeleteTreeW;

    /// A throwaway tree under the test user's key; the real classes root is never touched.
    struct Fixture {
        handler: FolderHandler,
        tree: String,
        _data: tempfile::TempDir,
    }

    impl Fixture {
        fn new(name: &str) -> Result<Self> {
            let data = tempfile::tempdir()?;
            let tree = format!(r"Software\FirewolTest-{}-{name}", std::process::id());
            let handler = FolderHandler {
                root: format!(r"{tree}\Classes"),
                backup: data.path().join(BACKUP_FILE),
                notify_shell: false,
            };
            create_key(&handler.root)?;
            Ok(Self {
                handler,
                tree,
                _data: data,
            })
        }

        fn value(&self, path: &str, name: PCWSTR) -> Option<String> {
            read_value(&open_key(&self.handler.full(path))?, name)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let name = wide(&self.tree);
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr()));
            }
        }
    }

    #[test]
    fn enable_writes_every_command_and_disable_leaves_nothing() -> Result<()> {
        let fixture = Fixture::new("clean")?;
        let exe = Path::new(r"C:\Apps\firewol-tauri.exe");
        fixture.handler.enable(exe)?;
        assert!(fixture.handler.is_enabled(exe));
        assert_eq!(
            fixture.value(FOLDER_OPEN, PCWSTR::null()).as_deref(),
            Some(r#""C:\Apps\firewol-tauri.exe" "%1""#)
        );
        assert_eq!(
            fixture.value(WIN_E, PCWSTR::null()).as_deref(),
            Some(r#""C:\Apps\firewol-tauri.exe""#)
        );
        assert_eq!(fixture.value(FOLDER_OPEN, DELEGATE).as_deref(), Some(""));
        fixture.handler.disable()?;
        assert!(!fixture.handler.is_enabled(exe));
        assert!(open_key(&fixture.handler.full("Folder")).is_none());
        assert!(open_key(&fixture.handler.full("CLSID")).is_none());
        assert!(fixture.handler.read_backup().is_none());
        Ok(())
    }

    #[test]
    fn another_programs_handler_comes_back_exactly() -> Result<()> {
        let fixture = Fixture::new("other")?;
        let earlier = create_key(&fixture.handler.full(FOLDER_OPEN))?;
        write_value(&earlier, PCWSTR::null(), r"C:\Other\manager.exe %1")?;
        write_value(&earlier, DELEGATE, "{ABC}")?;
        drop(earlier);
        let exe = Path::new(r"C:\Apps\firewol-tauri.exe");
        fixture.handler.enable(exe)?;
        // Enabling twice must not record Firewol itself as the thing to restore.
        fixture.handler.enable(exe)?;
        fixture.handler.disable()?;
        assert_eq!(
            fixture.value(FOLDER_OPEN, PCWSTR::null()).as_deref(),
            Some(r"C:\Other\manager.exe %1")
        );
        assert_eq!(
            fixture.value(FOLDER_OPEN, DELEGATE).as_deref(),
            Some("{ABC}")
        );
        assert!(open_key(&fixture.handler.full(FOLDER_EXPLORE)).is_none());
        Ok(())
    }

    #[test]
    fn a_lost_backup_still_removes_only_firewol() -> Result<()> {
        let fixture = Fixture::new("lost")?;
        let exe = Path::new(r"C:\Apps\firewol-tauri.exe");
        fixture.handler.enable(exe)?;
        fs::remove_file(&fixture.handler.backup)?;
        let foreign = create_key(&fixture.handler.full(WIN_E))?;
        write_value(&foreign, PCWSTR::null(), r"C:\Other\manager.exe")?;
        drop(foreign);
        fixture.handler.disable()?;
        assert!(fixture.value(FOLDER_OPEN, PCWSTR::null()).is_none());
        assert_eq!(
            fixture.value(WIN_E, PCWSTR::null()).as_deref(),
            Some(r"C:\Other\manager.exe")
        );
        Ok(())
    }

    #[test]
    fn the_picker_registers_the_open_window_and_leaves_nothing() -> Result<()> {
        let fixture = Fixture::new("probe")?;
        let probe = FileDialogs {
            root: fixture.handler.root.clone(),
        };
        let dll = Path::new(r"C:\Apps\firewol-picker-ab12.dll");
        probe.enable(dll)?;
        assert!(probe.is_enabled());
        assert!(
            probe.registered(DIALOG_CLASSES[1]).is_none(),
            "Save windows are left alone"
        );
        let key = open_key(&probe.server(DIALOG_CLASSES[0])).context("the key")?;
        assert_eq!(
            read_value(&key, w!("ThreadingModel")).as_deref(),
            Some("Apartment")
        );
        drop(key);
        probe.disable()?;
        assert!(!probe.is_enabled());
        let class = format!(r"{}\CLSID\{}", probe.root, DIALOG_CLASSES[0]);
        assert!(open_key(&class).is_none());
        Ok(())
    }

    #[test]
    fn the_picker_will_not_replace_another_program() -> Result<()> {
        let fixture = Fixture::new("foreign")?;
        let probe = FileDialogs {
            root: fixture.handler.root.clone(),
        };
        let foreign = create_key(&probe.server(DIALOG_CLASSES[0]))?;
        write_value(&foreign, PCWSTR::null(), r"C:\Other\dialog.dll")?;
        drop(foreign);
        assert!(
            probe
                .enable(Path::new(r"C:\Apps\firewol-picker-ab12.dll"))
                .is_err()
        );
        probe.disable()?;
        assert_eq!(
            probe.registered(DIALOG_CLASSES[0]).as_deref(),
            Some(r"C:\Other\dialog.dll")
        );
        Ok(())
    }

    #[test]
    fn launches_are_sorted_into_folders_and_explorer_targets() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let args = |value: &OsStr| vec![value.to_owned()].into_iter();
        assert_eq!(classify(std::iter::empty()), Launch::Default);
        assert_eq!(classify(args(OsStr::new("--anything"))), Launch::Default);
        let folder = classify(args(fixture.path().as_os_str()));
        assert!(matches!(folder, Launch::Folder(_)));
        for virtual_target in [
            "::{645FF040-5081-101B-9F08-00AA002F954E}",
            "shell:AppsFolder",
        ] {
            assert_eq!(
                classify(args(OsStr::new(virtual_target))),
                Launch::HandOff(virtual_target.into())
            );
        }
        let file = fixture.path().join("a.txt");
        fs::write(&file, b"x")?;
        assert!(matches!(
            classify(args(file.as_os_str())),
            Launch::HandOff(_)
        ));
        let pick = |pipe: &str| classify(["--pick".into(), pipe.into()].into_iter());
        assert_eq!(
            pick(r"\\.\pipe\firewol-pick-1-a"),
            Launch::Pick(r"\\.\pipe\firewol-pick-1-a".into())
        );
        assert_eq!(pick(r"\\.\pipe\other"), Launch::Default);
        Ok(())
    }

    #[test]
    fn a_target_that_comes_straight_back_stops_the_loop() -> Result<()> {
        let data = tempfile::tempdir()?;
        fs::write(data.path().join("handoff.last"), "shell:Loop")?;
        let refused = hand_off(OsStr::new("shell:Loop"), data.path())
            .err()
            .context("the repeat is refused")?;
        assert!(refused.to_string().contains("back to Firewol"));
        Ok(())
    }

    #[test]
    fn installing_replaces_an_older_copy() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let current = fixture.path().join("build.exe");
        let installed = fixture.path().join(r"Programs\Firewol\firewol-tauri.exe");
        fs::write(&current, b"new build")?;
        assert!(installed_copy_outdated(&current, &installed));
        install_copy(&current, &installed)?;
        assert!(!installed_copy_outdated(&current, &installed));
        fs::write(&current, b"newer build")?;
        assert!(installed_copy_outdated(&current, &installed));
        install_copy(&current, &installed)?;
        assert_eq!(fs::read(&installed)?, b"newer build");
        Ok(())
    }
}
