//! State that lives across samples: tracked kill feed rows (debouncing) and the
//! per round state machine that turns own kills into multi kills.

use crate::event::{Event, EventKind};

use super::feed::{FeedObservation, OwnRowKind};
use super::profile::{RoundProfile, TrackingProfile};

fn ms(seconds: f32) -> u64 {
    (seconds.max(0.0) as f64 * 1000.0).round() as u64
}

struct Track {
    kind: OwnRowKind,
    x: f32,
    y: f32,
    first_ms: u64,
    last_ms: u64,
    hits: u32,
    score: f32,
    anchored: bool,
    reported: bool,
}

/// A row that was seen often enough to count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ConfirmedRow {
    pub kind: OwnRowKind,
    /// When the row was seen first.
    pub first_ms: u64,
    pub score: f32,
    pub anchored: bool,
}

/// Follows highlighted rows over time. A row stays in the feed for seconds and
/// moves up when older rows expire. Each tracked row reports exactly once.
#[derive(Default)]
pub(crate) struct RowTracker {
    tracks: Vec<Track>,
}

impl RowTracker {
    pub fn clear(&mut self) {
        self.tracks.clear();
    }

    pub fn update(&mut self, t_ms: u64, observations: &[FeedObservation], p: &TrackingProfile) -> Vec<ConfirmedRow> {
        let lost = ms(p.lost_after_s);
        self.tracks.retain(|t| t_ms.saturating_sub(t.last_ms) <= lost);

        let mut used = vec![false; self.tracks.len()];
        for obs in observations {
            let mut best: Option<(usize, f32)> = None;
            for (i, t) in self.tracks.iter().enumerate() {
                if used[i] || t.kind != obs.kind {
                    continue;
                }
                let dx = (t.x - obs.x).abs();
                // Positive: the row moved up since the last sighting.
                let up = t.y - obs.y;
                if dx > p.match_dx || up > p.match_dy_up || up < -p.match_dy_down {
                    continue;
                }
                let cost = dx + up.abs();
                if best.map_or(true, |(_, c)| cost < c) {
                    best = Some((i, cost));
                }
            }
            match best {
                Some((i, _)) => {
                    used[i] = true;
                    let t = &mut self.tracks[i];
                    t.x = obs.x;
                    t.y = obs.y;
                    t.last_ms = t_ms;
                    t.hits += 1;
                    t.score = t.score.max(obs.score);
                    t.anchored |= obs.anchored;
                }
                None => {
                    self.tracks.push(Track {
                        kind: obs.kind,
                        x: obs.x,
                        y: obs.y,
                        first_ms: t_ms,
                        last_ms: t_ms,
                        hits: 1,
                        score: obs.score,
                        anchored: obs.anchored,
                        reported: false,
                    });
                    used.push(true);
                }
            }
        }

        let need = p.confirm_samples.max(1);
        let mut out = Vec::new();
        for t in &mut self.tracks {
            if !t.reported && t.hits >= need {
                t.reported = true;
                out.push(ConfirmedRow { kind: t.kind, first_ms: t.first_ms, score: t.score, anchored: t.anchored });
            }
        }
        out.sort_by_key(|r| r.first_ms);
        out
    }
}

/// Where the own player stands in the current round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundPhase {
    /// Nothing known yet, or between rounds.
    Idle,
    /// At least one own kill in this round, player alive as far as known.
    Alive,
    /// Own death seen in this round.
    Dead,
}

/// Counts own kills per round and names them (kill, double kill up to ace).
pub(crate) struct RoundLogic {
    kills: u32,
    first_kill_ms: Option<u64>,
    last_kill_ms: Option<u64>,
    death_ms: Option<u64>,
    round_end_ms: Option<u64>,
}

impl RoundLogic {
    pub fn new() -> RoundLogic {
        RoundLogic { kills: 0, first_kill_ms: None, last_kill_ms: None, death_ms: None, round_end_ms: None }
    }

    pub fn reset(&mut self) {
        *self = RoundLogic::new();
    }

    pub fn phase(&self) -> RoundPhase {
        if self.death_ms.is_some() {
            RoundPhase::Dead
        } else if self.kills > 0 {
            RoundPhase::Alive
        } else {
            RoundPhase::Idle
        }
    }

    pub fn kills_this_round(&self) -> u32 {
        self.kills
    }

    fn start_round(&mut self) {
        self.kills = 0;
        self.first_kill_ms = None;
        self.last_kill_ms = None;
        self.death_ms = None;
    }

