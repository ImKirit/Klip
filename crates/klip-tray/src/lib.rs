//! The parts of the recorder that make it a background program: the tray icon with its
//! menu, the autostart entry and the single instance lock.
//!
//! ANTI-CHEAT: nothing in this crate looks at or touches another program. The tray icon
//! is drawn by Explorer, the only window is an invisible one of our own, and every
//! message this crate posts goes to that own window. No hooks, no input, no foreign
//! windows. The rules are in the vault: `Anti-Cheat & Overlay/Anti-Cheat-Regeln.md`.

#![cfg(windows)]

pub mod autostart;
pub mod icon;
pub mod instance;
pub mod tray;

pub use icon::Rgb;
pub use instance::SingleInstance;
pub use tray::{MenuEntry, Tray, TrayColors, TrayEvent, TrayOptions, TrayState};
