//! Valorant detector. Works only on regions of the frame the recorder captured
//! from the outside. Main signal is the kill feed in the top right corner: rows
//! that involve the own player carry a yellow frame with a solid end cap.

mod feed;
mod logic;
mod profile;

pub use feed::{FeedObservation, OwnRowKind, RoundBannerFeatures};
pub use logic::RoundPhase;
pub use profile::{
    Colors, FeedProfile, KillBannerProfile, RoundBannerProfile, RoundProfile, TrackingProfile, ValorantProfile,
    DEFAULT_PROFILE_JSON,
};

use crate::detector::{Detector, SourceKind};
use crate::event::{Event, EventKind};
use crate::frame::{resample_bgra, RegionImage, RegionRequest, RgbImage, Sample};

use logic::{ConfirmedRow, RoundBannerState, RoundLogic, RowTracker};

/// Region ids of the Valorant detector.
pub const REGION_FEED: u32 = 0;
pub const REGION_ROUND_BANNER: u32 = 1;
pub const REGION_KILL_BANNER: u32 = 2;

const SUPPORTED: [EventKind; 8] = [
    EventKind::Kill,
    EventKind::DoubleKill,
    EventKind::TripleKill,
    EventKind::QuadraKill,
    EventKind::Ace,
    EventKind::Death,
    EventKind::RoundWon,
    EventKind::RoundLost,
];

/// What the detector saw in the last sample. For tuning and for the evaluation
/// tool. Must never be shown while a match is running.
#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    pub feed: Vec<FeedObservation>,
    pub round_banner: RoundBannerFeatures,
    pub kill_banner_white: f32,
}

struct PendingKill {
    row: ConfirmedRow,
}

pub struct ValorantDetector {
    profile: ValorantProfile,
    class_table: Vec<u8>,
    work: RgbImage,
    classes: Vec<u8>,
    tracker: RowTracker,
    round: RoundLogic,
    round_banner: RoundBannerState,
    pending_kills: Vec<PendingKill>,
    kill_banner_seen_ms: Option<u64>,
    kill_banner_available: bool,
    diagnostics: Diagnostics,
}

impl ValorantDetector {
    /// Detector with the embedded default profile.
    pub fn new() -> ValorantDetector {
        ValorantDetector::with_profile(ValorantProfile::builtin())
    }

    pub fn with_profile(profile: ValorantProfile) -> ValorantDetector {
        ValorantDetector {
            class_table: feed::build_class_table(&profile.colors),
            profile,
            work: RgbImage::default(),
            classes: Vec::new(),
            tracker: RowTracker::default(),
            round: RoundLogic::new(),
            round_banner: RoundBannerState::default(),
            pending_kills: Vec::new(),
            kill_banner_seen_ms: None,
            kill_banner_available: false,
            diagnostics: Diagnostics::default(),
        }
    }

    pub fn profile(&self) -> &ValorantProfile {
        &self.profile
    }

    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    pub fn round_phase(&self) -> RoundPhase {
        self.round.phase()
    }

    pub fn kills_this_round(&self) -> u32 {
        self.round.kills_this_round()
    }

    /// Scales a region to working size and classifies its pixels. Returns the
    /// working size and the number of working pixels per frame height.
    fn prepare(&mut self, region: &RegionImage<'_>, frame_h: u32) -> Option<(usize, usize, f32)> {
        if frame_h == 0 {
            return None;
        }
        let scale = (self.profile.work_ref_height as f32 / frame_h as f32).min(1.0);
        let stretch = self.profile.hud_x_stretch.max(0.05);
        let w = ((region.width as f32 * scale / stretch).round() as usize).max(1);
        let h = ((region.height as f32 * scale).round() as usize).max(1);
        if !resample_bgra(region, w, h, &mut self.work) {
            return None;
        }
        feed::classify(&self.work, &self.class_table, &mut self.classes);
        Some((w, h, frame_h as f32 * scale))
    }

    fn release_kill(&mut self, row: ConfirmedRow, with_banner: bool, out: &mut Vec<Event>) {
        let bonus = if with_banner { self.profile.kill_banner.confidence_bonus } else { 0.0 };
        let confidence = (0.6 + 0.2 * row.score + bonus).min(1.0);
        out.push(self.round.on_kill(row.first_ms, confidence, &self.profile.round));
    }

    /// Reports kills that got their banner or waited long enough.
    fn release_pending(&mut self, now_ms: u64, force: bool, out: &mut Vec<Event>) {
        let wait = (self.profile.kill_banner.wait_s.max(0.0) * 1000.0) as u64;
        let lookback = (self.profile.kill_banner.lookback_s.max(0.0) * 1000.0) as u64;
        let mut pending = std::mem::take(&mut self.pending_kills);
        pending.sort_by_key(|p| p.row.first_ms);
        for p in pending {
            let banner = self.kill_banner_seen_ms.is_some_and(|b| b + lookback >= p.row.first_ms);
            if banner || force || !self.kill_banner_available || now_ms >= p.row.first_ms + wait {
                self.release_kill(p.row, banner, out);
            } else {
                self.pending_kills.push(p);
            }
        }
    }
}

