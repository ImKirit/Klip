//! Klip recorder. Lives in the tray, watches for games, and runs the recording chain
//! (see `pipeline.rs`) either always or only while a game runs. Everything it does to a
//! game happens from outside: screen capture, audio from the Windows audio engine, and
//! its own registered hotkeys.

// No console window when started with Windows. A console the recorder was started from
// is attached below, so messages still show up there.
#![windows_subsystem = "windows"]

mod audio;
mod camera;
mod capture;
mod composite;
mod encoder;
mod gfx;
mod hotkeys;
mod ipc;
mod mux;
mod pipeline;
mod record;
mod replay;
mod ring;
mod shot;
mod tap;

use klip_core::{BufferMode, Config, HotkeyAction, IpcRequest, IpcResponse, Language, MonitorChoice, RecorderStatus, VideoCodec};
use klip_games::{GameCatalog, GameEvent, GameWatcher, ManualList, RunningGame, WatcherOptions};
use klip_overlay::{Action, ClipEntry, Cue, Overlay, OverlayConfig, OverlayState, PanelHints, SoundConfig, StatusKind, Toast};
use klip_tray::{MenuEntry, Tray, TrayEvent, TrayOptions, TrayState};
use pipeline::{GameInfo, Options, Pipeline, SourcePick};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows::Win32::Media::MediaFoundation::{MFShutdown, MFStartup, MFSTARTUP_FULL, MF_VERSION};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Console::{AttachConsole, SetConsoleCtrlHandler, ATTACH_PARENT_PROCESS};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId, GetProcessTimes};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetMessageW, GetWindowTextW, IsWindowVisible, PostThreadMessageW, MSG, WM_APP, WM_HOTKEY, WM_QUIT,
};

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Posted by the test mode, wParam is an index into its action list.
const WM_TEST_ACTION: u32 = WM_APP + 2;
/// Posted by the pipe server after the app changed the settings file.
const WM_RELOAD_CONFIG: u32 = WM_APP + 3;
/// Something waits in the event queue (tray, game watcher, housekeeping).
const WM_WAKE: u32 = WM_APP + 4;

static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

// Tray menu ids. 0 is reserved by Windows for "nothing chosen".
const MENU_CLIP: u32 = 1;
const MENU_RECORD: u32 = 2;
const MENU_SHOT: u32 = 3;
const MENU_GAME_ONLY: u32 = 4;
const MENU_LIBRARY: u32 = 5;
const MENU_AUTOSTART: u32 = 6;
const MENU_FOLDER: u32 = 7;
const MENU_QUIT: u32 = 9;

enum SourceArg {
    /// Follow the settings: primary monitor, or the monitor of the game.
    Auto,
    Monitor(usize),
    Window(String),
}

struct Args {
    cfg: Config,
    /// Set by `--out`. Without it the clip folder of the settings counts.
    out: Option<PathBuf>,
    source: SourceArg,
    /// Run for this many seconds, walk through every action once and exit.
    test_seconds: Option<u32>,
    /// Length of the clip saved in test mode.
    test_clip: u32,
    list: bool,
    /// Treat this exe as the game in the foreground. For tests without game detection.
    game_exe: Option<String>,
    no_tray: bool,
    /// Feed this video file instead of the screen and exit when it ends. Needs `--game-exe`
    /// to switch a detector on.
    replay: Option<PathBuf>,
}

/// Parses "left,top,width,height".
fn parse_region(text: &str) -> Res<[u32; 4]> {
    let parts: Vec<u32> = text.split(|c| c == ',' || c == 'x').map(|p| p.trim().parse()).collect::<Result<_, _>>()?;
    <[u32; 4]>::try_from(parts).map_err(|_| "a region needs four numbers: left,top,width,height".into())
}

fn parse_args() -> Res<Args> {
    let mut cfg = Config::load()?;
    let mut out = None;
    let mut source = SourceArg::Auto;
    let (mut test_seconds, mut test_clip, mut list, mut no_tray) = (None, 30, false, false);
    let mut game_exe = None;
    let mut replay = None;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--buffer" => cfg.buffer_seconds = value()?.parse()?,
            "--disk" => cfg.buffer_on_disk = true,
            "--always" => cfg.buffer_mode = BufferMode::Always,
            "--game-only" => cfg.buffer_mode = BufferMode::GameOnly,
            "--clip" => test_clip = value()?.parse()?,
            "--fps" => cfg.fps = value()?.parse()?,
            "--bitrate" => cfg.bitrate_kbps = value()?.parse()?,
            "--height" => cfg.height = Some(value()?.parse()?),
            "--cursor" => cfg.capture_cursor = true,
            "--out" => out = Some(PathBuf::from(value()?)),
            "--monitor" => source = SourceArg::Monitor(value()?.parse()?),
            "--window" => source = SourceArg::Window(value()?),
            "--test" => test_seconds = Some(value()?.parse()?),
            "--list" => list = true,
            "--no-audio" => cfg.audio.enabled = false,
            "--no-mic" => cfg.audio.microphone = false,
            "--no-input" => cfg.input.enabled = false,
            "--no-tray" => no_tray = true,
            "--replay" => replay = Some(PathBuf::from(value()?)),
            "--region" => cfg.region = Some(parse_region(&value()?)?),
            "--game-exe" => game_exe = Some(value()?),
            // Passed by the autostart entry. Nothing to do, the tray is the default.
            "--tray" => {}
            "--codec" => {
                cfg.codec = match value()?.as_str() {
                    "h264" => VideoCodec::H264,
                    "hevc" => VideoCodec::Hevc,
                    "av1" => VideoCodec::Av1,
                    other => return Err(format!("unknown codec {other}").into()),
                }
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    if cfg.fps == 0 || cfg.buffer_seconds == 0 {
        return Err("fps and buffer length must be above zero".into());
    }
    Ok(Args { cfg, out, source, test_seconds, test_clip, list, game_exe, no_tray, replay })
}

/// True when standard output goes into `recorder.log` because nobody else reads it.
static LOG_TO_FILE: AtomicBool = AtomicBool::new(false);

/// Started from the tray or by autostart there is no console, and everything the recorder
/// reports would be lost. Then it goes into `recorder.log` in the settings folder. The
/// file starts anew once it passes half a megabyte.
fn log_to_file_without_console() {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            return;
        }
        // Started with its output redirected (a test, a script): leave it there.
        if GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null()) {
            return;
        }
        let dir = klip_core::data_dir();
        let path = dir.join("recorder.log");
        let _ = std::fs::create_dir_all(&dir);
        let fresh = std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(true);
        let file = std::fs::OpenOptions::new().create(true).append(!fresh).write(true).truncate(fresh).open(&path);
        let Ok(file) = file else { return };
        let Ok(second) = file.try_clone() else { return };
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, HANDLE(file.into_raw_handle()));
        let _ = SetStdHandle(STD_ERROR_HANDLE, HANDLE(second.into_raw_handle()));
        LOG_TO_FILE.store(true, Ordering::Relaxed);
    }
    println!("---- klip-recorder {} started {} ----", env!("CARGO_PKG_VERSION"), stamp_now());
}

