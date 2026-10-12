//! Game detection for Klip: what is installed, what runs in the foreground, which games
//! carry a kernel anti-cheat, and the per game profiles.
//!
//! ANTI-CHEAT, read before changing anything in this crate. The owner plays Valorant with
//! Vanguard on this machine. This crate looks at games strictly from the outside:
//!
//! * It asks Windows for the exe path of a process with `PROCESS_QUERY_LIMITED_INFORMATION`
//!   and reads the process list through ToolHelp. Both live in `foreground.rs`, nowhere else.
//! * It never reads process memory, never lists modules, never opens a handle with any
//!   other right, never changes a foreign window and never produces input.
//! * Inside an install folder it reads names of files and folders, not their content.
//! * It never talks to the Riot client and never reads its lockfiles.
//! * Unknown games count as protected: `Protection::hook_allowed()` is true only for games
//!   on an explicit allow list, and that list is empty.
//!
//! The rules and their sources are in the vault: `Anti-Cheat & Overlay/Anti-Cheat-Regeln.md`.

#![cfg(windows)]

pub mod catalog;
pub mod foreground;
pub mod library;
pub mod profile;
pub mod protection;
mod reg;
pub mod vdf;
pub mod watcher;

pub use catalog::{GameCatalog, GameMatch, Identity, ManualGame, ManualList};
pub use foreground::{foreground_info, ForegroundInfo};
pub use library::{scan_all, GameSource, InstalledGame, ScanResult};
pub use profile::{resolve, AudioTrack, AudioTrackKind, AutoClipRule, EffectiveSettings, GameProfile, ProfileDefaults, ProfileStore};
pub use protection::{AntiCheat, AntiCheatMarker, Evidence, Protection, ProtectionDb, ProtectionReason};
pub use watcher::{GameEvent, GameWatcher, RunningGame, WatcherOptions, WatcherStats};
