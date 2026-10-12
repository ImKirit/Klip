//! Autostart with Windows: one value under the user's `Run` key. No service, no scheduled
//! task, no admin rights, and the user can see and remove it in Task Manager.

use std::io;
use std::path::Path;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// Name of the value. Also the name Task Manager shows in its autostart list.
pub const VALUE_NAME: &str = "Klip";

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0 as i32))
    }
}

/// The command line stored for Klip, `None` if autostart is off.
pub fn command() -> Option<String> {
    let key = HSTRING::from(RUN_KEY);
    let name = HSTRING::from(VALUE_NAME);
    unsafe {
        let mut bytes = 0u32;
        if RegGetValueW(HKEY_CURRENT_USER, &key, &name, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; bytes as usize / 2 + 2];
        let mut bytes = (buf.len() * 2) as u32;
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        );
        if status != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

pub fn is_enabled() -> bool {
    command().is_some()
}

/// True if the entry starts exactly this exe. False after the program was moved, then
/// the entry points at nothing and should be written again.
pub fn points_to(exe_path: &Path) -> bool {
    let quoted = format!("\"{}\"", exe_path.display());
    command().is_some_and(|c| c.to_lowercase().starts_with(&quoted.to_lowercase()))
}

/// Writes the raw command line. `enable` is the normal way in, this one exists for
/// arguments the caller formats itself.
pub fn write_command(command: &str) -> io::Result<()> {
    let mut key = HKEY::default();
    let data: Vec<u8> = command.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    unsafe {
        check(RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(RUN_KEY),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        ))?;
        let status = RegSetValueExW(key, &HSTRING::from(VALUE_NAME), None, REG_SZ, Some(&data));
        let _ = RegCloseKey(key);
        check(status)
    }
}

/// Turns autostart on for this exe. The path is quoted, so spaces in it are safe.
pub fn enable(exe_path: &Path) -> io::Result<()> {
    enable_with_args(exe_path, "")
}

/// Like `enable`, with arguments appended after the quoted path (for example a flag that
/// tells the recorder it was started by Windows and should stay silent).
pub fn enable_with_args(exe_path: &Path, args: &str) -> io::Result<()> {
    if !exe_path.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "autostart needs an absolute path"));
    }
    let mut command = format!("\"{}\"", exe_path.display());
    if !args.trim().is_empty() {
        command.push(' ');
        command.push_str(args.trim());
    }
    write_command(&command)
}

/// Turns autostart off. Not an error if it was off already.
pub fn disable() -> io::Result<()> {
    let mut key = HKEY::default();
    unsafe {
        match RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), None, KEY_SET_VALUE, &mut key) {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(()),
            other => return check(other),
        }
        let status = RegDeleteValueW(key, &HSTRING::from(VALUE_NAME));
        let _ = RegCloseKey(key);
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check(status)
    }
}