fn stamp_now() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

fn main() {
    log_to_file_without_console();
    if let Err(e) = run() {
        eprintln!("klip-recorder: {e}");
        std::process::exit(1);
    }
}

/// What reaches the main loop from other threads.
enum Event {
    Tray(TrayEvent),
    Game(GameEvent),
    /// The recorder saved a clip by itself. Path and length in seconds.
    AutoClip(PathBuf, u32),
    /// An action ran somewhere else (pipe): show its notice. The flag says whether a
    /// recording was running before.
    Announce(HotkeyAction, bool),
    /// A button of the overlay panel was clicked.
    Overlay(Action),
    /// The held keys changed, or the game lost the focus (`None`). For the live input overlay.
    Live(Option<(klip_input::OverlayLayout, klip_input::Held)>),
    /// Every five seconds: measurements and the guards of a running recording.
    Tick,
}

/// State shared between the main loop and the pipe server.
struct App {
    pipeline: Mutex<Option<Arc<Pipeline>>>,
    cfg: Mutex<Config>,
    /// Clip folder forced on the command line. Without it the settings decide, read anew
    /// every time, so a folder changed in the app counts at once.
    out: Option<PathBuf>,
    /// Hotkeys Windows refused, for the status the app reads.
    hotkey_problems: Mutex<Vec<String>>,
    /// The automatic upload: the last error, and how many went up since the start.
    cloud_error: Mutex<Option<String>>,
    cloud_uploaded: AtomicU32,
    /// The game that is running right now.
    game: Mutex<Option<GameInfo>>,
    /// The clips saved last, newest first, for the overlay panel.
    recent: Mutex<Vec<(ClipEntry, PathBuf)>>,
    /// Called by the recording chain after it saved a clip on its own.
    notify: pipeline::Notify,
    /// Hands an action that ran on another thread to the main loop for its notice.
    announce: Box<dyn Fn(HotkeyAction, bool) + Send + Sync>,
    replay: Option<PathBuf>,
    /// What the running chain captures, kept to start it again after a settings change.
    source: Mutex<Option<SourcePick>>,
}

impl App {
    /// The monitor the settings ask for when nothing else decides.
    fn default_source(&self) -> SourcePick {
        match self.cfg.lock().unwrap().monitor {
            MonitorChoice::Index { index } if gfx::monitors().is_ok_and(|m| index < m.len()) => SourcePick::Monitor(index),
            MonitorChoice::All => SourcePick::All,
            _ => SourcePick::Primary,
        }
    }

    fn out_dir(&self) -> PathBuf {
        self.out.clone().unwrap_or_else(|| self.cfg.lock().unwrap().clip_dir())
    }

    fn current(&self) -> Option<Arc<Pipeline>> {
        self.pipeline.lock().unwrap().clone()
    }

    fn perform(&self, action: &HotkeyAction) -> Res<()> {
        match self.current() {
            Some(p) => p.perform(action),
            None => Err("the buffer is off, no game is running".into()),
        }
    }

    fn status(&self) -> RecorderStatus {
        let mut status = self.status_of_chain();
        status.hotkey_problems = self.hotkey_problems.lock().unwrap().clone();
        status.cloud_error = self.cloud_error.lock().unwrap().clone();
        status.cloud_uploaded = self.cloud_uploaded.load(Ordering::Relaxed);
        status.cameras = camera::names();
        status.camera = self.current().and_then(|p| p.camera_name());
        status.monitors = gfx::monitors()
            .map(|list| {
                list.iter()
                    .map(|m| format!("{}x{} at {},{}", m.rect.right - m.rect.left, m.rect.bottom - m.rect.top, m.rect.left, m.rect.top))
                    .collect()
            })
            .unwrap_or_default();
        status
    }

    fn status_of_chain(&self) -> RecorderStatus {
        match self.current() {
            Some(p) => p.status(),
            None => RecorderStatus {
                version: env!("CARGO_PKG_VERSION").to_string(),
                buffer_seconds: self.cfg.lock().unwrap().buffer_seconds,
                game: self.game.lock().unwrap().as_ref().map(|g| g.name.clone()),
                ..Default::default()
            },
        }
    }

    fn start_pipeline(&self, source: SourcePick) {
        let mut slot = self.pipeline.lock().unwrap();
        if slot.is_some() {
            return;
        }
        // A game with a profile of its own gets its own quality, clip length and hotkeys.
        // The profile is applied when the chain starts, so in the mode "always" it only
        // takes effect for a chain that starts while the game runs.
        let base = self.cfg.lock().unwrap().clone();
        let game = self.game.lock().unwrap().clone();
        let cfg = match &game {
            Some(g) => {
                let store: klip_games::ProfileStore =
                    read_json(&klip_core::data_dir().join("profiles.json")).unwrap_or_default();
                if store.get(&g.id).is_some() {
                    println!("profile: settings of {} applied", g.name);
                }
                store.resolve(&base, Some(&g.id)).config
            }
            None => base,
        };
        *self.source.lock().unwrap() = Some(source);
        let opts = Options {
            cfg,
            out_dir: self.out_dir(),
            source,
            notify: Some(self.notify.clone()),
            replay: self.replay.clone(),
        };
        match Pipeline::start(opts) {
            Ok(p) => *slot = Some(p),
            Err(e) => eprintln!("klip-recorder: the buffer could not start: {e}"),
        }
    }

    fn stop_pipeline(&self) {
        if let Some(p) = self.pipeline.lock().unwrap().take() {
            p.stop();
            println!("buffer: stopped");
        }
    }
}

