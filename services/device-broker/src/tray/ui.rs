//! The Win32 half of the broker's icon: a hidden owner window on its own
//! thread, `Shell_NotifyIconW`, and the popup menu. This thread reads a copy
//! of the state thread's snapshot and forwards menu commands; it never takes a
//! broker lock and never calls the device controller.
use super::model::{self, IconTone, MenuAction, MenuRow, Presentation, Snapshot};
use super::state::{Notify, Shared};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, MessageBoxW, PostMessageW,
    RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, SetTimer, TrackPopupMenuEx,
    TranslateMessage, HICON, HMENU, ICONINFO, IDYES, MB_DEFBUTTON2, MB_ICONWARNING,
    MB_SETFOREGROUND, MB_YESNO, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_NULL, WM_TIMER,
    WNDCLASSEXW, WS_OVERLAPPED,
};

const ICON_ID: u32 = 1;
/// `NIN_SELECT | NINF_KEY`: Enter or Space on the focused icon.
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;
const CALLBACK: u32 = WM_APP + 1;
const REFRESH: u32 = WM_APP + 2;
const SYNC_TIMER: usize = 1;
/// Re-checks the snapshot's age even when the state thread stops posting.
const STALE_TIMER: usize = 2;
const STALE_CHECK_MS: u32 = 1000;
const SYNC_FRAME_MS: u32 = 225;

/// The window that owns the icon, for `remove_icon_now` on any thread.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// Per-thread UI state, reached from the window procedure.
static STATE: OnceLock<Mutex<UiState>> = OnceLock::new();

struct UiState {
    shared: Arc<Shared>,
    actions: mpsc::Sender<MenuAction>,
    taskbar_created: u32,
    shown: bool,
    tone: Option<IconTone>,
    frame: u8,
    icon: HICON,
    model: Option<Presentation>,
}

// SAFETY: the raw icon handle is created, used and destroyed only on the UI
// thread; the mutex exists so the window procedure can reach the state.
unsafe impl Send for UiState {}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn notify_data(window: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data; all-zero is its documented empty value.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = window;
    data.uID = ICON_ID;
    data
}

fn copy_tip(target: &mut [u16; 128], tip: &str) {
    let units: Vec<u16> = tip.encode_utf16().take(target.len() - 1).collect();
    target[..units.len()].copy_from_slice(&units);
    target[units.len()] = 0;
}

