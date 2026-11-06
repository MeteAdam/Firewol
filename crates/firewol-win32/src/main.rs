#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chrome;

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use firewol_core::{Entry, Fixture, PageRequest, SearchRequest, fixture_root};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    DEFAULT_GUI_FONT, GetStockObject, RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use chrome::Chrome;

const PAGE_SIZE: u32 = 128;
const WINDOW_CLASS: PCWSTR = w!("FirewolM0Window");

#[derive(Clone, Copy)]
struct Controls {
    edit: HWND,
    list: HWND,
    status: HWND,
}

struct Model {
    fixture: Fixture,
    listing: Vec<Entry>,
    page_offset: Option<u32>,
    results: Option<Vec<Entry>>,
}

impl Model {
    fn name(&mut self, row: u32) -> Result<Option<&str>> {
        if let Some(results) = &self.results {
            return Ok(results.get(row as usize).map(|item| item.name.as_str()));
        }
        let offset = row / PAGE_SIZE * PAGE_SIZE;
        if self.page_offset != Some(offset) {
            self.listing = self
                .fixture
                .list(PageRequest {
                    offset,
                    limit: PAGE_SIZE,
                })?
                .items;
            self.page_offset = Some(offset);
        }
        Ok(self
            .listing
            .get((row - offset) as usize)
            .map(|item| item.name.as_str()))
    }
}

struct App {
    model: RefCell<Model>,
    controls: Cell<Option<Controls>>,
    chrome: RefCell<Chrome>,
    info: String,
    error: RefCell<Option<String>>,
}

