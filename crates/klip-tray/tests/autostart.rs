//! Autostart against the real registry of the current user. One test function on purpose:
//! the steps share one registry value and must not run in parallel. Whatever was there
//! before the test is put back afterwards, also if an assertion fails.

use klip_tray::autostart;
use std::path::Path;

struct Restore(Option<String>);

impl Drop for Restore {
    fn drop(&mut self) {
        let result = match &self.0 {
            Some(previous) => autostart::write_command(previous),
            None => autostart::disable(),
        };
        if let Err(e) = result {
            eprintln!("could not restore the autostart entry: {e}");
        }
    }
}

#[test]
fn enable_read_disable_leaves_nothing_behind() {
    let restore = Restore(autostart::command());
    let exe = Path::new("C:\\Klip Test Folder\\klip-recorder.exe");

    autostart::disable().unwrap();
    assert!(!autostart::is_enabled());
    assert_eq!(autostart::command(), None);
    // Turning it off twice is fine.
    autostart::disable().unwrap();

    autostart::enable(exe).unwrap();
    assert!(autostart::is_enabled());
    assert_eq!(autostart::command().as_deref(), Some("\"C:\\Klip Test Folder\\klip-recorder.exe\""));
    assert!(autostart::points_to(exe));
    assert!(!autostart::points_to(Path::new("C:\\Elsewhere\\klip-recorder.exe")));

    autostart::enable_with_args(exe, " --autostart ").unwrap();
    assert_eq!(autostart::command().as_deref(), Some("\"C:\\Klip Test Folder\\klip-recorder.exe\" --autostart"));
    assert!(autostart::points_to(exe));

    assert!(autostart::enable(Path::new("klip-recorder.exe")).is_err(), "a relative path would depend on the start folder");

    autostart::disable().unwrap();
    assert!(!autostart::is_enabled());

    // The test started from "off" unless the machine really had Klip in autostart.
    let previous = restore.0.clone();
    drop(restore);
    assert_eq!(autostart::command(), previous);
}
