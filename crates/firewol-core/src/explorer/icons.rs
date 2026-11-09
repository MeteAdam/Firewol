//! Bounded Shell icon work, kept off the UI and file-operation COM threads.
use std::collections::VecDeque;
use std::mem::size_of;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON};
use windows_core::{HSTRING, PCWSTR};

pub const TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_BYTES: usize = 8 * 1024 * 1024;
const CACHE_ENTRIES: usize = 256;
type Reply = Option<Arc<Vec<u8>>>;

struct Job {
    key: String,
    size: u32,
    deadline: Instant,
    reply: SyncSender<Reply>,
}

enum Message {
    Get(Job),
    /// Drops every cached size of a key whose icon changed.
    Forget(String),
}

const MAX_ICO_BYTES: u64 = 8 << 20;
const MAX_ICO_SIDE: u32 = 256;

/// Only one STA touches Shell extensions. A stuck extension cannot block the UI;
/// callers time out and the bounded queue rejects overload without spawning workers.
pub struct Icons(SyncSender<Message>);

impl Icons {
    pub fn start() -> Result<Self> {
        let (send, receive) = mpsc::sync_channel::<Message>(64);
        std::thread::Builder::new()
            .name("shell-icons".into())
            .spawn(move || {
                if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
                    return;
                }
                let mut cache: VecDeque<(String, u32, Arc<Vec<u8>>)> = VecDeque::new();
                let mut bytes = 0;
                while let Ok(message) = receive.recv() {
                    let job = match message {
                        Message::Get(job) => job,
                        Message::Forget(key) => {
                            cache.retain(|(cached, _, data)| {
                                let keep = *cached != key;
                                if !keep {
                                    bytes -= data.len();
                                }
                                keep
                            });
                            continue;
                        }
                    };
                    if Instant::now() >= job.deadline {
                        continue;
                    }
                    let hit = cache
                        .iter()
                        .position(|(key, size, _)| key == &job.key && *size == job.size);
                    let value = if let Some(position) = hit {
                        let entry = cache.remove(position).expect("cache position");
                        let data = entry.2.clone();
                        cache.push_back(entry);
                        Some(data)
                    } else if !allowed(&job.key) {
                        None
                    } else {
                        extract(&job.key, job.size).ok().map(|data| {
                            let data = Arc::new(data);
                            bytes += data.len();
                            cache.push_back((job.key, job.size, data.clone()));
                            while bytes > CACHE_BYTES || cache.len() > CACHE_ENTRIES {
                                if let Some((_, _, removed)) = cache.pop_front() {
                                    bytes -= removed.len();
                                }
                            }
                            data
                        })
                    };
                    let _ = job.reply.send(value);
                }
                unsafe { CoUninitialize() };
            })?;
        Ok(Self(send))
    }

    pub fn request(&self, key: String, size: u32) -> Option<Receiver<Reply>> {
        if !valid_key(&key) || !(16..=256).contains(&size) {
            return None;
        }
        let (send, receive) = mpsc::sync_channel(1);
        self.0
            .try_send(Message::Get(Job {
                key,
                size,
                deadline: Instant::now() + TIMEOUT,
                reply: send,
            }))
            .ok()?;
        Some(receive)
    }

    /// Makes the next request read the changed icon again.
    pub fn forget(&self, key: String) {
        let _ = self.0.send(Message::Forget(key));
    }
}

/// The Shell parses the plain form of a path; the `\\?\` form yields no item for some types.
pub fn path_key(path: &Path) -> String {
    format!(
        "path:{}",
        PathBuf::from(super::display_path(path)).display()
    )
}

fn has_own_icon(extension: &str) -> bool {
    matches!(extension, "exe" | "lnk" | "ico" | "url")
}

pub fn key(path: &Path) -> String {
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if has_own_icon(&extension) {
        path_key(path)
    } else {
        format!("ext:{extension}")
    }
}

fn is_drive_root(path: &Path) -> bool {
    let path = PathBuf::from(super::display_path(path));
    let name = path.as_os_str().to_string_lossy();
    let bytes = name.as_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..] == b":\\"
}

/// Windows honours `desktop.ini` only on a read-only or system folder, so ordinary folders stay
/// cheap: only these two attributes make a listing look for the file.
fn customized(path: &Path, attributes: u32) -> bool {
    attributes & (FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_SYSTEM.0) != 0
        && path.join("desktop.ini").exists()
}

