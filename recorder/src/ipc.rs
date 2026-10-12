//! Named pipe the app uses to talk to the recorder. One line of JSON in, one line out.
//! A pipe instead of a socket on purpose: nothing listens on the network, not even on
//! localhost, and remote clients are rejected by the pipe itself.

use crate::Res;
use klip_core::{IpcRequest, IpcResponse, PIPE_NAME};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

/// Requests longer than this are refused, nothing legitimate comes close.
const MAX_LINE: usize = 64 * 1024;

struct Pipe(HANDLE);
// SAFETY: a pipe handle may be used from any thread, and each one is owned by one thread.
unsafe impl Send for Pipe {}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            let _ = FlushFileBuffers(self.0);
            let _ = DisconnectNamedPipe(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// Starts the server thread. `handler` runs on the thread of the connected client.
pub fn serve<F>(handler: F) -> Res<()>
where
    F: Fn(IpcRequest) -> IpcResponse + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let name: Vec<u16> = PIPE_NAME.encode_utf16().chain(Some(0)).collect();
    // Created once up front so a second recorder notices the name is taken.
    let mut next = Some(create(&name)?);
    std::thread::Builder::new().name("klip-ipc".into()).spawn(move || loop {
        let pipe = match next.take().map(Ok).unwrap_or_else(|| create(&name)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("klip-recorder: pipe closed: {e}");
                return;
            }
        };
        let connected = unsafe { ConnectNamedPipe(pipe.0, None) };
        if let Err(e) = connected {
            if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                continue;
            }
        }
        let handler = handler.clone();
        let _ = std::thread::Builder::new().name("klip-ipc-client".into()).spawn(move || client(pipe, &*handler));
    })?;
    Ok(())
}

fn create(name: &[u16]) -> Res<Pipe> {
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            None,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!("could not open {PIPE_NAME}: {}", windows::core::Error::from_thread()).into());
    }
    Ok(Pipe(handle))
}

fn client(pipe: Pipe, handler: &dyn Fn(IpcRequest) -> IpcResponse) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut read = 0u32;
        if unsafe { ReadFile(pipe.0, Some(&mut buf), Some(&mut read), None) }.is_err() || read == 0 {
            return;
        }
        pending.extend_from_slice(&buf[..read as usize]);
        if pending.len() > MAX_LINE {
            return;
        }
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let response = match serde_json::from_slice::<IpcRequest>(&line) {
                Ok(request) => handler(request),
                Err(e) => IpcResponse { ok: false, error: Some(format!("bad request: {e}")), status: None },
            };
            let mut out = serde_json::to_vec(&response).unwrap_or_default();
            out.push(b'\n');
            let mut written = 0u32;
            if unsafe { WriteFile(pipe.0, Some(&out), Some(&mut written), None) }.is_err() {
                return;
            }
        }
    }
}
