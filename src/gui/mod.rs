mod settings;
mod startup;
mod theme;

use self::{
    settings::Settings,
    startup::wide,
    theme::{Theme, scale},
};
use fasttimer::{
    Result,
    benchmark::{self, DEFAULT_CASES, Plan, Report},
    timer::{self, Case, Interval, Lease, Request, Snapshot},
};
use std::{
    cell::{OnceCell, RefCell},
    ffi::OsString,
    io,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute},
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{
        Controls::{DRAWITEMSTRUCT, Dialogs::*, EM_SETLIMITTEXT, EM_SETTABSTOPS, ShowScrollBar},
        HiDpi::{
            AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE, GetDpiForSystem,
            SetProcessDpiAwarenessContext,
        },
        Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
        Shell::*,
        WindowsAndMessaging::*,
    },
};

const WIDTH: i32 = 432;
const HEIGHT: i32 = 372;
const CLASS: &str = "FastTimer.Main.0";
const TRAY_EVENT: u32 = WM_APP + 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
const SHOW: u32 = WM_APP + 2;
const FINISHED: u32 = WM_APP + 3;
const QUIT: u32 = WM_APP + 4;
const REFRESH: usize = 1;
const FASTEST: u16 = 100;
const APPLY: u16 = 101;
const RESET: u16 = 102;
const BENCH: u16 = 103;
const JSON: u16 = 104;
const CSV: u16 = 105;
const STARTUP: u16 = 106;
const EXIT: u16 = 107;
const SYSTEM_EXIT: usize = 0x0100;
const OPEN: u16 = 108;
const CUSTOM: u16 = 110;
const CASES: u16 = 111;
const RESULTS: u16 = 112;
const REPORTED: u16 = 113;
const REQUEST: u16 = 114;
const FEEDBACK: u16 = 119;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct App {
    theme: Theme,
    module: HINSTANCE,
    icon: HICON,
    taskbar_created: u32,
    state: RefCell<State>,
    controls: OnceCell<Controls>,
    background_policy: bool,
}

struct Controls {
    reported: HWND,
    request: HWND,
    custom: HWND,
    cases: HWND,
    results: HWND,
    feedback: HWND,
    startup: HWND,
    run: HWND,
}

struct State {
    lease: Option<Lease>,
    last_query: Option<Interval>,
    worker: Option<Worker>,
    restore: Request,
    report: Option<Report>,
    shortcut: Result<PathBuf>,
    has_tray: bool,
    settings: Settings,
}

struct Worker {
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<Result<Report>>,
}

pub fn message(text: &str, error: bool) {
    let text = wide(text);
    let caption = wide("FastTimer");
    unsafe {
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK
                | if error {
                    MB_ICONERROR
                } else {
                    MB_ICONINFORMATION
                },
        )
    };
}

pub fn run(hidden: bool, preset: Case) -> Result<()> {
    let class = wide(CLASS);
    let mutex_name = wide("Local\\FastTimer.Desktop.0");
    // The named handle is held until the message loop and app state are gone.
    let mutex = unsafe { CreateMutexW(null(), 0, mutex_name.as_ptr()) };
    let mutex_status = unsafe { GetLastError() };
    if mutex.is_null() {
        return Err(io::Error::from_raw_os_error(mutex_status as i32).into());
    }
    let _mutex = Handle(mutex);
    if mutex_status == ERROR_ALREADY_EXISTS {
        if !hidden {
            let previous = unsafe { FindWindowW(class.as_ptr(), null()) };
            if !previous.is_null() {
                unsafe { PostMessageW(previous, SHOW, 0, 0) };
            }
        }
        return Ok(());
    }

    let range = timer::query()?;
    let initial = Request::resolve(preset, range)?;
    let background = timer::honor_background_requests();
    let lease = if initial == Request::None {
        None
    } else {
        Some(Lease::acquire(initial)?)
    };
    let title = wide("FastTimer");
    let taskbar = wide("TaskbarCreated");
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE) };
    let dpi = unsafe { GetDpiForSystem() }.max(96);
    let module = unsafe { GetModuleHandleW(null()) };
    let mut icon = unsafe { LoadIconW(module, 101usize as *const u16) };
    if icon.is_null() {
        icon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
    }
    let app = Box::new(App {
        theme: Theme::new(dpi)?,
        module,
        icon,
        taskbar_created: unsafe { RegisterWindowMessageW(taskbar.as_ptr()) },
        state: RefCell::new(State {
            lease,
            last_query: None,
            worker: None,
            restore: Request::None,
            report: None,
            shortcut: startup::shortcut_path(),
            has_tray: false,
            settings: Settings::load(),
        }),
        controls: OnceCell::new(),
        background_policy: background.is_ok(),
    });
    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: module,
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let mut rect = RECT {
        right: scale(dpi, WIDTH),
        bottom: scale(dpi, HEIGHT),
        ..Default::default()
    };
    unsafe { AdjustWindowRectExForDpi(&mut rect, style, 0, 0, dpi) };
    let mut work = RECT::default();
    let positioned =
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut work).cast(), 0) } != 0;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let (x, y) = if positioned {
        (
            work.left + ((work.right - work.left - width) / 2).max(0),
            work.top + ((work.bottom - work.top - height) / 2).max(0),
        )
    } else {
        (CW_USEDEFAULT, CW_USEDEFAULT)
    };
    // App stays at this address until DestroyWindow returns.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            style,
            x,
            y,
            width,
            height,
            null_mut(),
            null_mut(),
            module,
            (&raw const *app).cast(),
        )
    };
    if hwnd.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    let result = run_window(hwnd, &app, range, hidden, background);
    if unsafe { IsWindow(hwnd) } != 0 {
        unsafe { DestroyWindow(hwnd) };
    }
    result
}