fn run() -> Res<()> {
    let args = parse_args()?;
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    }
    if args.list {
        return list_sources();
    }
    let main_thread = unsafe { GetCurrentThreadId() };
    MAIN_THREAD.store(main_thread, Ordering::Release);
    let testing = args.test_seconds.is_some() || args.replay.is_some();

    // Two recorders would fight over hotkeys, the pipe and the encoder.
    let _instance = if testing {
        None
    } else {
        match klip_tray::SingleInstance::acquire_recorder() {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => return Err("Klip is already running".into()),
            Err(e) => return Err(format!("could not check for a running Klip: {e}").into()),
        }
    };

    let (tx, rx) = channel::<Event>();
    let wake = move |tx: &Sender<Event>, e: Event| {
        let _ = tx.send(e);
        unsafe {
            let _ = PostThreadMessageW(main_thread, WM_WAKE, WPARAM(0), LPARAM(0));
        }
    };
    let notify_tx = Mutex::new(tx.clone());
    let announce_tx = Mutex::new(tx.clone());
    let app = Arc::new(App {
        notify: Arc::new(move |path, seconds| wake(&notify_tx.lock().unwrap(), Event::AutoClip(path, seconds))),
        announce: Box::new(move |action, was| wake(&announce_tx.lock().unwrap(), Event::Announce(action, was))),
        pipeline: Mutex::new(None),
        cfg: Mutex::new(args.cfg.clone()),
        out: args.out.clone(),
        hotkey_problems: Mutex::new(Vec::new()),
        cloud_error: Mutex::new(None),
        cloud_uploaded: AtomicU32::new(0),
        game: Mutex::new(None),
        recent: Mutex::new(recent_from_disk(&args.out.clone().unwrap_or_else(|| args.cfg.clip_dir()))),
        source: Mutex::new(None),
        replay: args.replay.clone(),
    });
    let fixed_source = |source: &SourceArg| -> Res<Option<SourcePick>> {
        Ok(match source {
            SourceArg::Auto => None,
            SourceArg::Monitor(i) => Some(SourcePick::Monitor(*i)),
            SourceArg::Window(title) => {
                let (hwnd, full) = find_window(title).ok_or(format!("no visible window with '{title}' in its title"))?;
                println!("window: {full}");
                Some(SourcePick::Window(hwnd.0 as isize))
            }
        })
    };

    unsafe { SetConsoleCtrlHandler(Some(on_console_close), true)? };

    // The test mode walks through every action once, on the same code path as the hotkeys.
    let test_actions = [
        HotkeyAction::Screenshot,
        HotkeyAction::RecordToggle,
        HotkeyAction::Marker,
        HotkeyAction::RecordPause,
        HotkeyAction::RecordPause,
        HotkeyAction::Marker,
        HotkeyAction::RecordToggle,
        HotkeyAction::Clip { seconds: args.test_clip },
    ];

    let mut tray: Option<Tray> = None;
    let mut overlay: Option<Overlay> = None;
    let mut actions: Vec<HotkeyAction> = Vec::new();

    if args.replay.is_some() {
        // Replay: the file is the game. No scripted actions, only what the chain does by
        // itself. Ends a while after the file, so the last auto clip can still be written.
        app.start_pipeline(SourcePick::Primary);
        let Some(p) = app.current() else { return Err("the buffer did not start".into()) };
        if let Some(exe) = &args.game_exe {
            p.set_game(Some(GameInfo { id: "test".into(), name: "Replay".into(), exe_name: exe.clone() }), true);
        }
        std::thread::spawn(move || {
            while !p.replay_finished() {
                std::thread::sleep(Duration::from_millis(500));
            }
            p.flush_auto_clips();
            std::thread::sleep(Duration::from_secs(3));
            unsafe {
                let _ = PostThreadMessageW(main_thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        });
    } else if let Some(seconds) = args.test_seconds {
        // Tests start the chain right away and know nothing of tray or game detection.
        app.start_pipeline(fixed_source(&args.source)?.unwrap_or_else(|| app.default_source()));
        let Some(p) = app.current() else { return Err("the buffer did not start".into()) };
        if let Some(exe) = &args.game_exe {
            p.set_game(Some(GameInfo { id: "test".into(), name: "Test".into(), exe_name: exe.clone() }), true);
        }
        std::thread::spawn(move || {
            let post = |msg: u32, w: usize| unsafe {
                let _ = PostThreadMessageW(main_thread, msg, WPARAM(w), LPARAM(0));
            };
            let total = seconds.max(12) as u64 * 1000;
            // Screenshot, start, marker, pause, resume, marker, stop, clip.
            let at = [2000, 3000, 5000, 6000, 8000, 9500, total - 1500, total - 500];
            let started = Instant::now();
            for (i, t) in at.iter().enumerate() {
                std::thread::sleep(Duration::from_millis(*t).saturating_sub(started.elapsed()));
                post(WM_TEST_ACTION, i);
                if i == 1 {
                    // A made up event for the auto clip path, see `inject_test_event`.
                    post(WM_TEST_ACTION, 100);
                }
            }
            std::thread::sleep(Duration::from_millis(total).saturating_sub(started.elapsed()));
            post(WM_QUIT, 0);
        });
    } else {
        let always = args.cfg.buffer_mode == BufferMode::Always;
        if let Some(source) = fixed_source(&args.source)? {
            app.start_pipeline(source);
        } else if always {
            app.start_pipeline(app.default_source());
        } else {
            println!("buffer: waiting for a game");
        }

        if !args.no_tray {
            match Tray::spawn(TrayOptions { tooltip: "Klip.".into(), ..Default::default() }) {
                Ok((t, events)) => {
                    let tx = tx.clone();
                    std::thread::spawn(move || events.iter().for_each(|e| wake(&tx, Event::Tray(e))));
                    tray = Some(t);
                }
                Err(e) => eprintln!("klip-recorder: no tray icon: {e}"),
            }
        }

        // The overlay is a window of this process on top of the game. Nothing is loaded
        // into the game. While nothing is to be shown, no window exists at all.
        if args.cfg.overlay.enabled {
            let o = &args.cfg.overlay;
            let mut state = OverlayState::default();
            state.language = if args.cfg.language == Language::De { klip_overlay::Lang::De } else { klip_overlay::Lang::En };
            state.accent = parse_accent(&args.cfg.accent).unwrap_or(state.accent);
            state.panel.position = o.panel_pos.map(|[x, y]| (x, y));
            state.indicator.visible = o.indicator;
            state.toast.visible = o.toasts;
            let fonts = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| klip_overlay::font_files_in(&d.join("fonts"))))
                .unwrap_or_default();
            let config = OverlayConfig {
                state,
                font_files: fonts,
                sound: SoundConfig { enabled: o.sound, volume: o.sound_volume },
                exclude_from_capture: true,
            };
            // The user's own sounds, if there are any, replace the built in ones.
            klip_overlay::set_sound_dir(Some(klip_core::data_dir().join("sounds")));
            match Overlay::start(config) {
                Ok((ov, events)) => {
                    let tx = tx.clone();
                    std::thread::spawn(move || events.iter().for_each(|e| wake(&tx, Event::Overlay(e))));
                    overlay = Some(ov);
                }
                Err(e) => eprintln!("klip-recorder: no overlay: {e}"),
            }
        }

        // Live input overlay: look at the held keys thirty times a second while a game
        // has the focus, and pass on changes only. Asleep otherwise.
        // The keys are only shown in the clip, never over the game (`Config::migrated`).
        if args.cfg.input.live && overlay.is_some() {
            let tx = tx.clone();
            let app = app.clone();
            std::thread::spawn(move || {
                let mut last = None;
                loop {
                    let now = app.current().and_then(|p| p.live_input());
                    let held = now.as_ref().map(|(_, h)| h.clone());
                    if held != last {
                        last = held;
                        wake(&tx, Event::Live(now.clone()));
                    }
                    std::thread::sleep(Duration::from_millis(if now.is_some() { 33 } else { 500 }));
                }
            });
        }

        // Game detection. The library scan reads thousands of small files and takes up to
        // half a minute on a cold disk, so it runs beside everything else. After that the
        // watcher only takes a cheap look at the foreground window every two seconds.
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let manual: ManualList = read_json(&klip_core::data_dir().join("games.json")).unwrap_or_default();
                let (catalog, problems) = GameCatalog::scan(manual);
                println!("games: {} found in the installed libraries", catalog.games().len());
                problems.iter().for_each(|p| println!("games: {p}"));
                let (_watcher, events) = GameWatcher::start(catalog, WatcherOptions::default());
                events.iter().for_each(|e| wake(&tx, Event::Game(e)));
            });
        }

        let (registered, problems) = hotkeys::register(&args.cfg.hotkeys);
        actions = registered;
        for b in args.cfg.hotkeys.iter() {
            println!("hotkey: {} = {:?}", b.keys, b.action);
        }
        problems.iter().for_each(|p| println!("hotkey problem: {p}"));
        warn_hotkeys(&app, overlay.as_ref(), problems);
        start_cloud_uploads(app.clone());

        // The app talks to the recorder through a pipe. Actions run right on the pipe
        // thread, only things bound to the main thread (hotkeys, leaving) are posted over.
        let pipe_app = app.clone();
        let served = ipc::serve(move |request| match request {
            IpcRequest::Status => IpcResponse { ok: true, error: None, status: Some(pipe_app.status()) },
            IpcRequest::Action { action } => {
                let was_recording = pipe_app.current().is_some_and(|p| p.is_recording());
                match pipe_app.perform(&action) {
                    Ok(()) => {
                        (pipe_app.announce)(action.clone(), was_recording);
                        IpcResponse { ok: true, error: None, status: Some(pipe_app.status()) }
                    }
                    Err(e) => IpcResponse { ok: false, error: Some(e.to_string()), status: None },
                }
            }
            IpcRequest::ReloadConfig | IpcRequest::Quit => {
                let msg = if matches!(request, IpcRequest::Quit) { WM_QUIT } else { WM_RELOAD_CONFIG };
                unsafe {
                    let _ = PostThreadMessageW(main_thread, msg, WPARAM(0), LPARAM(0));
                }
                IpcResponse { ok: true, error: None, status: None }
            }
        });
        if let Err(e) = served {
            eprintln!("klip-recorder: no connection for the app: {e}");
        }
    }

    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            wake(&tx, Event::Tick);
        });
    }

    let mut meter = CpuMeter::new();
    refresh_tray(&app, tray.as_ref());
                refresh_overlay(&app, overlay.as_ref());

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let action = match msg.message {
                WM_HOTKEY => actions.get((msg.wParam.0 as usize).wrapping_sub(1)).cloned(),
                WM_TEST_ACTION if msg.wParam.0 == 100 => {
                    if let Some(p) = app.current() {
                        p.inject_test_event();
                    }
                    None
                }
                WM_TEST_ACTION => test_actions.get(msg.wParam.0).cloned(),
                WM_RELOAD_CONFIG => {
                    match Config::load() {
                        Ok(new) => {
                            for id in 1..=actions.len() as i32 {
                                let _ = UnregisterHotKey(None, id);
                            }
                            let (registered, problems) = hotkeys::register(&new.hotkeys);
                            actions = registered;
                            println!("config: reloaded, {} hotkeys", actions.len());
                            problems.iter().for_each(|p| println!("hotkey problem: {p}"));
                            warn_hotkeys(&app, overlay.as_ref(), problems);
                            // Hotkeys change in place. Anything else needs a fresh chain,
                            // which costs the buffer, so it waits for a running recording.
                            let rest = |c: &Config| {
                                let mut c = c.clone();
                                c.hotkeys.clear();
                                serde_json::to_string(&c).unwrap_or_default()
                            };
                            let changed = rest(&app.cfg.lock().unwrap()) != rest(&new);
                            *app.cfg.lock().unwrap() = new;
                            if changed {
                                match app.current() {
                                    Some(p) if p.is_recording() => {
                                        println!("config: new settings apply after the recording")
                                    }
                                    Some(_) => {
                                        // A source forced on the command line stays, else the
                                        // settings decide, they may just have changed.
                                        let source = fixed_source(&args.source).ok().flatten().unwrap_or_else(|| app.default_source());
                                        app.stop_pipeline();
                                        app.start_pipeline(source);
                                        println!("config: buffer restarted with the new settings");
                                    }
                                    None => {}
                                }
                            }
                            apply_buffer_mode(&app);
                            refresh_tray(&app, tray.as_ref());
                refresh_overlay(&app, overlay.as_ref());
                        }
                        Err(e) => eprintln!("klip-recorder: settings not reloaded: {e}"),
                    }
                    None
                }
                WM_WAKE => {
                    let mut chosen = None;
                    while let Ok(event) = rx.try_recv() {
                        match event {
                            Event::Tick => {
                                if let Some(p) = app.current() {
                                    // The numbers are for a console. In the log file they
                                    // would only bury the lines that matter.
                                    if !LOG_TO_FILE.load(Ordering::Relaxed) {
                                        println!("{}", p.stat_line(&mut meter));
                                    }
                                    if let Some(reason) = p.recording_must_stop() {
                                        eprintln!("klip-recorder: stopping the recording: {reason}");
                                        chosen = Some(HotkeyAction::RecordToggle);
                                    }
                                }
                            }
                            Event::Game(e) => on_game_event(&app, e),
                            Event::Announce(action, was_recording) => {
                                announce(&app, overlay.as_ref(), &action, was_recording)
                            }
                            Event::AutoClip(path, seconds) => {
                                remember(&app, path, seconds);
                                // Only "clip saved" is shown. What was detected stays out of
                                // sight during a match, see the anti-cheat rules in the vault.
                                if let Some(o) = &overlay {
                                    let de = app.cfg.lock().unwrap().language == Language::De;
                                    o.toast(if de {
                                        Toast::new("Automatischer Clip gespeichert", &format!("{seconds} s")).with_cue(Cue::AutoClip)
                                    } else {
                                        Toast::new("Automatic clip saved", &format!("{seconds} s")).with_cue(Cue::AutoClip)
                                    });
                                }
                            }
                            Event::Live(live) => {
                                if let Some(o) = &overlay {
                                    o.update(|s| match live {
                                        Some((layout, held)) => {
                                            s.input.visible = true;
                                            s.input.layout = layout;
                                            s.input.held = held;
                                        }
                                        None => s.input.visible = false,
                                    });
                                }
                            }
                            Event::Overlay(a) => match a {
                                Action::SaveClip => {
                                    let seconds = default_clip_seconds(&app.cfg.lock().unwrap());
                                    chosen = Some(HotkeyAction::Clip { seconds });
                                }
                                Action::ToggleRecording => chosen = Some(HotkeyAction::RecordToggle),
                                Action::Screenshot => chosen = Some(HotkeyAction::Screenshot),
                                Action::ToggleMic => {
                                    if let Some(p) = app.current() {
                                        println!("mic: {}", if p.toggle_mic() { "muted" } else { "on" });
                                    }
                                }
                                Action::OpenClip(i) => {
                                    let path = app.recent.lock().unwrap().get(i).map(|(_, p)| p.clone());
                                    if let Some(path) = path {
                                        open_in_app(&app, &path);
                                    }
                                }
                                Action::OpenLibrary | Action::OpenSettings => open_library(&app),
                                Action::ClosePanel => {}
                                Action::PanelMoved { x, y } => {
                                    // Remember where the user put the panel.
                                    let mut cfg = app.cfg.lock().unwrap();
                                    cfg.overlay.panel_pos = Some([x, y]);
                                    if let Err(e) = cfg.save() {
                                        eprintln!("klip-recorder: panel position not saved: {e}");
                                    }
                                }
                            },
                            Event::Tray(TrayEvent::Activated) => open_library(&app),
                            Event::Tray(TrayEvent::Menu(id)) => match id {
                                MENU_CLIP => {
                                    let seconds = default_clip_seconds(&app.cfg.lock().unwrap());
                                    chosen = Some(HotkeyAction::Clip { seconds });
                                }
                                MENU_RECORD => chosen = Some(HotkeyAction::RecordToggle),
                                MENU_SHOT => chosen = Some(HotkeyAction::Screenshot),
                                MENU_GAME_ONLY => {
                                    {
                                        let mut cfg = app.cfg.lock().unwrap();
                                        cfg.buffer_mode = match cfg.buffer_mode {
                                            BufferMode::Always => BufferMode::GameOnly,
                                            BufferMode::GameOnly => BufferMode::Always,
                                        };
                                        if let Err(e) = cfg.save() {
                                            eprintln!("klip-recorder: settings not saved: {e}");
                                        }
                                    }
                                    apply_buffer_mode(&app);
                                }
                                MENU_LIBRARY => open_library(&app),
                                MENU_FOLDER => open_folder(&app.out_dir()),
                                MENU_AUTOSTART => toggle_autostart(),
                                MENU_QUIT => {
                                    let _ = PostThreadMessageW(main_thread, WM_QUIT, WPARAM(0), LPARAM(0));
                                }
                                _ => {}
                            },
                        }
                    }
                    refresh_tray(&app, tray.as_ref());
                refresh_overlay(&app, overlay.as_ref());
                    chosen
                }
                _ => None,
            };
            if let Some(action) = action {
                if action == HotkeyAction::Overlay {
                    if let Some(o) = &overlay {
                        o.toggle_panel();
                    }
                } else {
                    let was_recording = app.current().is_some_and(|p| p.is_recording());
                    match app.perform(&action) {
                        Ok(()) => announce(&app, overlay.as_ref(), &action, was_recording),
                        Err(e) => {
                            eprintln!("klip-recorder: {action:?} failed: {e}");
                            announce_failure(&app, overlay.as_ref(), &action, &e.to_string());
                        }
                    }
                }
                refresh_tray(&app, tray.as_ref());
                refresh_overlay(&app, overlay.as_ref());
            }
        }
    }

    // Leaving: close a running recording properly and remove the buffer files.
    app.stop_pipeline();
    drop(overlay);
    if let Some(t) = tray {
        t.close();
    }
    unsafe {
        let _ = MFShutdown();
    }
    Ok(())
}

