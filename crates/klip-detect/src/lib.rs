//! Game event detection for Klip auto clips.
//!
//! Pixels and timestamps go in, events come out. This crate never opens a
//! process, a window, a file of a game or a network connection. Detectors work
//! on regions of the frame the recorder captures from the outside anyway.
//!
//! * [`Detector`]: one implementation per game, found through the [`Registry`].
//! * [`Event`] and [`EventKind`]: what a detector reports.
//! * [`RuleSet`], [`plan_clips`] and [`ClipPlanner`]: which events become clips.
//! * [`valorant`]: the first detector.

mod detector;
mod event;
mod frame;
mod registry;
mod rules;
pub mod valorant;

pub use detector::{Detector, SourceKind};
pub use event::{Event, EventKind};
pub use frame::{
    nv12_region_to_bgra, resample_bgra, rgb_to_hsv, AnchorX, HsvRange, PixelRect, RegionImage, RegionRequest, RelRect,
    RgbImage, Sample,
};
pub use registry::{DetectorFactory, Registry};
pub use rules::{plan_clips, ClipJob, ClipPlanner, Rule, RuleSet};
