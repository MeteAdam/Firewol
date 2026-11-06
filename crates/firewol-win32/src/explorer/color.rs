use std::cell::{Cell, RefCell};
use std::f32::consts::TAU;
use std::ffi::OsStr;
use std::sync::OnceLock;

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::Rgb;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::controls::{child, edit, get_text, place, send, set_text};
use super::form::Dialog;
use super::theme::{colorref, fill, readable_on, rgb};

const TBM_GETPOS: u32 = WM_USER;
const SIDE: i32 = 220;
const LEFT: i32 = 57;
const TOP: i32 = 12;
const RADIUS: f32 = SIDE as f32 / 2.0 - 2.0;

pub(super) fn hex(Rgb(r, g, b): Rgb) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

pub(super) fn parse(value: &OsStr) -> Result<Option<Rgb>> {
    let value = value.to_str().context("Enter a color as #RRGGBB")?.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let digits = value.strip_prefix('#').unwrap_or(value);
    ensure!(
        digits.len() == 6 && digits.bytes().all(|c| c.is_ascii_hexdigit()),
        "Enter a color as #RRGGBB"
    );
    Ok(Some(rgb(u32::from_str_radix(digits, 16)?)))
}

#[derive(Clone, Copy)]
struct Hsv {
    hue: f32,
    saturation: f32,
    value: f32,
}

impl Hsv {
    fn color(self) -> Rgb {
        let hue = self.hue * 6.0;
        let channel = |offset: f32| {
            let k = (offset + hue).rem_euclid(6.0);
            (255.0 * self.value * (1.0 - self.saturation * k.min(4.0 - k).clamp(0.0, 1.0))).round()
                as u8
        };
        Rgb(channel(5.0), channel(3.0), channel(1.0))
    }

    fn from_rgb(Rgb(r, g, b): Rgb) -> Self {
        let [r, g, b] = [r, g, b].map(|channel| f32::from(channel) / 255.0);
        let value = r.max(g).max(b);
        let delta = value - r.min(g).min(b);
        let hue = if delta == 0.0 {
            0.0
        } else if value == r {
            (g - b) / delta
        } else if value == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        Self {
            hue: hue.rem_euclid(6.0) / 6.0,
            saturation: if value == 0.0 { 0.0 } else { delta / value },
            value,
        }
    }

    fn point(x: f32, y: f32, value: f32) -> Self {
        Self {
            hue: (y.atan2(x) / TAU).rem_euclid(1.0),
            saturation: (x.hypot(y) / RADIUS).min(1.0),
            value,
        }
    }
}

struct Wheel {
    brightness: u8,
    pixels: Vec<u32>,
}

impl Wheel {
    fn render(&mut self, value: f32) -> &[u32] {
        static PALETTE: OnceLock<Vec<Option<Rgb>>> = OnceLock::new();
        let brightness = (value * 255.0).round() as u8;
        if self.pixels.is_empty() || self.brightness != brightness {
            self.brightness = brightness;
            let palette = PALETTE.get_or_init(|| {
                (0..SIDE)
                    .flat_map(|y| {
                        (0..SIDE).map(move |x| {
                            let x = (x - SIDE / 2) as f32;
                            let y = (y - SIDE / 2) as f32;
                            (x.hypot(y) <= RADIUS).then(|| Hsv::point(x, y, 1.0).color())
                        })
                    })
                    .collect()
            });
            let system = unsafe { GetSysColor(COLOR_WINDOW) }.to_le_bytes();
            let background = u32::from_le_bytes([system[2], system[1], system[0], 0]);
            let shade = |channel: u8| (u16::from(channel) * u16::from(brightness) / 255) as u8;
            self.pixels.clear();
            self.pixels.extend(palette.iter().map(|pixel| {
                pixel.map_or(background, |Rgb(r, g, b)| {
                    u32::from_le_bytes([shade(b), shade(g), shade(r), 0])
                })
            }));
        }
        &self.pixels
    }

