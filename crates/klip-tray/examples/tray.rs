//! Shows the tray icon for eight seconds, walks through the three states and ends.
//! Opens no window and needs no input. Clicking the icon during the run prints the event.
//!
//!   cargo run -p klip-tray --example tray

use klip_tray::{MenuEntry, SingleInstance, Tray, TrayEvent, TrayOptions, TrayState};
use std::time::{Duration, Instant};

const BUFFER: u32 = 1;
const SAVE_CLIP: u32 = 2;
const RECORD: u32 = 3;
const LIBRARY: u32 = 4;
const SETTINGS: u32 = 5;
const QUIT: u32 = 6;

fn menu(state: TrayState) -> Vec<MenuEntry> {
    let buffering = state != TrayState::Idle;
    let recording = state == TrayState::Recording;
    vec![
        MenuEntry::item(BUFFER, "Puffer").checked(buffering),
        MenuEntry::item(SAVE_CLIP, "Clip speichern").enabled(buffering),
        MenuEntry::item(RECORD, if recording { "Aufnahme stoppen" } else { "Aufnahme starten" }),
        MenuEntry::Separator,
        MenuEntry::item(LIBRARY, "Bibliothek öffnen"),
        MenuEntry::item(SETTINGS, "Einstellungen"),
        MenuEntry::Separator,
        MenuEntry::item(QUIT, "Beenden"),
    ]
}

fn main() {
    // A name of its own, so the example also runs next to a real recorder.
    let Some(_lock) = SingleInstance::acquire("Local\\Klip.Example.Tray").expect("creating the lock") else {
        println!("Another instance of this example is running, leaving.");
        return;
    };
    println!("Second lock attempt while the first is held: {}", match SingleInstance::acquire("Local\\Klip.Example.Tray") {
        Ok(None) => "refused (correct)",
        Ok(Some(_)) => "GRANTED (wrong)",
        Err(_) => "error",
    });

    let options = TrayOptions { tooltip: "Klip. Leerlauf".into(), menu: menu(TrayState::Idle), ..Default::default() };
    let (tray, events) = Tray::spawn(options).expect("creating the tray icon");
    println!("Icon accepted by the shell: {}, registrations: {}", tray.is_present(), tray.registrations());
    println!("Icon rectangle reported by the shell: {:?}", tray.icon_rect());

    let steps = [
        (2.5, Some((TrayState::Buffering, "Klip. Puffer aktiv"))),
        (5.0, Some((TrayState::Recording, "Klip. Aufnahme läuft"))),
        // Pretend Explorer restarted. Sent to our own window only.
        (6.5, None),
    ];
    let start = Instant::now();
    let end = start + Duration::from_secs(8);
    let mut next = 0;
    loop {
        let now = Instant::now();
        if now >= end {
            break;
        }
        let step_at = steps.get(next).map(|(at, _)| start + Duration::from_secs_f64(*at)).unwrap_or(end);
        if now >= step_at {
            match steps[next].1 {
                Some((state, tooltip)) => {
                    tray.set_state(state);
                    tray.set_tooltip(tooltip);
                    tray.set_menu(menu(state)).expect("menu ids");
                    println!("{:>4.1} s  state -> {state:?}", start.elapsed().as_secs_f64());
                }
                None => {
                    let before = tray.registrations();
                    tray.simulate_taskbar_created();
                    std::thread::sleep(Duration::from_millis(200));
                    println!(
                        "{:>4.1} s  simulated TaskbarCreated: registrations {before} -> {}, present: {}, rectangle: {:?}",
                        start.elapsed().as_secs_f64(),
                        tray.registrations(),
                        tray.is_present(),
                        tray.icon_rect()
                    );
                }
            }
            next += 1;
            continue;
        }
        match events.recv_timeout(step_at.min(end) - now) {
            Ok(TrayEvent::Menu(QUIT)) => {
                println!("{:>4.1} s  menu: Beenden", start.elapsed().as_secs_f64());
                break;
            }
            Ok(event) => println!("{:>4.1} s  {event:?}", start.elapsed().as_secs_f64()),
            Err(_) => {}
        }
    }
    tray.close();
    println!("Icon removed, done after {:.1} s.", start.elapsed().as_secs_f64());
}