/// A 32-bit icon from straight RGBA: BGRA colour plane plus an all-zero mask.
fn create_icon(rgba: &[u8]) -> HICON {
    let size = model::ICON_SIZE as i32;
    // SAFETY: the header describes exactly the buffer CreateDIBSection
    // allocates; the bits pointer is written only within its length.
    unsafe {
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(
            std::ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if color.is_null() || bits.is_null() {
            return std::ptr::null_mut();
        }
        let target = std::slice::from_raw_parts_mut(bits.cast::<u8>(), rgba.len());
        for (pixel, source) in target
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(rgba.as_chunks::<4>().0)
        {
            pixel.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
        }
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let icon_info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

impl UiState {
    fn snapshot(&self) -> Snapshot {
        let mut snapshot = self
            .shared
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        snapshot.stale = self
            .shared
            .refreshed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
            > model::STALE_AFTER;
        snapshot
    }

    fn replace_icon(&mut self, tone: IconTone, frame: u8) {
        let icon = create_icon(&model::tone_rgba(tone, frame));
        if !self.icon.is_null() {
            // SAFETY: the previous icon was created by this thread.
            unsafe { DestroyIcon(self.icon) };
        }
        self.icon = icon;
        self.tone = Some(tone);
        self.frame = frame;
    }

    fn add(&mut self, window: HWND, model: &Presentation) {
        let mut data = notify_data(window);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = self.icon;
        copy_tip(&mut data.szTip, &model.tooltip);
        // SAFETY: data is fully initialised for the duration of each call.
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
        if added {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            // SAFETY: as above.
            unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
        }
        self.shown = added;
        self.shared.registered.store(added, Ordering::Release);
    }

    fn remove(&mut self, window: HWND) {
        if self.shown {
            let data = notify_data(window);
            // SAFETY: data names this window's icon.
            unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        }
        self.shown = false;
        self.shared.registered.store(false, Ordering::Release);
    }

    fn refresh(&mut self, window: HWND) {
        let snapshot = self.snapshot();
        let next = model::presentation(&snapshot);
        let tone = model::icon_tone(&snapshot);
        let wanted = self.shared.wanted.load(Ordering::Acquire);
        if !wanted {
            self.remove(window);
            self.model = Some(next);
            return;
        }
        if self.tone != Some(tone) {
            self.replace_icon(tone, 0);
            // SAFETY: the timer belongs to this window and thread.
            unsafe {
                if tone == IconTone::Syncing {
                    SetTimer(window, SYNC_TIMER, SYNC_FRAME_MS, None);
                } else {
                    KillTimer(window, SYNC_TIMER);
                }
            }
            if self.shown {
                self.modify(window, NIF_ICON, &next);
            }
        }
        if !self.shown {
            self.add(window, &next);
        } else if self.model.as_ref().map(|model| &model.tooltip) != Some(&next.tooltip) {
            self.modify(window, NIF_TIP | NIF_SHOWTIP, &next);
        }
        self.model = Some(next);
    }

    fn modify(&mut self, window: HWND, flags: u32, model: &Presentation) {
        let mut data = notify_data(window);
        data.uFlags = flags;
        data.hIcon = self.icon;
        copy_tip(&mut data.szTip, &model.tooltip);
        // SAFETY: data is fully initialised for the call.
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } == 0 {
            // The shell lost the icon (Explorer restarting): add it again on
            // the next refresh, and hold nothing until it is back.
            self.shown = false;
            self.shared.registered.store(false, Ordering::Release);
        }
    }

    fn advance_sync_frame(&mut self, window: HWND) {
        if self.tone == Some(IconTone::Syncing) && self.shown {
            let frame = (self.frame + 1) % model::SYNC_FRAME_COUNT;
            self.replace_icon(IconTone::Syncing, frame);
            if let Some(current) = self.model.clone() {
                self.modify(window, NIF_ICON, &current);
            }
        }
    }
}

/// Runs the menu's modal loop. Called with the UI state unlocked: the loop
/// dispatches this window's own messages, which lock it again.
fn show_menu(window: HWND, model: &Presentation, actions: &mpsc::Sender<MenuAction>) {
    if CONFIRMING.load(Ordering::Acquire) {
        return;
    }
    {
        let mut commands: Vec<MenuAction> = Vec::new();
        // SAFETY: every menu handle is created, used and destroyed here.
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            for row in &model.rows {
                append_row(menu, row, &mut commands);
            }
            // The ids handed to Windows are exactly the model's numbering.
            debug_assert_eq!(commands, model::commands(&model.rows));
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            // Without the foreground the menu does not dismiss on click-away.
            SetForegroundWindow(window);
            let chosen = TrackPopupMenuEx(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                point.x,
                point.y,
                window,
                std::ptr::null(),
            );
            PostMessageW(window, WM_NULL, 0, 0);
            DestroyMenu(menu);
            if let Some(action) = usize::try_from(chosen)
                .ok()
                .and_then(|id| id.checked_sub(1))
                .and_then(|index| commands.get(index))
            {
                // On this UI thread, after the menu closed and with the UI
                // state unlocked: the box's modal loop keeps dispatching this
                // window's messages, and no broker lock or request waits on it.
                model::dispatch(
                    action.clone(),
                    |question| confirm(window, question),
                    |action| {
                        let _ = actions.send(action);
                    },
                );
            }
        }
    }
}

/// A native Yes/No question owned by the icon's window; No is the default.
fn confirm(window: HWND, question: &model::Confirmation) -> bool {
    let text = wide(&question.text);
    let title = wide(&question.title);
    CONFIRMING.store(true, Ordering::Release);
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let answer = unsafe {
        MessageBoxW(
            window,
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
        )
    };
    CONFIRMING.store(false, Ordering::Release);
    answer == IDYES
}

/// Set while a confirmation is up: the box's modal loop still delivers icon
/// clicks, and a second menu over an open question would be confusing.
static CONFIRMING: AtomicBool = AtomicBool::new(false);

fn menu_text(label: &str) -> Vec<u16> {
    wide(&label.replace('&', "&&"))
}

/// # Safety
/// `menu` must be a live menu handle owned by the caller.
unsafe fn append_row(menu: HMENU, row: &MenuRow, commands: &mut Vec<MenuAction>) {
    match row {
        MenuRow::Separator => {
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        }
        MenuRow::Text(label) => {
            AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, menu_text(label).as_ptr());
        }
        MenuRow::Action {
            label,
            enabled,
            action,
        } => {
            commands.push(action.clone());
            let flags = if *enabled {
                MF_STRING
            } else {
                MF_STRING | MF_GRAYED
            };
            AppendMenuW(menu, flags, commands.len(), menu_text(label).as_ptr());
        }
        MenuRow::Choices { label, choices } => {
            let submenu = CreatePopupMenu();
            if submenu.is_null() {
                commands.extend(choices.iter().map(|(_, _, action)| action.clone()));
                return;
            }
            for (name, checked, action) in choices {
                commands.push(action.clone());
                let flags = if *checked {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                AppendMenuW(submenu, flags, commands.len(), menu_text(name).as_ptr());
            }
            AppendMenuW(
                menu,
                MF_STRING | MF_POPUP,
                submenu as usize,
                menu_text(label).as_ptr(),
            );
        }
        MenuRow::Submenu { label, rows } => {
            let submenu = CreatePopupMenu();
            if submenu.is_null() {
                // Keep the numbering the model expects even without the item.
                commands.extend(model::commands(rows));
                return;
            }
            for row in rows {
                append_row(submenu, row, commands);
            }
            // Owned by `menu` from here: DestroyMenu on the root frees it.
            AppendMenuW(
                menu,
                MF_STRING | MF_POPUP,
                submenu as usize,
                menu_text(label).as_ptr(),
            );
        }
    }
}

// SAFETY: the system calls this as the window procedure registered in `start`, always on the
// thread that created the window, so that thread is the only one that locks STATE. The
// arguments are the system's own: they are forwarded to DefWindowProcW and PostMessageW
// or compared as integers, never dereferenced. STATE is set before the window exists.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(state) = STATE.get() else {
        return DefWindowProcW(window, message, wparam, lparam);
    };
    // Only this thread locks the state, so a held lock means re-entry: a shell
    // call made under it dispatched one of our own sent messages. Retry that
    // message later rather than deadlock on ourselves. A poisoned lock means an
    // earlier message panicked; its state is still the best available.
    let mut state = match state.try_lock() {
        Ok(state) => state,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            if message == REFRESH || message == CALLBACK || message == WM_TIMER {
                PostMessageW(window, message, wparam, lparam);
                return 0;
            }
            return DefWindowProcW(window, message, wparam, lparam);
        }
    };
    match message {
        REFRESH => {
            state.refresh(window);
            0
        }
        CALLBACK => {
            let event = (lparam as u32) & 0xffff;
            if matches!(event, WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT) {
                let model = model::presentation(&state.snapshot());
                let actions = state.actions.clone();
                drop(state);
                show_menu(window, &model, &actions);
            }
            0
        }
        WM_TIMER if wparam == STALE_TIMER => {
            state.refresh(window);
            0
        }
        WM_TIMER if wparam == SYNC_TIMER => {
            state.advance_sync_frame(window);
            0
        }
        _ if message == state.taskbar_created && message != 0 => {
            // Explorer restarted and forgot every icon: register ours again.
            state.shown = false;
            state.shared.registered.store(false, Ordering::Release);
            state.refresh(window);
            0
        }
        _ => {
            drop(state);
            DefWindowProcW(window, message, wparam, lparam)
        }
    }
}