/// The icon key of a folder with its own Windows icon, or of a drive; `None` for a plain folder.
pub fn folder_key(path: &Path, attributes: u32) -> Option<String> {
    (is_drive_root(path) || customized(path, attributes)).then(|| path_key(path))
}

/// [`folder_key`] for a folder whose attributes the caller has not read.
pub fn folder_icon(path: &Path) -> Option<String> {
    folder_key(path, std::fs::metadata(path).ok()?.file_attributes())
}

/// Shape only: this runs on the protocol callback and must not touch the disk.
fn valid_key(key: &str) -> bool {
    if key.contains('\0') {
        return false;
    }
    if let Some(extension) = key.strip_prefix("ext:") {
        extension.len() <= 64 && !extension.contains(['/', '\\', ':'])
    } else if let Some(path) = key.strip_prefix("path:") {
        Path::new(path).is_absolute()
    } else {
        false
    }
}

/// The worker's check of a `path:` key: only icons a listing could have asked for.
fn allowed(key: &str) -> bool {
    let Some(path) = key.strip_prefix("path:").map(Path::new) else {
        return true;
    };
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => folder_key(path, metadata.file_attributes()).is_some(),
        Ok(metadata) => {
            has_own_icon(&extension) && (extension != "ico" || safe_ico(path, metadata.len()))
        }
        Err(_) => false,
    }
}

/// Windows decodes an icon frame in full whatever the directory claims, so a small crafted file
/// can demand gigabytes in this process. Real icons stop at 256 pixels a side.
fn safe_ico(path: &Path, length: u64) -> bool {
    length <= MAX_ICO_BYTES && std::fs::read(path).is_ok_and(|data| plausible_ico(&data))
}

fn plausible_ico(data: &[u8]) -> bool {
    let word = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let long = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let big = |at: usize| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let frame_fits = |entry: usize| -> Option<bool> {
        let offset = long(entry + 12)? as usize;
        let header = data.get(offset..offset.checked_add(24)?)?;
        let (width, height) = if header.starts_with(b"\x89PNG\r\n\x1a\n") {
            (big(offset + 16)?, big(offset + 20)?)
        } else {
            // A BMP frame stores its height doubled, for the AND mask.
            let width = (long(offset + 4)? as i32).unsigned_abs();
            let height = (long(offset + 8)? as i32).unsigned_abs();
            (width, height / 2)
        };
        Some((1..=MAX_ICO_SIDE).contains(&width) && (1..=MAX_ICO_SIDE).contains(&height))
    };
    let (Some(0), Some(1 | 2), Some(count)) = (word(0), word(2), word(4)) else {
        return false;
    };
    count > 0 && (0..usize::from(count)).all(|index| frame_fits(6 + 16 * index) == Some(true))
}

struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}
struct Dc(HDC);
impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}
struct Icon(HICON);
impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

pub(super) fn bitmap_info(size: u32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn extract(key: &str, size: u32) -> Result<Vec<u8>> {
    let dc = Dc(unsafe { CreateCompatibleDC(None) });
    ensure!(!dc.0.is_invalid(), "Create icon DC");
    let mut pixels = if let Some(path) = key.strip_prefix("path:") {
        let path = HSTRING::from(path);
        let factory: IShellItemImageFactory = unsafe { SHCreateItemFromParsingName(&path, None)? };
        let bitmap = Bitmap(unsafe {
            factory.GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF_ICONONLY,
            )?
        });
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        let mut info = bitmap_info(size);
        let rows = unsafe {
            GetDIBits(
                dc.0,
                bitmap.0,
                0,
                size,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        ensure!(rows == size as i32, "Read Shell bitmap");
        if pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
        pixels
    } else {
        let extension = key.strip_prefix("ext:").unwrap_or_default();
        let name = HSTRING::from(format!("file.{extension}"));
        // Up to 32 px the association icon itself is sharp; tiles draw from the system image
        // list (48 px, or 256 px above 48) so a large tile is not an enlarged 32 px icon.
        if size <= 32 {
            let flags = if size <= 16 {
                SHGFI_SMALLICON
            } else {
                SHGFI_LARGEICON
            };
            unmask(&dc, &association(&name, SHGFI_ICON | flags)?, size)?
        } else {
            let index = system_index(&name)?;
            let large = size > 48;
            let pixels = unmask(
                &dc,
                &listed(index, if large { SHIL_JUMBO } else { SHIL_EXTRALARGE })?,
                size,
            )?;
            // Many types have no 256 px image; the jumbo list then centres a small icon.
            if large && content_width(&pixels, size) * 2 < size {
                unmask(&dc, &listed(index, SHIL_EXTRALARGE)?, size)?
            } else {
                pixels
            }
        }
    };
    encode_png(&mut pixels, size, size)
}

/// Encodes premultiplied BGRA (what the Shell draws) as straight-alpha RGBA PNG.
pub(super) fn encode_png(pixels: &mut [u8], width: u32, height: u32) -> Result<Vec<u8>> {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255) as u8;
        }
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(pixels)?;
    }
    Ok(png)
}

