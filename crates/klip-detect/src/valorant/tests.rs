//! Synthetic end to end tests: frames are painted with the geometry and colours
//! measured on real captures, then run through the detector.

use super::*;
use crate::frame::PixelRect;

const YELLOW: [u8; 3] = [231, 228, 112];
const TEAL: [u8; 3] = [94, 177, 164];
const RED: [u8; 3] = [237, 104, 84];
const PORTRAIT: [u8; 3] = [70, 40, 90];
const WHITE: [u8; 3] = [245, 245, 245];
const BACKGROUND: [u8; 3] = [60, 62, 70];

struct Canvas {
    w: u32,
    h: u32,
    /// Scale against the measured 1080p layout.
    k: f32,
    data: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Canvas {
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            data.extend_from_slice(&[BACKGROUND[2], BACKGROUND[1], BACKGROUND[0], 255]);
        }
        Canvas { w, h, k: h as f32 / 1080.0, data }
    }

    /// Fills a rectangle given in 1080p layout coordinates, where x is measured
    /// from the right frame edge of a 1920 wide frame.
    fn fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, rgb: [u8; 3]) {
        let fx = |x: i32| -> i32 { (self.w as f32 - (1920 - x) as f32 * self.k).round() as i32 };
        let fy = |y: i32| -> i32 { (y as f32 * self.k).round() as i32 };
        self.fill_px(fx(x0), fy(y0), fx(x1), fy(y1), rgb);
    }

    /// Same, but x is measured from the horizontal centre.
    fn fill_centered(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, rgb: [u8; 3]) {
        let fx = |x: i32| -> i32 { (self.w as f32 / 2.0 + (x - 960) as f32 * self.k).round() as i32 };
        let fy = |y: i32| -> i32 { (y as f32 * self.k).round() as i32 };
        self.fill_px(fx(x0), fy(y0), fx(x1), fy(y1), rgb);
    }

    fn fill_px(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, rgb: [u8; 3]) {
        for y in y0.max(0)..y1.min(self.h as i32) {
            for x in x0.max(0)..x1.min(self.w as i32) {
                let i = ((y as u32 * self.w + x as u32) * 4) as usize;
                self.data[i] = rgb[2];
                self.data[i + 1] = rgb[1];
                self.data[i + 2] = rgb[0];
            }
        }
    }

    fn row_top(slot: i32) -> i32 {
        96 + 39 * slot
    }

    /// Row of two other players, no highlight.
    fn other_row(&mut self, slot: i32, left: i32, ally_kills: bool) {
        let y = Canvas::row_top(slot);
        let (a, b) = if ally_kills { (TEAL, RED) } else { (RED, TEAL) };
        self.fill(left, y, left + 55, y + 34, PORTRAIT);
        self.fill(left + 55, y, 1720, y + 34, a);
        self.fill(1720, y, 1845, y + 34, b);
        self.fill(1845, y, 1900, y + 34, PORTRAIT);
    }

    /// Own kill: the row starts with the yellow cap, then portrait and ally segment.
    fn kill_row(&mut self, slot: i32, left: i32) {
        let y = Canvas::row_top(slot);
        self.fill(left - 6, y, left + 10, y + 34, YELLOW);
        self.fill(left + 10, y, left + 92, y + 2, YELLOW);
        self.fill(left + 10, y + 32, left + 92, y + 34, YELLOW);
        self.fill(left + 10, y + 2, left + 62, y + 32, PORTRAIT);
        self.fill(left + 62, y + 2, left + 92, y + 32, TEAL);
        self.fill(left + 92, y, 1720, y + 34, TEAL);
        // Name in white on the ally segment.
        self.fill(left + 70, y + 12, left + 130, y + 22, WHITE);
        self.fill(1720, y, 1845, y + 34, RED);
        self.fill(1845, y, 1900, y + 34, PORTRAIT);
    }

    /// Own death: the row ends with portrait and yellow cap.
    fn death_row(&mut self, slot: i32, left: i32) {
        let y = Canvas::row_top(slot);
        self.fill(left, y, left + 55, y + 34, PORTRAIT);
        self.fill(left + 55, y, 1770, y + 34, RED);
        self.fill(1770, y, 1840, y + 34, TEAL);
        self.fill(1780, y + 12, 1830, y + 22, WHITE);
        self.fill(1840, y, 1890, y + 34, PORTRAIT);
        self.fill(1840, y, 1890, y + 2, YELLOW);
        self.fill(1840, y + 32, 1890, y + 34, YELLOW);
        self.fill(1890, y, 1900, y + 34, YELLOW);
    }

    fn round_banner(&mut self, won: bool) {
        let colour = if won { [75, 110, 100] } else { [115, 55, 50] };
        self.fill_centered(832, 135, 1088, 277, colour);
        self.fill_centered(900, 170, 1020, 225, WHITE);
        for (x, y) in [(832, 135), (1078, 135), (832, 273), (1078, 273)] {
            self.fill_centered(x, y, x + 10, y + 4, WHITE);
        }
    }

    fn kill_banner(&mut self) {
        self.fill_centered(930, 830, 990, 890, WHITE);
    }
}