/// Starts or stops the chain so that it matches the buffer mode and the running game.
fn apply_buffer_mode(app: &App) {
    let always = app.cfg.lock().unwrap().buffer_mode == BufferMode::Always;
    let game = app.game.lock().unwrap().clone();
    match (always, game.is_some(), app.current().is_some()) {
        (true, _, false) => app.start_pipeline(app.default_source()),
        (false, false, true) => app.stop_pipeline(),
        _ => {}
    }
    if let (Some(p), Some(g)) = (app.current(), game) {
        p.set_game(Some(g), true);
    }
}

fn on_game_event(app: &App, event: GameEvent) {
    let info = |g: &RunningGame| GameInfo { id: g.id.clone(), name: g.name.clone(), exe_name: g.exe_name.clone() };
    match event {
        GameEvent::Started(g) => {
            println!(
                "game: {} started ({}){}",
                g.name,
                g.exe_name,
                if g.protection.is_protected() { ", protected by anti-cheat" } else { "" }
            );
            *app.game.lock().unwrap() = Some(info(&g));
        }
        GameEvent::Foreground(g) => {
            *app.game.lock().unwrap() = Some(info(&g));
            if app.current().is_none() {
                // By default the main monitor. With the choice "monitor of the game" the
                // handle of the foreground window is used, only to ask Windows which
                // monitor that is.
                let follow = app.cfg.lock().unwrap().monitor == MonitorChoice::Game;
                let source = match klip_games::foreground::foreground_ids() {
                    Some((hwnd, pid)) if follow && pid == g.pid => {
                        let monitor = unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST) };
                        SourcePick::MonitorHandle(monitor.0 as isize)
                    }
                    _ => app.default_source(),
                };
                app.start_pipeline(source);
            }
            if let Some(p) = app.current() {
                p.set_game(Some(info(&g)), true);
            }
        }
        GameEvent::Background(g) => {
            if let Some(p) = app.current() {
                p.set_game(Some(info(&g)), false);
            }
        }
        GameEvent::Stopped(g) => {
            println!("game: {} stopped", g.name);
            *app.game.lock().unwrap() = None;
            if let Some(p) = app.current() {
                p.set_game(None, false);
            }
            if app.cfg.lock().unwrap().buffer_mode == BufferMode::GameOnly {
                app.stop_pipeline();
            }
        }
        GameEvent::UnknownFullscreen(f) => {
            println!("game: unknown fullscreen program {}, add it in the app if it is a game", f.exe_name.as_deref().unwrap_or("?"));
        }
    }
}