impl App {
    fn update_search(&self, controls: Controls) -> Result<()> {
        let mut text = [0; 257];
        let length = unsafe { GetWindowTextW(controls.edit, &mut text) } as usize;
        let text = String::from_utf16_lossy(&text[..length]);
        let (count, status) = {
            let mut model = self.model.borrow_mut();
            if text.trim().is_empty() {
                model.results = None;
                (model.fixture.info.entries as usize, self.info.clone())
            } else {
                let page = model.fixture.search(&SearchRequest {
                    text,
                    extension: None,
                    page: PageRequest {
                        offset: 0,
                        limit: PAGE_SIZE,
                    },
                })?;
                let count = page.items.len();
                let status = if page.has_more {
                    format!("Showing first {count} matches")
                } else {
                    format!("{count} matches")
                };
                model.results = Some(page.items);
                (count, status)
            }
        };
        unsafe {
            SendMessageW(controls.list, LVM_SETITEMCOUNT, Some(WPARAM(count)), None);
            if count > 0 {
                SendMessageW(controls.list, LVM_ENSUREVISIBLE, Some(WPARAM(0)), None);
            }
            set_text(controls.status, &status)?;
        }
        Ok(())
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // The owner keeps the boxed App alive through window destruction.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let data = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const App;
    if let Some(app) = unsafe { data.as_ref() } {
        match message {
            WM_SIZE => {
                if let Some(controls) = app.controls.get() {
                    resize(window, controls);
                }
            }
            WM_PAINT => {
                app.chrome.borrow_mut().paint(window);
                return LRESULT(0);
            }
            WM_COMMAND => {
                if let Some(controls) = app.controls.get()
                    && lparam.0 == controls.edit.0 as isize
                    && (wparam.0 >> 16) as u32 == EN_CHANGE
                    && let Err(error) = app.update_search(controls)
                {
                    let message = error.to_string();
                    let _ = unsafe { set_text(controls.status, &message) };
                    *app.error.borrow_mut() = Some(message);
                }
            }
            WM_NOTIFY if lparam.0 != 0 => {
                let header = unsafe { &*(lparam.0 as *const NMHDR) };
                if app
                    .controls
                    .get()
                    .is_some_and(|controls| controls.list == header.hwndFrom)
                    && header.code == LVN_GETDISPINFOW
                {
                    let info = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                    if info.item.mask.contains(LVIF_TEXT)
                        && info.item.iItem >= 0
                        && !info.item.pszText.is_null()
                        && info.item.cchTextMax > 0
                    {
                        // The ListView provides a writable buffer for this notification.
                        let text = unsafe {
                            std::slice::from_raw_parts_mut(
                                info.item.pszText.0,
                                info.item.cchTextMax as usize,
                            )
                        };
                        let mut model = app.model.borrow_mut();
                        let name = match model.name(info.item.iItem as u32) {
                            Ok(name) => name.unwrap_or(""),
                            Err(error) => {
                                *app.error.borrow_mut() = Some(error.to_string());
                                "Unable to read entry"
                            }
                        };
                        let mut written = 0;
                        let capacity = text.len() - 1;
                        for unit in name.encode_utf16().take(capacity) {
                            text[written] = unit;
                            written += 1;
                        }
                        text[written] = 0;
                    }
                }
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn resize(window: HWND, controls: Controls) {
    let mut bounds = RECT::default();
    if unsafe { GetClientRect(window, &mut bounds) }.is_err() {
        return;
    }
    let width = (bounds.right - 36).max(1);
    unsafe {
        let _ = MoveWindow(controls.edit, 18, 16, width, 28, true);
        let _ = MoveWindow(
            controls.list,
            18,
            55,
            width,
            (bounds.bottom - 91).max(1),
            true,
        );
        let _ = MoveWindow(
            controls.status,
            18,
            (bounds.bottom - 27).max(55),
            width,
            20,
            true,
        );
        SendMessageW(
            controls.list,
            LVM_SETCOLUMNWIDTH,
            Some(WPARAM(0)),
            Some(LPARAM((width - 22).max(1) as isize)),
        );
    }
}

unsafe fn set_text(window: HWND, text: &str) -> windows::core::Result<()> {
    let wide: Vec<_> = text.encode_utf16().chain([0]).collect();
    unsafe { SetWindowTextW(window, PCWSTR(wide.as_ptr())) }
}

fn create_controls(window: HWND, instance: HINSTANCE) -> Result<Controls> {
    let child = |class: PCWSTR, style: WINDOW_STYLE, frame: WINDOW_EX_STYLE| unsafe {
        CreateWindowExW(
            frame,
            class,
            w!(""),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            1,
            1,
            Some(window),
            None,
            Some(instance),
            None,
        )
    };
    let edit = child(
        w!("EDIT"),
        WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        WS_EX_CLIENTEDGE,
    )?;
    let list = child(
        WC_LISTVIEWW,
        WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_NOSORTHEADER),
        WS_EX_CLIENTEDGE,
    )?;
    let status = child(
        w!("STATIC"),
        WINDOW_STYLE::default(),
        WINDOW_EX_STYLE::default(),
    )?;
    let mut title: Vec<_> = "Name".encode_utf16().chain([0]).collect();
    let column = LVCOLUMNW {
        mask: LVCF_TEXT | LVCF_WIDTH,
        cx: 600,
        pszText: windows::core::PWSTR(title.as_mut_ptr()),
        ..Default::default()
    };
    unsafe {
        let font = GetStockObject(DEFAULT_GUI_FONT);
        for control in [edit, list, status] {
            SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(256)), None);
        SendMessageW(
            edit,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(w!("Search indexed names").0 as isize)),
        );
        ensure!(
            SendMessageW(
                list,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(0)),
                Some(LPARAM(std::ptr::from_ref(&column) as isize))
            )
            .0 != -1,
            "Insert virtual list column"
        );
    }
    Ok(Controls { edit, list, status })
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

fn benchmark(
    app: &App,
    window: HWND,
    controls: Controls,
    path: &Path,
    first_list_ms: f64,
) -> Result<()> {
    ensure!(
        app.chrome.borrow().ready(),
        "Direct2D did not present the window"
    );
    ensure!(
        app.model.borrow().page_offset == Some(0),
        "The virtual ListView did not request its first page"
    );
    let mut times = Vec::new();
    for query in ["ré", "cord", "port", "résumé", "record-0000"]
        .into_iter()
        .cycle()
        .take(25)
    {
        let start = Instant::now();
        unsafe { set_text(controls.edit, query)? };
        present(window)?;
        ensure!(
            app.model
                .borrow()
                .results
                .as_ref()
                .is_some_and(|items| !items.is_empty()),
            "Expected search results in the benchmark fixture"
        );
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    unsafe { set_text(controls.edit, "")? };
    let start = Instant::now();
    let mut frames = 0;
    while start.elapsed() < Duration::from_secs(3) {
        unsafe { SendMessageW(controls.list, LVM_SCROLL, None, Some(LPARAM(84))) };
        present(window)?;
        frames += 1;
    }
    let scroll_fps = f64::from(frames) / start.elapsed().as_secs_f64();
    ensure!(
        unsafe { SendMessageW(controls.list, LVM_GETTOPINDEX, None, None) }.0 > 0,
        "The virtual ListView did not scroll"
    );
    let entries = app.model.borrow().fixture.info.entries;
    ensure!(entries > 0, "Expected a populated benchmark fixture");
    unsafe {
        SendMessageW(
            controls.list,
            LVM_ENSUREVISIBLE,
            Some(WPARAM(entries as usize - 1)),
            None,
        )
    };
    present(window)?;
    ensure!(
        app.model.borrow_mut().name(entries - 1)?.is_some(),
        "Last virtual row was missing"
    );
    unsafe { SendMessageW(controls.list, LVM_ENSUREVISIBLE, Some(WPARAM(0)), None) };
    if let Some(error) = app.error.borrow().as_ref() {
        anyhow::bail!("{error}");
    }
    times.sort_by(f64::total_cmp);
    serde_json::to_writer(
        File::create_new(path)?,
        &serde_json::json!({
            "first_list_ms": first_list_ms,
            "search_p95_ms": times[23],
            "search_samples": times.len(),
            "scroll_fps": scroll_fps,
            "last_row_verified": true,
        }),
    )?;
    Ok(())
}

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn run() -> Result<()> {
    let start = Instant::now();
    let fixture = Fixture::open(&fixture_root()?)?;
    let info = format!(
        "{} files  ·  {} indexed names",
        fixture.info.entries, fixture.info.index_rows
    );
    let app = Box::new(App {
        model: RefCell::new(Model {
            fixture,
            listing: Vec::new(),
            page_offset: None,
            results: None,
        }),
        controls: Cell::new(None),
        chrome: RefCell::new(Chrome::new()?),
        info,
        error: RefCell::new(None),
    });
    let common = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LISTVIEW_CLASSES,
    };
    unsafe {
        ensure!(
            InitCommonControlsEx(&common).as_bool(),
            "Initialize Windows ListView"
        );
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let class = WNDCLASSW {
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpfnWndProc: Some(window_proc),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        ensure!(RegisterClassW(&class) != 0, "Register Firewol window class");
        let window = Window(CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WINDOW_CLASS,
            w!("Firewol M0 — Win32"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            680,
            None,
            None,
            Some(instance),
            Some(std::ptr::from_ref(app.as_ref()).cast()),
        )?);
        let controls = create_controls(window.0, instance)?;
        app.controls.set(Some(controls));
        resize(window.0, controls);
        app.update_search(controls)?;
        let _ = ShowWindow(window.0, SW_SHOW);
        present(window.0)?;
        if let Some(path) = std::env::var_os("FIREWOL_M0_REPORT") {
            benchmark(
                &app,
                window.0,
                controls,
                Path::new(&path),
                start.elapsed().as_secs_f64() * 1000.0,
            )?;
        }
        let mut message = MSG::default();
        loop {
            let next = GetMessageW(&mut message, None, 0, 0).0;
            ensure!(next >= 0, "Read the Win32 message queue");
            if next == 0 {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        if std::env::var_os("FIREWOL_M0_REPORT").is_none() {
            let message: Vec<_> = format!("{error:#}").encode_utf16().chain([0]).collect();
            unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(message.as_ptr()),
                    w!("Firewol M0"),
                    MB_ICONERROR,
                )
            };
        }
        std::process::exit(1);
    }
}
