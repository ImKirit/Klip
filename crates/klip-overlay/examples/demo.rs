//! Shows the four overlay elements on the SECOND monitor for about fourteen seconds and
//! writes PNG pictures of them (design version 2).
//!
//!   cargo run -p klip-overlay --example demo
//!   cargo run -p klip-overlay --example demo -- --measure status 22
//!
//! Flags:
//!   --sound            play the clip tone (off by default, the demo is silent)
//!   --capturable       do not exclude the windows from capture (to prove they render)
//!   --system-fonts     skip the font files of the app, use what Windows has
//!   --no-window        only write the PNGs
//!   --no-png           only show the windows
//!   --german           German texts in the windows (the PNGs show both anyway)
//!   --measure <idle|status|recording> [seconds]
//!                      hold one state and print the CPU time the process used
//!
//! The demo creates no input and touches no foreign window. It refuses to open a window
//! when there is no second monitor right of the primary one.

use klip_input::{Held, KeyId, LayoutKind, MouseButton, OverlayLayout, Preset};
use klip_overlay::{
    font_files_in, list_monitors, render_png, window_coverage, Action, ClipEntry, Element, Lang, MonitorTarget,
    Overlay, OverlayConfig, OverlayState, PanelHints, SnapshotOptions, SoundConfig, StatusKind, Toast,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FILETIME, HWND, POINT};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, WindowFromPoint};

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The three families of the design, if the app has them in node_modules. Read only.
fn app_fonts() -> Vec<PathBuf> {
    let base = project_root().join("app/node_modules/@fontsource");
    let mut files = Vec::new();
    for family in ["archivo", "jetbrains-mono"] {
        for file in font_files_in(&base.join(family).join("files")) {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let wanted = ["400", "500", "600", "700"].iter().any(|w| name == format!("{family}-latin-{w}-normal.woff2"));
            if wanted {
                files.push(file);
            }
        }
    }
    files
}

fn clips() -> Vec<ClipEntry> {
    [("Ace on Ascent B", "21:47", 32), ("Clutch 1v3", "21:31", 45), ("Full session, ranked", "20:58", 2294)]
        .into_iter()
        .map(|(title, time, secs)| ClipEntry { title: title.into(), time: time.into(), duration_secs: secs })
        .collect()
}

fn base_state(monitor: MonitorTarget) -> OverlayState {
    let mut state = OverlayState { monitor, ..Default::default() };
    state.input.layout = OverlayLayout::preset(LayoutKind::IsoQwertz, Preset::LeftHandShooter);
    state.input.layout.mouse.movement = true;
    state.input.layout.widgets.kps = true;
    state.panel.clips = clips();
    state.panel.hints = PanelHints {
        save_clip: "Alt+S".into(),
        record: "Ctrl+Shift+F9".into(),
        screenshot: "Ctrl+Shift+F6".into(),
        mic: "Alt+M".into(),
        toggle: "Alt+Z".into(),
    };
    state
}

fn held(keys: &[u16], buttons: &[MouseButton]) -> Held {
    let mut h = Held::default();
    let mut keys: Vec<KeyId> = keys.iter().map(|k| KeyId(*k)).collect();
    keys.sort();
    h.keys = keys;
    h.buttons = buttons.to_vec();
    h.buttons.sort();
    h
}

fn cpu_time() -> Duration {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    Duration::from_nanos((ticks(k) + ticks(u)) * 100)
}