fn feed(det: &mut ValorantDetector, canvas: &Canvas, t_ms: u64) -> Vec<Event> {
    let rects: Vec<(u32, PixelRect)> = det.regions().iter().map(|r| (r.id, r.pixel_rect(canvas.w, canvas.h))).collect();
    let regions: Vec<RegionImage<'_>> = rects
        .iter()
        .map(|(id, rect)| RegionImage::from_frame(*id, &canvas.data, canvas.w, canvas.h, *rect).unwrap())
        .collect();
    det.process(&Sample { t_ms, frame_width: canvas.w, frame_height: canvas.h, regions: &regions })
}

/// Feeds the same frame every 100 ms.
fn hold(det: &mut ValorantDetector, canvas: &Canvas, from_ms: u64, to_ms: u64) -> Vec<Event> {
    let mut out = Vec::new();
    let mut t = from_ms;
    while t < to_ms {
        out.extend(feed(det, canvas, t));
        t += 100;
    }
    out
}

fn kinds(events: &[Event]) -> Vec<EventKind> {
    events.iter().map(|e| e.kind).collect()
}

#[test]
fn empty_frame_reports_nothing() {
    let mut det = ValorantDetector::new();
    let c = Canvas::new(1920, 1080);
    assert!(hold(&mut det, &c, 0, 3000).is_empty());
    assert_eq!(det.round_phase(), RoundPhase::Idle);
}

#[test]
fn rows_of_other_players_report_nothing() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.other_row(0, 1400, true);
    c.other_row(1, 1480, false);
    c.other_row(2, 1300, true);
    assert!(hold(&mut det, &c, 0, 3000).is_empty());
    assert!(det.diagnostics().feed.is_empty());
}

#[test]
fn own_kill_reports_once_at_first_sighting() {
    let mut det = ValorantDetector::new();
    let empty = Canvas::new(1920, 1080);
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    let mut events = hold(&mut det, &empty, 0, 1000);
    events.extend(hold(&mut det, &c, 1000, 6000));
    events.extend(hold(&mut det, &empty, 6000, 8000));
    assert_eq!(kinds(&events), vec![EventKind::Kill]);
    assert_eq!(events[0].t_ms, 1000);
    assert!(events[0].confidence >= 0.6 && events[0].confidence < 0.85);
    assert_eq!(det.kills_this_round(), 1);
}

#[test]
fn kill_banner_raises_the_confidence_and_releases_early() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    assert!(feed(&mut det, &c, 1000).is_empty());
    assert!(feed(&mut det, &c, 1100).is_empty());
    c.kill_banner();
    let events = feed(&mut det, &c, 1200);
    assert_eq!(kinds(&events), vec![EventKind::Kill]);
    assert!(events[0].confidence >= 0.8);
    assert!(det.diagnostics().kill_banner_white > 0.1);
}

