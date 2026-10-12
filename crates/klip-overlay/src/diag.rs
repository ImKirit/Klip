//! Diagnosis: does a foreign window cover its whole monitor?
//!
//! The recorder needs this for the one time hint "the overlay needs borderless mode in
//! this game". Only three read calls are used: `GetWindowRect`, `MonitorFromWindow` and
//! `GetMonitorInfoW`. The window is never changed and no process handle is opened.
//!
//! Limit: a covering window says "fullscreen of some kind". Borderless and exclusive
//! fullscreen both cover the monitor, and from outside they look the same here.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

impl From<RECT> for PxRect {
    fn from(r: RECT) -> Self {
        PxRect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub window: PxRect,
    pub monitor: PxRect,
    pub covers_monitor: bool,
}

/// True when `window` reaches every edge of `monitor`. A window that is larger than the
/// monitor (some borderless games hang over by a few pixels) covers it too.
pub fn covers(window: PxRect, monitor: PxRect) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
        && monitor.width() > 0
        && monitor.height() > 0
}

/// Rectangles of the window and of the monitor it is mostly on. `None` when the handle
/// is not a window or is on no monitor (minimized windows sit far off screen).
pub fn window_coverage(hwnd: isize) -> Option<Coverage> {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let (window, monitor) = (PxRect::from(rect), PxRect::from(info.rcMonitor));
        Some(Coverage { window, monitor, covers_monitor: covers(window, monitor) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: PxRect = PxRect { left: 1920, top: 0, right: 3840, bottom: 1080 };

    #[test]
    fn covering() {
        assert!(covers(MON, MON));
        assert!(covers(PxRect { left: 1912, top: -8, right: 3848, bottom: 1088 }, MON));
        // A maximized window leaves the taskbar free, a normal window is smaller.
        assert!(!covers(PxRect { left: 1920, top: 0, right: 3840, bottom: 1040 }, MON));
        assert!(!covers(PxRect { left: 2000, top: 100, right: 3000, bottom: 800 }, MON));
        // A window on the other monitor.
        assert!(!covers(PxRect { left: 0, top: 0, right: 1920, bottom: 1080 }, MON));
        assert!(!covers(MON, PxRect { left: 0, top: 0, right: 0, bottom: 0 }));
    }

    #[test]
    fn a_handle_that_is_no_window_gives_none() {
        assert_eq!(window_coverage(0), None);
        assert_eq!(window_coverage(0x7FFF_FFF0), None);
    }
}