fn write_pngs(fonts: &[PathBuf]) {
    let dir = project_root().join("Claude/drafts/screenshots");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        println!("png: cannot create {}: {e}", dir.display());
        return;
    }
    let crop = |only: &[Element]| SnapshotOptions {
        crop: true,
        zoom: 2,
        font_files: fonts.to_vec(),
        only: only.to_vec(),
        ..Default::default()
    };
    let save = |name: &str, state: &OverlayState, options: &SnapshotOptions| {
        let path = dir.join(name);
        match render_png(state, options, &path) {
            Ok(f) => println!(
                "png: {} (ui font: {}, files ok {} failed {} {} families {:?})",
                path.display(),
                f.ui,
                f.files_loaded,
                f.files_failed,
                f.first_error,
                f.families
            ),
            Err(e) => println!("png: FAILED {e}"),
        }
    };
    let now = Instant::now();
    let base = base_state(MonitorTarget::Primary);

    let mut s = base.clone();
    s.status = StatusKind::Buffering { seconds: 60 };
    save("overlay-v2-status-buffer.png", &s, &crop(&[Element::Status]));
    s.status = StatusKind::Recording { since: now - Duration::from_secs(72) };
    save("overlay-v2-status-recording.png", &s, &crop(&[Element::Status]));
    s.status = StatusKind::Paused { elapsed: Duration::from_secs(72) };
    save("overlay-v2-status-paused.png", &s, &crop(&[Element::Status]));

    let toast = Toast::clip_saved("Last 60 s / 1920 x 1080 / 60 FPS");
    let mut o = crop(&[Element::Toast]);
    o.toast = Some(toast.clone());
    save("overlay-v2-toast.png", &base, &o);
    // The stack of the design draft: accent, red for the recording, accent.
    let stack = vec![
        Toast::clip_saved("Last 60 s"),
        Toast::recording("Recording started", "Ctrl Shift F9 to stop"),
        Toast::new("Auto clip saved", "15 s"),
    ];
    let mut o = crop(&[Element::Toast]);
    o.toasts = stack.clone();
    save("overlay-v2-toasts.png", &base, &o);

    let mut s = base.clone();
    s.input.visible = true;
    s.input.held = held(&[0x11, 0x20, 0x2A, 0x39], &[MouseButton::Left]);
    s.input.motion = Some(7);
    s.input.wheel = 1;
    s.input.kps = Some(6.0);
    save("overlay-v2-input.png", &s, &crop(&[Element::Input]));

    let mut s = base.clone();
    s.status = StatusKind::Buffering { seconds: 60 };
    s.panel.placement.visible = true;
    let mut o = crop(&[Element::Panel]);
    o.hover = Some(Action::OpenClip(1));
    save("overlay-v2-panel.png", &s, &o);
    s.status = StatusKind::Recording { since: now - Duration::from_secs(72) };
    s.mic_muted = true;
    let mut o = crop(&[Element::Panel]);
    o.hover = Some(Action::SaveClip);
    save("overlay-v2-panel-recording.png", &s, &o);
    s.language = Lang::De;
    s.status = StatusKind::Paused { elapsed: Duration::from_secs(72) };
    save("overlay-v2-panel-de.png", &s, &crop(&[Element::Panel]));
    let mut s = base.clone();
    s.panel.placement.visible = true;
    s.panel.clips.clear();
    s.panel.hints = PanelHints::default();
    save("overlay-v2-panel-empty.png", &s, &crop(&[Element::Panel]));

    // Everything at once at real size, the way a 1080p monitor would show it. The panel
    // stands where a user dragged it.
    let mut s = base.clone();
    s.status = StatusKind::Buffering { seconds: 60 };
    s.panel.placement.visible = true;
    s.panel.position = Some((60, 120));
    s.input.visible = true;
    s.input.held = held(&[0x11, 0x1E], &[]);
    let o = SnapshotOptions { font_files: fonts.to_vec(), toasts: stack.clone(), ..Default::default() };
    save("overlay-v2-all.png", &s, &o);

    // The two pictures of the README are the same views.
    let docs = project_root().join("docs/screenshots");
    if docs.is_dir() {
        for (from, to) in [("overlay-v2-all.png", "overlay.png"), ("overlay-v2-toasts.png", "overlay-toast.png")] {
            match std::fs::copy(dir.join(from), docs.join(to)) {
                Ok(_) => println!("png: {} -> docs/screenshots/{to}", from),
                Err(e) => println!("png: FAILED to copy {from}: {e}"),
            }
        }
    }
}