/// Remembers which hotkeys Windows refused and says so on screen. A clip key that does
/// nothing without a word was the first thing the owner ran into.
fn warn_hotkeys(app: &App, overlay: Option<&Overlay>, problems: Vec<String>) {
    let de = app.cfg.lock().unwrap().language == Language::De;
    if let Some(o) = overlay {
        for p in problems.iter().take(3) {
            // The sentence starts with the key in quotes: 'Alt+S' is already taken ...
            let key = p.split('\'').nth(1).unwrap_or("");
            let title = if de { format!("Taste {key} geht nicht") } else { format!("Key {key} does not work") };
            let detail = if p.contains("already taken") {
                if de { "Ein anderes Programm belegt sie" } else { "Another program holds it" }
            } else if de {
                "Siehe Einstellungen, Tasten"
            } else {
                "See settings, hotkeys"
            };
            o.toast(Toast { hold: Some(Duration::from_secs(8)), ..Toast::recording(&title, detail).with_cue(Cue::Tick) });
        }
    }
    *app.hotkey_problems.lock().unwrap() = problems;
}

/// Says on screen why an action did nothing.
fn announce_failure(app: &App, overlay: Option<&Overlay>, action: &HotkeyAction, error: &str) {
    let Some(o) = overlay else { return };
    let de = app.cfg.lock().unwrap().language == Language::De;
    let what = match (action, de) {
        (HotkeyAction::Clip { .. }, true) => "Kein Clip gespeichert",
        (HotkeyAction::Clip { .. }, false) => "No clip saved",
        (HotkeyAction::Screenshot, true) => "Kein Screenshot",
        (HotkeyAction::Screenshot, false) => "No screenshot",
        (_, true) => "Das ging nicht",
        (_, false) => "That did not work",
    };
    let why = if error.contains("buffer is off") {
        if de { "Puffer aus: kein Spiel erkannt" } else { "Buffer off: no game detected" }.to_string()
    } else {
        error.to_string()
    };
    o.toast(Toast { hold: Some(Duration::from_secs(5)), ..Toast::recording(what, &why).with_cue(Cue::Tick) });
}

