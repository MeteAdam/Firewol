use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use firewol_core::explorer::elevation::{self, ElevationPolicy};
use firewol_core::explorer::{
    Cancellation, Directory, ItemId, ItemPage, Operation, OperationResult, Operations, PAGE_SIZE,
    SearchIndex, SearchQuery, SearchReader, TransferControl, TransferRequest, page,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub(super) const WAKE: u32 = WM_APP + 1;
pub(super) const FILE_WAKE: u32 = WM_APP + 2;

pub(super) type Ticket = u64;
pub(super) type Threads = Rc<RefCell<Vec<JoinHandle<()>>>>;

pub(super) fn finish(threads: Threads) {
    let threads = threads.take();
    // A network read or a pending Windows consent prompt can outlive the windows.
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline && threads.iter().any(|thread| !thread.is_finished()) {
        thread::sleep(Duration::from_millis(10));
    }
    for thread in threads.into_iter().filter(JoinHandle::is_finished) {
        let _ = thread.join();
    }
}

fn track(threads: &Threads, handle: JoinHandle<()>) {
    let mut threads = threads.borrow_mut();
    threads.retain(|thread| !thread.is_finished());
    threads.push(handle);
}

pub(super) struct Snapshot {
    pub path: PathBuf,
    pub id: Option<ItemId>,
    pub count: u32,
    pub complete: bool,
    pub page: ItemPage,
}

pub(super) enum Event {
    Navigated(Ticket, Result<Snapshot>),
    Count(Ticket, u32, bool),
    ListPage(Ticket, u32, Result<ItemPage>),
    ListError(Ticket, anyhow::Error),
    Indexed(Ticket, SearchIndex, u64),
    IndexError(Ticket, anyhow::Error),
    Results(Ticket, Result<ItemPage>),
}

pub(super) enum ListJob {
    Open(Ticket, PathBuf, Cancellation),
    Page(Ticket, u32),
    Clear,
}

pub(super) struct QueryJob(
    pub Ticket,
    pub SearchIndex,
    pub SearchQuery,
    pub Cancellation,
);

pub(super) enum FileJob {
    Run(Operation),
    Transfer(TransferRequest, TransferControl),
    Elevate {
        policy: ElevationPolicy,
        program: ItemId,
        argument: Option<ItemId>,
    },
    Undo(TransferControl),
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Scope {
    Folder(PathBuf),
    Volume(char),
}

pub(super) struct Workers {
    pub listing: Sender<ListJob>,
    pub search: Sender<QueryJob>,
    pub events: Receiver<Event>,
    sender: EventOutput,
    threads: Threads,
}

#[derive(Clone)]
struct EventOutput {
    sender: Sender<Event>,
    window: Arc<AtomicUsize>,
}

impl EventOutput {
    fn send(&self, event: Event) {
        if self.sender.send(event).is_err() {
            return;
        }
        let window = self.window.load(Ordering::Relaxed);
        if window != 0 {
            let _ =
                unsafe { PostMessageW(Some(HWND(window as *mut _)), WAKE, WPARAM(0), LPARAM(0)) };
        }
    }
}

impl Workers {
    pub fn start(threads: Threads) -> Self {
        let (sender, events) = mpsc::channel();
        let sender = EventOutput {
            sender,
            window: Arc::new(AtomicUsize::new(0)),
        };
        let (listing, list_jobs) = mpsc::channel();
        let (search, search_jobs) = mpsc::channel::<QueryJob>();
        let send = sender.clone();
        let list_thread = thread::spawn(move || list_loop(&list_jobs, &send));
        let send = sender.clone();
        let query_thread = thread::spawn(move || {
            let mut reader: Option<SearchReader> = None;
            loop {
                let QueryJob(ticket, index, query, cancel) =
                    match search_jobs.recv_timeout(Duration::from_secs(2)) {
                        Ok(job) => job,
                        Err(RecvTimeoutError::Timeout) => {
                            reader = None;
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                if cancel.is_cancelled() {
                    continue;
                }
                let result =
                    if let Some(current) = reader.as_mut().filter(|reader| reader.reads(&index)) {
                        current.search(&query, &cancel)
                    } else {
                        index.reader().and_then(|mut current| {
                            let page = current.search(&query, &cancel);
                            reader = Some(current);
                            page
                        })
                    };
                if !cancel.is_cancelled() {
                    send.send(Event::Results(ticket, result));
                }
            }
        });
        track(&threads, list_thread);
        track(&threads, query_thread);
        Self {
            listing,
            search,
            events,
            sender,
            threads,
        }
    }

    pub fn bind(&self, window: HWND) {
        self.sender
            .window
            .store(window.0 as usize, Ordering::Relaxed);
    }

    pub fn index(&self, ticket: Ticket, scope: Scope, data: PathBuf, cancel: Cancellation) {
        let send = self.sender.clone();
        let handle = thread::spawn(move || {
            let result = (|| {
                cancel.check()?;
                let index = SearchIndex::create(&data)?;
                let updated = |count| {
                    send.send(Event::Indexed(ticket, index.clone(), count));
                };
                match scope {
                    Scope::Folder(root) => index.watch_folder(&root, &data, &cancel, updated),
                    Scope::Volume(drive) => index.watch_volume(drive, &data, &cancel, updated),
                }
            })();
            if let Err(error) = result
                && !cancel.is_cancelled()
            {
                send.send(Event::IndexError(ticket, error));
            }
        });
        track(&self.threads, handle);
    }
}

pub(super) struct FileWorker {
    jobs: Sender<(usize, FileJob)>,
    pub results: Receiver<Result<OperationResult>>,
}

impl FileWorker {
    pub fn start(threads: &Threads) -> Self {
        let (jobs, incoming) = mpsc::channel();
        let (finished, results) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut operations = None;
            while let Ok((window, job)) = incoming.recv() {
                let result = match operations.get_or_insert_with(Operations::open) {
                    Ok(operations) => match job {
                        FileJob::Run(operation) => operations.submit_operation(operation),
                        FileJob::Transfer(request, control) => {
                            operations.transfer(request, &control)
                        }
                        FileJob::Elevate {
                            policy,
                            program,
                            argument,
                        } => elevation::launch(
                            &policy,
                            &program,
                            argument.as_ref(),
                            HWND(window as *mut _),
                        )
                        .map(|()| OperationResult {
                            message: "Launched with administrator consent".into(),
                            transferred: Vec::new(),
                        }),
                        FileJob::Undo(control) => operations.undo_with_control(&control),
                    },
                    Err(error) => Err(anyhow::anyhow!("Start the Windows Shell worker: {error:#}")),
                };
                if finished.send(result).is_err() {
                    break;
                }
                let _ = unsafe {
                    PostMessageW(
                        Some(HWND(window as *mut _)),
                        FILE_WAKE,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            }
        });
        track(threads, handle);
        Self { jobs, results }
    }

    pub fn submit(&self, window: HWND, job: FileJob) -> Result<()> {
        self.jobs.send((window.0 as usize, job))?;
        Ok(())
    }
}

fn list_loop(jobs: &Receiver<ListJob>, send: &EventOutput) {
    let mut active: Option<(Ticket, Cancellation, Directory)> = None;
    let mut last = Instant::now();
    let mut enumerating = false;
    loop {
        let job = if enumerating {
            match jobs.try_recv() {
                Ok(job) => Some(job),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => break,
            }
        } else {
            match jobs.recv() {
                Ok(job) => Some(job),
                Err(_) => break,
            }
        };
        match job {
            Some(ListJob::Open(ticket, path, cancel)) => {
                if cancel.is_cancelled() {
                    continue;
                }
                let result = (|| {
                    let mut directory = Directory::open(&path)?;
                    directory.advance(PAGE_SIZE, &cancel)?;
                    let first = directory.list_dir(page(0), &cancel)?;
                    cancel.check()?;
                    let path = directory.path().to_owned();
                    let snapshot = Snapshot {
                        id: ItemId::at(path.clone()).ok(),
                        path,
                        page: first,
                        count: directory.count(),
                        complete: directory.is_complete(),
                    };
                    enumerating = !snapshot.complete;
                    active = Some((ticket, cancel.clone(), directory));
                    last = Instant::now();
                    Ok(snapshot)
                })();
                if !cancel.is_cancelled() {
                    send.send(Event::Navigated(ticket, result));
                }
            }
            Some(ListJob::Page(ticket, offset)) => {
                if let Some((current, cancel, directory)) = &active
                    && ticket == *current
                {
                    send.send(Event::ListPage(
                        ticket,
                        offset,
                        directory.list_dir(page(offset), cancel),
                    ));
                }
            }
            Some(ListJob::Clear) => {
                active = None;
                enumerating = false;
            }
            None => {
                let Some((ticket, cancel, directory)) = &mut active else {
                    break;
                };
                match directory.advance(PAGE_SIZE * 4, cancel) {
                    Ok(()) => {
                        enumerating = !directory.is_complete();
                        if !enumerating || last.elapsed() >= Duration::from_millis(85) {
                            send.send(Event::Count(*ticket, directory.count(), !enumerating));
                            last = Instant::now();
                        }
                    }
                    Err(error) => {
                        enumerating = false;
                        if !cancel.is_cancelled() {
                            send.send(Event::ListError(*ticket, error));
                        }
                    }
                }
            }
        }
    }
}
