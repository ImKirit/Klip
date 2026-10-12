//! The plugin interface: one detector per game.

use crate::event::{Event, EventKind};
use crate::frame::{RegionRequest, Sample};

/// Where a detector gets its information from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// Regions of the frame the recorder captures anyway. The detector is fed
    /// through [`Detector::process`].
    Frames,
    /// The game reports events itself through an official interface (for example
    /// a game state integration). Such a detector collects on its own and hands
    /// events out through [`Detector::poll`]. It asks for no regions.
    External,
}

/// A detector turns samples into events. It never touches a game, a window or a
/// process: pixels and timestamps go in, events come out.
pub trait Detector: Send {
    /// Stable identifier, for example `valorant`.
    fn id(&self) -> &'static str;

    /// Name for the settings screen.
    fn display_name(&self) -> &'static str;

    fn source(&self) -> SourceKind;

    /// Event kinds this detector can actually report. Kinds missing here are not
    /// implemented and must not be offered as working in the settings.
    fn supported_events(&self) -> &'static [EventKind];

    /// Regions and rates the detector needs. Empty for external sources. The
    /// recorder resolves each request with [`RegionRequest::pixel_rect`] against
    /// the current frame size.
    fn regions(&self) -> Vec<RegionRequest>;

    /// Feeds one sampling step. Regions may be missing, the detector then skips
    /// the part that needs them. Timestamps must not go backwards.
    fn process(&mut self, sample: &Sample<'_>) -> Vec<Event>;

    /// Hands out events that did not come from a sample (external sources) or
    /// that were held back for confirmation. Safe to call at any rate.
    fn poll(&mut self, _now_ms: u64) -> Vec<Event> {
        Vec::new()
    }

    /// Forgets all state, for example when the game exits.
    fn reset(&mut self);
}