pub fn exit() -> Result<()> {
    let class = wide(CLASS);
    let window = unsafe { FindWindowW(class.as_ptr(), null()) };
    if !window.is_null() && unsafe { PostMessageW(window, QUIT, 0, 0) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn run_window(
    hwnd: HWND,
    app: &App,
    range: Snapshot,
    hidden: bool,
    background: io::Result<()>,
) -> Result<()> {
    let dark = 1i32;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of_val(&dark) as u32,
        );
    }
    let controls = Controls::create(hwnd, app, range, &app.state.borrow().settings)?;
    let _ = app.controls.set(controls);
    let menu = unsafe { GetSystemMenu(hwnd, 0) };
    if !menu.is_null() {
        let exit = wide("Exit FastTimer");
        unsafe {
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, SYSTEM_EXIT, exit.as_ptr());
        }
    }
    let has_tray = tray_add(hwnd, app.icon);
    {
        let mut state = app.state.borrow_mut();
        state.has_tray = has_tray;
        app.refresh(&mut state);
        app.update_request(&state);
        app.update_startup(&state);
    }
    let ui = app.controls.get().expect("controls were created");
    let mut notice = String::new();
    if !has_tray {
        notice.push_str("Tray unavailable. Closing will exit FastTimer.");
    }
    if background.is_err() {
        if !notice.is_empty() {
            notice.push(' ');
        }
        notice.push_str("Windows background timer policy is unavailable.");
    }
    if !notice.is_empty() {
        set_text(ui.feedback, &notice);
    }
    if !hidden {
        show_window(hwnd, ui);
    }

    let mut msg = MSG::default();
    loop {
        let code = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if code == -1 {
            return Err(io::Error::last_os_error().into());
        }
        if code == 0 {
            return Ok(());
        }
        if unsafe { IsDialogMessageW(hwnd, &msg) } == 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

impl Controls {
    fn create(hwnd: HWND, app: &App, range: Snapshot, settings: &Settings) -> io::Result<Self> {
        let add = |class: &str, text: &str, id, rect, flags| -> io::Result<HWND> {
            let (x, y, w, h) = rect;
            let s = |n| scale(app.theme.dpi, n);
            let class = wide(class);
            let text = wide(text);
            let child = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    text.as_ptr(),
                    WS_CHILD | WS_VISIBLE | flags,
                    s(x),
                    s(y),
                    s(w),
                    s(h),
                    hwnd,
                    usize::from(id) as HMENU,
                    app.module,
                    null(),
                )
            };
            if child.is_null() {
                return Err(io::Error::last_os_error());
            }
            unsafe { SendMessageW(child, WM_SETFONT, app.theme.font() as usize, 0) };
            Ok(child)
        };
        let label = |text: &str, id, rect| add("STATIC", text, id, rect, 0);
        let button = |text: &str, id, x, y, width| {
            add(
                "BUTTON",
                text,
                id,
                (x, y, width, 26),
                WS_TABSTOP | BS_OWNERDRAW as u32,
            )
        };
        let edit = |text: &str, id, rect| {
            add(
                "EDIT",
                text,
                id,
                rect,
                WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            )
        };
        label("Current resolution", 120, (12, 12, 138, 18))?;
        let reported = label("—", REPORTED, (152, 12, 270, 18))?;
        label("Minimum resolution", 121, (12, 32, 138, 18))?;
        label(
            &format!("{} ms", range.finest_100ns),
            122,
            (152, 32, 270, 18),
        )?;
        label("Requested resolution", 123, (12, 52, 138, 18))?;
        let request = label("—", REQUEST, (152, 52, 270, 18))?;

        label("Request (ms or winmm:1)", 124, (12, 83, 222, 18))?;
        let default_interval = range.finest_100ns.to_string();
        let custom = edit(
            settings.interval.as_deref().unwrap_or(&default_interval),
            CUSTOM,
            (12, 104, 132, 26),
        )?;
        button("Apply", APPLY, 152, 104, 80)?;
        button("Fastest", FASTEST, 240, 104, 80)?;
        button("Restore", RESET, 328, 104, 92)?;

        label("Sleep(1) benchmark (ms)", 125, (12, 145, 220, 18))?;
        let cases = edit(
            settings.cases.as_deref().unwrap_or(DEFAULT_CASES),
            CASES,
            (12, 166, 308, 26),
        )?;
        let run = button("Run", BENCH, 328, 166, 92)?;
        let results = add(
            "EDIT",
            "",
            RESULTS,
            (12, 198, 408, 88),
            WS_TABSTOP
                | WS_BORDER
                | WS_VSCROLL
                | WS_HSCROLL
                | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32,
        )?;
        let tabs = [68i32, 108, 148, 188];
        unsafe {
            SendMessageW(results, EM_SETTABSTOPS, tabs.len(), tabs.as_ptr() as LPARAM);
            ShowScrollBar(results, SB_VERT, 0);
            ShowScrollBar(results, SB_HORZ, 0);
        }
        let startup = button("Enable startup", STARTUP, 12, 294, 132)?;
        button("Export JSON", JSON, 204, 294, 104)?;
        button("Export CSV", CSV, 316, 294, 104)?;
        let feedback = label("", FEEDBACK, (12, 328, 408, 38))?;
        for (field, max) in [(custom, 32), (cases, 512)] {
            unsafe { SendMessageW(field, EM_SETLIMITTEXT, max, 0) };
        }
        for id in [JSON, CSV] {
            set_enabled(hwnd, id, false);
        }
        Ok(Self {
            reported,
            request,
            custom,
            cases,
            results,
            feedback,
            startup,
            run,
        })
    }
}

