//! Bounded Shell icon work, kept off the UI and file-operation COM threads.
use std::collections::VecDeque;
use std::mem::size_of;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
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

/// Only one STA touches Shell extensions. A stuck extension cannot block the UI;
/// callers time out and the bounded queue rejects overload without spawning workers.
pub struct Icons(SyncSender<Job>);

impl Icons {
    pub fn start() -> Result<Self> {
        let (send, receive) = mpsc::sync_channel::<Job>(64);
        std::thread::Builder::new()
            .name("shell-icons".into())
            .spawn(move || {
                if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
                    return;
                }
                let mut cache: VecDeque<(String, u32, Arc<Vec<u8>>)> = VecDeque::new();
                let mut bytes = 0;
                while let Ok(job) = receive.recv() {
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
            .try_send(Job {
                key,
                size,
                deadline: Instant::now() + TIMEOUT,
                reply: send,
            })
            .ok()?;
        Some(receive)
    }
}

pub fn key(path: &Path) -> String {
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if matches!(extension.as_str(), "exe" | "lnk" | "ico" | "url") {
        format!("path:{}", path.display())
    } else {
        format!("ext:{extension}")
    }
}

fn valid_key(key: &str) -> bool {
    if key.contains('\0') {
        return false;
    }
    if let Some(extension) = key.strip_prefix("ext:") {
        extension.len() <= 64 && !extension.contains(['/', '\\', ':'])
    } else if let Some(path) = key.strip_prefix("path:") {
        Path::new(path).is_absolute() && self::key(Path::new(path)).starts_with("path:")
    } else {
        false
    }
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

fn bitmap_info(size: u32) -> BITMAPINFO {
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
        let mut info = SHFILEINFOW::default();
        let flags = SHGFI_ICON
            | SHGFI_USEFILEATTRIBUTES
            | if size <= 16 {
                SHGFI_SMALLICON
            } else {
                SHGFI_LARGEICON
            };
        let found = unsafe {
            SHGetFileInfoW(
                PCWSTR(name.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut info),
                size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };
        ensure!(found != 0, "Read Shell association icon");
        let icon = Icon(info.hIcon);
        // Two backgrounds recover the alpha mask for older non-ARGB Shell icons too.
        let black = draw(&dc, &icon, size, 0)?;
        let white = draw(&dc, &icon, size, 255)?;
        black
            .as_chunks::<4>()
            .0
            .iter()
            .zip(white.as_chunks::<4>().0)
            .flat_map(|(b, w)| {
                let alpha = 255 - w[0].saturating_sub(b[0]);
                [b[0], b[1], b[2], alpha]
            })
            .collect()
    };
    // Shell bitmaps are premultiplied BGRA; PNG uses straight RGBA.
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
        let mut encoder = png::Encoder::new(&mut png, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
    }
    Ok(png)
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
        let program = key(&std::env::current_exe()?);
        for size in [16, 32, 96, 160] {
            let data = fetch(&program, size);
            let decoder = png::Decoder::new(std::io::Cursor::new(data.as_slice())).read_info()?;
            assert_eq!((decoder.info().width, decoder.info().height), (size, size));
        }
        assert!(icons.request("path:C:\\test.txt".into(), 16).is_none());
        assert!(icons.request("ext:txt".into(), 1024).is_none());
        Ok(())
    }
}