    /// Decides whether a kill at `t_ms` can still belong to the counted round.
    fn is_new_round(&self, t_ms: u64, p: &RoundProfile) -> bool {
        // Dead players do not respawn within a round.
        if let Some(d) = self.death_ms {
            if t_ms > d + ms(p.new_round_after_death_s) {
                return true;
            }
        }
        if let Some(l) = self.last_kill_ms {
            if t_ms > l + ms(p.idle_reset_s) {
                return true;
            }
            // Kills right after the round end banner belong to the old round.
            // Once the next round runs they must not count any more.
            if let Some(end) = self.round_end_ms {
                if l >= end && t_ms > end + ms(p.new_round_after_death_s) {
                    return true;
                }
            }
        }
        false
    }

    pub fn on_kill(&mut self, t_ms: u64, confidence: f32, p: &RoundProfile) -> Event {
        if self.is_new_round(t_ms, p) {
            self.start_round();
            self.round_end_ms = None;
        }
        self.kills += 1;
        if self.kills == 1 {
            self.first_kill_ms = Some(t_ms);
        }
        self.last_kill_ms = Some(t_ms);
        Event {
            kind: EventKind::for_kill_count(self.kills),
            t_ms,
            start_ms: if (2..=5).contains(&self.kills) { self.first_kill_ms } else { None },
            confidence,
        }
    }

    pub fn on_death(&mut self, t_ms: u64, confidence: f32) -> Event {
        self.death_ms = Some(t_ms);
        Event::new(EventKind::Death, t_ms, confidence)
    }

    pub fn on_round_end(&mut self, t_ms: u64, won: bool, confidence: f32) -> Event {
        self.start_round();
        self.round_end_ms = Some(t_ms);
        Event::new(if won { EventKind::RoundWon } else { EventKind::RoundLost }, t_ms, confidence)
    }
}

/// Debounces the round banner: it has to be seen in consecutive samples and
/// reports once per round.
#[derive(Default)]
pub(crate) struct RoundBannerState {
    current: Option<bool>,
    since_ms: u64,
    run: u32,
    last_report_ms: Option<u64>,
}

impl RoundBannerState {
    pub fn clear(&mut self) {
        *self = RoundBannerState::default();
    }

