//! Raw input capture: a message-only window on its own thread that turns WM_INPUT into
//! [`RawEvent`] values for the [`Processor`].
//!
//! Raw input with RIDEV_INPUTSINK delivers input to our own window. Nothing is hooked,
//! nothing is loaded into another process and no input is ever generated. While capture
//! is off the devices are unregistered (RIDEV_REMOVE), so Windows does not even wake
//! this thread outside a game.

use crate::filter::{Allowlist, Processor, RawEvent, Stats};
use crate::layout::{MouseButton, OverlayLayout};
use crate::track::{Held, InputClip};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Thread message: flags or layout changed, bring the device registration in line.
const WM_SYNC: u32 = WM_APP + 1;

// HID usages on the generic desktop page.
const USAGE_PAGE_GENERIC: u16 = 1;
const USAGE_MOUSE: u16 = 2;
const USAGE_KEYBOARD: u16 = 6;

// RAWKEYBOARD.Flags
const KEY_BREAK: u16 = 1;
const KEY_E0: u16 = 2;
const KEY_E1: u16 = 4;

// RAWMOUSE.usButtonFlags: (down, up, button)
const BUTTONS: [(u16, u16, MouseButton); 5] = [
    (0x0001, 0x0002, MouseButton::Left),
    (0x0004, 0x0008, MouseButton::Right),
    (0x0010, 0x0020, MouseButton::Middle),
    (0x0040, 0x0080, MouseButton::Side1),
    (0x0100, 0x0200, MouseButton::Side2),
];
const MOUSE_WHEEL: u16 = 0x0400;
const MOUSE_HWHEEL: u16 = 0x0800;

const REG_KEYBOARD: u8 = 1;
const REG_MOUSE: u8 = 2;

/// Now on the QueryPerformanceCounter clock in 100 ns units, the same formula as the
/// recorder's `qpc_100ns`, so input and video times can be compared directly.
pub fn now_100ns() -> i64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        f.max(1)
    });
    let mut now = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut now);
    }
    (now as i128 * 10_000_000 / freq as i128) as i64
}

