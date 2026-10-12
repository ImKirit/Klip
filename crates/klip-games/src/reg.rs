//! Read only registry helpers. This crate never writes to the registry.

use windows::core::HSTRING;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY, KEY_QUERY_VALUE, RRF_RT_REG_SZ,
};

pub(crate) use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

/// Reads a `REG_SZ` value, `None` if the key or value is missing or has another type.
pub(crate) fn read_string(hive: HKEY, subkey: &str, value: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);
    unsafe {
        let mut bytes = 0u32;
        if RegGetValueW(hive, &subkey, &value, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; bytes as usize / 2 + 2];
        let mut bytes = (buf.len() * 2) as u32;
        let status = RegGetValueW(
            hive,
            &subkey,
            &value,
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

/// True if the key exists and may be opened for reading.
pub(crate) fn key_exists(hive: HKEY, subkey: &str) -> bool {
    let mut key = HKEY::default();
    unsafe {
        if RegOpenKeyExW(hive, &HSTRING::from(subkey), None, KEY_QUERY_VALUE, &mut key) != ERROR_SUCCESS {
            return false;
        }
        let _ = RegCloseKey(key);
    }
    true
}