    /// Returns `(first_seen_ms, won)` when a round end is confirmed.
    pub fn update(&mut self, t_ms: u64, outcome: Option<bool>, confirm_samples: u32, cooldown_s: f32) -> Option<(u64, bool)> {
        let Some(won) = outcome else {
            self.current = None;
            self.run = 0;
            return None;
        };
        if self.current == Some(won) {
            self.run += 1;
        } else {
            self.current = Some(won);
            self.since_ms = t_ms;
            self.run = 1;
        }
        if self.run != confirm_samples.max(1) {
            return None;
        }
        if let Some(last) = self.last_report_ms {
            if t_ms.saturating_sub(last) < ms(cooldown_s) {
                return None;
            }
        }
        self.last_report_ms = Some(t_ms);
        Some((self.since_ms, won))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracking() -> TrackingProfile {
        TrackingProfile { confirm_samples: 2, lost_after_s: 1.0, match_dx: 0.012, match_dy_up: 0.085, match_dy_down: 0.012 }
    }

    fn round() -> RoundProfile {
        RoundProfile { new_round_after_death_s: 25.0, idle_reset_s: 150.0 }
    }

    const PITCH: f32 = 0.0361;

    fn obs(kind: OwnRowKind, x: f32, slot: u32) -> FeedObservation {
        FeedObservation { kind, x, y: 0.02 + slot as f32 * PITCH, score: 1.0, anchored: false }
    }

    #[test]
    fn row_reports_once_after_confirmation() {
        let mut t = RowTracker::default();
        let p = tracking();
        let o = [obs(OwnRowKind::Kill, 0.4, 0)];
        assert!(t.update(1000, &o, &p).is_empty());
        let c = t.update(1100, &o, &p);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].first_ms, 1000);
        assert_eq!(c[0].kind, OwnRowKind::Kill);
        for i in 0..50 {
            assert!(t.update(1200 + i * 100, &o, &p).is_empty());
        }
    }

    #[test]
    fn single_sighting_never_reports() {
        let mut t = RowTracker::default();
        let p = tracking();
        assert!(t.update(1000, &[obs(OwnRowKind::Kill, 0.4, 0)], &p).is_empty());
        for i in 1..30 {
            assert!(t.update(1000 + i * 100, &[], &p).is_empty());
        }
        // The same place much later is a new row and needs two sightings again.
        assert!(t.update(9000, &[obs(OwnRowKind::Kill, 0.4, 0)], &p).is_empty());
    }

    #[test]
    fn row_moving_up_is_the_same_row() {
        let mut t = RowTracker::default();
        let p = tracking();
        t.update(1000, &[obs(OwnRowKind::Kill, 0.4, 2)], &p);
        assert_eq!(t.update(1100, &[obs(OwnRowKind::Kill, 0.4, 2)], &p).len(), 1);
        assert!(t.update(1200, &[obs(OwnRowKind::Kill, 0.4, 1)], &p).is_empty());
        assert!(t.update(1300, &[obs(OwnRowKind::Kill, 0.4, 0)], &p).is_empty());
        // Two slots at once (a skipped sample) is still the same row.
        let mut t = RowTracker::default();
        t.update(1000, &[obs(OwnRowKind::Kill, 0.4, 2)], &p);
        t.update(1100, &[obs(OwnRowKind::Kill, 0.4, 2)], &p);
        assert!(t.update(1300, &[obs(OwnRowKind::Kill, 0.4, 0)], &p).is_empty());
    }

    #[test]
    fn short_dropout_does_not_report_twice() {
        let mut t = RowTracker::default();
        let p = tracking();
        let o = [obs(OwnRowKind::Death, 0.02, 1)];
        t.update(1000, &o, &p);
        assert_eq!(t.update(1100, &o, &p).len(), 1);
        assert!(t.update(1200, &[], &p).is_empty());
        assert!(t.update(1700, &[], &p).is_empty());
        assert!(t.update(1900, &o, &p).is_empty());
        assert!(t.update(2000, &o, &p).is_empty());
    }

    #[test]
    fn two_rows_at_once_are_two_kills() {
        let mut t = RowTracker::default();
        let p = tracking();
        let two = [obs(OwnRowKind::Kill, 0.40, 0), obs(OwnRowKind::Kill, 0.40, 1)];
        t.update(1000, &two, &p);
        assert_eq!(t.update(1100, &two, &p).len(), 2);
        assert!(t.update(1200, &two, &p).is_empty());
    }

    #[test]
    fn second_kill_below_a_running_row_is_a_new_row() {
        let mut t = RowTracker::default();
        let p = tracking();
        let first = [obs(OwnRowKind::Kill, 0.40, 0)];
        t.update(1000, &first, &p);
        assert_eq!(t.update(1100, &first, &p).len(), 1);
        let both = [obs(OwnRowKind::Kill, 0.40, 0), obs(OwnRowKind::Kill, 0.33, 1)];
        for i in 2..=20 {
            assert!(t.update(1000 + i * 100, &first, &p).is_empty());
        }
        assert!(t.update(3100, &both, &p).is_empty());
        let c = t.update(3200, &both, &p);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].first_ms, 3100);
    }

    #[test]
    fn new_row_while_the_old_one_slides_up_counts_once_more() {
        // Old row moves from slot 1 to slot 0 in the same sample in which a new
        // row with the same cap position appears in slot 1.
        let mut t = RowTracker::default();
        let p = tracking();
        let old = [obs(OwnRowKind::Kill, 0.40, 1)];
        t.update(1000, &old, &p);
        assert_eq!(t.update(1100, &old, &p).len(), 1);
        let both = [obs(OwnRowKind::Kill, 0.40, 0), obs(OwnRowKind::Kill, 0.40, 1)];
        assert!(t.update(1200, &both, &p).is_empty());
        assert_eq!(t.update(1300, &both, &p).len(), 1);
        assert!(t.update(1400, &both, &p).is_empty());
    }

    #[test]
    fn kill_and_death_rows_are_tracked_apart() {
        let mut t = RowTracker::default();
        let p = tracking();
        let rows = [obs(OwnRowKind::Kill, 0.40, 0), obs(OwnRowKind::Death, 0.02, 1)];
        t.update(1000, &rows, &p);
        let c = t.update(1100, &rows, &p);
        assert_eq!(c.len(), 2);
        assert!(c.iter().any(|r| r.kind == OwnRowKind::Kill));
        assert!(c.iter().any(|r| r.kind == OwnRowKind::Death));
    }

    #[test]
    fn kills_in_one_round_count_up_to_ace() {
        let mut r = RoundLogic::new();
        let p = round();
        let kinds: Vec<EventKind> = (0..6).map(|i| r.on_kill(10_000 + i * 4000, 0.8, &p).kind).collect();
        assert_eq!(
            kinds,
            vec![
                EventKind::Kill,
                EventKind::DoubleKill,
                EventKind::TripleKill,
                EventKind::QuadraKill,
                EventKind::Ace,
                EventKind::Kill
            ]
        );
        assert_eq!(r.kills_this_round(), 6);
    }

    #[test]
    fn multi_kill_carries_the_time_of_the_first_kill() {
        let mut r = RoundLogic::new();
        let p = round();
        assert_eq!(r.on_kill(10_000, 0.8, &p).start_ms, None);
        let e = r.on_kill(31_000, 0.8, &p);
        assert_eq!(e.kind, EventKind::DoubleKill);
        assert_eq!(e.start_ms, Some(10_000));
        assert_eq!(e.t_ms, 31_000);
    }

    #[test]
    fn round_end_resets_the_counter() {
        let mut r = RoundLogic::new();
        let p = round();
        r.on_kill(10_000, 0.8, &p);
        r.on_kill(12_000, 0.8, &p);
        assert_eq!(r.on_round_end(20_000, true, 0.9).kind, EventKind::RoundWon);
        assert_eq!(r.phase(), RoundPhase::Idle);
        assert_eq!(r.on_kill(70_000, 0.8, &p).kind, EventKind::Kill);
        assert_eq!(r.on_round_end(90_000, false, 0.9).kind, EventKind::RoundLost);
    }

    #[test]
    fn kills_after_the_banner_do_not_leak_into_the_next_round() {
        let mut r = RoundLogic::new();
        let p = round();
        r.on_round_end(20_000, true, 0.9);
        // Kill in the seconds after the banner still belongs to the old round.
        assert_eq!(r.on_kill(22_000, 0.8, &p).kind, EventKind::Kill);
        assert_eq!(r.on_kill(24_000, 0.8, &p).kind, EventKind::DoubleKill);
        // First kill of the next round, more than 25 s after the banner.
        assert_eq!(r.on_kill(70_000, 0.8, &p).kind, EventKind::Kill);
        assert_eq!(r.on_kill(72_000, 0.8, &p).kind, EventKind::DoubleKill);
    }

    #[test]
    fn death_then_late_kill_means_a_new_round_even_without_banner() {
        let mut r = RoundLogic::new();
        let p = round();
        r.on_kill(10_000, 0.8, &p);
        assert_eq!(r.on_death(12_000, 0.8).kind, EventKind::Death);
        assert_eq!(r.phase(), RoundPhase::Dead);
        // A trade kill by a lingering ability shortly after the death still counts.
        assert_eq!(r.on_kill(14_000, 0.8, &p).kind, EventKind::DoubleKill);
        // 60 s later the player must have respawned: new round.
        let e = r.on_kill(74_000, 0.8, &p);
        assert_eq!(e.kind, EventKind::Kill);
        assert_eq!(r.phase(), RoundPhase::Alive);
    }

    #[test]
    fn long_silence_resets_the_counter() {
        let mut r = RoundLogic::new();
        let p = round();
        r.on_kill(10_000, 0.8, &p);
        assert_eq!(r.on_kill(10_000 + 151_000, 0.8, &p).kind, EventKind::Kill);
    }

    #[test]
    fn banner_needs_consecutive_samples_and_reports_once() {
        let mut b = RoundBannerState::default();
        assert_eq!(b.update(1000, Some(true), 2, 20.0), None);
        assert_eq!(b.update(1100, Some(true), 2, 20.0), Some((1000, true)));
        for i in 0..40 {
            assert_eq!(b.update(1200 + i * 100, Some(true), 2, 20.0), None);
        }
        // Hidden by the scoreboard, then visible again: still the same round.
        assert_eq!(b.update(6000, None, 2, 20.0), None);
        assert_eq!(b.update(6100, Some(true), 2, 20.0), None);
        assert_eq!(b.update(6200, Some(true), 2, 20.0), None);
        // Next round, lost.
        assert_eq!(b.update(90_000, Some(false), 2, 20.0), None);
        assert_eq!(b.update(90_100, Some(false), 2, 20.0), Some((90_000, false)));
    }

    #[test]
    fn banner_flicker_does_not_report() {
        let mut b = RoundBannerState::default();
        assert_eq!(b.update(1000, Some(true), 2, 20.0), None);
        assert_eq!(b.update(1100, None, 2, 20.0), None);
        assert_eq!(b.update(1200, Some(false), 2, 20.0), None);
        assert_eq!(b.update(1300, Some(true), 2, 20.0), None);
    }
}
