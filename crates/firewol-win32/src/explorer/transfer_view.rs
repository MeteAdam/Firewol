use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use firewol_core::explorer::{TransferControl, TransferState};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::controls::{place, set_text};
use super::form::Dialog;

pub(super) const TICK_MS: u32 = 250;
const SAMPLES: usize = 60;
const MIB: f64 = 1_048_576.0;
const PLOT: RECT = RECT {
    left: 16,
    top: 132,
    right: 514,
    bottom: 243,
};

pub(super) struct TransferView {
    control: TransferControl,
    total: usize,
    bytes: u64,
    sampled: Instant,
    speed: VecDeque<f64>,
    result: Option<String>,
}

impl TransferView {
    pub fn new(control: TransferControl, total: usize) -> Self {
        let mut speed = VecDeque::with_capacity(SAMPLES);
        speed.push_back(0.0);
        Self {
            control,
            total,
            bytes: 0,
            sampled: Instant::now(),
            speed,
            result: None,
        }
    }

    pub fn sample(&mut self) {
        if self.result.is_none()
            && self.sampled.elapsed() >= Duration::from_millis(u64::from(TICK_MS))
        {
            self.record_sample();
        }
    }

    fn record_sample(&mut self) {
        let now = Instant::now();
        let seconds = (now - self.sampled).as_secs_f64();
        if seconds > 0.0 {
            let bytes = self.control.progress().bytes;
            if self.speed.len() == SAMPLES {
                self.speed.pop_front();
            }
            self.speed
                .push_back(bytes.saturating_sub(self.bytes) as f64 / MIB / seconds);
            self.bytes = bytes;
            self.sampled = now;
        }
    }

    pub fn finish(&mut self, message: String) {
        self.record_sample();
        self.result = Some(message);
    }

    fn peak(&self) -> f64 {
        self.speed.iter().copied().fold(1.0, f64::max)
    }
}

pub(super) fn show(owner: HWND, last: &RefCell<Option<TransferView>>) -> Result<()> {
    let dialog = Dialog::create(owner, w!("Transfers"), 550, 348)?;
    let status = dialog.text(10, "", 16, 12, 498)?;
    place(status, 16, 12, 498, 64);
    let rate = dialog.text(11, "", 16, 86, 498)?;
    let axis = dialog.text(12, "", 16, 109, 498)?;
    let pause = dialog.button(20, w!("Pause"), 16, 268, 90)?;
    let cancel = dialog.button(21, w!("Cancel transfer"), 114, 268, 110)?;
    let close = dialog.button(2, w!("Close"), 422, 268, 92)?;

    let update = || {
        let mut last = last.borrow_mut();
        if let Some(view) = last.as_mut() {
            view.sample();
            let progress = view.control.progress();
            let phase = match progress.state {
                TransferState::Paused => "Paused",
                TransferState::Cancelled => "Cancelling",
                TransferState::Running if progress.verifying > 0 => "Verifying SHA-256",
                TransferState::Running => "Transferring",
            };
            set_text(
                status,
                view.result.clone().unwrap_or_else(|| {
                    format!(
                        "{phase} · {}/{} items processed",
                        progress.finished, view.total
                    )
                }),
            );
            let speed = if view.result.is_some() {
                0.0
            } else {
                view.speed.back().copied().unwrap_or_default()
            };
            set_text(
                rate,
                format!(
                    "{:.1} MiB copied · {:.1} MiB/s",
                    progress.bytes as f64 / MIB,
                    speed
                ),
            );
            set_text(
                axis,
                format!("Copy speed over time · scale 0–{:.1} MiB/s", view.peak()),
            );
            set_text(
                pause,
                if progress.state == TransferState::Paused {
                    "Resume"
                } else {
                    "Pause"
                },
            );
            let enabled = view.result.is_none() && progress.state != TransferState::Cancelled;
            unsafe {
                let _ = EnableWindow(pause, enabled);
                let _ = EnableWindow(cancel, enabled);
            }
        } else {
            set_text(status, "No transfer in this session.");
            unsafe {
                let _ = EnableWindow(pause, false);
                let _ = EnableWindow(cancel, false);
            }
        }
        let _ = unsafe { InvalidateRect(Some(dialog.window), Some(&PLOT), false) };
    };
    update();
    unsafe { SetTimer(Some(dialog.window), 1, TICK_MS, None) };
    dialog.run::<()>(close, &[], |_, message, wparam, _| {
        match message {
            WM_TIMER => update(),
            WM_PAINT => paint(dialog.window, last.borrow().as_ref()),
            WM_COMMAND if matches!(wparam.0 & 0xffff, 20 | 21) => {
                if let Some(view) = last.borrow().as_ref().filter(|view| view.result.is_none()) {
                    if wparam.0 & 0xffff == 20 {
                        view.control.toggle_pause();
                    } else {
                        view.control.cancel();
                    }
                }
                update();
            }
            _ => return None,
        }
        Some(LRESULT(0))
    })?;
    Ok(())
}

fn paint(window: HWND, view: Option<&TransferView>) {
    let mut paint = PAINTSTRUCT::default();
    unsafe {
        let dc = BeginPaint(window, &mut paint);
        FillRect(dc, &PLOT, GetSysColorBrush(COLOR_BTNFACE));
        FrameRect(dc, &PLOT, GetSysColorBrush(COLOR_3DSHADOW));
        if let Some(view) = view {
            let peak = view.peak();
            let pen = CreatePen(PS_SOLID, 2, COLORREF(GetSysColor(COLOR_HIGHLIGHT)));
            let previous = SelectObject(dc, pen.into());
            let first = SAMPLES - view.speed.len();
            for (i, speed) in view.speed.iter().enumerate() {
                let x = PLOT.left
                    + 2
                    + (PLOT.right - PLOT.left - 4) * (first + i) as i32 / (SAMPLES - 1) as i32;
                let y =
                    PLOT.bottom - 3 - ((PLOT.bottom - PLOT.top - 6) as f64 * speed / peak) as i32;
                if i == 0 {
                    let _ = MoveToEx(dc, x, y, None);
                } else {
                    let _ = LineTo(dc, x, y);
                }
            }
            SelectObject(dc, previous);
            let _ = DeleteObject(pen.into());
        }
        let _ = EndPaint(window, &paint);
    }
}
