use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

pub(super) struct Chrome {
    factory: ID2D1Factory,
    target: Option<ID2D1HwndRenderTarget>,
    size: D2D_SIZE_U,
}

impl Chrome {
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            factory: unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? },
            target: None,
            size: D2D_SIZE_U::default(),
        })
    }

    pub fn paint(&mut self, window: HWND) {
        let mut paint = PAINTSTRUCT::default();
        unsafe { BeginPaint(window, &mut paint) };
        if self.draw(window).is_err() {
            self.target = None;
        }
        let _ = unsafe { EndPaint(window, &paint) };
    }

    pub fn ready(&self) -> bool {
        self.target.is_some()
    }

    fn draw(&mut self, window: HWND) -> windows::core::Result<()> {
        let mut bounds = RECT::default();
        unsafe { GetClientRect(window, &mut bounds)? };
        if bounds.right == 0 || bounds.bottom == 0 {
            return Ok(());
        }
        let size = D2D_SIZE_U {
            width: bounds.right as u32,
            height: bounds.bottom as u32,
        };
        if self.target.is_none() {
            self.target = Some(unsafe {
                self.factory.CreateHwndRenderTarget(
                    &D2D1_RENDER_TARGET_PROPERTIES::default(),
                    &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd: window,
                        pixelSize: size,
                        ..Default::default()
                    },
                )?
            });
            self.size = size;
        }
        let Some(target) = self.target.as_ref() else {
            return Ok(());
        };
        unsafe {
            if size != self.size {
                target.Resize(&size)?;
                self.size = size;
            }
            let brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.76,
                    g: 0.30,
                    b: 0.12,
                    a: 1.0,
                },
                None,
            )?;
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.95,
                g: 0.95,
                b: 0.95,
                a: 1.0,
            }));
            target.FillRectangle(
                &D2D_RECT_F {
                    left: 18.0,
                    top: 48.0,
                    right: 58.0,
                    bottom: 51.0,
                },
                &brush,
            );
            target.EndDraw(None, None)?;
        }
        Ok(())
    }
}