/// Create the window and start its message loop on a dedicated thread.
/// Returns once the window exists, with the closure the state thread calls to
/// ask for a re-render.
pub fn start(shared: Arc<Shared>, actions: mpsc::Sender<MenuAction>) -> Result<Notify, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<isize, String>>();
    std::thread::Builder::new()
        .name("qc-broker-tray-ui".into())
        .spawn(move || {
            let class = wide("QCRemoteDeviceBrokerTray");
            // SAFETY: the class and window names outlive both calls, and the
            // window procedure only reaches STATE, initialised below first.
            let window = unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let mut class_info: WNDCLASSEXW = std::mem::zeroed();
                class_info.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
                class_info.lpfnWndProc = Some(window_proc);
                class_info.hInstance = instance;
                class_info.lpszClassName = class.as_ptr();
                RegisterClassExW(&class_info);
                let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
                if STATE
                    .set(Mutex::new(UiState {
                        shared,
                        actions,
                        taskbar_created,
                        shown: false,
                        tone: None,
                        frame: 0,
                        icon: std::ptr::null_mut(),
                        model: None,
                    }))
                    .is_err()
                {
                    let _ = ready_tx.send(Err("The broker icon is already running".into()));
                    return;
                }
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    wide("QC Remote device broker").as_ptr(),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            if window.is_null() {
                let _ = ready_tx.send(Err(std::io::Error::last_os_error().to_string()));
                return;
            }
            WINDOW.store(window as isize, Ordering::Release);
            // SAFETY: the timer belongs to the window this thread just created.
            unsafe { SetTimer(window, STALE_TIMER, STALE_CHECK_MS, None) };
            let _ = ready_tx.send(Ok(window as isize));
            // SAFETY: a standard message loop for the window this thread owns.
            unsafe {
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        })
        .map_err(|error| error.to_string())?;
    let window = ready_rx
        .recv()
        .map_err(|_| "The broker icon thread ended during startup".to_owned())??;
    Ok(Box::new(move || {
        // SAFETY: posting to a window handle is safe even after it is gone.
        unsafe { PostMessageW(window as HWND, REFRESH, 0, 0) };
    }))
}

/// Remove the icon from any thread just before the process exits, so Windows
/// does not leave a ghost until the pointer passes over it.
pub fn remove_icon_now() {
    let window = WINDOW.load(Ordering::Acquire);
    if window != 0 {
        let data = notify_data(window as HWND);
        // SAFETY: NIM_DELETE only reads the window/id pair from data.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltips_are_terminated_inside_the_shells_buffer() {
        let mut tip = [7_u16; 128];
        copy_tip(&mut tip, &"x".repeat(500));
        assert_eq!(tip[127], 0);
        assert_eq!(tip[126], u16::from(b'x'));
        copy_tip(&mut tip, "ok");
        assert_eq!(&tip[..3], &[u16::from(b'o'), u16::from(b'k'), 0]);
    }

    #[test]
    fn menu_text_escapes_mnemonics() {
        assert_eq!(menu_text("A&B"), wide("A&&B"));
    }
}
