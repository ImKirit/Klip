//! Single instance lock through a named mutex. Two recorders would fight over the hotkeys
//! and both hold a capture session and an encoder, so the second one has to leave.

use std::io;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// Name of the recorder's lock. `Local\` keeps it inside the logon session: another user
/// on the same machine (fast user switching) gets a recorder of their own.
pub const RECORDER_LOCK: &str = "Local\\Klip.Recorder.SingleInstance";

/// Holds the lock for as long as it lives. Windows also releases it when the process
/// dies, so a crash never leaves a stale lock behind.
#[derive(Debug)]
pub struct SingleInstance {
    handle: HANDLE,
}

// The handle is only closed, from whichever thread drops the value.
unsafe impl Send for SingleInstance {}

impl SingleInstance {
    /// `Ok(Some(lock))` for the first instance, `Ok(None)` if another one holds the name.
    pub fn acquire(name: &str) -> io::Result<Option<Self>> {
        unsafe {
            let handle = CreateMutexW(None, false, &HSTRING::from(name)).map_err(io::Error::other)?;
            // CreateMutexW succeeds for an existing mutex too and says so in the last error.
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(Self { handle }))
        }
    }

    /// The lock of the recorder itself.
    pub fn acquire_recorder() -> io::Result<Option<Self>> {
        Self::acquire(RECORDER_LOCK)
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_refused_until_the_first_is_dropped() {
        let name = format!("Local\\Klip.Test.SingleInstance.{}", std::process::id());
        let first = SingleInstance::acquire(&name).unwrap();
        assert!(first.is_some());
        assert!(SingleInstance::acquire(&name).unwrap().is_none());
        assert!(SingleInstance::acquire(&name).unwrap().is_none(), "a refused attempt must not hold the name");
        drop(first);
        assert!(SingleInstance::acquire(&name).unwrap().is_some());
    }
}
