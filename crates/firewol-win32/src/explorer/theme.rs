use std::ffi::OsStr;

use firewol_core::explorer::{Appearance, CaptionShape, FolderStyle, Preset, Rgb};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::controls::{get_text, send, wide};

pub(super) struct Brush(HBRUSH);

impl Brush {
    fn new(color: Rgb) -> Self {
        Self(unsafe { CreateSolidBrush(colorref(color)) })
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

pub(super) struct Theme {
    pub appearance: Appearance,
    pub background: Rgb,
    pub surface: Rgb,
    pub text: Rgb,
    pub accent: Rgb,
    pub folder: Rgb,
    pub folder_label: Rgb,
    pub border: Rgb,
    pub muted: Rgb,
    pub radius: i32,
    pub gradient: u8,
    pub shadow: i32,
    background_brush: Brush,
    surface_brush: Brush,
}

impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        use Preset::*;
        let (background, surface, text, accent, folder, radius, gradient, shadow) =
            match appearance.preset {
                Neubrutalism => (0xfff2a8, 0xfffdf2, 0x18181a, 0xff714d, 0x764bff, 0, 0, 2),
                Skeuomorphism => (0x342c25, 0x524336, 0xffefdb, 0xe9ad68, 0xd39a57, 7, 24, 2),
                Neumorphism => (0xe0e6ed, 0xe0e6ed, 0x263546, 0x42658b, 0x668ab1, 12, 7, 2),
                Glassmorphism => (0x1b273c, 0x344662, 0xf4f8ff, 0x98d1ff, 0x7dbdff, 14, 19, 0),
                Claymorphism => (0xf0e5fb, 0xfbf2ff, 0x392b4b, 0xb67cde, 0xb990e5, 20, 14, 2),
                LightMorphism => (0xf3f5f8, 0xffffff, 0x232a34, 0x2468ca, 0xc78820, 8, 3, 0),
                MetalMorphism => (0xc5cbd2, 0xe3e7eb, 0x222b32, 0x376478, 0x597789, 4, 32, 1),
            };
        let background = appearance.background.unwrap_or(rgb(background));
        let surface = appearance.surface.unwrap_or(rgb(surface));
        let text = appearance.text.unwrap_or(rgb(text));
        let accent = appearance.accent.unwrap_or(rgb(accent));
        Self {
            appearance,
            background,
            surface,
            text,
            accent,
            folder: appearance.folder.unwrap_or(rgb(folder)),
            folder_label: appearance.folder_label.unwrap_or(text),
            border: if appearance.preset == Neubrutalism {
                text
            } else {
                mix(surface, text, 22)
            },
            muted: mix(text, surface, 36),
            radius,
            gradient,
            shadow,
            background_brush: Brush::new(background),
            surface_brush: Brush::new(surface),
        }
    }

    pub fn erase(&self, dc: HDC, window: HWND) {
        let mut bounds = RECT::default();
        if unsafe { GetClientRect(window, &mut bounds) }.is_ok() {
            unsafe { FillRect(dc, &bounds, self.background_brush.0) };
        }
    }

    pub fn control_color(&self, dc: HDC, surface: bool) -> LRESULT {
        let (background, brush) = if surface {
            (self.surface, &self.surface_brush)
        } else {
            (self.background, &self.background_brush)
        };
        unsafe {
            SetTextColor(dc, colorref(self.text));
            SetBkColor(dc, colorref(background));
        }
        LRESULT(brush.0.0 as isize)
    }

    pub fn list(&self, list: HWND) {
        send(list, LVM_SETBKCOLOR, 0, colorref(self.surface).0 as isize);
        send(list, LVM_SETTEXTBKCOLOR, 0, CLR_NONE as isize);
        send(list, LVM_SETTEXTCOLOR, 0, colorref(self.text).0 as isize);
    }