struct Shared {
    processor: Mutex<Processor>,
    /// Which devices are registered right now, as the capture thread last set them.
    registered: AtomicU8,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn processor(&self) -> MutexGuard<'_, Processor> {
        // A panic elsewhere must not end input capture for the rest of the session.
        self.processor.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Running capture. Starts with "game in foreground" off, so nothing is registered and
/// nothing is stored until [`InputCapture::set_game_active`] is called with `true`.
pub struct InputCapture {
    shared: Arc<Shared>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl InputCapture {
    /// `retention` is how far back the track reaches, normally the replay buffer length.
    pub fn start(layout: &OverlayLayout, retention: Duration) -> Result<Self, String> {
        let retention = (retention.as_nanos() / 100).min(i64::MAX as u128) as i64;
        let shared = Arc::new(Shared {
            processor: Mutex::new(Processor::new(Allowlist::from_layout(layout), retention)),
            registered: AtomicU8::new(0),
            error: Mutex::new(None),
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        let for_thread = shared.clone();
        let thread = std::thread::Builder::new()
            .name("klip-input".into())
            .spawn(move || run(for_thread, ready_tx))
            .map_err(|e| format!("input thread: {e}"))?;
        // Wait for the window: only then does the thread have a message queue that
        // PostThreadMessageW can reach.
        let thread_id = ready_rx.recv().map_err(|_| "input thread ended early".to_string())??;
        Ok(InputCapture { shared, thread_id, thread: Some(thread) })
    }

    /// The switch "a recognised game is in the foreground". Off means: devices
    /// unregistered, nothing stored.
    pub fn set_game_active(&self, active: bool) {
        self.shared.processor().set_game_active(active, now_100ns());
        self.sync();
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.processor().set_paused(paused, now_100ns());
        self.sync();
    }

    /// Switches to another layout. From now on only its keys are stored.
    pub fn set_layout(&self, layout: &OverlayLayout) {
        self.shared.processor().set_allowlist(Allowlist::from_layout(layout), now_100ns());
        self.sync();
    }

    pub fn set_retention(&self, retention: Duration) {
        let retention = (retention.as_nanos() / 100).min(i64::MAX as u128) as i64;
        self.shared.processor().set_retention(retention);
    }

    /// Events from `t0` to `t1` (100 ns, QPC clock) with the keys held at `t0`.
    pub fn slice(&self, t0: i64, t1: i64) -> InputClip {
        self.shared.processor().slice(t0, t1)
    }

    /// What is held right now, for the live overlay.
    pub fn held(&self) -> Held {
        self.shared.processor().track().held().clone()
    }

    /// Presses per second over the last `window`: `(keys, clicks)`.
    pub fn rates(&self, window: Duration) -> (f32, f32) {
        let (now, window) = (now_100ns(), (window.as_nanos() / 100) as i64);
        let p = self.shared.processor();
        (p.track().rate_at(now, window, true), p.track().rate_at(now, window, false))
    }

    pub fn stats(&self) -> Stats {
        self.shared.processor().stats()
    }

    /// Drops old events while no input arrives. Optional, memory is bounded either way.
    pub fn trim(&self) {
        self.shared.processor().trim(now_100ns());
    }

    /// Forgets everything captured so far.
    pub fn clear(&self) {
        self.shared.processor().clear();
    }

    /// `(keyboard, mouse)`: whether the raw input devices are registered at this moment.
    /// Changes follow the setters after a moment, the capture thread does the work.
    pub fn registered(&self) -> (bool, bool) {
        let bits = self.shared.registered.load(Ordering::SeqCst);
        (bits & REG_KEYBOARD != 0, bits & REG_MOUSE != 0)
    }

    /// The last failure to register or unregister a device, if there was one.
    pub fn last_error(&self) -> Option<String> {
        self.shared.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn sync(&self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_SYNC, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn run(shared: Arc<Shared>, ready: mpsc::Sender<Result<u32, String>>) {
    unsafe {
        let instance = match GetModuleHandleW(None) {
            Ok(i) => i,
            Err(e) => {
                let _ = ready.send(Err(format!("GetModuleHandleW: {e}")));
                return;
            }
        };
        let class = w!("KlipInputSink");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        // Fails harmlessly when a second capture in this process registered it already.
        RegisterClassW(&wc);
        // A child of HWND_MESSAGE is a message-only window: never visible, not in the
        // taskbar, not enumerated, and it cannot take focus from the game.
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("CreateWindowExW: {e}")));
                return;
            }
        };
        let _ = ready.send(Ok(GetCurrentThreadId()));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            match msg.message {
                WM_INPUT => {
                    on_input(msg.lParam, &shared);
                    // DefWindowProc must still see WM_INPUT, it frees the input record.
                    DispatchMessageW(&msg);
                }
                WM_SYNC if msg.hwnd.0.is_null() => sync(hwnd, &shared),
                _ => {
                    DispatchMessageW(&msg);
                }
            }
        }

        for (usage, bit) in [(USAGE_KEYBOARD, REG_KEYBOARD), (USAGE_MOUSE, REG_MOUSE)] {
            if shared.registered.load(Ordering::SeqCst) & bit != 0 {
                let _ = register(usage, None);
            }
        }
        shared.registered.store(0, Ordering::SeqCst);
        let _ = DestroyWindow(hwnd);
    }
}

/// Registers the device for `target`, or unregisters it when `target` is `None`.
unsafe fn register(usage: u16, target: Option<HWND>) -> windows::core::Result<()> {
    let device = RAWINPUTDEVICE {
        usUsagePage: USAGE_PAGE_GENERIC,
        usUsage: usage,
        // INPUTSINK: deliver while another window (the game) has the focus. No NOLEGACY,
        // that would take the normal key messages away from the whole process.
        dwFlags: if target.is_some() { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        // RIDEV_REMOVE requires a null target.
        hwndTarget: target.unwrap_or_default(),
    };
    RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32)
}

unsafe fn sync(hwnd: HWND, shared: &Shared) {
    let (keyboard, mouse) = {
        let p = shared.processor();
        let on = p.capturing();
        (on && p.allowlist().needs_keyboard(), on && p.allowlist().needs_mouse())
    };
    let mut bits = shared.registered.load(Ordering::SeqCst);
    for (usage, bit, want) in [(USAGE_KEYBOARD, REG_KEYBOARD, keyboard), (USAGE_MOUSE, REG_MOUSE, mouse)] {
        if (bits & bit != 0) == want {
            continue;
        }
        match register(usage, want.then_some(hwnd)) {
            Ok(()) => bits = if want { bits | bit } else { bits & !bit },
            Err(e) => {
                let what = if want { "register" } else { "unregister" };
                *shared.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{what} raw input {usage}: {e}"));
            }
        }
    }
    shared.registered.store(bits, Ordering::SeqCst);
}

unsafe fn on_input(lparam: LPARAM, shared: &Shared) {
    // Keyboard and mouse records both fit into the plain struct, so no allocation.
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let got = GetRawInputData(
        HRAWINPUT(lparam.0 as *mut c_void),
        RID_INPUT,
        Some(&mut raw as *mut RAWINPUT as *mut c_void),
        &mut size,
        size_of::<RAWINPUTHEADER>() as u32,
    );
    if got == 0 || got == u32::MAX {
        return;
    }
    let t = now_100ns();
    let mut p = shared.processor();
    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        // Only MakeCode and Flags are read. VKey and Message are never looked at: they
        // depend on the system layout and are a step closer to text.
        let k = raw.data.keyboard;
        p.handle(t, RawEvent::Key { make: k.MakeCode, e0: k.Flags & KEY_E0 != 0, e1: k.Flags & KEY_E1 != 0, down: k.Flags & KEY_BREAK == 0 });
    } else if raw.header.dwType == RIM_TYPEMOUSE.0 {
        let m = raw.data.mouse;
        let flags = m.Anonymous.Anonymous.usButtonFlags;
        let data = m.Anonymous.Anonymous.usButtonData as i16;
        for (down, up, button) in BUTTONS {
            if flags & down != 0 {
                p.handle(t, RawEvent::Button { button, down: true });
            }
            if flags & up != 0 {
                p.handle(t, RawEvent::Button { button, down: false });
            }
        }
        if flags & MOUSE_WHEEL != 0 {
            p.handle(t, RawEvent::Wheel { delta: data, horizontal: false });
        }
        if flags & MOUSE_HWHEEL != 0 {
            p.handle(t, RawEvent::Wheel { delta: data, horizontal: true });
        }
        // Absolute devices (tablets, remote desktop) report positions here. Those are
        // dropped right away: the track must never contain where the pointer is.
        let relative = m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 == 0;
        if relative && (m.lLastX != 0 || m.lLastY != 0) {
            p.handle(t, RawEvent::Move { dx: m.lLastX, dy: m.lLastY });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LayoutKind, Preset, Scope};
    use std::time::Instant;

    /// What Windows itself lists for this process: (keyboard, mouse) registered as sink.
    fn os_registered() -> (bool, bool) {
        unsafe {
            let mut count = 0u32;
            let size = size_of::<RAWINPUTDEVICE>() as u32;
            GetRegisteredRawInputDevices(None, &mut count, size);
            let mut list = vec![RAWINPUTDEVICE::default(); count as usize];
            if count > 0 {
                GetRegisteredRawInputDevices(Some(list.as_mut_ptr()), &mut count, size);
            }
            let has = |usage| list.iter().any(|d| d.usUsagePage == USAGE_PAGE_GENERIC && d.usUsage == usage && d.dwFlags.0 & RIDEV_INPUTSINK.0 != 0);
            (has(USAGE_KEYBOARD), has(USAGE_MOUSE))
        }
    }

    fn wait_for(capture: &InputCapture, want: (bool, bool)) {
        let start = Instant::now();
        while capture.registered() != want || os_registered() != want {
            assert!(start.elapsed() < Duration::from_secs(3), "registration stayed at {:?}, wanted {want:?}", os_registered());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    // One test for the whole life cycle: raw input registration is per process, two
    // tests running side by side would see each other's devices.
    #[test]
    fn devices_are_registered_only_while_capturing() {
        let layout = OverlayLayout::preset(LayoutKind::IsoQwertz, Preset::LeftHandShooter);
        let capture = InputCapture::start(&layout, Duration::from_secs(30)).unwrap();
        // Fresh capture: no game in the foreground, so nothing is registered at all.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!((capture.registered(), os_registered()), ((false, false), (false, false)));

        capture.set_game_active(true);
        wait_for(&capture, (true, true));
        capture.set_paused(true);
        wait_for(&capture, (false, false));
        capture.set_paused(false);
        wait_for(&capture, (true, true));

        // A layout without mouse parts does not listen to the mouse.
        capture.set_layout(&OverlayLayout::new("wasd", LayoutKind::IsoQwertz, Scope::Preset { preset: Preset::Wasd }));
        wait_for(&capture, (true, false));
        // A layout without keys does not listen to the keyboard either.
        capture.set_layout(&OverlayLayout::new("none", LayoutKind::IsoQwertz, Scope::Keys { keys: vec![] }));
        wait_for(&capture, (false, false));

        capture.set_layout(&layout);
        wait_for(&capture, (true, true));
        capture.set_game_active(false);
        wait_for(&capture, (false, false));
        assert_eq!(capture.last_error(), None);

        // Dropping while registered must clean up as well.
        capture.set_game_active(true);
        wait_for(&capture, (true, true));
        drop(capture);
        assert_eq!(os_registered(), (false, false));
    }

    #[test]
    fn clock_runs_forward_in_100ns() {
        let a = now_100ns();
        std::thread::sleep(Duration::from_millis(20));
        let passed = now_100ns() - a;
        assert!((150_000..2_000_000).contains(&passed), "{passed}");
    }
}