fn association(name: &HSTRING, flags: SHGFI_FLAGS) -> Result<Icon> {
    let mut info = SHFILEINFOW::default();
    let found = unsafe {
        SHGetFileInfoW(
            PCWSTR(name.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            flags | SHGFI_USEFILEATTRIBUTES,
        )
    };
    ensure!(found != 0, "Read Shell association icon");
    Ok(Icon(info.hIcon))
}

fn system_index(name: &HSTRING) -> Result<i32> {
    let mut info = SHFILEINFOW::default();
    let found = unsafe {
        SHGetFileInfoW(
            PCWSTR(name.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES,
        )
    };
    ensure!(found != 0, "Read Shell association icon index");
    Ok(info.iIcon)
}

fn listed(index: i32, list: u32) -> Result<Icon> {
    let images: IImageList = unsafe { SHGetImageList(list as i32)? };
    Ok(Icon(unsafe { images.GetIcon(index, ILD_TRANSPARENT.0)? }))
}

/// Draws `icon` on black and on white, which recovers the alpha mask of older icons too.
fn unmask(dc: &Dc, icon: &Icon, size: u32) -> Result<Vec<u8>> {
    let black = draw(dc, icon, size, 0)?;
    let white = draw(dc, icon, size, 255)?;
    Ok(black
        .as_chunks::<4>()
        .0
        .iter()
        .zip(white.as_chunks::<4>().0)
        .flat_map(|(b, w)| {
            let alpha = 255 - w[0].saturating_sub(b[0]);
            [b[0], b[1], b[2], alpha]
        })
        .collect())
}

/// Width of the columns that hold any visible pixel.
fn content_width(pixels: &[u8], size: u32) -> u32 {
    let size = size as usize;
    let visible = |x: usize| (0..size).any(|y| pixels[(y * size + x) * 4 + 3] > 16);
    match (
        (0..size).find(|&x| visible(x)),
        (0..size).rev().find(|&x| visible(x)),
    ) {
        (Some(left), Some(right)) => (right - left + 1) as u32,
        _ => 0,
    }
}

fn draw(dc: &Dc, icon: &Icon, size: u32, background: u8) -> Result<Vec<u8>> {
    let info = bitmap_info(size);
    let mut bits = std::ptr::null_mut();
    let bitmap =
        Bitmap(unsafe { CreateDIBSection(Some(dc.0), &info, DIB_RGB_COLORS, &mut bits, None, 0)? });
    let previous = unsafe { SelectObject(dc.0, bitmap.0.into()) };
    let length = (size * size * 4) as usize;
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), length) };
    pixels.fill(background);
    let result = unsafe {
        DrawIconEx(
            dc.0,
            0,
            0,
            icon.0,
            size as i32,
            size as i32,
            0,
            None,
            DI_NORMAL,
        )
    };
    let pixels = pixels.to_vec();
    unsafe {
        SelectObject(dc.0, previous);
    }
    result?;
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ico_with(frame: &[u8]) -> Vec<u8> {
        let mut data = vec![0, 0, 1, 0, 1, 0];
        data.extend([0u8; 8]);
        data.extend((frame.len() as u32).to_le_bytes());
        data.extend(22u32.to_le_bytes());
        data.extend(frame);
        data
    }

    fn png_frame(width: u32, height: u32) -> Vec<u8> {
        let mut frame = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        frame.extend(width.to_be_bytes());
        frame.extend(height.to_be_bytes());
        frame
    }

    fn bmp_frame(width: i32, height: i32) -> Vec<u8> {
        let mut frame = 40u32.to_le_bytes().to_vec();
        frame.extend(width.to_le_bytes());
        frame.extend(height.to_le_bytes());
        frame.extend([0u8; 12]);
        frame
    }

    #[test]
    fn only_icons_of_a_normal_size_reach_the_shell() {
        assert!(plausible_ico(&ico_with(&png_frame(256, 256))));
        assert!(plausible_ico(&ico_with(&bmp_frame(48, 96))));
        assert!(!plausible_ico(&ico_with(&png_frame(14_000, 14_000))));
        assert!(!plausible_ico(&ico_with(&png_frame(16, 257))));
        assert!(!plausible_ico(&ico_with(&bmp_frame(4096, 96))));
        assert!(!plausible_ico(b"not an icon"));
        assert!(!plausible_ico(&[0, 0, 1, 0, 200, 0]));
    }

    #[test]
    fn shell_png_and_lru_hit() -> Result<()> {
        let icons = Icons::start()?;
        let fetch = |key: &str, size| {
            icons
                .request(key.into(), size)
                .unwrap()
                .recv_timeout(TIMEOUT)
                .unwrap()
                .unwrap()
        };
        let first = fetch("ext:txt", 16);
        assert!(Arc::ptr_eq(&first, &fetch("ext:txt", 16)));
        for size in [16, 32, 96, 160, 256] {
            let data = fetch("ext:txt", size);
            let decoder = png::Decoder::new(std::io::Cursor::new(data.as_slice())).read_info()?;
            assert_eq!((decoder.info().width, decoder.info().height), (size, size));
        }
        // A tile icon is drawn from a large image, not a small one in an empty frame.
        let tile = fetch("ext:txt", 96);
        let mut reader = png::Decoder::new(std::io::Cursor::new(tile.as_slice())).read_info()?;
        let mut rgba = vec![0; reader.output_buffer_size().unwrap_or_default()];
        reader.next_frame(&mut rgba)?;
        assert!(content_width(&rgba, 96) * 2 >= 96);
        let program = key(&std::env::current_exe()?);
        for size in [16, 32, 96, 160] {
            let data = fetch(&program, size);
            let decoder = png::Decoder::new(std::io::Cursor::new(data.as_slice())).read_info()?;
            assert_eq!((decoder.info().width, decoder.info().height), (size, size));
        }
        // The worker refuses a path no listing would have sent.
        assert!(
            icons
                .request(format!("path:{}", std::env::temp_dir().display()), 16)
                .unwrap()
                .recv_timeout(TIMEOUT)
                .unwrap()
                .is_none()
        );
        assert!(icons.request("path:relative.exe".into(), 16).is_none());
        assert!(icons.request("ext:txt".into(), 1024).is_none());
        Ok(())
    }

    #[test]
    fn only_customized_folders_and_drives_get_a_path_key() -> Result<()> {
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
        let fixture = tempfile::tempdir()?;
        let directory = FILE_ATTRIBUTE_DIRECTORY.0;
        let read_only = directory | FILE_ATTRIBUTE_READONLY.0;
        assert_eq!(folder_key(fixture.path(), directory), None);
        assert_eq!(folder_key(fixture.path(), read_only), None);
        std::fs::write(fixture.path().join("desktop.ini"), "[.ShellClassInfo]")?;
        assert_eq!(folder_key(fixture.path(), directory), None);
        assert_eq!(
            folder_key(fixture.path(), read_only),
            Some(path_key(fixture.path()))
        );
        assert_eq!(
            folder_key(fixture.path(), directory | FILE_ATTRIBUTE_SYSTEM.0),
            Some(path_key(fixture.path()))
        );
        assert_eq!(
            folder_key(Path::new("C:\\"), directory).as_deref(),
            Some("path:C:\\")
        );
        assert_eq!(
            path_key(Path::new(r"\\?\C:\Tools\app.exe")),
            r"path:C:\Tools\app.exe"
        );
        Ok(())
    }

    #[test]
    fn a_changed_icon_is_read_again() -> Result<()> {
        let icons = Icons::start()?;
        let program = key(&std::env::current_exe()?);
        let fetch = || {
            icons
                .request(program.clone(), 16)
                .unwrap()
                .recv_timeout(TIMEOUT)
                .unwrap()
                .unwrap()
        };
        let first = fetch();
        assert!(Arc::ptr_eq(&first, &fetch()));
        icons.forget(program.clone());
        assert!(!Arc::ptr_eq(&first, &fetch()));
        Ok(())
    }
}