    pub fn button(
        &self,
        info: &DRAWITEMSTRUCT,
        glyph: Option<&str>,
        selected: bool,
        caption: bool,
        base: Rgb,
    ) {
        let mut rect = info.rcItem;
        fill(info.hDC, rect, base);
        let pressed = info.itemState.0 & ODS_SELECTED.0 != 0;
        let disabled = info.itemState.0 & ODS_DISABLED.0 != 0;
        let hover = !disabled && (pressed || is_hovered(info.hwndItem));
        let mut face = self.surface;
        let mut radius = self.radius;
        let mut border = self.border;
        if caption {
            let shape = self.appearance.caption_shape;
            radius = match shape {
                CaptionShape::Rounded => 9,
                CaptionShape::Square | CaptionShape::Minimal => 0,
                CaptionShape::Circle => 100,
            };
            if matches!(shape, CaptionShape::Circle) {
                let inset = (rect.right - rect.left - (rect.bottom - rect.top)).max(0) / 2;
                rect.left += inset;
                rect.right -= inset;
            }
            face = self.appearance.caption_fill.unwrap_or(self.surface);
            if matches!(shape, CaptionShape::Minimal)
                && !hover
                && self.appearance.caption_fill.is_none()
            {
                face = self.background;
                border = face;
            }
            if hover {
                face = self.appearance.caption_hover.unwrap_or(self.accent);
            }
        } else if selected || hover {
            face = mix(
                self.surface,
                self.accent,
                if pressed || selected { 32 } else { 19 },
            );
            border = self.accent;
        }
        inset(&mut rect, 1, 1);
        if self.shadow > 0 && !pressed && !caption {
            let mut shadow = rect;
            shadow.left += self.shadow;
            shadow.top += self.shadow;
            if self.appearance.preset == Preset::Neumorphism {
                let mut light = rect;
                light.right -= self.shadow;
                light.bottom -= self.shadow;
                let highlight = mix(face, Rgb(255, 255, 255), 70);
                fill_shape(info.hDC, light, highlight, highlight, radius);
            }
            fill_shape(info.hDC, shadow, self.border, self.border, radius);
            rect.right -= self.shadow;
            rect.bottom -= self.shadow;
        }
        fill_shape(info.hDC, rect, face, border, radius);
        if self.gradient >= 12 && !hover && !selected && !caption {
            let mut highlight = rect;
            inset(&mut highlight, (radius / 2 + 1).max(2), 2);
            highlight.bottom = highlight.top + (rect.bottom - rect.top) / 3;
            gradient(
                info.hDC,
                highlight,
                mix(face, Rgb(255, 255, 255), self.gradient),
                face,
            );
        }
        let text = if disabled {
            self.muted
        } else if caption && (hover || self.appearance.caption_fill.is_some()) {
            readable_on(face)
        } else {
            self.text
        };
        let title;
        let label = if let Some(glyph) = glyph {
            OsStr::new(glyph)
        } else {
            title = get_text(info.hwndItem);
            &title
        };
        draw_text(info.hDC, rect, label, text, DT_CENTER);
        if info.itemState.0 & ODS_FOCUS.0 != 0 && info.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
            inset(&mut rect, 4, 4);
            let _ = unsafe { DrawFocusRect(info.hDC, &rect) };
        }
    }

    pub fn header(&self, dc: HDC, rect: RECT) {
        let top = mix(self.background, self.surface, self.gradient);
        gradient(dc, rect, top, self.background);
        match self.appearance.preset {
            Preset::Skeuomorphism => {
                for left in (rect.left + 8..rect.right - 8).step_by(9) {
                    fill(
                        dc,
                        RECT {
                            left,
                            right: left + 4,
                            top: rect.bottom - 4,
                            bottom: rect.bottom - 3,
                        },
                        self.muted,
                    );
                }
            }
            Preset::MetalMorphism => {
                let highlight = mix(top, Rgb(255, 255, 255), 22);
                for top in (rect.top + 1..rect.bottom - 1).step_by(4) {
                    fill(
                        dc,
                        RECT {
                            top,
                            bottom: top + 1,
                            ..rect
                        },
                        highlight,
                    );
                }
            }
            _ => {}
        }
    }

    pub fn tab(&self, dc: HDC, mut rect: RECT, label: &OsStr, selected: bool) {
        inset(&mut rect, 1, 1);
        let face = if selected {
            self.surface
        } else {
            self.background
        };
        fill_shape(dc, rect, face, self.border, self.radius.min(10));
        if selected {
            fill(
                dc,
                RECT {
                    top: rect.bottom - 3,
                    left: rect.left + 5,
                    right: rect.right - 5,
                    ..rect
                },
                self.accent,
            );
        }
        inset(&mut rect, 9, 2);
        draw_text(dc, rect, label, self.text, DT_LEFT);
    }

    pub fn folder_icon(&self, dc: HDC, mut rect: RECT, style: Option<&FolderStyle>) {
        let color = style.and_then(|style| style.color).unwrap_or(self.folder);
        rect.right = (rect.left + 16).min(rect.right);
        rect.bottom = (rect.top + 16).min(rect.bottom);
        if let Some(style) = style.filter(|style| !style.icon.is_empty()) {
            draw_text(dc, rect, OsStr::new(&style.icon), color, DT_CENTER);
            return;
        }
        let body = RECT {
            top: rect.top + 5,
            bottom: rect.bottom - 2,
            ..rect
        };
        let tab = RECT {
            right: rect.left + 8,
            top: rect.top + 3,
            bottom: body.top + 2,
            ..rect
        };
        fill_shape(dc, tab, color, color, 3);
        fill_shape(dc, body, mix(color, Rgb(255, 255, 255), 18), color, 4);
    }
}