fn set_text(hwnd: HWND, text: &str) {
    let text = wide(text);
    unsafe { SetWindowTextW(hwnd, text.as_ptr()) };
}

fn get_text(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    let mut text = vec![0; len as usize + 1];
    let actual = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), len + 1) };
    String::from_utf16_lossy(&text[..actual as usize])
}

fn set_enabled(hwnd: HWND, id: u16, enabled: bool) {
    unsafe { EnableWindow(GetDlgItem(hwnd, id.into()), i32::from(enabled)) };
}

fn show_window(hwnd: HWND, ui: &Controls) {
    unsafe {
        ShowWindow(
            hwnd,
            if IsIconic(hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        SetForegroundWindow(hwnd);
        if GetFocus().is_null() {
            SetFocus(ui.custom);
        }
    }
}

fn tray_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: TRAY_EVENT,
        hIcon: icon,
        ..Default::default()
    };
    let title = wide("FastTimer · timer diagnostics");
    data.szTip[..title.len()].copy_from_slice(&title);
    data
}

fn tray_add(hwnd: HWND, icon: HICON) -> bool {
    let mut data = tray_data(hwnd, icon);
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
        return false;
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
    true
}

fn tray_menu(hwnd: HWND, busy: bool) -> u16 {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return 0;
    }
    let enabled = if busy { MF_GRAYED } else { MF_ENABLED };
    let items = [
        (OPEN, "Open FastTimer", MF_ENABLED),
        (FASTEST, "Request fastest interval", enabled),
        (RESET, "Release FastTimer request", enabled),
        (0, "", MF_SEPARATOR),
        (EXIT, "Exit", MF_ENABLED),
    ];
    let mut point = POINT::default();
    // TrackPopupMenu is modal; the caller holds no State borrow.
    unsafe {
        for (id, text, flags) in items {
            let text = wide(text);
            AppendMenuW(menu, MF_STRING | flags, usize::from(id), text.as_ptr());
        }
        SetForegroundWindow(hwnd);
        GetCursorPos(&mut point);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        selected as u16
    }
}

