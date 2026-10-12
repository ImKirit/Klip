//! The tray icon: `Shell_NotifyIconW` on an invisible window of our own, with a context
//! menu whose entries the caller supplies.
//!
//! The icon lives on its own thread with its own message loop, so the recorder's threads
//! never wait for the shell. Everything the caller does (`set_state`, `set_menu`, ...) is
//! handed to that thread; menu choices come back over a channel.
//!
//! Messages are only ever posted to our own window. Nothing here touches a foreign
//! window, which matters because a game may be in the foreground while this runs.

use crate::icon::{create_dot_icon, Rgb};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NINF_KEY, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU, MF_CHECKED,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSEXW, WS_OVERLAPPED,
};

/// What the icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    /// Klip runs, nothing is captured.
    Idle,
    /// The replay buffer is running.
    Buffering,
    /// A manual recording is running.
    Recording,
}

/// One line of the context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item {
        /// Reported back in `TrayEvent::Menu`. Must not be 0, Windows uses 0 for "nothing chosen".
        id: u32,
        label: String,
        enabled: bool,
        checked: bool,
    },
    Separator,
}

impl MenuEntry {
    pub fn item(id: u32, label: impl Into<String>) -> Self {
        MenuEntry::Item { id, label: label.into(), enabled: true, checked: false }
    }

    pub fn enabled(mut self, value: bool) -> Self {
        if let MenuEntry::Item { enabled, .. } = &mut self {
            *enabled = value;
        }
        self
    }