    fn paint(&mut self, window: HWND, hsv: Hsv) {
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(window, &mut paint) };
        let pixels = self.render(hsv.value);
        let bitmap = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIDE,
                biHeight: -SIDE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let angle = hsv.hue * TAU;
        let x = LEFT + SIDE / 2 + (angle.cos() * RADIUS * hsv.saturation).round() as i32;
        let y = TOP + SIDE / 2 + (angle.sin() * RADIUS * hsv.saturation).round() as i32;
        let color = hsv.color();
        unsafe {
            StretchDIBits(
                dc,
                LEFT,
                TOP,
                SIDE,
                SIDE,
                0,
                0,
                SIDE,
                SIDE,
                Some(pixels.as_ptr().cast()),
                &bitmap,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            let pen = CreatePen(PS_SOLID, 2, colorref(readable_on(color)));
            let old_pen = SelectObject(dc, pen.into());
            let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
            let _ = Ellipse(dc, x - 6, y - 6, x + 6, y + 6);
            SelectObject(dc, old_brush);
            SelectObject(dc, old_pen);
            let _ = DeleteObject(pen.into());
        }
        fill(
            dc,
            RECT {
                left: 232,
                top: 289,
                right: 310,
                bottom: 314,
            },
            color,
        );
        let _ = unsafe { EndPaint(window, &paint) };
    }
}

pub(super) fn pick(owner: HWND, initial: Option<Rgb>) -> Result<Option<Rgb>> {
    let dialog = Dialog::create(owner, w!("Choose a color"), 352, 417)?;
    let window = dialog.window;
    let hsv = Cell::new(Hsv::from_rgb(initial.unwrap_or(rgb(0x3684dd))));
    let wheel = RefCell::new(Wheel {
        brightness: 0,
        pixels: Vec::new(),
    });
    let update = Cell::new(false);
    let value = child(
        window,
        dialog.instance,
        10,
        TRACKBAR_CLASSW,
        w!("Brightness"),
        WS_TABSTOP | WINDOW_STYLE(TBS_NOTICKS),
        WINDOW_EX_STYLE::default(),
    )?;
    dialog.text(11, "Brightness", 19, 251, 69)?;
    place(value, 93, 246, 225, 30);
    send(value, TBM_SETRANGE, 0, 100 << 16);
    send(
        value,
        TBM_SETPOS,
        1,
        (hsv.get().value * 100.0).round() as isize,
    );
    let field = edit(window, dialog.instance, 12, 7)?;
    dialog.text(13, "Hex color", 19, 294, 69)?;
    place(field, 93, 288, 122, 27);
    set_text(field, hex(hsv.get().color()));
    let status = dialog.text(14, "", 19, 322, 297)?;
    dialog.button(1, w!("Use color"), 154, 348, 78)?;
    dialog.button(2, w!("Cancel"), 240, 348, 78)?;
    let repaint = || {
        let _ = unsafe { InvalidateRect(Some(window), None, false) };
    };
    let apply = |next: Hsv| {
        hsv.set(next);
        update.set(true);
        set_text(field, hex(next.color()));
        update.set(false);
        set_text(status, "");
        repaint();
    };
    dialog.run(field, &[field], |reply, message, wparam, lparam| {
        match message {
            WM_PAINT => wheel.borrow_mut().paint(window, hsv.get()),
            WM_HSCROLL if HWND(lparam.0 as *mut _) == value => {
                let level = send(value, TBM_GETPOS, 0, 0) as f32 / 100.0;
                apply(Hsv {
                    value: level,
                    ..hsv.get()
                });
            }
            WM_COMMAND if wparam.0 & 0xffff == 1 => match parse(&get_text(field)) {
                Ok(Some(color)) => reply.accept(color),
                _ => set_text(status, "Enter a color as #RRGGBB"),
            },
            WM_COMMAND
                if wparam.0 & 0xffff == 12
                    && (wparam.0 >> 16) as u32 == EN_CHANGE
                    && !update.get() =>
            {
                if let Ok(Some(color)) = parse(&get_text(field)) {
                    let next = Hsv::from_rgb(color);
                    hsv.set(next);
                    send(value, TBM_SETPOS, 1, (next.value * 100.0).round() as isize);
                    repaint();
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE => {
                let point = super::point(lparam);
                let x = (point.x - LEFT - SIDE / 2) as f32;
                let y = (point.y - TOP - SIDE / 2) as f32;
                if message == WM_LBUTTONDOWN && x.hypot(y) <= RADIUS {
                    unsafe { SetCapture(window) };
                }
                if unsafe { GetCapture() } == window {
                    apply(Hsv::point(x, y, hsv.get().value));
                    if message == WM_LBUTTONUP {
                        let _ = unsafe { ReleaseCapture() };
                    }
                }
            }
            _ => return None,
        }
        Some(LRESULT(0))
    })
}