impl State {
    fn active(&self) -> Request {
        self.lease.as_ref().map_or(Request::None, Lease::request)
    }

    fn set_request(&mut self, request: Request) -> Result<()> {
        let old = self.active();
        if old == request {
            return Ok(());
        }
        if let Some(lease) = &mut self.lease {
            lease.release()?;
        }
        self.lease = None;
        if request != Request::None {
            match Lease::acquire(request) {
                Ok(lease) => self.lease = Some(lease),
                Err(error) => {
                    if old != Request::None {
                        match Lease::acquire(old) {
                            Ok(lease) => self.lease = Some(lease),
                            Err(restore) => {
                                return Err(format!("{error}. Restore failed: {restore}").into());
                            }
                        }
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.cancel.store(true, Ordering::Relaxed);
            let _ = worker.thread.join();
        }
        self.lease = None;
    }
}

impl App {
    fn refresh(&self, state: &mut State) {
        let Some(ui) = self.controls.get() else {
            return;
        };
        match timer::query() {
            Ok(range) => {
                if state.last_query != Some(range.current_100ns) {
                    set_text(ui.reported, &format!("{} ms", range.current_100ns));
                    state.last_query = Some(range.current_100ns);
                }
            }
            Err(_) => {
                set_text(ui.reported, "Unavailable");
                state.last_query = None;
            }
        }
    }

    fn update_request(&self, state: &State) {
        let Some(ui) = self.controls.get() else {
            return;
        };
        let request = match (state.worker.is_some(), state.active()) {
            (true, _) => "Benchmark".to_owned(),
            (false, Request::WinMm { period_ms }) => format!("{period_ms} ms WinMM"),
            (false, request) => request.to_string(),
        };
        set_text(ui.request, &request);
    }

    fn update_startup(&self, state: &State) {
        let Some(ui) = self.controls.get() else {
            return;
        };
        match &state.shortcut {
            Ok(path) => {
                let text = if path.exists() {
                    "Disable startup"
                } else {
                    "Enable startup"
                };
                set_text(ui.startup, text);
            }
            Err(_) => {
                set_text(ui.startup, "Startup unavailable");
                unsafe { EnableWindow(ui.startup, 0) };
            }
        }
    }

    fn command(&self, hwnd: HWND, id: u16) {
        let Some(ui) = self.controls.get() else {
            return;
        };
        if id == OPEN {
            show_window(hwnd, ui);
            return;
        }
        if id == EXIT {
            unsafe { PostMessageW(hwnd, QUIT, 0, 0) };
            return;
        }
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        if state.worker.is_some() && id != BENCH {
            return;
        }
        if let Err(error) = self.action(hwnd, ui, &mut state, id) {
            set_text(ui.feedback, &error.to_string());
        }
        self.update_request(&state);
        self.refresh(&mut state);
    }

    fn action(&self, hwnd: HWND, ui: &Controls, state: &mut State, id: u16) -> Result<()> {
        match id {
            APPLY | FASTEST | RESET => {
                let case = match id {
                    APPLY => get_text(ui.custom).parse()?,
                    FASTEST => Case::Fastest,
                    _ => Case::Baseline,
                };
                let request = Request::resolve(case, timer::query()?)?;
                state.set_request(request)?;
                let text = match state.lease.as_ref().and_then(|lease| lease.reported_100ns) {
                    Some(value) => format!("Native request active. Windows returned {value} ms."),
                    None if request == Request::None => "Request released.".to_owned(),
                    None => format!("Request active: {request}."),
                };
                set_text(ui.feedback, &text);
                if id == APPLY {
                    state.settings.interval = Some(case.to_string());
                    if state.settings.save().is_err() {
                        set_text(ui.feedback, "Request applied. Could not save settings.");
                    }
                }
            }
            BENCH => {
                if let Some(worker) = &state.worker {
                    worker.cancel.store(true, Ordering::Relaxed);
                    set_text(ui.feedback, "Cancelling benchmark…");
                    return Ok(());
                }
                let cases = get_text(ui.cases);
                let plan = Plan::new(&cases, 100, 12, 3)?;
                plan.resolve(timer::query()?)?;
                let previous = state.active();
                state.set_request(Request::None)?;
                let cancel = Arc::new(AtomicBool::new(false));
                let task_cancel = Arc::clone(&cancel);
                let window = hwnd as isize;
                let policy = self.background_policy;
                let worker =
                    thread::Builder::new()
                        .name("sleep-benchmark".into())
                        .spawn(move || {
                            let report = benchmark::run(&plan, policy, &task_cancel);
                            unsafe { PostMessageW(window as HWND, FINISHED, 0, 0) };
                            report
                        });
                match worker {
                    Ok(worker) => {
                        state.restore = previous;
                        state.worker = Some(Worker {
                            cancel,
                            thread: worker,
                        });
                        for id in [APPLY, FASTEST, RESET, STARTUP, JSON, CSV] {
                            set_enabled(hwnd, id, false);
                        }
                        set_text(ui.run, "Cancel");
                        set_text(ui.feedback, "Measuring Sleep(1)…");
                        state.settings.cases = Some(cases);
                        if state.settings.save().is_err() {
                            set_text(ui.feedback, "Measuring Sleep(1)… Could not save settings.");
                        }
                    }
                    Err(error) => {
                        state.set_request(previous)?;
                        return Err(error.into());
                    }
                }
            }
            JSON | CSV => {
                let Some(report) = &state.report else {
                    return Ok(());
                };
                if let Some(path) = save_path(hwnd, id == JSON)? {
                    if id == JSON {
                        report.write_json(&path)?;
                    } else {
                        report.write_csv(&path)?;
                    }
                    set_text(
                        ui.feedback,
                        &format!(
                            "Saved {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                    );
                }
            }
            STARTUP => {
                let path = state.shortcut.as_ref().map_err(ToString::to_string)?;
                if path.try_exists()? {
                    startup::disable(path)?;
                    set_text(ui.feedback, "Startup shortcut removed.");
                } else {
                    let case = match state.active() {
                        Request::None => Case::Baseline,
                        Request::Native { interval_100ns } => Case::Native(interval_100ns),
                        Request::WinMm { period_ms } => Case::WinMm(period_ms),
                    };
                    startup::enable(path, case)?;
                    set_text(
                        ui.feedback,
                        &format!("Startup saved with {}.", state.active()),
                    );
                }
                self.update_startup(state);
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&self, hwnd: HWND) {
        let Some(ui) = self.controls.get() else {
            return;
        };
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        let Some(worker) = state.worker.take() else {
            return;
        };
        let note = match worker.thread.join() {
            Ok(Ok(report)) => {
                let mut table = "Case\tMean\tP50\tP95\tP99".to_owned();
                use std::fmt::Write as _;
                for row in &report.cases {
                    let s = &row.stats;
                    let _ = write!(
                        table,
                        "\r\n{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}",
                        row.case, s.mean_ms, s.median_ms, s.p95_ms, s.p99_ms
                    );
                }
                set_text(ui.results, &table);
                unsafe { ShowScrollBar(ui.results, SB_VERT, i32::from(report.cases.len() > 4)) };
                state.report = Some(report);
                "Benchmark complete. Results are in milliseconds.".to_owned()
            }
            Ok(Err(error)) => error.to_string(),
            Err(_) => "Benchmark thread stopped unexpectedly.".to_owned(),
        };
        let restore = std::mem::replace(&mut state.restore, Request::None);
        let note = match state.set_request(restore) {
            Ok(()) => note,
            Err(error) => format!("Could not restore request: {error}"),
        };
        for id in [APPLY, FASTEST, RESET] {
            set_enabled(hwnd, id, true);
        }
        set_enabled(hwnd, STARTUP, state.shortcut.is_ok());
        for id in [JSON, CSV] {
            set_enabled(hwnd, id, state.report.is_some());
        }
        set_text(ui.run, "Run");
        set_text(ui.feedback, &note);
        self.update_request(&state);
        self.refresh(&mut state);
    }
}

fn save_path(hwnd: HWND, json: bool) -> Result<Option<PathBuf>> {
    let (name, filter, ext) = if json {
        ("fasttimer.json", "JSON files\0*.json\0", "json")
    } else {
        ("fasttimer.csv", "CSV files\0*.csv\0", "csv")
    };
    let name = wide(name);
    let filter = wide(filter);
    let ext = wide(ext);
    let mut file = vec![0; 32_768];
    file[..name.len()].copy_from_slice(&name);
    let mut options = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrDefExt: ext.as_ptr(),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut options) } == 0 {
        let code = unsafe { CommDlgExtendedError() };
        return if code == 0 {
            Ok(None)
        } else {
            Err(format!("Save dialog failed (0x{code:X})").into())
        };
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Ok(Some(OsString::from_wide(&file[..len]).into()))
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // Windows invokes this on the owner thread. The App pointer lives for the
    // entire HWND lifetime. RefCell prevents aliasing across reentrant messages.
    if msg == WM_NCCREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const App;
    let Some(app) = (unsafe { ptr.as_ref() }) else {
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    };
    match msg {
        WM_PAINT => app.theme.paint(hwnd),
        WM_ERASEBKGND => return 1,
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let id = unsafe { GetDlgCtrlID(l as HWND) } as u16;
            let field = matches!(id, CUSTOM | CASES | RESULTS);
            return app.theme.colors(w as _, field, id == FEEDBACK);
        }
        WM_DRAWITEM => {
            let item = unsafe { &*(l as *const DRAWITEMSTRUCT) };
            app.theme.button(item, &get_text(item.hwndItem));
            return 1;
        }
        WM_COMMAND if w >> 16 == 0 => app.command(hwnd, w as u16),
        WM_SYSCOMMAND if w & 0xfff0 == SYSTEM_EXIT => app.command(hwnd, EXIT),
        WM_SHOWWINDOW | WM_SIZE => {
            let visible = if msg == WM_SHOWWINDOW {
                w != 0 && unsafe { IsIconic(hwnd) } == 0
            } else {
                w != SIZE_MINIMIZED as usize && unsafe { IsWindowVisible(hwnd) } != 0
            };
            if visible {
                unsafe { SetTimer(hwnd, REFRESH, 1250, None) };
            } else {
                unsafe { KillTimer(hwnd, REFRESH) };
            }
        }
        WM_TIMER if w == REFRESH => {
            if let Ok(mut state) = app.state.try_borrow_mut() {
                app.refresh(&mut state);
            }
        }
        WM_CLOSE => {
            let has_tray = app.state.try_borrow().is_ok_and(|s| s.has_tray);
            if has_tray {
                unsafe { ShowWindow(hwnd, SW_HIDE) };
            } else {
                unsafe { PostMessageW(hwnd, QUIT, 0, 0) };
            }
        }
        WM_QUERYENDSESSION => return 1,
        WM_ENDSESSION if w != 0 => {
            if let Ok(mut state) = app.state.try_borrow_mut() {
                state.shutdown();
            }
        }
        SHOW => app.command(hwnd, OPEN),
        QUIT => {
            if let Ok(mut state) = app.state.try_borrow_mut() {
                state.shutdown();
                drop(state);
                unsafe { DestroyWindow(hwnd) };
            }
        }
        FINISHED => app.finish(hwnd),
        TRAY_EVENT => match (l as u32) & 0xffff {
            NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONUP => app.command(hwnd, OPEN),
            WM_CONTEXTMENU | WM_RBUTTONUP => {
                let busy = app.state.try_borrow().is_ok_and(|s| s.worker.is_some());
                app.command(hwnd, tray_menu(hwnd, busy));
            }
            _ => {}
        },
        WM_DESTROY => {
            if let Ok(mut state) = app.state.try_borrow_mut() {
                state.shutdown();
            }
            unsafe {
                KillTimer(hwnd, REFRESH);
                Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd, app.icon));
                PostQuitMessage(0);
            }
        }
        WM_NCDESTROY => {
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            return unsafe { DefWindowProcW(hwnd, msg, w, l) };
        }
        message if message == app.taskbar_created && message != 0 => {
            if let Ok(mut state) = app.state.try_borrow_mut() {
                state.has_tray = tray_add(hwnd, app.icon);
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
    0
}