/// Notice and tone after an action, and the list of recent clips for the panel.
fn announce(app: &App, overlay: Option<&Overlay>, action: &HotkeyAction, was_recording: bool) {
    let de = app.cfg.lock().unwrap().language == Language::De;
    let toast = match action {
        HotkeyAction::Clip { seconds } => {
            if let Some(path) = app.current().and_then(|p| p.status().last_saved) {
                remember(app, PathBuf::from(path), *seconds);
            }
            if de {
                Toast::new("Clip gespeichert", &format!("Letzte {seconds} s"))
            } else {
                Toast::new("Clip saved", &format!("Last {seconds} s"))
            }
        }
        HotkeyAction::RecordToggle if was_recording => {
            Toast::new(if de { "Aufnahme gespeichert" } else { "Recording saved" }, "").with_cue(Cue::RecordStop)
        }
        HotkeyAction::RecordToggle => {
            Toast::recording(if de { "Aufnahme gestartet" } else { "Recording started" }, "")
        }
        HotkeyAction::RecordPause => {
            Toast::recording(if de { "Aufnahme: Pause umgeschaltet" } else { "Recording: pause toggled" }, "").with_cue(Cue::Tick)
        }
        HotkeyAction::Marker => Toast::new(if de { "Marker gesetzt" } else { "Marker set" }, "").with_cue(Cue::Tick),
        HotkeyAction::Screenshot => Toast::new(if de { "Screenshot gespeichert" } else { "Screenshot saved" }, "").with_cue(Cue::Screenshot),
        HotkeyAction::InputPause => {
            Toast::new(if de { "Eingabe-Erfassung umgeschaltet" } else { "Input capture toggled" }, "").with_cue(Cue::Tick)
        }
        _ => return,
    };
    if let Some(o) = overlay {
        o.toast(toast);
    }
}

/// Puts a saved clip at the top of the list the overlay panel shows.
fn remember(app: &App, path: PathBuf, seconds: u32) {
    let t = unsafe { GetLocalTime() };
    let entry = ClipEntry {
        title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        time: format!("{:02}:{:02}", t.wHour, t.wMinute),
        duration_secs: seconds,
    };
    let mut recent = app.recent.lock().unwrap();
    recent.insert(0, (entry, path));
    recent.truncate(5);
}

