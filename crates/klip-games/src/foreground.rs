//! Which program is in the foreground, and which processes run.
//!
//! ANTI-CHEAT: this file is the only place in the crate that touches other processes, and
//! it is limited to exactly three things the rules in the vault allow:
//!
//! 1. `OpenProcess` with `PROCESS_QUERY_LIMITED_INFORMATION` and nothing else, followed by
//!    `QueryFullProcessImageNameW`. That right cannot read or write memory, cannot touch
//!    threads and cannot list modules. The handle is closed again at once.
//! 2. ToolHelp snapshots of the PROCESS list (`TH32CS_SNAPPROCESS`). Never module, heap
//!    or thread snapshots: those walk into the address space of the target.
//! 3. Read only questions about a window: who owns it and where it is on screen.
//!
//! Nothing here changes a foreign window, sends it a message or produces input. Do not
//! add another access right, `ReadProcessMemory`, `EnumProcessModules`,
//! `TH32CS_SNAPMODULE`, `SetWindowPos`, `PostMessage` to a foreign window or `SendInput`.

use std::path::PathBuf;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId};

/// Result of asking for the exe path of a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessQuery {
    Path(PathBuf),
    /// The process exists but does not hand out even the limited query right (system
    /// processes, or an anti-cheat that strips handles). Klip does not try harder.
    Denied,
    /// No process with this id.
    Gone,
}

/// Asks Windows for the full path of the exe of a process.
pub fn query_process_path(pid: u32) -> ProcessQuery {
    unsafe {
        // The one and only access right this crate ever requests on a foreign process.
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(e) if e.code() == ERROR_INVALID_PARAMETER.to_hresult() => return ProcessQuery::Gone,
            Err(_) => return ProcessQuery::Denied,
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        match result {
            Ok(()) => ProcessQuery::Path(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))),
            // Typically a process that has exited while something still holds a handle.
            Err(_) => ProcessQuery::Denied,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    /// File name of the exe without the folder.
    pub exe_name: String,
}

/// The process list as a ToolHelp snapshot: ids and exe names, nothing from inside the
/// processes.
pub fn process_list() -> Vec<ProcessEntry> {
    let mut list = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return list };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            list.push(ProcessEntry {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                exe_name: String::from_utf16_lossy(&entry.szExeFile[..len]),
            });
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    list
}

/// Exe name of one process out of a fresh snapshot. The fallback when the path is denied.
pub fn process_name(pid: u32) -> Option<String> {
    process_list().into_iter().find(|p| p.pid == pid).map(|p| p.exe_name)
}

/// Window handle and process id of the foreground window. Two cheap calls, this is what
/// the watcher polls. The handle is kept as a number so the value can cross threads; it
/// is only ever compared and passed back to the read only calls in this file.
pub fn foreground_ids() -> Option<(isize, u32)> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some((hwnd.0 as isize, pid))
    }
}

/// How many pixels a window may miss a monitor edge and still count as covering it.
/// Klip may run without DPI awareness, then Windows scales foreign window rectangles and
/// rounds them.
const COVER_TOLERANCE: i32 = 2;

/// The "fullscreen foreground window" heuristic: does the window rectangle cover the whole
/// monitor it is on? True for exclusive fullscreen and for borderless windows. Only reads
/// the rectangle, the window is not changed.
pub fn covers_monitor(hwnd: isize) -> bool {
    unsafe {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let mut window = RECT::default();
        if GetWindowRect(hwnd, &mut window).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        if monitor.is_invalid() {
            return false;
        }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let m = info.rcMonitor;
        window.left <= m.left + COVER_TOLERANCE
            && window.top <= m.top + COVER_TOLERANCE
            && window.right >= m.right - COVER_TOLERANCE
            && window.bottom >= m.bottom - COVER_TOLERANCE
    }
}

/// Everything Klip knows about the program in the foreground.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundInfo {
    pub hwnd: isize,
    pub pid: u32,
    /// `None` if Windows refused the path.
    pub exe_path: Option<PathBuf>,
    /// From the path, or from the process list if the path was refused.
    pub exe_name: Option<String>,
    pub covers_monitor: bool,
}

/// Resolves a foreground window to its program.
pub fn resolve_foreground(hwnd: isize, pid: u32) -> ForegroundInfo {
    let exe_path = match query_process_path(pid) {
        ProcessQuery::Path(path) => Some(path),
        ProcessQuery::Denied | ProcessQuery::Gone => None,
    };
    let exe_name = match &exe_path {
        Some(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()),
        None => process_name(pid),
    };
    ForegroundInfo { hwnd, pid, exe_path, exe_name, covers_monitor: covers_monitor(hwnd) }
}

/// The program in the foreground right now, `None` if no window has the focus.
pub fn foreground_info() -> Option<ForegroundInfo> {
    let (hwnd, pid) = foreground_ids()?;
    Some(resolve_foreground(hwnd, pid))
}

/// Is this process still running as the program we saw? Guards against a reused id.
/// Cheap path first, the process list only if Windows refuses the path.
pub fn is_alive(pid: u32, exe_path: Option<&std::path::Path>, exe_name: &str) -> bool {
    match query_process_path(pid) {
        ProcessQuery::Gone => false,
        ProcessQuery::Path(path) => match exe_path {
            Some(expected) => path.as_os_str().eq_ignore_ascii_case(expected.as_os_str()),
            None => path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(exe_name)),
        },
        // Refused: fall back to the snapshot, which needs no handle to the process.
        ProcessQuery::Denied => process_name(pid).is_some_and(|n| n.eq_ignore_ascii_case(exe_name)),
    }
}