#[test]
fn kill_without_banner_is_released_by_poll() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    assert!(feed(&mut det, &c, 1000).is_empty());
    assert!(feed(&mut det, &c, 1100).is_empty());
    assert!(det.poll(1500).is_empty());
    assert_eq!(kinds(&det.poll(1900)), vec![EventKind::Kill]);
    assert!(det.poll(2500).is_empty());
}

#[test]
fn own_death_reports_once() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.other_row(0, 1400, true);
    c.death_row(1, 1520);
    let events = hold(&mut det, &c, 2000, 7000);
    assert_eq!(kinds(&events), vec![EventKind::Death]);
    assert_eq!(events[0].t_ms, 2000);
    assert!(events[0].confidence >= 0.9, "anchored death, got {}", events[0].confidence);
    assert_eq!(det.round_phase(), RoundPhase::Dead);
}

#[test]
fn three_kills_in_a_round_become_a_triple_kill() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    let mut events = hold(&mut det, &c, 1000, 3000);
    c.kill_row(1, 1430);
    events.extend(hold(&mut det, &c, 3000, 5000));
    c.kill_row(2, 1530);
    events.extend(hold(&mut det, &c, 5000, 8000));
    assert_eq!(kinds(&events), vec![EventKind::Kill, EventKind::DoubleKill, EventKind::TripleKill]);
    assert_eq!(events[2].t_ms, 5000);
    assert_eq!(events[2].start_ms, Some(1000));
}

#[test]
fn rows_sliding_up_are_not_counted_again() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.other_row(0, 1400, false);
    c.kill_row(1, 1492);
    let mut events = hold(&mut det, &c, 1000, 3000);
    // The top row expires, the own row moves from slot 1 to slot 0.
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    events.extend(hold(&mut det, &c, 3000, 6000));
    assert_eq!(kinds(&events), vec![EventKind::Kill]);
}

#[test]
fn kill_then_death_keeps_the_order() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    let mut events = hold(&mut det, &c, 1000, 1300);
    c.death_row(1, 1520);
    events.extend(hold(&mut det, &c, 1300, 4000));
    assert_eq!(kinds(&events), vec![EventKind::Kill, EventKind::Death]);
    assert!(events[0].t_ms < events[1].t_ms);
}

#[test]
fn round_banner_reports_and_resets_the_kill_counter() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    let mut events = hold(&mut det, &c, 1000, 2500);
    c.round_banner(true);
    events.extend(hold(&mut det, &c, 2500, 6000));
    assert_eq!(kinds(&events), vec![EventKind::Kill, EventKind::RoundWon]);
    assert_eq!(events[1].t_ms, 2500);
    assert_eq!(det.kills_this_round(), 0);

    // Next round: the first kill is a single kill again.
    let empty = Canvas::new(1920, 1080);
    hold(&mut det, &empty, 6000, 8000);
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1450);
    let mut events = hold(&mut det, &c, 60_000, 62_000);
    c.round_banner(false);
    events.extend(hold(&mut det, &c, 62_000, 64_000));
    assert_eq!(kinds(&events), vec![EventKind::Kill, EventKind::RoundLost]);
}

#[test]
fn kill_seen_together_with_the_banner_counts_for_the_old_round() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    let mut events = hold(&mut det, &c, 1000, 3000);
    // Second kill and banner arrive within the banner wait time.
    c.kill_row(1, 1430);
    events.extend(hold(&mut det, &c, 3000, 3200));
    c.round_banner(true);
    events.extend(hold(&mut det, &c, 3200, 5000));
    assert_eq!(kinds(&events), vec![EventKind::Kill, EventKind::DoubleKill, EventKind::RoundWon]);
}