/// The newest clips of the clip folder, so the panel shows them right after a start and
/// not only those saved since. Camera files and exports are no clips of their own.
fn recent_from_disk(dir: &Path) -> Vec<(ClipEntry, PathBuf)> {
    fn walk(dir: &Path, depth: u32, found: &mut Vec<(std::time::SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 3 && !name.starts_with('.') && name != "exporte" {
                    walk(&path, depth + 1, found);
                }
            } else if name.ends_with(".mp4") && !name.ends_with(".cam.mp4") {
                if let Ok(modified) = meta.modified() {
                    found.push((modified, path));
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, 0, &mut found);
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
        .into_iter()
        .take(5)
        .map(|(modified, path)| {
            let seconds = read_json::<klip_core::ClipMeta>(&klip_core::ClipMeta::sidecar_path(&path))
                .map(|m| (m.duration_ms / 1000) as u32)
                .unwrap_or(0);
            let time = {
                // The time of day in local time, from the file's clock.
                let ft = modified.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                local_hh_mm(ft)
            };
            let entry = ClipEntry {
                title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                time,
                duration_secs: seconds,
            };
            (entry, path)
        })
        .collect()
}

/// "HH:MM" of a Unix time in the local time zone of Windows.
fn local_hh_mm(unix_secs: u64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = (unix_secs + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            return String::new();
        }
    }
    format!("{:02}:{:02}", local.wHour, local.wMinute)
}

/// "#RRGGBB" as a colour of the overlay.
fn parse_accent(text: &str) -> Option<klip_overlay::Rgb> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(klip_overlay::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Keys of the first hotkey bound to an action, shown next to the panel buttons.
fn hint(cfg: &Config, wanted: impl Fn(&HotkeyAction) -> bool) -> String {
    cfg.hotkeys.iter().find(|b| wanted(&b.action)).map(|b| b.keys.clone()).unwrap_or_default()
}

/// The overlay shows the same state as the tray.
fn refresh_overlay(app: &App, overlay: Option<&Overlay>) {
    let Some(overlay) = overlay else { return };
    let pipeline = app.current();
    let status = match &pipeline {
        Some(p) => {
            let s = p.status();
            if s.recording {
                StatusKind::Recording { since: Instant::now() - Duration::from_millis(s.recording_ms) }
            } else {
                StatusKind::Buffering { seconds: s.buffer_seconds }
            }
        }
        None => StatusKind::Off,
    };
    let muted = pipeline.as_ref().is_some_and(|p| p.mic_muted());
    let clips: Vec<ClipEntry> = app.recent.lock().unwrap().iter().map(|(e, _)| e.clone()).collect();
    let cfg = app.cfg.lock().unwrap();
    let hints = PanelHints {
        save_clip: hint(&cfg, |a| matches!(a, HotkeyAction::Clip { .. })),
        record: hint(&cfg, |a| *a == HotkeyAction::RecordToggle),
        screenshot: hint(&cfg, |a| *a == HotkeyAction::Screenshot),
        mic: String::new(),
        toggle: hint(&cfg, |a| *a == HotkeyAction::Overlay),
    };
    overlay.update(|s| {
        // Comparing first keeps the running clock of a recording from restarting.
        let same_kind = std::mem::discriminant(&s.status) == std::mem::discriminant(&status);
        if !same_kind || !matches!(status, StatusKind::Recording { .. }) {
            s.status = status;
        }
        s.mic_muted = muted;
        s.panel.clips = clips;
        s.panel.hints = hints;
    });
}

/// Icon, tooltip and menu follow the state of the recorder.
fn refresh_tray(app: &App, tray: Option<&Tray>) {
    let Some(tray) = tray else { return };
    let pipeline = app.current();
    let recording = pipeline.as_ref().is_some_and(|p| p.is_recording());
    let game = app.game.lock().unwrap().as_ref().map(|g| g.name.clone());
    let cfg = app.cfg.lock().unwrap();
    let de = cfg.language == Language::De;
    let (state, text) = match (&pipeline, recording) {
        (Some(_), true) => (TrayState::Recording, if de { "Aufnahme läuft" } else { "Recording" }.to_string()),
        (Some(_), false) if de => (TrayState::Buffering, format!("Puffer aktiv, {} s", cfg.buffer_seconds)),
        (Some(_), false) => (TrayState::Buffering, format!("Buffer active, {} s", cfg.buffer_seconds)),
        (None, _) => (TrayState::Idle, if de { "Wartet auf ein Spiel" } else { "Waiting for a game" }.to_string()),
    };
    tray.set_state(state);
    tray.set_tooltip(&match &game {
        Some(g) => format!("Klip. {text} ({g})"),
        None => format!("Klip. {text}"),
    });
    let on = pipeline.is_some();
    let _ = tray.set_menu(vec![
        MenuEntry::item(
            MENU_CLIP,
            format!("{} ({} s)", if de { "Clip speichern" } else { "Save clip" }, default_clip_seconds(&cfg)),
        )
        .enabled(on),
        MenuEntry::item(
            MENU_RECORD,
            match (recording, de) {
                (true, true) => "Aufnahme stoppen",
                (true, false) => "Stop recording",
                (false, true) => "Aufnahme starten",
                (false, false) => "Start recording",
            },
        )
        .enabled(on),
        MenuEntry::item(MENU_SHOT, "Screenshot").enabled(on),
        MenuEntry::Separator,
        MenuEntry::item(MENU_GAME_ONLY, if de { "Nur im Spiel puffern" } else { "Buffer only in games" }).checked(cfg.buffer_mode == BufferMode::GameOnly),
        MenuEntry::item(MENU_AUTOSTART, if de { "Mit Windows starten" } else { "Start with Windows" }).checked(klip_tray::autostart::is_enabled()),
        MenuEntry::Separator,
        MenuEntry::item(MENU_LIBRARY, if de { "Bibliothek öffnen" } else { "Open library" }),
        MenuEntry::item(MENU_FOLDER, if de { "Clip-Ordner öffnen" } else { "Open clip folder" }),
        MenuEntry::Separator,
        MenuEntry::item(MENU_QUIT, if de { "Beenden" } else { "Quit" }),
    ]);
}

/// Length of the first clip hotkey, the "standard clip".
fn default_clip_seconds(cfg: &Config) -> u32 {
    cfg.hotkeys
        .iter()
        .find_map(|b| match b.action {
            HotkeyAction::Clip { seconds } => Some(seconds),
            _ => None,
        })
        .unwrap_or(30)
}

fn toggle_autostart() {
    let result = if klip_tray::autostart::is_enabled() {
        klip_tray::autostart::disable()
    } else {
        std::env::current_exe().and_then(|exe| klip_tray::autostart::enable_with_args(&exe, "--tray"))
    };
    if let Err(e) = result {
        eprintln!("klip-recorder: autostart not changed: {e}");
    }
}

/// Starts the app that sits next to the recorder. Falls back to the clip folder.
fn open_library(app: &App) {
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("Klip.exe")));
    match exe {
        Some(path) if path.exists() => {
            if let Err(e) = std::process::Command::new(&path).spawn() {
                eprintln!("klip-recorder: could not start the app: {e}");
            }
        }
        _ => open_folder(&app.out_dir()),
    }
}

/// Klip Cloud in the mode "all new clips": every few seconds the clip folder is checked
/// for finished clips that are newer than the moment the mode was switched on and not in
/// the cloud yet. A clip counts as finished once its sidecar exists, a running recording
/// has none. Failures wait a minute, the next round tries again, nothing is lost.
fn start_cloud_uploads(app: Arc<App>) {
    let started = std::time::SystemTime::now();
    let _ = std::thread::Builder::new().name("klip-cloud".into()).spawn(move || loop {
        std::thread::sleep(Duration::from_secs(15));
        let cfg = app.cfg.lock().unwrap().cloud.clone();
        if cfg.mode != klip_core::CloudMode::All || klip_cloud::account().is_none() {
            continue;
        }
        let since = cfg
            .since
            .as_deref()
            .and_then(parse_rfc3339)
            .unwrap_or(started);
        for video in cloud_candidates(&app.out_dir(), since) {
            match klip_cloud::upload(&video, &cfg) {
                Ok(done) => {
                    println!("cloud: {} uploaded as {}", video.display(), done.id);
                    app.cloud_uploaded.fetch_add(1, Ordering::Relaxed);
                    *app.cloud_error.lock().unwrap() = None;
                }
                Err(e) => {
                    eprintln!("klip-recorder: cloud upload of {} failed: {e}", video.display());
                    *app.cloud_error.lock().unwrap() = Some(e);
                    std::thread::sleep(Duration::from_secs(60));
                    break;
                }
            }
        }
    });
}