    pub fn checked(mut self, value: bool) -> Self {
        if let MenuEntry::Item { checked, .. } = &mut self {
            *checked = value;
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// The user chose the menu entry with this id.
    Menu(u32),
    /// Left click on the icon, or Enter while it has the keyboard focus.
    Activated,
}

/// Colours of the dot per state. The accent is a setting in Klip, so it is not fixed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayColors {
    pub idle: Rgb,
    pub buffering: Rgb,
    pub recording: Rgb,
}

impl Default for TrayColors {
    fn default() -> Self {
        Self { idle: Rgb::IDLE, buffering: Rgb::ACCENT, recording: Rgb::RECORDING }
    }
}

#[derive(Debug, Clone)]
pub struct TrayOptions {
    pub tooltip: String,
    pub menu: Vec<MenuEntry>,
    pub state: TrayState,
    pub colors: TrayColors,
}

impl Default for TrayOptions {
    fn default() -> Self {
        Self { tooltip: "Klip.".into(), menu: Vec::new(), state: TrayState::Idle, colors: TrayColors::default() }
    }
}

const WM_TRAY: u32 = WM_APP + 1;
const WM_COMMANDS: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
/// Longest tooltip Windows shows: 127 characters plus the terminator.
const TIP_MAX: usize = 127;

enum Command {
    State(TrayState),
    Tooltip(String),
    Menu(Vec<MenuEntry>),
    Quit,
}

#[derive(Default)]
struct Shared {
    /// How often the icon was handed to the shell (first time plus every taskbar restart).
    registrations: AtomicU32,
    /// False while the shell has not accepted the icon, for example before Explorer is up.
    present: AtomicBool,
}

struct WindowState {
    hwnd: HWND,
    icons: [HICON; 3],
    state: TrayState,
    tooltip: String,
    menu: Vec<MenuEntry>,
    events: Sender<TrayEvent>,
    commands: Receiver<Command>,
    taskbar_created: u32,
    /// True if the shell accepted the version 4 behaviour (context menu message).
    version4: bool,
    shared: Arc<Shared>,
}

thread_local! {
    /// State of the tray window on its own thread. A thread local instead of window user
    /// data keeps the window procedure free of raw pointer casts.
    static STATE: RefCell<Option<WindowState>> = const { RefCell::new(None) };
}

fn validate(menu: &[MenuEntry]) -> Result<(), String> {
    match menu.iter().any(|e| matches!(e, MenuEntry::Item { id: 0, .. })) {
        true => Err("menu entry with id 0: Windows reports 0 when nothing was chosen".into()),
        false => Ok(()),
    }
}

/// Handle to the tray icon. Dropping it removes the icon and ends its thread.
pub struct Tray {
    hwnd: isize,
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Tray {
    /// Creates the icon. Menu choices and clicks arrive on the returned channel.
    ///
    /// Succeeds even if the shell does not take the icon yet (Klip started by autostart
    /// before Explorer is ready): the icon then appears with the next "TaskbarCreated".
    pub fn spawn(options: TrayOptions) -> Result<(Self, Receiver<TrayEvent>), String> {
        validate(&options.menu)?;
        let (commands, commands_rx) = mpsc::channel();
        let (events, events_rx) = mpsc::channel();
        let (ready, ready_rx) = mpsc::channel::<Result<isize, String>>();
        let shared = Arc::new(Shared::default());
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("klip-tray".into())
            .spawn(move || match create(options, events, commands_rx, thread_shared) {
                Ok(hwnd) => {
                    let _ = ready.send(Ok(hwnd.0 as isize));
                    message_loop();
                    cleanup();
                }
                Err(e) => {
                    cleanup();
                    let _ = ready.send(Err(e));
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(hwnd)) => Ok((Self { hwnd, commands, thread: Some(thread), shared }, events_rx)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("the tray thread ended before the icon was created".into()),
        }
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            // Wakes our own message loop. The target is our own invisible window.
            unsafe {
                let _ = PostMessageW(Some(self.window()), WM_COMMANDS, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn window(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    /// Switches the icon to another state.
    pub fn set_state(&self, state: TrayState) {
        self.send(Command::State(state));
    }

    pub fn set_tooltip(&self, tooltip: &str) {
        self.send(Command::Tooltip(tooltip.to_string()));
    }

    /// Replaces the context menu, for example to flip a check mark or a label.
    pub fn set_menu(&self, menu: Vec<MenuEntry>) -> Result<(), String> {
        validate(&menu)?;
        self.send(Command::Menu(menu));
        Ok(())
    }

    /// True once the shell has accepted the icon.
    pub fn is_present(&self) -> bool {
        self.shared.present.load(Ordering::Relaxed)
    }

    /// How often the icon was registered with the shell: 1 after the start, one more for
    /// every restart of Explorer. For diagnostics.
    pub fn registrations(&self) -> u32 {
        self.shared.registrations.load(Ordering::Relaxed)
    }

    /// Where the shell draws the icon, as left, top, right, bottom in screen pixels.
    /// `None` if the shell does not know the icon. For diagnostics and tests.
    pub fn icon_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let id = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.window(),
            uID: ICON_ID,
            ..Default::default()
        };
        unsafe { Shell_NotifyIconGetRect(&id) }.ok().map(|r| (r.left, r.top, r.right, r.bottom))
    }

    /// Sends our own window the message Explorer broadcasts after a restart, to exercise
    /// that path without restarting Explorer. It goes to our window only, not as a
    /// broadcast, so no other program sees it.
    pub fn simulate_taskbar_created(&self) {
        unsafe {
            let message = RegisterWindowMessageW(w!("TaskbarCreated"));
            let _ = PostMessageW(Some(self.window()), message, WPARAM(0), LPARAM(0));
        }
    }

    /// Removes the icon and waits for the thread.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Frees the icons once the window is gone.
fn cleanup() {
    if let Some(state) = STATE.with(|s| s.borrow_mut().take()) {
        for icon in state.icons {
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

fn create(
    options: TrayOptions,
    events: Sender<TrayEvent>,
    commands: Receiver<Command>,
    shared: Arc<Shared>,
) -> Result<HWND, String> {
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("KlipTrayWindow"),
            ..Default::default()
        };
        // Fails harmlessly if the class exists already (a second tray in one process).
        RegisterClassExW(&class);

        let size = GetSystemMetrics(SM_CXSMICON).max(16) as u32;
        let colors = [options.colors.idle, options.colors.buffering, options.colors.recording];
        let mut icons = [HICON::default(); 3];
        for (slot, color) in icons.iter_mut().zip(colors) {
            *slot = create_dot_icon(size, color).map_err(|e| format!("creating the tray icon: {e}"))?;
        }

        // A normal top level window that is never shown. It must not be a message only
        // window (HWND_MESSAGE): those do not receive broadcasts, and "TaskbarCreated" is
        // one. Without it the icon would be gone for good after Explorer restarts.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("KlipTrayWindow"),
            w!("Klip Tray"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| format!("creating the tray window: {e}"))?;

        let state = WindowState {
            hwnd,
            icons,
            state: options.state,
            tooltip: options.tooltip,
            menu: options.menu,
            events,
            commands,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            version4: false,
            shared,
        };
        STATE.with(|s| *s.borrow_mut() = Some(state));
        register_icon();
        Ok(hwnd)
    }
}

fn icon_data(state: &WindowState) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: state.hwnd,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: state.icons[state.state as usize],
        ..Default::default()
    };
    for (slot, unit) in data.szTip.iter_mut().zip(state.tooltip.encode_utf16().take(TIP_MAX)) {
        *slot = unit;
    }
    data
}

/// Hands the icon to the shell: at start and again after every restart of Explorer.
fn register_icon() {
    STATE.with(|s| {
        let mut guard = s.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        let mut data = icon_data(state);
        unsafe {
            // After a real restart the shell has forgotten the icon and NIM_ADD works. If
            // the icon is still known (the message came from somewhere else) NIM_ADD
            // fails and NIM_MODIFY refreshes it instead.
            let present = Shell_NotifyIconW(NIM_ADD, &data).as_bool() || Shell_NotifyIconW(NIM_MODIFY, &data).as_bool();
            state.shared.present.store(present, Ordering::Relaxed);
            if present {
                state.shared.registrations.fetch_add(1, Ordering::Relaxed);
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                state.version4 = Shell_NotifyIconW(NIM_SETVERSION, &data).as_bool();
            }
        }
    });
}

/// Pushes the current icon and tooltip to the shell.
fn refresh_icon() {
    STATE.with(|s| {
        if let Some(state) = s.borrow().as_ref() {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &icon_data(state));
            }
        }
    });
}

/// Builds the popup menu. The caller destroys it.
fn build_menu(entries: &[MenuEntry]) -> windows::core::Result<HMENU> {
    unsafe {
        let menu = CreatePopupMenu()?;
        for entry in entries {
            let result = match entry {
                MenuEntry::Separator => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
                MenuEntry::Item { id, label, enabled, checked } => {
                    let mut flags = MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    AppendMenuW(menu, flags, *id as usize, &HSTRING::from(label.as_str()))
                }
            };
            if let Err(e) = result {
                let _ = DestroyMenu(menu);
                return Err(e);
            }
        }
        Ok(menu)
    }
}

fn show_menu() {
    // Copy what is needed and let go of the state: TrackPopupMenu runs its own message
    // loop, and messages arriving meanwhile need to borrow the state themselves.
    let Some((hwnd, entries, events)) =
        STATE.with(|s| s.borrow().as_ref().map(|st| (st.hwnd, st.menu.clone(), st.events.clone())))
    else {
        return;
    };
    if entries.is_empty() {
        return;
    }
    let Ok(menu) = build_menu(&entries) else { return };
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // Required by Windows for tray menus: without our own window in the foreground
        // the menu does not close when the user clicks elsewhere. This only ever happens
        // right after the user clicked our icon, never on Klip's own initiative.
        let _ = SetForegroundWindow(hwnd);
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
        let chosen = TrackPopupMenu(menu, flags, cursor.x, cursor.y, None, hwnd, None).0 as u32;
        // The second half of the same Windows quirk, see the documentation of TrackPopupMenu.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if chosen != 0 {
            let _ = events.send(TrayEvent::Menu(chosen));
        }
    }
}

fn run_commands(hwnd: HWND) {
    let pending: Vec<Command> = STATE.with(|s| s.borrow().as_ref().map(|st| st.commands.try_iter().collect()).unwrap_or_default());
    let mut quit = false;
    let mut dirty = false;
    STATE.with(|s| {
        let mut guard = s.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        for command in pending {
            match command {
                Command::State(new) => {
                    dirty |= state.state != new;
                    state.state = new;
                }
                Command::Tooltip(new) => {
                    dirty |= state.tooltip != new;
                    state.tooltip = new;
                }
                Command::Menu(new) => state.menu = new,
                Command::Quit => quit = true,
            }
        }
    });
    if dirty {
        refresh_icon();
    }
    if quit {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let (taskbar_created, version4, events) = STATE.with(|s| {
        s.borrow().as_ref().map(|st| (st.taskbar_created, st.version4, Some(st.events.clone()))).unwrap_or((0, false, None))
    });
    match msg {
        WM_TRAY => {
            // With version 4 the low word of lparam is the event. Without it lparam is
            // the mouse message itself, which fits the same mask.
            let event = lparam.0 as u32 & 0xFFFF;
            let activate = if version4 { event == NIN_SELECT || event == (NIN_SELECT | NINF_KEY) } else { event == WM_LBUTTONUP };
            let menu = if version4 { event == WM_CONTEXTMENU } else { event == WM_RBUTTONUP };
            if menu {
                show_menu();
            } else if activate {
                if let Some(events) = events {
                    let _ = events.send(TrayEvent::Activated);
                }
            }
            LRESULT(0)
        }
        WM_COMMANDS => {
            run_commands(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            STATE.with(|s| {
                if let Some(state) = s.borrow().as_ref() {
                    unsafe {
                        let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(state));
                    }
                    state.shared.present.store(false, Ordering::Relaxed);
                }
            });
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // Explorer was restarted (crash, update, the user ended it): every icon is gone
        // and has to be handed over again.
        _ if taskbar_created != 0 && msg == taskbar_created => {
            register_icon();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID, GetMenuState, MF_BYPOSITION};

    #[test]
    fn menu_carries_ids_and_states() {
        let entries = vec![
            MenuEntry::item(10, "Puffer").checked(true),
            MenuEntry::item(11, "Clip speichern").enabled(false),
            MenuEntry::Separator,
            MenuEntry::item(12, "Beenden"),
        ];
        let menu = build_menu(&entries).unwrap();
        unsafe {
            assert_eq!(GetMenuItemCount(Some(menu)), 4);
            assert_eq!(GetMenuItemID(menu, 0), 10);
            assert_eq!(GetMenuItemID(menu, 3), 12);
            let state = |pos: u32| GetMenuState(menu, pos, MF_BYPOSITION);
            assert_ne!(state(0) & MF_CHECKED.0, 0);
            assert_eq!(state(0) & MF_GRAYED.0, 0);
            assert_ne!(state(1) & MF_GRAYED.0, 0);
            assert_eq!(state(1) & MF_CHECKED.0, 0);
            assert_ne!(state(2) & MF_SEPARATOR.0, 0);
            assert_eq!(state(3) & (MF_GRAYED.0 | MF_CHECKED.0), 0);
            DestroyMenu(menu).unwrap();
        }
    }

    #[test]
    fn id_zero_is_rejected() {
        assert!(validate(&[MenuEntry::item(0, "broken")]).is_err());
        assert!(validate(&[MenuEntry::Separator, MenuEntry::item(1, "fine")]).is_ok());
    }

    #[test]
    fn tooltip_is_cut_to_what_windows_shows() {
        let (events, _) = mpsc::channel();
        let (_, commands) = mpsc::channel();
        let state = WindowState {
            hwnd: HWND::default(),
            icons: [HICON::default(); 3],
            state: TrayState::Recording,
            tooltip: "x".repeat(500),
            menu: Vec::new(),
            events,
            commands,
            taskbar_created: 0,
            version4: false,
            shared: Arc::new(Shared::default()),
        };
        let data = icon_data(&state);
        assert_eq!(data.szTip.iter().filter(|&&c| c != 0).count(), TIP_MAX);
        assert_eq!(data.szTip[TIP_MAX], 0);
    }
}
