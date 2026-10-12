//! Input overlay data for Klip: keyboard geometry, the capture filter, the input track
//! and its file format.
//!
//! Klip is not a keylogger, and this crate is where that promise is kept. The rules live
//! in one place, [`filter::Processor`], which is the only path into the track:
//!
//! * only keys of the active layout scope are stored, everything else is dropped on arrival
//! * nothing is stored unless the caller says a game is in the foreground, and nothing
//!   while paused
//! * what is stored is a scancode, pressed or released, and a time. Never text, never a
//!   character, never a virtual key code
//! * mouse movement is stored as a coarse direction and amount per time window, never as
//!   a position
//!
//! The Windows part ([`capture`]) only turns raw input messages into [`filter::RawEvent`]
//! values and hands them to the processor, so the rules are testable without real input.

pub mod filter;
pub mod layout;
pub mod track;

#[cfg(windows)]
pub mod capture;

pub use filter::{Allowlist, Processor, RawEvent, Stats, MOVE_WINDOW};
pub use layout::{
    bounds, visible_keys, Animation, AnimationKind, Appearance, Block, Colors, Corner, Key, KeyId, KeyboardLayout,
    LayoutKind, LayoutLibrary, MouseButton, MouseConfig, OverlayLayout, Preset, Rect, Scope, Widgets,
};
pub use track::{ClipEvent, Event, EventKind, Held, InputClip, Motion, Snapshot, Track, FORMAT_VERSION};

#[cfg(windows)]
pub use capture::{now_100ns, InputCapture};
