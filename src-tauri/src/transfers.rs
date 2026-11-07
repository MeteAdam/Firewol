//! Samples the running transfer every 250 ms for the Transfers popover and keeps the last
//! result, so every window (including one opened mid-transfer) shows the same speed graph.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use firewol_core::explorer::{Session, TransferControl, TransferProgress, TransferState};

use crate::contract::{TransferPhase, TransferStatus};
use crate::folders::lock;

pub const TICK: Duration = Duration::from_millis(250);
const SAMPLES: usize = 60;

pub type Emit = Arc<dyn Fn(TransferStatus) + Send + Sync>;

#[derive(Default)]
pub struct Transfers(Mutex<Monitor>);

#[derive(Default)]
struct Monitor {
    generation: u64,
    control: Option<TransferControl>,
    started: Option<Instant>,
    sampled: Option<Instant>,
    sampled_bytes: u64,
    samples: VecDeque<f64>,
    last: Option<TransferStatus>,
    /// Set from `prepare` until `attach`, so a result that arrives first is kept.
    pending: bool,
    early: Option<(String, bool)>,
}

impl Monitor {
    fn status(&self, progress: &TransferProgress) -> TransferStatus {
        let recent = self.samples.iter().rev().take(4);
        let count = recent.len().max(1) as f64;
        TransferStatus {
            phase: match progress.state {
                TransferState::Running => TransferPhase::Running,
                TransferState::Paused => TransferPhase::Paused,
                TransferState::Cancelled => TransferPhase::Cancelled,
            },
            bytes: progress.bytes,
            total_bytes: progress.total_bytes,
            finished: progress.finished as u32,
            total_items: progress.total_items as u32,
            verifying: progress.verifying as u32,
            speed: recent.sum::<f64>() / count,
            samples: self.samples.iter().copied().collect(),
            elapsed_ms: self
                .started
                .map_or(0, |started| started.elapsed().as_millis() as u64),
            message: None,
            succeeded: None,
        }
    }

    fn sample(&mut self) -> Option<TransferStatus> {
        let progress = self.control.as_ref()?.progress();
        let now = Instant::now();
        if let Some(sampled) = self.sampled {
            let seconds = (now - sampled).as_secs_f64();
            if seconds > 0.0 {
                if self.samples.len() == SAMPLES {
                    self.samples.pop_front();
                }
                self.samples
                    .push_back(progress.bytes.saturating_sub(self.sampled_bytes) as f64 / seconds);
            }
        }
        self.sampled = Some(now);
        self.sampled_bytes = progress.bytes;
        Some(self.status(&progress))
    }

    /// Records the result; the speed samples stay for the popover's last-transfer view.
    fn finish(
        &mut self,
        message: Option<String>,
        succeeded: Option<bool>,
    ) -> Option<TransferStatus> {
        self.control.as_ref()?;
        let mut status = self.sample()?;
        status.phase = TransferPhase::Finished;
        status.message = message;
        status.succeeded = succeeded;
        self.control = None;
        self.last = Some(status.clone());
        Some(status)
    }
}

impl Transfers {
    /// Call before submitting a transfer, so its result cannot arrive unobserved.
    /// Returns 0 while another transfer is watched; the session refuses that submit anyway.
    pub fn prepare(&self) -> u64 {
        let mut monitor = lock(&self.0);
        if monitor.control.is_some() {
            return 0;
        }
        let generation = monitor.generation + 1;
        *monitor = Monitor {
            generation,
            pending: true,
            last: monitor.last.take(),
            ..Monitor::default()
        };
        generation
    }

    /// Forgets a prepared transfer the session refused.
    pub fn abandon(&self, generation: u64) {
        let mut monitor = lock(&self.0);
        if monitor.generation == generation {
            monitor.pending = false;
            monitor.early = None;
        }
    }

    /// Starts sampling the accepted job every 250 ms.
    pub fn attach(
        self: &Arc<Self>,
        generation: u64,
        control: TransferControl,
        session: Arc<Session>,
        emit: Emit,
    ) {
        let finished = {
            let mut monitor = lock(&self.0);
            if monitor.generation != generation {
                return;
            }
            monitor.pending = false;
            monitor.control = Some(control);
            monitor.started = Some(Instant::now());
            monitor.sample();
            monitor
                .early
                .take()
                .and_then(|(message, succeeded)| monitor.finish(Some(message), Some(succeeded)))
        };
        if let Some(status) = finished {
            emit(status);
            return;
        }
        let transfers = self.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(TICK);
                let mut monitor = lock(&transfers.0);
                if monitor.generation != generation || monitor.control.is_none() {
                    return;
                }
                // A job that stopped without reporting a result still ends the view.
                let status = if session.busy() {
                    monitor.sample()
                } else {
                    monitor.finish(None, None)
                };
                drop(monitor);
                let Some(status) = status else { return };
                let done = status.phase == TransferPhase::Finished;
                emit(status);
                if done {
                    return;
                }
            }
        });
    }

    /// Called with every operation result; only a prepared or watched transfer records it.
    pub fn finish(&self, message: &str, succeeded: bool) -> Option<TransferStatus> {
        let mut monitor = lock(&self.0);
        if monitor.pending {
            monitor.early = Some((message.to_owned(), succeeded));
            return None;
        }
        monitor.finish(Some(message.to_owned()), Some(succeeded))
    }

    /// The running transfer, or the last result once it finished.
    pub fn current(&self) -> Option<TransferStatus> {
        let monitor = lock(&self.0);
        match &monitor.control {
            Some(control) => Some(monitor.status(&control.progress())),
            None => monitor.last.clone(),
        }
    }
}
