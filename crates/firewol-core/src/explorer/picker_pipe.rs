//! Firewol's end of the Open and Save window pipe; the other end is the `firewol-picker` DLL.
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, SECURITY_ANONYMOUS,
    SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{GetNamedPipeServerProcessId, PeekNamedPipe};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_READ, RegCloseKey, RegOpenKeyExW,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows_core::HSTRING;

use super::ListFilter;

const PREFIX: &str = r"\\.\pipe\firewol-pick-";
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// What a program asked of its Open or Save window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickRequest {
    pub folders: bool,
    pub multi: bool,
    /// A Save window: one file, which need not exist yet.
    pub save: bool,
    pub title: Option<String>,
    pub folder: Option<PathBuf>,
    pub file_name: Option<String>,
    pub types: Vec<(String, String)>,
    /// One-based, as the Windows dialog counts them.
    pub type_index: u32,
    /// Windows adds an extension to a name without one only when the program set this.
    pub default_extension: Option<String>,
    /// The program's own text for the Open or Save button.
    pub ok_label: Option<String>,
    /// The file the program is saving again.
    pub save_as: Option<PathBuf>,
    pub overwrite_prompt: bool,
    /// The name must end in one of the chosen type's extensions.
    pub strict_types: bool,
    pub no_read_only: bool,
    /// The folder must accept a new file before the answer goes back.
    pub test_create: bool,
}

/// Where a name typed in a Save window leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    /// A folder to open rather than a file to save.
    Folder(PathBuf),
    New(PathBuf),
    /// The file exists; replacing it needs the person's word when the program asked for that.
    Exists(PathBuf),
    /// Why this name cannot be saved.
    Refused(String),
}

const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why Windows would refuse `name` as a file name, if it would.
fn name_problem(name: &OsStr) -> Option<String> {
    let text = name.to_string_lossy();
    if text
        .chars()
        .any(|letter| letter < ' ' || "<>:\"/\\|?*".contains(letter))
    {
        return Some(
            r#"A file name can't contain any of these characters: \ / : * ? " < > |"#.into(),
        );
    }
    if text.ends_with(['.', ' ']) {
        return Some("A file name can't end with a space or a period".into());
    }
    if text.encode_utf16().count() > 255 {
        return Some("The file name is too long".into());
    }
    let stem = text.split('.').next().unwrap_or_default().trim_end();
    if RESERVED_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
    {
        return Some(format!("{text} is a name Windows keeps for itself"));
    }
    None
}

/// The extension is one Windows knows, so a name that ends in it is kept as typed.
fn registered_extension(extension: &str) -> bool {
    let mut key = HKEY::default();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CLASSES_ROOT,
            &HSTRING::from(format!(".{extension}")),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if opened.is_err() {
        return false;
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    true
}

/// Creates and drops a hidden file, the way Windows' Save window checks a folder before answering.
pub fn accepts_new_files(folder: &Path) -> bool {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    let probe = folder.join(format!("~firewol-{}-{stamp:x}.tmp", std::process::id()));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_TEMPORARY.0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE.0)
        .open(probe)
        .is_ok()
}

