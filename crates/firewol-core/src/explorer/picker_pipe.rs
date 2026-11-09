//! Firewol's end of the Open window pipe; the other end is the `firewol-picker` DLL.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT};
use windows::Win32::System::Pipes::PeekNamedPipe;

use super::ListFilter;

const PREFIX: &str = r"\\.\pipe\firewol-pick-";
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// What a program asked of its Open window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickRequest {
    pub folders: bool,
    pub multi: bool,
    pub title: Option<String>,
    pub folder: Option<PathBuf>,
    pub file_name: Option<String>,
    pub types: Vec<(String, String)>,
    /// One-based, as the Windows dialog counts them.
    pub type_index: u32,
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
            "mode" => request.folders = value == "folder",
            "multi" => request.multi = value == "1",
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

    pub fn cancel(&self) -> Result<()> {
        self.send("cancel\n")
    }

    pub fn finish(&self, paths: &[PathBuf]) -> Result<()> {
        let mut reply = String::from("ok\n");
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

/// True when `path` is what the request asks for: a folder, or a file.
pub fn acceptable(request: &PickRequest, path: &Path) -> bool {
    if request.folders {
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
    fn a_folder_request_lists_only_folders() {
        let request = PickRequest {
            folders: true,
            ..PickRequest::default()
        };
        assert_eq!(request.filter(1), ListFilter::FoldersOnly);
    }
}
