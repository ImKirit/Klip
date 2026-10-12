//! The overlay thread: windows, composition, the message loop.
//!
//! One window per visible element, exactly as large as the element. A window is created
//! when its element appears and destroyed when it has faded out, so with nothing to show
//! there is no window at all and the loop waits without a timeout.
//!
//! Window styles and why:
//! * `WS_EX_NOREDIRECTIONBITMAP` with a DirectComposition swap chain: per pixel alpha
//!   without a GDI surface behind the window.
//! * `WS_EX_TOPMOST`: above a borderless game.
//! * `WS_EX_NOACTIVATE` and `MA_NOACTIVATE`: the game never loses focus to the overlay.
//! * `WS_EX_TOOLWINDOW`: no taskbar button, not in Alt+Tab.
//! * `WS_EX_LAYERED | WS_EX_TRANSPARENT` on everything but the panel: mouse input falls
//!   through to the window below. `WS_EX_TRANSPARENT` alone only does that inside one
//!   thread, across processes Windows wants the layered bit as well.
//! * `WDA_EXCLUDEFROMCAPTURE`: not part of any screen capture.
//!
//! The panel can be dragged by its grip. That moves our own panel window with
//! `SetWindowPos` and takes the mouse capture for our own window while the button is
//! down. Nothing else moves.
//!
//! Nothing here calls into another process's window.

use crate::diag::PxRect;
use crate::fonts::{FontReport, Fonts};
use crate::gfx::{color, with_offset_and_opacity, Gfx};
use crate::scene::{clamp_to_area, resolve, ticks, Resolved, ToastItem};
use crate::sound::{play_cue, SoundConfig};
use crate::state::{Action, Element, MonitorTarget, OverlayState, Toast, MAX_TOASTS};
use crate::theme::Rgb;
use crate::views::{grip_height, Hit};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{w, Interface, BOOL};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1, ID2D1Image, ID2D1RenderTarget,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_CREATE_FACTORY_FLAGS,
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, LoadCursorW, MsgWaitForMultipleObjectsEx,
    PeekMessageW, PostMessageW, PostThreadMessageW, RegisterClassExW, SetWindowDisplayAffinity, SetWindowPos,
    ShowWindow, TranslateMessage, HWND_TOPMOST, IDC_ARROW, MA_NOACTIVATE, MONITORINFOF_PRIMARY, MSG,
    MWMO_INPUTAVAILABLE, PM_NOREMOVE, PM_REMOVE, QS_ALLINPUT, SWP_NOACTIVATE, SW_SHOWNOACTIVATE,
    WDA_EXCLUDEFROMCAPTURE, WM_APP, WM_CAPTURECHANGED, WM_DISPLAYCHANGE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_QUIT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const WM_WAKE: u32 = WM_APP + 1;
const FADE_IN: f32 = 0.15;
const FADE_OUT: f32 = 0.20;
/// Pause between two frames of a running fade.
const FRAME: Duration = Duration::from_millis(15);