impl Default for ValorantDetector {
    fn default() -> Self {
        ValorantDetector::new()
    }
}

impl Detector for ValorantDetector {
    fn id(&self) -> &'static str {
        "valorant"
    }

    fn display_name(&self) -> &'static str {
        "VALORANT"
    }

    fn source(&self) -> SourceKind {
        SourceKind::Frames
    }

    fn supported_events(&self) -> &'static [EventKind] {
        &SUPPORTED
    }

    fn regions(&self) -> Vec<RegionRequest> {
        let p = &self.profile;
        let make = |id, name, rect| RegionRequest {
            id,
            name,
            rect,
            x_stretch: p.hud_x_stretch,
            rate_hz: p.sample_rate_hz,
            work_frame_height: p.work_ref_height,
        };
        vec![
            make(REGION_FEED, "kill_feed", p.feed.roi),
            make(REGION_ROUND_BANNER, "round_banner", p.round_banner.roi),
            make(REGION_KILL_BANNER, "kill_banner", p.kill_banner.roi),
        ]
    }

    fn process(&mut self, sample: &Sample<'_>) -> Vec<Event> {
        let t = sample.t_ms;
        let frame_h = sample.frame_height;
        let mut out = Vec::new();

        // Kill banner first, so a kill confirmed in this sample can use it.
        self.kill_banner_available = false;
        self.diagnostics.kill_banner_white = 0.0;
        if let Some(region) = sample.region(REGION_KILL_BANNER) {
            if self.prepare(region, frame_h).is_some() {
                self.kill_banner_available = true;
                let white = feed::white_fraction(&self.classes);
                self.diagnostics.kill_banner_white = white;
                if white >= self.profile.kill_banner.white_min_fraction {
                    self.kill_banner_seen_ms = Some(t);
                }
            }
        }

        self.diagnostics.feed.clear();
        if let Some(region) = sample.region(REGION_FEED) {
            if let Some((w, h, px_per_h)) = self.prepare(region, frame_h) {
                let rows = feed::find_own_rows(&self.classes, w, h, px_per_h, &self.profile.feed);
                let confirmed = self.tracker.update(t, &rows, &self.profile.tracking);
                self.diagnostics.feed = rows;
                for row in confirmed {
                    match row.kind {
                        OwnRowKind::Kill => self.pending_kills.push(PendingKill { row }),
                        OwnRowKind::Death => {
                            // Kills seen earlier come first so the order stays right.
                            let earlier: Vec<ConfirmedRow> = {
                                let (a, b): (Vec<PendingKill>, Vec<PendingKill>) =
                                    std::mem::take(&mut self.pending_kills).into_iter().partition(|p| p.row.first_ms <= row.first_ms);
                                self.pending_kills = b;
                                a.into_iter().map(|p| p.row).collect()
                            };
                            let lookback = (self.profile.kill_banner.lookback_s.max(0.0) * 1000.0) as u64;
                            for k in earlier {
                                let banner = self.kill_banner_seen_ms.is_some_and(|b| b + lookback >= k.first_ms);
                                self.release_kill(k, banner, &mut out);
                            }
                            let anchor = if row.anchored { 0.15 } else { 0.0 };
                            let confidence = (0.65 + 0.2 * row.score + anchor).min(1.0);
                            out.push(self.round.on_death(row.first_ms, confidence));
                        }
                    }
                }
            }
        }

        self.diagnostics.round_banner = RoundBannerFeatures::default();
        if let Some(region) = sample.region(REGION_ROUND_BANNER) {
            if let Some((w, h, px_per_h)) = self.prepare(region, frame_h) {
                let p = &self.profile.round_banner;
                let features = feed::round_banner_features(&self.classes, w, h, px_per_h, p);
                self.diagnostics.round_banner = features;
                let outcome = features.outcome(p);
                if let Some((first_ms, won)) = self.round_banner.update(t, outcome, p.confirm_samples, p.cooldown_s) {
                    // Kills seen before the banner belong to the round that just ended.
                    self.release_pending(t, true, &mut out);
                    out.push(self.round.on_round_end(first_ms, won, 0.9));
                }
            }
        }

        self.release_pending(t, false, &mut out);
        out.sort_by_key(|e| e.t_ms);
        out
    }

    fn poll(&mut self, now_ms: u64) -> Vec<Event> {
        let mut out = Vec::new();
        self.release_pending(now_ms, false, &mut out);
        out
    }

    fn reset(&mut self) {
        self.tracker.clear();
        self.round.reset();
        self.round_banner.clear();
        self.pending_kills.clear();
        self.kill_banner_seen_ms = None;
        self.kill_banner_available = false;
        self.diagnostics = Diagnostics::default();
    }
}

#[cfg(test)]
mod tests;
