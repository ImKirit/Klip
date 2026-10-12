//! Test backend for the browser preview. It offers the same commands as the app over
//! HTTP on 127.0.0.1, so the UI can be opened and driven in a normal browser.
//!
//! This binary is never shipped. The real app has no socket at all.
//! It refuses to start without KLIP_CLIP_DIR, so it cannot touch the real clip folder
//! by accident.

use klip_app_lib::backend::{self, Backend};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "mp4" | "m4v" | "mov" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

fn serve_file(request: Request, query: &str) {
    let path = query.split('&').find_map(|kv| kv.strip_prefix("p=")).map(percent_decode).unwrap_or_default();
    let path = Path::new(&path).to_path_buf();
    // Same rule as the asset scope of the real app.
    let allowed = backend::is_inside(&path, &backend::clip_dir()) || backend::is_inside(&path, &backend::cache_dir());
    let file = if allowed { std::fs::File::open(&path).ok() } else { None };
    let Some(mut file) = file else {
        let _ = request.respond(Response::from_string("not found").with_status_code(404));
        return;
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| h.value.as_str().strip_prefix("bytes=").map(str::to_string));
    let mut headers = vec![header("Content-Type", mime(&path)), header("Accept-Ranges", "bytes"), header("Cache-Control", "no-store")];
    let (status, start, len) = match range {
        Some(spec) if size > 0 => {
            let (from, to) = spec.split_once('-').unwrap_or((&spec, ""));
            let start = from.trim().parse::<u64>().unwrap_or(0).min(size - 1);
            let end = to.trim().parse::<u64>().unwrap_or(size - 1).min(size - 1).max(start);
            headers.push(header("Content-Range", &format!("bytes {start}-{end}/{size}")));
            (206, start, end - start + 1)
        }
        _ => (200, 0, size),
    };
    let _ = file.seek(SeekFrom::Start(start));
    let response = Response::new(StatusCode(status), headers, file.take(len), Some(len as usize), None);
    let _ = request.respond(response);
}

fn serve_invoke(mut request: Request, backend: &Backend) {
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    let result = serde_json::from_str::<Value>(&body).map_err(|e| e.to_string()).and_then(|call| {
        let cmd = call.get("cmd").and_then(Value::as_str).unwrap_or_default().to_string();
        let args = call.get("args").cloned().unwrap_or_else(backend::empty_args);
        backend.dispatch(&cmd, &args)
    });
    let payload = match result {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({ "error": error }),
    };
    let _ = request.respond(Response::from_string(payload.to_string()).with_header(header("Content-Type", "application/json")));
}

fn main() {
    if std::env::var_os("KLIP_CLIP_DIR").is_none() || std::env::var_os("KLIP_DATA_DIR").is_none() {
        eprintln!("Set KLIP_CLIP_DIR, KLIP_DATA_DIR and KLIP_CACHE_DIR to test folders first.");
        std::process::exit(2);
    }
    let port = std::env::var("KLIP_DEV_PORT").unwrap_or_else(|_| "1431".into());
    let server = Arc::new(Server::http(format!("127.0.0.1:{port}")).expect("port is free"));
    let backend = Arc::new(Backend::new());
    println!("klip test backend on http://127.0.0.1:{port}, clips in {}", backend::clip_dir().display());
    let workers: Vec<_> = (0..12)
        .map(|_| {
            let server = server.clone();
            let backend = backend.clone();
            std::thread::spawn(move || {
                while let Ok(request) = server.recv() {
                    let url = request.url().to_string();
                    let (route, query) = url.split_once('?').unwrap_or((&url, ""));
                    match (request.method(), route) {
                        (Method::Post, "/__klip/invoke") => serve_invoke(request, &backend),
                        (Method::Get, "/__klip/file") => serve_file(request, query),
                        _ => {
                            let _ = request.respond(Response::from_string("not found").with_status_code(404));
                        }
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
}