#[derive(Clone, Debug)]
pub struct OverlayConfig {
    pub state: OverlayState,
    /// Font files for a private collection, see [`crate::font_files_in`]. May be empty.
    pub font_files: Vec<PathBuf>,
    pub sound: SoundConfig,
    /// Keep the windows out of screen captures. Only switch this off to diagnose.
    pub exclude_from_capture: bool,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        OverlayConfig {
            state: OverlayState::default(),
            font_files: Vec::new(),
            sound: SoundConfig::default(),
            exclude_from_capture: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: usize,
    pub rect: PxRect,
    pub primary: bool,
}

/// One overlay window that exists right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub element: Element,
    pub hwnd: isize,
    pub rect: PxRect,
    /// False when clicks fall through.
    pub takes_clicks: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverlayStats {
    /// Frames drawn per element, in the order of [`Element::ALL`].
    pub frames: [u64; 4],
    /// How often the thread woke up, for whatever reason.
    pub wakeups: u64,
    /// Windows that exist right now.
    pub windows: Vec<WindowInfo>,
    /// False when `SetWindowDisplayAffinity` failed for a window since the start.
    pub capture_excluded: bool,
    /// How long building the Direct3D, Direct2D and composition devices took at start.
    pub graphics_init_ms: u32,
}

struct Shared {
    state: OverlayState,
    /// Toasts the caller handed over since the last tick.
    toasts: Vec<Toast>,
    click: Option<Action>,
    /// Test only: a drag of the grip by this many pixels, see `Overlay::test_drag`.
    drag: Option<(i32, i32)>,
    quit: bool,
    windows: Vec<WindowInfo>,
}

struct Inner {
    shared: Mutex<Shared>,
    thread_id: AtomicU32,
    frames: [AtomicU64; 4],
    wakeups: AtomicU64,
    affinity_failed: AtomicBool,
    graphics_init_ms: AtomicU32,
    exclude: bool,
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wake(&self) {
        let id = self.thread_id.load(Ordering::Relaxed);
        if id != 0 {
            unsafe {
                let _ = PostThreadMessageW(id, WM_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Handle to the running overlay. Dropping it closes every window and ends the thread.
pub struct Overlay {
    inner: Arc<Inner>,
    join: Option<JoinHandle<()>>,
    sound: Mutex<SoundConfig>,
    fonts: FontReport,
}

impl Overlay {
    /// Starts the overlay thread. With the default state nothing becomes visible and no
    /// window is created. Panel clicks arrive on the returned channel.
    pub fn start(config: OverlayConfig) -> Result<(Overlay, Receiver<Action>), String> {
        let inner = Arc::new(Inner {
            shared: Mutex::new(Shared {
                state: config.state,
                toasts: Vec::new(),
                click: None,
                drag: None,
                quit: false,
                windows: Vec::new(),
            }),
            thread_id: AtomicU32::new(0),
            frames: Default::default(),
            wakeups: AtomicU64::new(0),
            affinity_failed: AtomicBool::new(false),
            graphics_init_ms: AtomicU32::new(0),
            exclude: config.exclude_from_capture,
        });
        let (actions_tx, actions_rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let thread_inner = inner.clone();
        let files = config.font_files;
        let join = std::thread::Builder::new()
            .name("klip-overlay".into())
            .spawn(move || run(thread_inner, files, actions_tx, ready_tx))
            .map_err(|e| format!("overlay thread: {e}"))?;
        let fonts = match ready_rx.recv() {
            Ok(Ok(report)) => report,
            Ok(Err(e)) => {
                let _ = join.join();
                return Err(e);
            }
            Err(_) => return Err("overlay thread ended during start".into()),
        };
        Ok((Overlay { inner, join: Some(join), sound: Mutex::new(config.sound), fonts }, actions_rx))
    }

    /// Replaces the whole state.
    pub fn set_state(&self, state: OverlayState) {
        self.update(|s| *s = state);
    }

    /// Changes the state in place. The overlay redraws only what looks different after.
    pub fn update(&self, change: impl FnOnce(&mut OverlayState)) {
        change(&mut self.inner.lock().state);
        self.inner.wake();
    }

    pub fn state(&self) -> OverlayState {
        self.inner.lock().state.clone()
    }

    /// Shows a toast for its own `hold`, or for `OverlayState::toast_hold`, and plays the
    /// tone if sound is on. Toasts stack below each other. With more than
    /// [`MAX_TOASTS`] on screen the oldest one drops out.
    pub fn toast(&self, toast: Toast) {
        let cue = toast.cue;
        self.inner.lock().toasts.push(toast);
        self.inner.wake();
        let sound = *self.sound.lock().unwrap_or_else(|e| e.into_inner());
        if sound.enabled {
            play_cue(cue, sound.volume);
        }
    }

    pub fn set_sound(&self, sound: SoundConfig) {
        *self.sound.lock().unwrap_or_else(|e| e.into_inner()) = sound;
    }

    /// Opens or closes the panel. Returns the new state.
    pub fn toggle_panel(&self) -> bool {
        let open = {
            let mut shared = self.inner.lock();
            let v = &mut shared.state.panel.placement.visible;
            *v = !*v;
            *v
        };
        self.inner.wake();
        open
    }

    /// Which font family each role uses.
    pub fn fonts(&self) -> &FontReport {
        &self.fonts
    }

    pub fn stats(&self) -> OverlayStats {
        OverlayStats {
            frames: std::array::from_fn(|i| self.inner.frames[i].load(Ordering::Relaxed)),
            wakeups: self.inner.wakeups.load(Ordering::Relaxed),
            windows: self.inner.lock().windows.clone(),
            capture_excluded: self.inner.exclude && !self.inner.affinity_failed.load(Ordering::Relaxed),
            graphics_init_ms: self.inner.graphics_init_ms.load(Ordering::Relaxed),
        }
    }

    /// For tests: posts a left click to the overlay's own panel window, in the middle of
    /// the button for `action`. Only our own window gets a message, nothing is sent to
    /// the system input queue. Does nothing while the panel is closed.
    #[doc(hidden)]
    pub fn test_click(&self, action: Action) {
        self.inner.lock().click = Some(action);
        self.inner.wake();
    }

    /// For tests: posts a drag of the grip by `(dx, dy)` pixels to the overlay's own
    /// panel window (button down on the grip, one move, button up). Only our own window
    /// gets messages, nothing is sent to the system input queue and the mouse pointer
    /// does not move. Does nothing while the panel is closed.
    #[doc(hidden)]
    pub fn test_drag(&self, dx: i32, dy: i32) {
        self.inner.lock().drag = Some((dx, dy));
        self.inner.wake();
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.inner.lock().quit = true;
        self.inner.wake();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The monitors in the order Windows enumerates them, in physical pixels.
pub fn list_monitors() -> Vec<MonitorInfo> {
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<MonitorInfo>);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            list.push(MonitorInfo {
                index: list.len(),
                rect: info.rcMonitor.into(),
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        BOOL(1)
    }
    let mut list: Vec<MonitorInfo> = Vec::new();
    unsafe {
        // Physical pixels whatever the caller's DPI mode is.
        let before = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut _ as isize));
        if !before.0.is_null() {
            SetThreadDpiAwarenessContext(before);
        }
    }
    list
}

fn monitor_rect(target: MonitorTarget) -> PxRect {
    let fallback = PxRect { left: 0, top: 0, right: 1920, bottom: 1080 };
    let list = list_monitors();
    let primary = || list.iter().find(|m| m.primary).or(list.first()).map_or(fallback, |m| m.rect);
    match target {
        MonitorTarget::Primary => primary(),
        MonitorTarget::Index(i) => list.get(i).map_or_else(primary, |m| m.rect),
        MonitorTarget::At { x, y } => unsafe {
            let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                info.rcMonitor.into()
            } else {
                primary()
            }
        },
    }
}

// ---------------------------------------------------------------------------------------
// Everything below runs on the overlay thread.

#[derive(Clone, Copy)]
struct MouseEvent {
    hwnd: isize,
    msg: u32,
    x: i32,
    y: i32,
}

thread_local! {
    static EVENTS: RefCell<Vec<MouseEvent>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // A click on the panel must not take the focus away from the game.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSELEAVE | WM_CAPTURECHANGED | WM_DISPLAYCHANGE => {
            let (x, y) = ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
            EVENTS.with(|e| e.borrow_mut().push(MouseEvent { hwnd: hwnd.0 as isize, msg, x, y }));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

struct Graphics {
    d3d: ID3D11Device,
    dxgi: IDXGIFactory2,
    dc: ID2D1DeviceContext,
    dcomp: IDCompositionDevice,
}

impl Graphics {
    fn new() -> windows::core::Result<Graphics> {
        unsafe {
            let create = |kind: D3D_DRIVER_TYPE| -> windows::core::Result<ID3D11Device> {
                let mut device = None;
                D3D11CreateDevice(
                    None,
                    kind,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
                device.ok_or_else(windows::core::Error::empty)
            };
            let d3d = create(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| create(D3D_DRIVER_TYPE_WARP))?;
            let dxgi_device: IDXGIDevice = d3d.cast()?;
            let dxgi: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dc = factory.CreateDevice(&dxgi_device)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            Ok(Graphics { d3d, dxgi, dc, dcomp })
        }
    }

    fn swap_chain(&self, w: u32, h: u32) -> windows::core::Result<IDXGISwapChain1> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        unsafe { self.dxgi.CreateSwapChainForComposition(&self.d3d, &desc, None) }
    }
}

struct Surface {
    hwnd: HWND,
    swap: IDXGISwapChain1,
    visual: IDCompositionVisual,
    // Kept alive: releasing the target detaches the visual tree from the window.
    _target: IDCompositionTarget,
    rect: PxRect,
    shown: bool,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

struct Fade {
    from: f32,
    to: f32,
    start: Instant,
}

impl Fade {
    fn value(&self, now: Instant) -> f32 {
        let length = if self.to > self.from { FADE_IN } else { FADE_OUT };
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / length).min(1.0);
        self.from + (self.to - self.from) * t
    }

    fn aim(&mut self, to: f32, now: Instant) {
        if self.to != to {
            self.from = self.value(now);
            self.to = to;
            self.start = now;
        }
    }
}

struct Slot {
    element: Element,
    fade: Fade,
    surface: Option<Surface>,
    /// What the element showed last, kept so it can fade out after it is no longer wanted.
    last: Option<Resolved>,
    /// What is on screen: scene and opacity. Equal means no redraw.
    drawn: Option<(Resolved, f32)>,
    hits: Vec<Hit>,
}

/// A toast on screen with its own clock.
struct LiveToast {
    toast: Toast,
    start: Instant,
    hold: Duration,
}

impl LiveToast {
    /// Fade of this toast right now: in, hold, out. `None` once it is gone.
    fn alpha(&self, now: Instant) -> Option<f32> {
        let age = now.saturating_duration_since(self.start).as_secs_f32();
        let over = age - FADE_IN - self.hold.as_secs_f32();
        if over >= FADE_OUT {
            return None;
        }
        Some((age / FADE_IN).min(1.0).min(1.0 - over.max(0.0) / FADE_OUT))
    }

    /// When this toast changes next: the next frame while it fades, else the moment
    /// its fade out begins.
    fn next_change(&self, now: Instant) -> Instant {
        let fade_out = self.start + Duration::from_secs_f32(FADE_IN) + self.hold;
        if now.saturating_duration_since(self.start).as_secs_f32() < FADE_IN || now >= fade_out {
            now + FRAME
        } else {
            fade_out
        }
    }
}

/// The grip is held. `grab` is where inside the window the button went down, `from`
/// where the panel stood then, `at` where it stands now, both relative to the monitor.
struct Drag {
    grab: (i32, i32),
    from: (i32, i32),
    at: (i32, i32),
}

struct Engine {
    inner: Arc<Inner>,
    fonts: Fonts,
    gfx: Option<Graphics>,
    slots: [Slot; 4],
    actions: Sender<Action>,
    monitor: Option<(MonitorTarget, PxRect)>,
    /// Oldest first.
    toasts: Vec<LiveToast>,
    hover: Option<Action>,
    pressed: Option<Action>,
    tracking: bool,
    drag: Option<Drag>,
}

fn run(
    inner: Arc<Inner>,
    font_files: Vec<PathBuf>,
    actions: Sender<Action>,
    ready: Sender<Result<FontReport, String>>,
) {
    unsafe {
        // Physical pixels on every monitor. Without this Windows would stretch the
        // windows on a scaled display and the text would blur.
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Make sure the thread has a message queue before anyone posts to it.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    }
    let fonts = match Fonts::new(&font_files) {
        Ok(f) => f,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if let Err(e) = register_class() {
        let _ = ready.send(Err(e));
        return;
    }
    let now = Instant::now();
    let slot = |element| Slot {
        element,
        fade: Fade { from: 0.0, to: 0.0, start: now },
        surface: None,
        last: None,
        drawn: None,
        hits: Vec::new(),
    };
    let mut engine = Engine {
        inner: inner.clone(),
        fonts,
        gfx: None,
        slots: Element::ALL.map(slot),
        actions,
        monitor: None,
        toasts: Vec::new(),
        hover: None,
        pressed: None,
        tracking: false,
        drag: None,
    };
    inner.thread_id.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    let _ = ready.send(Ok(engine.fonts.report().clone()));
    // Build the devices now, on this thread. The first device creation in a process can
    // take over a second, and the first toast must not wait for that.
    let began = Instant::now();
    engine.gfx = Graphics::new().ok();
    inner.graphics_init_ms.store(began.elapsed().as_millis().max(1) as u32, Ordering::Relaxed);

    let mut deadline = engine.tick();
    loop {
        // Without a deadline this waits forever: no timer, no polling.
        let timeout = match deadline {
            Some(at) => at.saturating_duration_since(Instant::now()).as_millis().min(60_000) as u32,
            None => u32::MAX,
        };
        unsafe {
            MsgWaitForMultipleObjectsEx(None, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                if msg.message != WM_WAKE {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        inner.wakeups.fetch_add(1, Ordering::Relaxed);
        if inner.lock().quit {
            // Dropping the engine destroys the windows.
            return;
        }
        deadline = engine.tick();
    }
}

fn register_class() -> Result<(), String> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| format!("module handle: {e}"))?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("KlipOverlayWindow"),
            ..Default::default()
        };
        // A second overlay in the same process finds the class already there. Fine.
        RegisterClassExW(&class);
        Ok(())
    }
}

impl Engine {
    /// Brings the windows in line with the state and returns when to wake up next.
    fn tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let events: Vec<MouseEvent> = EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
        let display_changed = events.iter().any(|e| e.msg == WM_DISPLAYCHANGE);
        self.mouse(&events);

        let (state, new_toasts, click, drag) = {
            let mut shared = self.inner.lock();
            (shared.state.clone(), std::mem::take(&mut shared.toasts), shared.click.take(), shared.drag.take())
        };
        for toast in new_toasts {
            let hold = toast.hold.unwrap_or(state.toast_hold);
            self.toasts.push(LiveToast { toast, start: now, hold });
        }
        if !state.toast.visible {
            // Switched off: nothing waits in the dark and nothing wakes the thread.
            self.toasts.clear();
        }
        // The oldest ones make room, the rest runs out on its own clock.
        let extra = self.toasts.len().saturating_sub(MAX_TOASTS);
        self.toasts.drain(..extra);
        self.toasts.retain(|t| t.alpha(now).is_some());
        let stack: Vec<ToastItem> = self
            .toasts
            .iter()
            .map(|t| ToastItem {
                title: t.toast.title.clone(),
                detail: t.toast.detail.clone(),
                kind: t.toast.kind,
                alpha: t.alpha(now).unwrap_or(0.0),
            })
            .collect();
        if self.monitor.map(|(t, _)| t) != Some(state.monitor) || display_changed {
            self.monitor = Some((state.monitor, monitor_rect(state.monitor)));
        }
        let monitor = self.monitor.map_or(PxRect { left: 0, top: 0, right: 1920, bottom: 1080 }, |(_, r)| r);
        let area = (monitor.width() as f32, monitor.height() as f32);
        let k = (area.1 / 1080.0).clamp(0.25, 8.0);

        if !self.panel_open() {
            self.hover = None;
            self.pressed = None;
        }

        let mut next: Option<Instant> = None;
        let mut sooner = |at: Instant| next = Some(next.map_or(at, |n: Instant| n.min(at)));
        let mut lost = false;
        for i in 0..4 {
            let element = self.slots[i].element;
            let hover = if element == Element::Panel { self.hover } else { None };
            let wanted = resolve(&state, element, &stack, hover, now, k);
            let slot = &mut self.slots[i];
            if element == Element::Toast {
                // Every toast fades by itself, the window around them does not.
                let v = if wanted.is_some() { 1.0 } else { 0.0 };
                slot.fade = Fade { from: v, to: v, start: now };
            }
            slot.fade.aim(if wanted.is_some() { 1.0 } else { 0.0 }, now);
            if wanted.is_some() {
                slot.last = wanted;
            }
            let alpha = slot.fade.value(now);
            let Some(resolved) = slot.last.clone().filter(|_| alpha > 0.0 || slot.fade.to > 0.0) else {
                // Faded out: the window goes away completely.
                slot.surface = None;
                slot.last = None;
                slot.drawn = None;
                slot.hits.clear();
                continue;
            };
            if alpha != slot.fade.to {
                sooner(now + FRAME);
            }
            let opacity = alpha * resolved.opacity;
            if slot.drawn.as_ref().is_some_and(|(r, o)| *r == resolved && *o == opacity) {
                continue;
            }
            if self.gfx.is_none() {
                self.gfx = Graphics::new().ok();
            }
            let Some(gfx) = self.gfx.as_ref() else { continue };
            match draw_slot(gfx, &self.fonts, slot, &resolved, opacity, monitor, area, k, self.inner.exclude) {
                Ok(affinity_ok) => {
                    if !affinity_ok {
                        self.inner.affinity_failed.store(true, Ordering::Relaxed);
                    }
                    slot.drawn = Some((resolved, opacity));
                    self.inner.frames[i].fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => lost = true,
            }
        }
        if lost {
            // Device lost or a window could not be built: drop everything and try again
            // from scratch on the next tick.
            for slot in &mut self.slots {
                slot.surface = None;
                slot.drawn = None;
            }
            self.gfx = None;
            sooner(now + Duration::from_millis(500));
        }

        for toast in &self.toasts {
            sooner(toast.next_change(now));
        }
        if let Some(since) = ticks(&state) {
            // Wake at the next full second of the recording clock.
            let secs = now.saturating_duration_since(since).as_secs();
            sooner(since + Duration::from_secs(secs + 1));
        }

        self.publish_windows();
        if let Some(action) = click {
            self.post_click(action);
        }
        if let Some((dx, dy)) = drag {
            self.post_drag(dx, dy);
        }
        next
    }

    fn panel_open(&self) -> bool {
        self.slots[Element::Panel as usize].surface.is_some()
    }

    fn publish_windows(&self) {
        let windows: Vec<WindowInfo> = self
            .slots
            .iter()
            .filter_map(|s| {
                s.surface.as_ref().map(|surface| WindowInfo {
                    element: s.element,
                    hwnd: surface.hwnd.0 as isize,
                    rect: surface.rect,
                    takes_clicks: s.element == Element::Panel,
                })
            })
            .collect();
        let mut shared = self.inner.lock();
        if shared.windows != windows {
            shared.windows = windows;
        }
    }

    fn mouse(&mut self, events: &[MouseEvent]) {
        let panel = &self.slots[Element::Panel as usize];
        let Some((hwnd, rect)) = panel.surface.as_ref().map(|s| (s.hwnd, s.rect)) else {
            // No window, so no capture either.
            self.drag = None;
            return;
        };
        let grip = panel.last.as_ref().map_or(0.0, |r| grip_height(r.scale));
        let hits = panel.hits.clone();
        for e in events.iter().filter(|e| e.hwnd == hwnd.0 as isize) {
            let at = hits.iter().find(|h| h.contains(e.x as f32, e.y as f32)).map(|h| h.action);
            // The grip is the bar at the top, minus the close button.
            let on_grip = at.is_none() && e.x >= 0 && e.x < rect.width() && e.y >= 0 && (e.y as f32) < grip;
            match e.msg {
                WM_MOUSEMOVE if self.drag.is_some() => {
                    let (Some(drag), Some((_, monitor))) = (self.drag.as_mut(), self.monitor) else { continue };
                    // Where the pointer is on the monitor, minus where it holds the panel.
                    let want = (
                        (rect.left + e.x - drag.grab.0 - monitor.left) as f32,
                        (rect.top + e.y - drag.grab.1 - monitor.top) as f32,
                    );
                    let area = (monitor.width() as f32, monitor.height() as f32);
                    let (x, y) = clamp_to_area(want, area, (rect.width() as f32, rect.height() as f32));
                    drag.at = (x as i32, y as i32);
                    // The tick that follows moves our own window there.
                    self.inner.lock().state.panel.position = Some(drag.at);
                }
                WM_MOUSEMOVE => {
                    self.hover = at;
                    if !self.tracking {
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        self.tracking = unsafe { TrackMouseEvent(&mut track).is_ok() };
                    }
                }
                WM_MOUSELEAVE => {
                    self.hover = None;
                    self.pressed = None;
                    self.tracking = false;
                }
                WM_LBUTTONDOWN if on_grip => {
                    let Some((_, monitor)) = self.monitor else { continue };
                    let from = (rect.left - monitor.left, rect.top - monitor.top);
                    self.drag = Some(Drag { grab: (e.x, e.y), from, at: from });
                    self.hover = None;
                    self.pressed = None;
                    // Our own window keeps getting the moves while the button is down,
                    // also when the pointer is faster than the window.
                    unsafe {
                        SetCapture(hwnd);
                    }
                }
                WM_LBUTTONDOWN => self.pressed = at,
                WM_LBUTTONUP if self.drag.is_some() => self.end_drag(true),
                // Something else took the capture: the drag ends where it is.
                WM_CAPTURECHANGED => self.end_drag(false),
                WM_LBUTTONUP => {
                    // A click counts when it is released on the button it went down on.
                    if let Some(action) = at.filter(|a| self.pressed == Some(*a)) {
                        if action == Action::ClosePanel {
                            self.inner.lock().state.panel.placement.visible = false;
                        }
                        let _ = self.actions.send(action);
                    }
                    self.pressed = None;
                }
                _ => {}
            }
        }
    }

    /// Ends a drag and reports where the panel stands now, if it moved at all.
    fn end_drag(&mut self, release: bool) {
        let Some(drag) = self.drag.take() else { return };
        if release {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        if drag.at != drag.from {
            let _ = self.actions.send(Action::PanelMoved { x: drag.at.0, y: drag.at.1 });
        }
    }

    fn post_drag(&self, dx: i32, dy: i32) {
        let panel = &self.slots[Element::Panel as usize];
        let (Some(surface), Some(last)) = (panel.surface.as_ref(), panel.last.as_ref()) else { return };
        // The middle of the grip is a handle in every language, see the tests in views.
        let (x, y) = (surface.rect.width() / 2, (grip_height(last.scale) / 2.0) as i32);
        let pack = |x: i32, y: i32| LPARAM((((y & 0xFFFF) << 16) | (x & 0xFFFF)) as isize);
        unsafe {
            // Our own window, our own thread. Never a foreign window, never the input queue.
            let _ = PostMessageW(Some(surface.hwnd), WM_LBUTTONDOWN, WPARAM(1), pack(x, y));
            let _ = PostMessageW(Some(surface.hwnd), WM_MOUSEMOVE, WPARAM(1), pack(x + dx, y + dy));
            let _ = PostMessageW(Some(surface.hwnd), WM_LBUTTONUP, WPARAM(0), pack(x + dx, y + dy));
        }
    }

    fn post_click(&self, action: Action) {
        let panel = &self.slots[Element::Panel as usize];
        let (Some(surface), Some(hit)) = (panel.surface.as_ref(), panel.hits.iter().find(|h| h.action == action))
        else {
            return;
        };
        let (x, y) = ((hit.x + hit.w / 2.0) as isize, (hit.y + hit.h / 2.0) as isize);
        let lparam = LPARAM((y << 16) | (x & 0xFFFF));
        unsafe {
            // Our own window, our own thread. Never a foreign window.
            let _ = PostMessageW(Some(surface.hwnd), WM_LBUTTONDOWN, WPARAM(1), lparam);
            let _ = PostMessageW(Some(surface.hwnd), WM_LBUTTONUP, WPARAM(0), lparam);
        }
    }
}

/// Creates or resizes the window of one element and draws it. Returns whether the
/// capture exclusion is in place.
#[allow(clippy::too_many_arguments)]
fn draw_slot(
    gfx: &Graphics,
    fonts: &Fonts,
    slot: &mut Slot,
    resolved: &Resolved,
    opacity: f32,
    monitor: PxRect,
    area: (f32, f32),
    k: f32,
    exclude: bool,
) -> windows::core::Result<bool> {
    let mut hits = Vec::new();
    let size = resolved.scene.view(&Gfx::dry(fonts), resolved.scale, &mut hits);
    let (ox, oy) = resolved.place.origin(area, size, k);
    let (w, h) = (size.0 as i32, size.1 as i32);
    let rect = PxRect {
        left: monitor.left + ox as i32,
        top: monitor.top + oy as i32,
        right: monitor.left + ox as i32 + w,
        bottom: monitor.top + oy as i32 + h,
    };
    let mut affinity_ok = true;
    unsafe {
        match slot.surface.as_mut() {
            None => {
                let mut ex = WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
                if slot.element != Element::Panel {
                    ex |= WS_EX_LAYERED | WS_EX_TRANSPARENT;
                }
                let instance = GetModuleHandleW(None)?;
                let hwnd = CreateWindowExW(
                    ex,
                    w!("KlipOverlayWindow"),
                    w!("Klip Overlay"),
                    WS_POPUP,
                    rect.left,
                    rect.top,
                    w,
                    h,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )?;
                if exclude {
                    affinity_ok = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok();
                }
                let built = (|| {
                    let swap = gfx.swap_chain(w as u32, h as u32)?;
                    let target = gfx.dcomp.CreateTargetForHwnd(hwnd, true)?;
                    let visual = gfx.dcomp.CreateVisual()?;
                    visual.SetContent(&swap)?;
                    target.SetRoot(&visual)?;
                    Ok::<_, windows::core::Error>((swap, target, visual))
                })();
                match built {
                    Ok((swap, target, visual)) => {
                        slot.surface = Some(Surface { hwnd, swap, visual, _target: target, rect, shown: false })
                    }
                    Err(e) => {
                        let _ = DestroyWindow(hwnd);
                        return Err(e);
                    }
                }
            }
            Some(surface) if surface.rect != rect => {
                if surface.rect.width() != w || surface.rect.height() != h {
                    surface.swap = gfx.swap_chain(w as u32, h as u32)?;
                    surface.visual.SetContent(&surface.swap)?;
                }
                SetWindowPos(surface.hwnd, Some(HWND_TOPMOST), rect.left, rect.top, w, h, SWP_NOACTIVATE)?;
                surface.rect = rect;
            }
            Some(_) => {}
        }
        let surface = slot.surface.as_mut().expect("surface was just ensured");

        let buffer: IDXGISurface = surface.swap.GetBuffer(0)?;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap = gfx.dc.CreateBitmapFromDxgiSurface(&buffer, Some(&props))?;
        gfx.dc.SetTarget(&bitmap);
        gfx.dc.BeginDraw();
        gfx.dc.Clear(Some(&color(Rgb(0, 0, 0), 0.0)));
        let rt: &ID2D1RenderTarget = &gfx.dc;
        hits.clear();
        let painter = Gfx::on(fonts, rt);
        if let Ok(painter) = &painter {
            with_offset_and_opacity(rt, 0.0, 0.0, opacity, || {
                resolved.scene.view(painter, resolved.scale, &mut hits);
            });
        }
        drop(painter);
        let ended = gfx.dc.EndDraw(None, None);
        gfx.dc.SetTarget(None::<&ID2D1Image>);
        ended?;
        surface.swap.Present(0, DXGI_PRESENT(0)).ok()?;
        gfx.dcomp.Commit()?;
        if !surface.shown {
            // Shown only after the first frame is in the swap chain, without activation.
            let _ = ShowWindow(surface.hwnd, SW_SHOWNOACTIVATE);
            surface.shown = true;
        }
    }
    slot.hits = hits;
    Ok(affinity_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_fades_in_holds_and_fades_out_on_its_own_clock() {
        let start = Instant::now();
        let live = LiveToast { toast: Toast::new("a", ""), start, hold: Duration::from_millis(1000) };
        let at = |ms: u64| live.alpha(start + Duration::from_millis(ms));
        assert_eq!(at(0), Some(0.0));
        assert!(at(75).is_some_and(|a| a > 0.4 && a < 0.6));
        assert_eq!(at(160), Some(1.0));
        assert_eq!(at(1100), Some(1.0));
        assert!(at(1250).is_some_and(|a| a > 0.4 && a < 0.6));
        assert_eq!(at(1360), None);
        // While it stands still the thread sleeps until the fade out begins.
        let now = start + Duration::from_millis(400);
        let wait = live.next_change(now).saturating_duration_since(start).as_secs_f32();
        assert!((wait - 1.15).abs() < 0.001, "{wait}");
        assert_eq!(live.next_change(start), start + FRAME);
        let late = start + Duration::from_millis(1200);
        assert_eq!(live.next_change(late), late + FRAME);
    }
}