/// Finished videos in the clip folder, saved after `since`, not in the cloud yet. Oldest
/// first, so the order in the cloud matches the order of saving.
fn cloud_candidates(dir: &Path, since: std::time::SystemTime) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, since: std::time::SystemTime, out: &mut Vec<(std::time::SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 3 && !name.starts_with('.') && name != "exporte" {
                    walk(&path, depth + 1, since, out);
                }
                continue;
            }
            if !name.ends_with(".mp4") || name.ends_with(".cam.mp4") {
                continue;
            }
            let Ok(modified) = meta.modified() else { continue };
            if modified < since {
                continue;
            }
            let Some(sidecar) = read_json::<klip_core::ClipMeta>(&klip_core::ClipMeta::sidecar_path(&path)) else { continue };
            if sidecar.cloud.is_none() {
                out.push((modified, path));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, 0, since, &mut found);
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// "2026-10-15T20:00:00+02:00" or with "Z" as a point in time. Seconds are enough.
fn parse_rfc3339(text: &str) -> Option<std::time::SystemTime> {
    let b = text.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let rest = &text[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = if rest.is_empty() || rest.starts_with('Z') {
        0
    } else {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let oh: i64 = rest.get(1..3)?.parse().ok()?;
        let om: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (oh * 3600 + om * 60)
    };
    // Days since 1970 of a calendar date (Howard Hinnant's algorithm).
    let (yy, mm) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + s - offset;
    (secs >= 0).then(|| std::time::UNIX_EPOCH + Duration::from_secs(secs as u64))
}

/// Opens a clip in the editor of the app. Without the app next to the recorder the clip
/// opens in the player Windows has for it.
fn open_in_app(app: &App, clip: &Path) {
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("Klip.exe")));
    match exe {
        Some(exe) if exe.exists() => {
            if let Err(e) = std::process::Command::new(&exe).arg("--open").arg(clip).spawn() {
                eprintln!("klip-recorder: could not start the app: {e}");
            }
        }
        _ => {
            let _ = std::process::Command::new("explorer.exe").arg(clip).spawn();
        }
    }
    let _ = app;
}

fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Local time as text, for file names (`'_'`, `'-'`) and for metadata (`'T'`, `':'`).
pub(crate) fn stamp(date_time: char, time_sep: char) -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}{date_time}{:02}{time_sep}{:02}{time_sep}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// A file name in `dir` that does not exist yet.
pub(crate) fn new_path(dir: &Path, suffix: &str, ext: &str) -> Res<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let base = format!("Klip_{}{suffix}", stamp('_', '-'));
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}_{n}.{ext}"));
        n += 1;
    }
    Ok(path)
}

pub(crate) fn free_bytes(dir: &Path) -> Option<u64> {
    let wide: Vec<u16> = dir.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None).ok()? };
    Some(free)
}

/// Ctrl+C or a closed console window: ask the main loop to leave so files get closed.
unsafe extern "system" fn on_console_close(_kind: u32) -> BOOL {
    let _ = PostThreadMessageW(MAIN_THREAD.load(Ordering::Acquire), WM_QUIT, WPARAM(0), LPARAM(0));
    // Give the main thread time to finish a recording before Windows ends the process.
    std::thread::sleep(Duration::from_secs(3));
    true.into()
}

/// Visible top level windows with a title. Only titles are read, nothing is sent to or
/// changed in the other program.
fn windows_with_titles() -> Vec<(HWND, String)> {
    unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> BOOL {
        let list = &mut *(list.0 as *mut Vec<(HWND, String)>);
        if IsWindowVisible(hwnd).as_bool() {
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf) as usize;
            if len > 0 {
                list.push((hwnd, String::from_utf16_lossy(&buf[..len])));
            }
        }
        true.into()
    }
    let mut list: Vec<(HWND, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

fn find_window(part: &str) -> Option<(HWND, String)> {
    let part = part.to_lowercase();
    windows_with_titles().into_iter().find(|(_, title)| title.to_lowercase().contains(&part))
}

fn list_sources() -> Res<()> {
    for (i, m) in gfx::monitors()?.iter().enumerate() {
        println!(
            "monitor {i}: {}x{} at {},{} on {}",
            m.rect.right - m.rect.left,
            m.rect.bottom - m.rect.top,
            m.rect.left,
            m.rect.top,
            m.adapter
        );
    }
    for (_, title) in windows_with_titles() {
        println!("window: {title}");
    }
    Ok(())
}

/// Current time on the clock that capture timestamps use, in 100 ns units.
pub(crate) fn qpc_100ns() -> i64 {
    let (mut now, mut freq) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut now);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    (now as i128 * 10_000_000 / freq.max(1) as i128) as i64
}

/// CPU use of this process as a share of the whole machine, the number Task Manager shows.
pub(crate) struct CpuMeter {
    last_cpu: u64,
    last_wall: Instant,
    cores: u32,
}

impl CpuMeter {
    fn new() -> Self {
        let mut info = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut info) };
        Self { last_cpu: process_cpu_100ns(), last_wall: Instant::now(), cores: info.dwNumberOfProcessors.max(1) }
    }

    pub(crate) fn percent(&mut self) -> f64 {
        let cpu = process_cpu_100ns();
        let now = Instant::now();
        let used = (cpu - self.last_cpu) as f64 / 10_000_000.0;
        let wall = now.duration_since(self.last_wall).as_secs_f64();
        self.last_cpu = cpu;
        self.last_wall = now;
        used / wall / self.cores as f64 * 100.0
    }
}

fn process_cpu_100ns() -> u64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    ft(k) + ft(u)
}

/// Working set and private bytes in megabytes.
pub(crate) fn memory_mb() -> (f64, f64) {
    let mut pmc = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        let _ = K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        );
    }
    (pmc.WorkingSetSize as f64 / 1_048_576.0, pmc.PrivateUsage as f64 / 1_048_576.0)
}

#[cfg(test)]
mod cloud_tests {
    use super::*;

    #[test]
    fn rfc3339_times() {
        let t = |s: &str| parse_rfc3339(s).map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
        assert_eq!(t("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(t("2026-10-15T20:00:00+02:00"), Some(1_792_087_200));
        assert_eq!(t("2026-10-15T18:00:00.123Z"), Some(1_792_087_200));
        assert_eq!(t("nonsense"), None);
    }
}
