use std::cell::{Cell, RefCell};
use std::fs::File;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow};
use windows::Win32::UI::Controls::{LVM_ENSUREVISIBLE, LVM_GETTOPINDEX, LVM_SCROLL};

use super::{App, Controls, send};

pub(super) struct Measurement {
    path: PathBuf,
    started: Instant,
    first: Cell<Option<f64>>,
    completed: Cell<bool>,
    startup: RefCell<Vec<(&'static str, f64)>>,
}

impl Measurement {
    pub fn requested() -> Option<Self> {
        Some(Self {
            path: std::env::var_os("FIREWOL_M1_LIST_REPORT")?.into(),
            started: Instant::now(),
            first: Cell::new(None),
            completed: Cell::new(false),
            startup: RefCell::new(Vec::new()),
        })
    }

    pub fn mark(&self, phase: &'static str) {
        self.startup
            .borrow_mut()
            .push((phase, self.started.elapsed().as_secs_f64() * 1000.0));
    }

    pub fn tick(&self, app: &App, window: HWND, controls: Controls) -> Result<()> {
        if self.completed.get() {
            return Ok(());
        }
        let (count, complete) = {
            let model = app.model.borrow();
            let Some(folder) = &model.folder else {
                return Ok(());
            };
            (folder.count, folder.complete)
        };
        if count == 0 {
            return Ok(());
        }
        if self.first.get().is_none() {
            present(window)?;
            self.first
                .set(Some(self.started.elapsed().as_secs_f64() * 1000.0));
        }
        if !complete {
            return Ok(());
        }
        self.completed.set(true);
        let started = Instant::now();
        let mut frames = 0;
        while started.elapsed() < Duration::from_secs(3) {
            app.events(window, controls)?;
            send(controls.list, LVM_SCROLL, 0, 84);
            present(controls.list)?;
            frames += 1;
        }
        let fps = f64::from(frames) / started.elapsed().as_secs_f64();
        ensure!(
            send(controls.list, LVM_GETTOPINDEX, 0, 0) > 0,
            "The virtual list did not scroll"
        );
        send(controls.list, LVM_ENSUREVISIBLE, count as usize - 1, 0);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            app.events(window, controls)?;
            if app
                .model
                .borrow_mut()
                .item(count - 1, &app.workers)
                .is_some()
            {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "The virtual list did not load its last row"
            );
            thread::sleep(Duration::from_millis(5));
        }
        present(window)?;
        serde_json::to_writer(
            File::create_new(&self.path).context("Create the M1 list measurement report")?,
            &serde_json::json!({"rows": count, "first_list_ms": self.first.get(), "scroll_fps": fps, "last_row_verified": true, "startup": *self.startup.borrow()}),
        )?;
        Ok(())
    }
}

fn present(window: HWND) -> Result<()> {
    unsafe {
        let _ = RedrawWindow(
            Some(window),
            None,
            None,
            RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
        );
        DwmFlush()?;
    }
    Ok(())
}
