//! Captures for 8 seconds with the "left hand for shooters" template and prints totals.
//!
//! The person at this machine may be typing something private right now. So this
//! program prints counts and nothing else: no key, no button, no time of a single
//! event. It writes no file, and what it captured is gone when it exits.
//!
//! cargo run -p klip-input --example capture

use klip_input::{now_100ns, InputCapture, LayoutKind, OverlayLayout, Preset};
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

/// Tick of the last input Windows saw from anyone. A time only, it says nothing about
/// which key or button it was. Used to tell "no input arrived" from "nobody was there".
fn last_input_tick() -> u32 {
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        let _ = GetLastInputInfo(&mut info);
    }
    info.dwTime
}

fn main() {
    let mut layout = OverlayLayout::preset(LayoutKind::IsoQwertz, Preset::LeftHandShooter);
    // Movement on, so that a hand resting on the mouse is enough to show input arriving.
    layout.mouse.movement = true;
    let scope_keys = layout.visible_keys().len();

    let capture = match InputCapture::start(&layout, Duration::from_secs(30)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("capture did not start: {e}");
            std::process::exit(1);
        }
    };
    println!("layout scope: {scope_keys} keys, mouse buttons, wheel, movement");
    println!("registered before start (keyboard, mouse): {:?}", capture.registered());

    // The example stands in for the game detection and simply says "a game is in front".
    let t0 = now_100ns();
    capture.set_game_active(true);
    std::thread::sleep(Duration::from_millis(200));
    println!("registered while capturing (keyboard, mouse): {:?}", capture.registered());
    println!("capturing for 8 seconds ...");
    let tick_before = last_input_tick();
    std::thread::sleep(Duration::from_millis(7800));
    let system_saw_input = last_input_tick() != tick_before;
    let t1 = now_100ns();

    capture.set_game_active(false);
    std::thread::sleep(Duration::from_millis(200));
    let stats = capture.stats();
    let clip = capture.slice(t0, t1);
    println!("registered after stop (keyboard, mouse): {:?}", capture.registered());
    println!("raw events received:   {}", stats.received);
    println!("events discarded:      {}", stats.discarded);
    println!("events folded:         {}", stats.folded);
    println!("events stored:         {}", stats.stored);
    println!("events in the 8 s clip: {}", clip.events.len());
    println!("windows saw any input during the run: {system_saw_input}");
    match (system_saw_input, stats.received) {
        (true, 0) => println!("RESULT: input happened but none arrived here, raw input is NOT working"),
        (false, 0) => println!("RESULT: nobody touched the machine, this run proves nothing"),
        _ => println!("RESULT: raw input arrives"),
    }
    if let Some(e) = capture.last_error() {
        println!("error: {e}");
    }
}