#[test]
fn large_yellow_area_is_not_a_row() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    // Ability effect: yellow over a big part of the feed area, next to teal.
    c.fill(1300, 90, 1500, 300, YELLOW);
    c.fill(1500, 90, 1900, 300, TEAL);
    assert!(hold(&mut det, &c, 0, 3000).is_empty());
    // A yellow bar as high as a row but far too wide.
    let mut c = Canvas::new(1920, 1080);
    c.fill(1400, 96, 1500, 130, YELLOW);
    c.fill(1500, 96, 1900, 130, TEAL);
    assert!(hold(&mut det, &c, 3000, 6000).is_empty());
}

#[test]
fn yellow_cap_without_ally_colour_is_not_a_row() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.fill(1486, 96, 1502, 130, YELLOW);
    assert!(hold(&mut det, &c, 0, 3000).is_empty());
}

#[test]
fn other_resolutions_use_the_same_relative_layout() {
    for (w, h) in [(1280u32, 720u32), (2560, 1440), (1600, 900)] {
        let mut det = ValorantDetector::new();
        let mut c = Canvas::new(w, h);
        c.other_row(0, 1400, true);
        c.kill_row(1, 1492);
        let mut events = hold(&mut det, &c, 1000, 1400);
        c.death_row(2, 1520);
        events.extend(hold(&mut det, &c, 1400, 1800));
        c.round_banner(false);
        events.extend(hold(&mut det, &c, 1800, 2200));
        assert_eq!(
            kinds(&events),
            vec![EventKind::Kill, EventKind::Death, EventKind::RoundLost],
            "frame {w}x{h}"
        );
    }
}

#[test]
fn right_anchored_layout_works_on_a_4_to_3_frame() {
    // Assumption, not measured: a 4:3 frame keeps the HUD at height scale and
    // glued to the right edge.
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1440, 1080);
    c.kill_row(0, 1492);
    let events = hold(&mut det, &c, 1000, 3000);
    assert_eq!(kinds(&events), vec![EventKind::Kill]);
}

#[test]
fn missing_or_broken_regions_are_skipped() {
    let mut det = ValorantDetector::new();
    assert!(det.process(&Sample { t_ms: 0, frame_width: 1920, frame_height: 1080, regions: &[] }).is_empty());
    let short = [0u8; 16];
    let broken = RegionImage { id: REGION_FEED, data: &short, width: 100, height: 100, stride: 400 };
    assert!(det.process(&Sample { t_ms: 100, frame_width: 1920, frame_height: 1080, regions: &[broken] }).is_empty());
    assert!(det.process(&Sample { t_ms: 200, frame_width: 0, frame_height: 0, regions: &[broken] }).is_empty());
}

#[test]
fn reset_forgets_the_round() {
    let mut det = ValorantDetector::new();
    let mut c = Canvas::new(1920, 1080);
    c.kill_row(0, 1492);
    hold(&mut det, &c, 1000, 3000);
    assert_eq!(det.kills_this_round(), 1);
    det.reset();
    assert_eq!(det.kills_this_round(), 0);
    // The row is still on screen: after a reset it is seen as new.
    assert_eq!(kinds(&hold(&mut det, &c, 3000, 5000)), vec![EventKind::Kill]);
}

#[test]
fn detector_declares_what_it_can_and_cannot_report() {
    let det = ValorantDetector::new();
    let s = det.supported_events();
    assert!(s.contains(&EventKind::Kill) && s.contains(&EventKind::Ace) && s.contains(&EventKind::Death));
    for missing in [EventKind::Clutch, EventKind::SpikePlanted, EventKind::SpikeDefused, EventKind::MatchWon, EventKind::MatchLost] {
        assert!(!s.contains(&missing));
    }
    assert_eq!(det.regions().len(), 3);
    assert_eq!(det.source(), SourceKind::Frames);
}
