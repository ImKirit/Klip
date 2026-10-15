//! The Klip app: one window with library, editor and settings. No tray and no background
//! process, closing the window ends everything including running exports.

pub mod backend;

use backend::Backend;
use serde_json::Value;
use std::sync::Arc;
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

/// Lets the webview load files from the clip folder and the cache, nothing else.
fn allow_media_dirs(app: &AppHandle) {
    let scope = app.asset_protocol_scope();
    for dir in [backend::clip_dir(), backend::cache_dir()] {
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = scope.allow_directory(&dir, true) {
            eprintln!("asset scope {}: {e}", dir.display());
        }
    }
}

/// The single door from the UI to the backend. Runs off the main thread because some
/// commands wait for ffmpeg.
#[tauri::command]
async fn klip(app: AppHandle, state: State<'_, Arc<Backend>>, cmd: String, args: Option<Value>) -> Result<Value, String> {
    let backend = state.inner().clone();
    let name = cmd.clone();
    let args = args.unwrap_or_else(backend::empty_args);
    let out = tauri::async_runtime::spawn_blocking(move || backend.dispatch(&name, &args))
        .await
        .map_err(|e| e.to_string())??;
    if cmd == "config_save" {
        // The clip folder may have changed.
        allow_media_dirs(&app);
    }
    Ok(out)
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// The clip after `--open`, if the app was started with one.
fn open_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--open" {
            return args.next().filter(|p| std::path::Path::new(p).is_file());
        }
    }
    None
}

/// Percent encoding for a path inside the URL, as `encodeURIComponent` does it.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Windows 11 (build 22000 on) can put Mica behind a transparent window. Windows 10 cannot,
/// there the window stays solid instead of showing the bare desktop.
fn glass_supported() -> bool {
    #[repr(C)]
    struct OsVersion {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersion) -> i32;
    }
    let mut v = OsVersion { size: std::mem::size_of::<OsVersion>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
    // SAFETY: the struct has the layout of OSVERSIONINFOW and its size is set.
    unsafe { RtlGetVersion(&mut v) == 0 && v.build >= 22000 }
}

pub fn run() {
    let backend = Arc::new(Backend::new());
    let on_exit = backend.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .manage(backend)
        .invoke_handler(tauri::generate_handler![klip, quit])
        .setup(|app| {
            allow_media_dirs(app.handle());
            let selftest = std::env::var("KLIP_SELFTEST").ok();
            // `Klip.exe --open <clip>` comes from the overlay panel: start in the editor.
            let page = match open_arg() {
                Some(clip) => format!("index.html#/editor/{}", encode(&clip)),
                None => "index.html".to_string(),
            };
            let mut window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App(page.into()))
                .title("Klip.")
                .inner_size(1440.0, 900.0)
                .min_inner_size(1100.0, 680.0)
                // Files are dragged out of the window, never into it.
                .disable_drag_drop_handler();
            if glass_supported() {
                window = window
                    .transparent(true)
                    .effects(EffectsBuilder::new().effect(Effect::Mica).state(EffectState::FollowsWindowActiveState).build())
                    .initialization_script("document.documentElement.dataset.glass = '1';");
            }
            if let Some(mode) = selftest.as_deref() {
                // The self test must not take the focus away from a running game.
                window = window.focused(false).visible(mode != "hidden");
            }
            window.build()?;
            if selftest.is_none() {
                // Off the main thread: asking the pipe can take a moment.
                std::thread::spawn(|| {
                    if let Err(e) = backend::recorder_start() {
                        eprintln!("recorder: {e}");
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("could not start Klip");
    app.run(move |_handle, event| {
        if let RunEvent::Exit = event {
            on_exit.kill_all();
        }
    });
}