pub(super) fn rgb(hex: u32) -> Rgb {
    Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub(super) fn colorref(Rgb(r, g, b): Rgb) -> COLORREF {
    COLORREF(u32::from_le_bytes([r, g, b, 0]))
}

pub(super) fn mix(a: Rgb, b: Rgb, percent: u8) -> Rgb {
    let p = u32::from(percent);
    let blend = |a: u8, b: u8| ((u32::from(a) * (100 - p) + u32::from(b) * p) / 100) as u8;
    Rgb(blend(a.0, b.0), blend(a.1, b.1), blend(a.2, b.2))
}

pub(super) fn readable_on(Rgb(r, g, b): Rgb) -> Rgb {
    if 2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b) > 1_420_000 {
        Rgb(19, 24, 31)
    } else {
        Rgb(255, 255, 255)
    }
}

pub(super) fn fill(dc: HDC, rect: RECT, color: Rgb) {
    let brush = Brush::new(color);
    unsafe { FillRect(dc, &rect, brush.0) };
}

pub(super) fn gradient(dc: HDC, rect: RECT, top: Rgb, bottom: Rgb) {
    let vertex = |x, y, Rgb(r, g, b)| TRIVERTEX {
        x,
        y,
        Red: u16::from(r) << 8,
        Green: u16::from(g) << 8,
        Blue: u16::from(b) << 8,
        Alpha: 0,
    };
    let corners = [
        vertex(rect.left, rect.top, top),
        vertex(rect.right, rect.bottom, bottom),
    ];
    let region = GRADIENT_RECT {
        UpperLeft: 0,
        LowerRight: 1,
    };
    let _ = unsafe {
        GdiGradientFill(
            dc,
            &corners,
            std::ptr::from_ref(&region).cast(),
            1,
            GRADIENT_FILL_RECT_V,
        )
    };
}

fn fill_shape(dc: HDC, rect: RECT, face: Rgb, border: Rgb, radius: i32) {
    let brush = Brush::new(face);
    let pen = unsafe { CreatePen(PS_SOLID, 1, colorref(border)) };
    let diameter = radius
        .min(rect.right - rect.left)
        .min(rect.bottom - rect.top)
        .max(1);
    unsafe {
        let old_pen = SelectObject(dc, pen.into());
        let old_brush = SelectObject(dc, brush.0.into());
        let _ = RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            diameter,
            diameter,
        );
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(pen.into());
    }
}

fn inset(rect: &mut RECT, x: i32, y: i32) {
    rect.left += x;
    rect.right -= x;
    rect.top += y;
    rect.bottom -= y;
}

pub(super) fn draw_text(
    dc: HDC,
    mut rect: RECT,
    label: &OsStr,
    color: Rgb,
    align: DRAW_TEXT_FORMAT,
) {
    let mut label = wide(label);
    let end = label.len() - 1;
    unsafe {
        let old_font = SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
        let old_mode = SetBkMode(dc, TRANSPARENT);
        let old_color = SetTextColor(dc, colorref(color));
        DrawTextW(
            dc,
            &mut label[..end],
            &mut rect,
            align | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SetTextColor(dc, old_color);
        SetBkMode(dc, BACKGROUND_MODE(old_mode as u32));
        SelectObject(dc, old_font);
    }
}

pub(super) fn track_hover(control: HWND) {
    let _ = unsafe { SetWindowSubclass(control, Some(hover_subclass), 0, 0) };
}

unsafe extern "system" fn hover_subclass(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    hovered: usize,
) -> LRESULT {
    if message == WM_MOUSEMOVE && hovered == 0 {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: window,
            dwHoverTime: 0,
        };
        if unsafe { TrackMouseEvent(&mut tracking) }.is_ok() {
            let _ = unsafe { SetWindowSubclass(window, Some(hover_subclass), id, 1) };
            let _ = unsafe { InvalidateRect(Some(window), None, false) };
        }
    } else if message == WM_MOUSELEAVE {
        let _ = unsafe { SetWindowSubclass(window, Some(hover_subclass), id, 0) };
        let _ = unsafe { InvalidateRect(Some(window), None, false) };
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

fn is_hovered(window: HWND) -> bool {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point).is_ok() && WindowFromPoint(point) == window }
}