fn window_at(x: i32, y: i32) -> isize {
    unsafe { WindowFromPoint(POINT { x, y }).0 as isize }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let fonts = if flag("--system-fonts") { Vec::new() } else { app_fonts() };
    println!("font files handed over: {}", fonts.len());

    if !flag("--no-png") && !flag("--measure") {
        write_pngs(&fonts);
    }
    if flag("--no-window") {
        return;
    }

    // Only the second monitor: the user may be playing on the first one.
    let monitors = list_monitors();
    for m in &monitors {
        println!("monitor {}: {}x{} at {},{} primary={}", m.index, m.rect.width(), m.rect.height(), m.rect.left, m.rect.top, m.primary);
    }
    let Some(second) = monitors.iter().find(|m| !m.primary && m.rect.left >= 1920) else {
        println!("no second monitor right of the primary one, not opening any window");
        return;
    };
    let target = MonitorTarget::At { x: second.rect.left + second.rect.width() / 2, y: second.rect.top + second.rect.height() / 2 };

    let config = OverlayConfig {
        state: OverlayState {
            language: if flag("--german") { Lang::De } else { Lang::En },
            ..base_state(target)
        },
        font_files: fonts,
        sound: SoundConfig { enabled: flag("--sound"), volume: 0.4 },
        exclude_from_capture: !flag("--capturable"),
    };
    let (overlay, actions) = match Overlay::start(config) {
        Ok(v) => v,
        Err(e) => {
            println!("overlay did not start: {e}");
            return;
        }
    };
    let f = overlay.fonts();
    println!("fonts: display={} | ui={} | mono={} | files ok {} failed {}", f.display, f.ui, f.mono, f.files_loaded, f.files_failed);
    println!("font families in the files: {:?}", f.families);

    if let Some(at) = args.iter().position(|a| a == "--measure") {
        let mode = args.get(at + 1).map(String::as_str).unwrap_or("idle");
        let seconds: u64 = args.get(at + 2).and_then(|s| s.parse().ok()).unwrap_or(22);
        match mode {
            "status" => overlay.update(|s| s.status = StatusKind::Buffering { seconds: 30 }),
            "recording" => overlay.update(|s| s.status = StatusKind::Recording { since: Instant::now() }),
            _ => {}
        }
        // Let the fade in finish before the measurement starts.
        std::thread::sleep(Duration::from_millis(1500));
        let (cpu0, stats0, t0) = (cpu_time(), overlay.stats(), Instant::now());
        std::thread::sleep(Duration::from_secs(seconds));
        let (cpu1, stats1, wall) = (cpu_time(), overlay.stats(), t0.elapsed());
        let cpu = cpu1 - cpu0;
        println!(
            "MEASURE mode={mode} wall={:.2}s cpu={:.1}ms cpu_percent_of_one_core={:.4} frames={} wakeups={} windows={}",
            wall.as_secs_f64(),
            cpu.as_secs_f64() * 1000.0,
            cpu.as_secs_f64() / wall.as_secs_f64() * 100.0,
            stats1.frames.iter().sum::<u64>() - stats0.frames.iter().sum::<u64>(),
            stats1.wakeups - stats0.wakeups,
            stats1.windows.len(),
        );
        return;
    }

    // For the capture test: everything visible at once, standing still.
    if let Some(at) = args.iter().position(|a| a == "--hold") {
        let seconds: u64 = args.get(at + 1).and_then(|s| s.parse().ok()).unwrap_or(10).min(12);
        overlay.update(|s| {
            s.status = StatusKind::Buffering { seconds: 30 };
            s.toast_hold = Duration::from_secs(seconds);
            s.input.visible = true;
            s.input.held = held(&[0x11, 0x1E, 0x39], &[MouseButton::Left]);
            s.panel.placement.visible = true;
        });
        overlay.toast(Toast::clip_saved("Last 30 s / 1920 x 1080 / 60 FPS"));
        overlay.toast(Toast::recording("Recording started", "Ctrl Shift F9 to stop"));
        std::thread::sleep(Duration::from_secs(seconds));
        let s = overlay.stats();
        println!("HOLD {seconds}s windows={} capture_excluded={}", s.windows.len(), s.capture_excluded);
        for w in &s.windows {
            println!("RECT {:?} {} {} {} {}", w.element, w.rect.left, w.rect.top, w.rect.right, w.rect.bottom);
        }
        return;
    }

    let pause = |ms: u64| std::thread::sleep(Duration::from_millis(ms));
    let report = |what: &str| {
        let s = overlay.stats();
        let list: Vec<String> = s
            .windows
            .iter()
            .map(|w| format!("{:?} {}x{} at {},{}", w.element, w.rect.width(), w.rect.height(), w.rect.left, w.rect.top))
            .collect();
        println!(
            "[{what}] windows: [{}] frames={:?} capture_excluded={} graphics_init_ms={}",
            list.join("; "),
            s.frames,
            s.capture_excluded,
            s.graphics_init_ms
        );
    };
    let started = Instant::now();
    report("start, nothing visible");

    // 1. Status indicator: buffer, recording, paused.
    overlay.update(|s| s.status = StatusKind::Buffering { seconds: 30 });
    pause(1200);
    report("status buffer");
    // Clicks must fall through the indicator: the window under its middle is not ours.
    if let Some(w) = overlay.stats().windows.iter().find(|w| w.element == Element::Status) {
        let under = window_at((w.rect.left + w.rect.right) / 2, (w.rect.top + w.rect.bottom) / 2);
        println!("CLICK-THROUGH status: window under the indicator is ours = {} (expected false)", under == w.hwnd);
    }
    overlay.update(|s| s.status = StatusKind::Recording { since: Instant::now() });
    pause(2200);
    report("status recording, two ticks");
    overlay.update(|s| s.status = StatusKind::Paused { elapsed: Duration::from_secs(2) });
    pause(600);
    overlay.update(|s| s.status = StatusKind::Buffering { seconds: 30 });

    // 2. Toasts, stacked, each on its own clock. The tone only plays with --sound.
    overlay.update(|s| s.toast_hold = Duration::from_millis(1500));
    overlay.toast(Toast::clip_saved("Last 60 s / 1920 x 1080 / 60 FPS"));
    pause(400);
    report("toast 1");
    overlay.toast(Toast::recording("Recording started", "Ctrl Shift F9 to stop").with_hold(Duration::from_millis(600)));
    overlay.toast(Toast::new("Auto clip saved", "15 s"));
    pause(400);
    report("toasts 3");
    // Three more: six were handed over, four stay, the oldest ones dropped out.
    for i in 4..=6 {
        overlay.toast(Toast::new(&format!("Notice {i}"), ""));
    }
    pause(300);
    report("toasts 6 handed over, at most 4 shown");
    pause(900);
    report("toasts stand still, no new frames");
    pause(1300);
    report("toasts: all ran out");

    // 3. Input overlay with changing keys. The demo makes the states up, it reads no input.
    overlay.update(|s| s.input.visible = true);
    let frames: [(&[u16], &[MouseButton], Option<u8>); 8] = [
        (&[0x11], &[], None),
        (&[0x11, 0x20], &[], Some(0)),
        (&[0x11, 0x20, 0x2A], &[MouseButton::Left], Some(7)),
        (&[0x20, 0x39], &[MouseButton::Left], Some(6)),
        (&[0x1E], &[MouseButton::Right], Some(4)),
        (&[0x1E, 0x1F, 0x1D], &[], Some(3)),
        (&[0x13], &[MouseButton::Side1], None),
        (&[], &[], None),
    ];
    for (i, (keys, buttons, motion)) in frames.iter().enumerate() {
        overlay.update(|s| {
            s.input.held = held(keys, buttons);
            s.input.motion = *motion;
            s.input.kps = Some((i % 5) as f32 + 2.0);
        });
        pause(300);
    }
    report("input");
    overlay.update(|s| s.input.visible = false);

    // 4. Panel. A click is posted to our own window to prove the path to the channel.
    let open = overlay.toggle_panel();
    pause(900);
    report("panel");
    if let Some(w) = overlay.stats().windows.iter().find(|w| w.element == Element::Panel) {
        let under = window_at((w.rect.left + w.rect.right) / 2, (w.rect.top + w.rect.bottom) / 2);
        println!("CLICK-TARGET panel (open={open}): window under the panel is ours = {} (expected true)", under == w.hwnd);
    }
    for action in [Action::SaveClip, Action::ToggleMic, Action::OpenClip(1)] {
        overlay.test_click(action);
        match actions.recv_timeout(Duration::from_millis(500)) {
            Ok(got) => println!("ACTION posted {action:?} -> received {got:?}"),
            Err(_) => println!("ACTION posted {action:?} -> NOTHING received"),
        }
    }
    overlay.update(|s| s.mic_muted = true);
    pause(500);

    // 5. Dragging the grip. Messages go to our own panel window, the pointer stays put.
    let panel_rect = || overlay.stats().windows.iter().find(|w| w.element == Element::Panel).map(|w| w.rect);
    let before = panel_rect();
    for (what, dx, dy) in [("by 300, 80", 300, 80), ("far beyond the corner", 9000, 9000), ("far beyond the origin", -9000, -9000), ("by 240, 160", 240, 160)] {
        overlay.test_drag(dx, dy);
        let got = actions.recv_timeout(Duration::from_millis(500));
        pause(250);
        let (rect, pos) = (panel_rect(), overlay.state().panel.position);
        let inside = rect.is_some_and(|r| {
            r.left >= second.rect.left && r.top >= second.rect.top && r.right <= second.rect.right && r.bottom <= second.rect.bottom
        });
        println!(
            "DRAG {what}: action {got:?}, state position {pos:?}, window at {:?} (was {:?}), inside the monitor = {inside}",
            rect.map(|r| (r.left - second.rect.left, r.top - second.rect.top)),
            before.map(|r| (r.left - second.rect.left, r.top - second.rect.top)),
        );
    }
    // A press on the grip without a move reports nothing.
    overlay.test_drag(0, 0);
    println!("DRAG by 0, 0: action {:?} (expected a timeout)", actions.recv_timeout(Duration::from_millis(300)));
    // The buttons still work where the panel stands now.
    overlay.test_click(Action::Screenshot);
    println!("ACTION after the drag: {:?}", actions.recv_timeout(Duration::from_millis(500)));
    pause(600);
    overlay.test_click(Action::ClosePanel);
    println!("ACTION close -> {:?}", actions.recv_timeout(Duration::from_millis(500)));
    pause(500);
    println!("panel open after close click: {}", overlay.state().panel.placement.visible);

    // 6. Everything off: the windows must be gone.
    overlay.update(|s| s.status = StatusKind::Off);
    pause(800);
    report("end, nothing visible");
    println!("windows were possible for {:.1} s", started.elapsed().as_secs_f32());

    // Diagnosis helper on whatever is in the foreground. Reads three rectangles, no title.
    let foreground: HWND = unsafe { GetForegroundWindow() };
    match window_coverage(foreground.0 as isize) {
        Some(c) => println!(
            "foreground window {}x{} on a {}x{} monitor, covers it: {}",
            c.window.width(),
            c.window.height(),
            c.monitor.width(),
            c.monitor.height(),
            c.covers_monitor
        ),
        None => println!("foreground window: none or not on a monitor"),
    }
}