/// Command lines are untrusted: only a name of this exact shape is ever opened.
pub fn is_pick_pipe(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|rest| {
        (1..=96).contains(&rest.len())
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn parse_request(text: &str) -> Result<PickRequest> {
    let mut request = PickRequest {
        type_index: 1,
        ..PickRequest::default()
    };
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once('\t').context("Malformed picker request")?;
        match key {
            "mode" => {
                request.folders = value == "folder";
                request.save = value == "save";
            }
            "multi" => request.multi = value == "1",
            "defaultExt" => request.default_extension = Some(value.to_owned()),
            "okLabel" => request.ok_label = Some(value.to_owned()),
            "saveAs" => request.save_as = Some(PathBuf::from(value)),
            "overwrite" => request.overwrite_prompt = value == "1",
            "strict" => request.strict_types = value == "1",
            "noReadOnly" => request.no_read_only = value == "1",
            "testCreate" => request.test_create = value == "1",
            "typeIndex" => request.type_index = value.parse().unwrap_or(1),
            "title" => request.title = Some(value.to_owned()),
            "folder" => request.folder = Some(PathBuf::from(value)),
            "name" => request.file_name = Some(value.to_owned()),
            "type" => {
                let (name, spec) = value.split_once('\t').context("Malformed file type")?;
                request.types.push((name.to_owned(), spec.to_owned()));
            }
            _ => {}
        }
    }
    Ok(request)
}

impl PickRequest {
    /// What the list shows for the file type at `index` (one-based).
    pub fn filter(&self, index: u32) -> ListFilter {
        if self.folders {
            return ListFilter::FoldersOnly;
        }
        let spec = index
            .checked_sub(1)
            .and_then(|index| self.types.get(index as usize));
        spec.map_or(ListFilter::All, |(_, spec)| ListFilter::from_spec(spec))
    }

    /// The plain extensions of the type at `index`, such as `jpg` and `jpeg`, lowercase.
    pub fn type_extensions(&self, index: u32) -> Vec<String> {
        match self.filter(index) {
            ListFilter::Suffixes(suffixes) => suffixes
                .iter()
                .map(|suffix| suffix.trim_start_matches('.').to_owned())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The folder a Save window starts in when the program is saving a file again.
    pub fn save_as_folder(&self) -> Option<&Path> {
        self.save_as.as_deref()?.parent()
    }

    /// What a name typed in a Save window means, with the extension Windows would add.
    pub fn save_target(&self, folder: &Path, typed: &str, index: u32) -> SaveTarget {
        self.save_target_with(folder, typed, index, registered_extension)
    }

    fn save_target_with(
        &self,
        folder: &Path,
        typed: &str,
        index: u32,
        registered: impl Fn(&str) -> bool,
    ) -> SaveTarget {
        let typed = typed.trim().trim_matches('"').trim();
        if typed.is_empty() {
            return SaveTarget::Refused("Type a name for the file".into());
        }
        let candidate = folder.join(typed);
        if candidate.is_dir() {
            return SaveTarget::Folder(candidate);
        }
        let Some(name) = candidate.file_name() else {
            return SaveTarget::Refused(format!("{typed} is not a file name"));
        };
        if let Some(problem) = name_problem(name) {
            return SaveTarget::Refused(problem);
        }
        let Some(parent) = candidate.parent().filter(|parent| parent.is_dir()) else {
            let parent = candidate.parent().unwrap_or(folder);
            return SaveTarget::Refused(format!("The folder {} does not exist", parent.display()));
        };
        let allowed = self.type_extensions(index);
        let extension = Path::new(name)
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .filter(|extension| !extension.is_empty());
        let offered = |extension: &str| {
            (1..=self.types.len() as u32).any(|other| {
                self.type_extensions(other)
                    .iter()
                    .any(|known| known == extension)
            })
        };
        let keep = match &extension {
            None => false,
            Some(extension) if self.strict_types => {
                allowed.is_empty() || allowed.contains(extension)
            }
            Some(extension) => offered(extension) || registered(extension),
        };
        // As in Windows' window: the chosen type's extension, added only when the program set a
        // default one, or always when the name must match the type.
        let added = (!keep)
            .then(|| {
                let from_type = allowed.first().cloned();
                if self.strict_types {
                    from_type.or_else(|| self.default_extension.clone())
                } else {
                    self.default_extension.as_ref()?;
                    from_type.or_else(|| self.default_extension.clone())
                }
            })
            .flatten();
        let path = match added {
            Some(extension) => {
                let mut name = name.to_owned();
                name.push(format!(".{extension}"));
                if let Some(problem) = name_problem(&name) {
                    return SaveTarget::Refused(problem);
                }
                parent.join(name)
            }
            None => candidate.clone(),
        };
        match fs::metadata(&path) {
            Ok(item) if item.is_dir() => SaveTarget::Folder(path),
            Ok(item) if self.no_read_only && item.permissions().readonly() => {
                SaveTarget::Refused(format!(
                    "{} is read-only",
                    path.file_name().unwrap_or(name).display()
                ))
            }
            Ok(_) => SaveTarget::Exists(path),
            Err(_) => SaveTarget::New(path),
        }
    }
}

/// The connection to the program waiting for an answer.
pub struct PickClient(File);

impl PickClient {
    pub fn connect(name: &str) -> Result<Self> {
        ensure!(is_pick_pipe(name), "Invalid picker pipe name");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            // Firewol must not lend its identity to whoever created the pipe.
            .security_qos_flags((SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS).0)
            .open(name)
            .context("Connect to the program asking for a file")?;
        Ok(Self(file))
    }

    pub fn read_request(&self) -> Result<PickRequest> {
        let mut bytes = Vec::new();
        let mut byte = [0u8; 1];
        while !bytes.ends_with(b"\n\n") {
            ensure!(
                bytes.len() < MAX_REQUEST_BYTES,
                "The picker request is too long"
            );
            (&self.0).read_exact(&mut byte)?;
            bytes.push(byte[0]);
        }
        parse_request(std::str::from_utf8(&bytes)?)
    }

    fn send(&self, text: &str) -> Result<()> {
        (&self.0).write_all(text.as_bytes())?;
        Ok(())
    }

    /// The window is on screen, so the program can stop timing the start.
    pub fn ready(&self) -> Result<()> {
        self.send("ready\n")
    }

    /// Lets the waiting program take the foreground back. Only the program in front may grant
    /// that, and Firewol is in front while the person answers; without it Windows hands the focus
    /// to whatever window comes next once Firewol's closes.
    fn hand_foreground_back(&self) {
        let mut program = 0u32;
        if unsafe { GetNamedPipeServerProcessId(HANDLE(self.0.as_raw_handle()), &mut program) }
            .is_ok()
        {
            let _ = unsafe { AllowSetForegroundWindow(program) };
        }
    }

    pub fn cancel(&self) -> Result<()> {
        self.hand_foreground_back();
        self.send("cancel\n")
    }

    /// Waits until the program says its window is enabled and in front again (`back`), so that
    /// closing Firewol's window afterwards cannot pass the focus to another program. A program
    /// that never says so (an older picker DLL) costs at most `limit`.
    pub fn wait_back(&self, limit: Duration) {
        let started = Instant::now();
        let mut line = Vec::new();
        while started.elapsed() < limit {
            let mut available = 0u32;
            let peeked = unsafe {
                PeekNamedPipe(
                    HANDLE(self.0.as_raw_handle()),
                    None,
                    0,
                    None,
                    Some(&mut available),
                    None,
                )
            };
            if peeked.is_err() {
                return;
            }
            if available == 0 {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let mut byte = [0u8; 1];
            if (&self.0).read_exact(&mut byte).is_err() {
                return;
            }
            if byte[0] == b'\n' {
                if line == b"back" {
                    return;
                }
                line.clear();
            } else {
                line.push(byte[0]);
            }
        }
    }

    /// Sends the chosen items; a Save window also gets the file type that was chosen.
    pub fn finish(&self, paths: &[PathBuf], type_index: Option<u32>) -> Result<()> {
        self.hand_foreground_back();
        let mut reply =
            type_index.map_or_else(String::new, |index| format!("typeIndex\t{index}\n"));
        reply.push_str("ok\n");
        for path in paths {
            let text = path.to_str().context("A chosen path is not valid text")?;
            if text.contains('\n') {
                bail!("A chosen path holds a line break");
            }
            reply.push_str(text);
            reply.push('\n');
        }
        reply.push_str("end\n");
        self.send(&reply)
    }

    /// False once the program has gone away or stopped waiting.
    pub fn alive(&self) -> bool {
        unsafe { PeekNamedPipe(HANDLE(self.0.as_raw_handle()), None, 0, None, None, None) }.is_ok()
    }
}

/// True when `path` is what the request asks for: a folder, a file, or a file to save.
pub fn acceptable(request: &PickRequest, path: &Path) -> bool {
    if request.save {
        let existing = fs::metadata(path).ok();
        path.is_absolute()
            && path.parent().is_some_and(Path::is_dir)
            && path
                .file_name()
                .is_some_and(|name| name_problem(name).is_none())
            && existing.is_none_or(|item| {
                item.is_file() && !(request.no_read_only && item.permissions().readonly())
            })
    } else if request.folders {
        path.is_dir()
    } else {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_firewol_pipe_name_is_accepted() {
        assert!(is_pick_pipe(r"\\.\pipe\firewol-pick-1234-abc"));
        assert!(!is_pick_pipe(r"\\.\pipe\other"));
        assert!(!is_pick_pipe(r"\\server\pipe\firewol-pick-1"));
        assert!(!is_pick_pipe(r"\\.\pipe\firewol-pick-..\evil"));
        assert!(!is_pick_pipe(r"\\.\pipe\firewol-pick-"));
    }

    #[test]
    fn the_request_format_matches_what_the_dll_writes() -> Result<()> {
        let request = parse_request(
            "mode\topen\nmulti\t1\ntypeIndex\t2\ntitle\tBad title here\nfolder\tC:\\Users\ntype\tPictures\t*.png;*.jpg\n\n",
        )?;
        assert!(request.multi && !request.folders);
        assert_eq!(request.title.as_deref(), Some("Bad title here"));
        assert_eq!(request.folder, Some(PathBuf::from(r"C:\Users")));
        assert_eq!(
            request.types,
            [("Pictures".to_owned(), "*.png;*.jpg".to_owned())]
        );
        assert_eq!(request.type_index, 2);
        assert_eq!(request.filter(1), ListFilter::from_spec("*.png;*.jpg"));
        assert_eq!(request.filter(9), ListFilter::All);
        Ok(())
    }

    #[test]
    fn a_save_request_carries_its_options() -> Result<()> {
        let request = parse_request(
            "mode\tsave\nmulti\t0\ntypeIndex\t1\noverwrite\t1\nstrict\t0\nnoReadOnly\t1\ntestCreate\t1\nname\timage\ndefaultExt\tpng\nokLabel\tExport\nsaveAs\tC:\\Docs\\old.png\ntype\tPNG\t*.png\n\n",
        )?;
        assert!(request.save && !request.folders && !request.multi);
        assert!(request.overwrite_prompt && request.no_read_only && request.test_create);
        assert!(!request.strict_types);
        assert_eq!(request.default_extension.as_deref(), Some("png"));
        assert_eq!(request.ok_label.as_deref(), Some("Export"));
        assert_eq!(request.save_as_folder(), Some(Path::new(r"C:\Docs")));
        Ok(())
    }

    fn pictures(default_extension: Option<&str>, strict: bool) -> PickRequest {
        PickRequest {
            save: true,
            types: vec![
                ("PNG".into(), "*.png".into()),
                ("JPEG".into(), "*.jpg;*.jpeg".into()),
                ("All files".into(), "*.*".into()),
            ],
            type_index: 1,
            default_extension: default_extension.map(str::to_owned),
            strict_types: strict,
            no_read_only: true,
            ..PickRequest::default()
        }
    }

    #[test]
    fn a_typed_name_gets_the_extension_windows_would_add() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let folder = fixture.path();
        let known = |extension: &str| extension == "txt";
        let target = |request: &PickRequest, typed: &str, index| {
            request.save_target_with(folder, typed, index, known)
        };
        let png = pictures(Some("png"), false);
        assert_eq!(
            target(&png, "photo", 1),
            SaveTarget::New(folder.join("photo.png"))
        );
        assert_eq!(
            target(&png, "photo", 2),
            SaveTarget::New(folder.join("photo.jpg"))
        );
        // A name ending in an offered or known type keeps it; an unknown one gets the type's.
        assert_eq!(
            target(&png, "photo.jpeg", 1),
            SaveTarget::New(folder.join("photo.jpeg"))
        );
        assert_eq!(
            target(&png, "notes.txt", 1),
            SaveTarget::New(folder.join("notes.txt"))
        );
        assert_eq!(
            target(&png, "v1.2", 1),
            SaveTarget::New(folder.join("v1.2.png"))
        );
        // All files has no extension of its own, so the default one stands in.
        assert_eq!(
            target(&png, "photo", 3),
            SaveTarget::New(folder.join("photo.png"))
        );
        // Without a default extension Windows adds nothing, unless the type is enforced.
        let plain = pictures(None, false);
        assert_eq!(
            target(&plain, "photo", 1),
            SaveTarget::New(folder.join("photo"))
        );
        let strict = pictures(None, true);
        assert_eq!(
            target(&strict, "notes.txt", 2),
            SaveTarget::New(folder.join("notes.txt.jpg"))
        );
        assert_eq!(
            target(&strict, "photo.JPEG", 2),
            SaveTarget::New(folder.join("photo.JPEG"))
        );
        Ok(())
    }

    #[test]
    fn a_save_target_is_a_folder_an_existing_file_or_refused() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let folder = fixture.path();
        std::fs::create_dir(folder.join("albums"))?;
        std::fs::write(folder.join("old.png"), b"x")?;
        std::fs::write(folder.join("locked.png"), b"x")?;
        let mut locked = std::fs::metadata(folder.join("locked.png"))?.permissions();
        locked.set_readonly(true);
        std::fs::set_permissions(folder.join("locked.png"), locked)?;
        let png = pictures(Some("png"), false);
        let target = |typed: &str| png.save_target_with(folder, typed, 1, |_| false);
        assert_eq!(target("albums"), SaveTarget::Folder(folder.join("albums")));
        assert_eq!(
            target(" \"old\" "),
            SaveTarget::Exists(folder.join("old.png"))
        );
        assert!(matches!(target("locked"), SaveTarget::Refused(_)));
        assert!(matches!(target(""), SaveTarget::Refused(_)));
        assert!(matches!(target("a|b"), SaveTarget::Refused(_)));
        assert!(matches!(target("CON.png"), SaveTarget::Refused(_)));
        assert!(matches!(target("trailing."), SaveTarget::Refused(_)));
        assert!(matches!(target(r"missing\photo"), SaveTarget::Refused(_)));
        assert_eq!(
            target(r"albums\cover"),
            SaveTarget::New(folder.join("albums").join("cover.png"))
        );
        assert!(acceptable(&png, &folder.join("new.png")));
        assert!(acceptable(&png, &folder.join("old.png")));
        assert!(!acceptable(&png, &folder.join("locked.png")));
        assert!(!acceptable(&png, &folder.join("albums")));
        assert!(!acceptable(&png, &folder.join("missing").join("new.png")));
        assert!(accepts_new_files(folder));
        assert_eq!(
            std::fs::read_dir(folder)?.count(),
            3,
            "the probe file is gone"
        );
        let mut writable = std::fs::metadata(folder.join("locked.png"))?.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        writable.set_readonly(false);
        std::fs::set_permissions(folder.join("locked.png"), writable)?;
        Ok(())
    }

    #[test]
    fn a_folder_request_lists_only_folders() {
        let request = PickRequest {
            folders: true,
            ..PickRequest::default()
        };
        assert_eq!(request.filter(1), ListFilter::FoldersOnly);
    }
}
