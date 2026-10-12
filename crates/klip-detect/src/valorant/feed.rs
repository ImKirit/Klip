//! Image analysis for Valorant: kill feed rows of the own player, the round end
//! banner and the kill banner. Colour and shape only, no text is read.

use crate::frame::{rgb_to_hsv, RgbImage};

use super::profile::{Colors, FeedProfile, RoundBannerProfile};

pub(crate) const C_OTHER: u8 = 0;
pub(crate) const C_OWN: u8 = 1;
pub(crate) const C_ALLY: u8 = 2;
pub(crate) const C_ENEMY: u8 = 3;
pub(crate) const C_WHITE: u8 = 4;

fn class_of(rgb: [u8; 3], colors: &Colors) -> u8 {
    let (h, s, v) = rgb_to_hsv(rgb);
    if v >= colors.white_v_min && s <= colors.white_s_max {
        C_WHITE
    } else if colors.own_highlight.contains(h, s, v) {
        C_OWN
    } else if colors.ally.contains(h, s, v) {
        C_ALLY
    } else if colors.enemy.contains(h, s, v) {
        C_ENEMY
    } else {
        C_OTHER
    }
}

/// Colour class for every RGB value at 5 bit per channel (32768 entries). Built
/// once per profile so the per pixel work is a table lookup.
pub(crate) fn build_class_table(colors: &Colors) -> Vec<u8> {
    let mut table = Vec::with_capacity(1 << 15);
    for i in 0..(1u32 << 15) {
        let centre = |v: u32| ((v << 3) | 4) as u8;
        table.push(class_of([centre(i >> 10), centre((i >> 5) & 31), centre(i & 31)], colors));
    }
    table
}

/// Assigns every pixel one colour class.
pub(crate) fn classify(img: &RgbImage, table: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(img.width * img.height);
    out.extend(img.data.chunks_exact(3).map(|p| {
        table[((p[0] as usize >> 3) << 10) | ((p[1] as usize >> 3) << 5) | (p[2] as usize >> 3)]
    }));
}

/// Role of the own player in a kill feed row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnRowKind {
    Kill,
    Death,
}

/// One highlighted row seen in one sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedObservation {
    pub kind: OwnRowKind,
    /// Centre of the yellow end cap, measured from the right edge of the region,
    /// in frame heights.
    pub x: f32,
    /// Vertical centre of the cap from the top of the region, in frame heights.
    pub y: f32,
    /// 0 to 1, how clearly the row colours were found next to the cap.
    pub score: f32,
    /// The cap sits at the right end of the rows (always true for a death).
    pub anchored: bool,
}

struct Block {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    last_y0: usize,
    last_y1: usize,
}

fn fraction(cls: &[u8], w: usize, h: usize, x0: f32, x1: f32, y0: f32, y1: f32, class: u8) -> f32 {
    let wanted = (x1 - x0).max(1.0);
    let xa = x0.round().max(0.0) as usize;
    let xb = (x1.round().max(0.0) as usize).min(w);
    let ya = y0.round().max(0.0) as usize;
    let yb = (y1.round().max(0.0) as usize).min(h);
    if xb <= xa || yb <= ya || ((xb - xa) as f32) < wanted * 0.5 {
        return 0.0;
    }
    let mut hit = 0usize;
    for y in ya..yb {
        hit += cls[y * w + xa..y * w + xb].iter().filter(|&&c| c == class).count();
    }
    hit as f32 / ((xb - xa) * (yb - ya)) as f32
}

/// Finds rows in which the own player is highlighted. `px_per_h` is the number of
/// working pixels per frame height.
///
/// Signal: the yellow end cap of the own player's block. It is a solid yellow
/// patch one row high and a few pixels wide. Every row ends at the same place on
/// the right. A cap at that place closes a row, the own player is the victim. A
/// cap anywhere else opens a row, the own player is the killer. In both cases
/// the stretch between the portraits must carry the ally and enemy row colours,
/// which rules out yellow effects of the game world.
pub(crate) fn find_own_rows(cls: &[u8], w: usize, h: usize, px_per_h: f32, feed: &FeedProfile) -> Vec<FeedObservation> {
    let mut found: Vec<FeedObservation> = Vec::new();
    if w == 0 || h == 0 || cls.len() < w * h {
        return found;
    }
    let s = px_per_h;
    let rh = feed.row_height * s;
    let min_run = (feed.cap_min_run * rh).max(2.0);
    let max_run = feed.cap_max_run * rh;
    let min_w = (feed.cap_min_width * s).max(2.0);
    let max_w = feed.cap_max_width * s;

    let mut open: Vec<Block> = Vec::new();
    let mut done: Vec<Block> = Vec::new();
    for x in 0..w {
        let mut y = 0;
        while y < h {
            if cls[y * w + x] != C_OWN {
                y += 1;
                continue;
            }
            let y0 = y;
            while y < h && cls[y * w + x] == C_OWN {
                y += 1;
            }
            let y1 = y; // exclusive
            let len = (y1 - y0) as f32;
            if len < min_run || len > max_run {
                continue;
            }
            let joined = open.iter_mut().find(|b| {
                if b.x1 + 1 != x {
                    return false;
                }
                let overlap = y1.min(b.last_y1) as i64 - y0.max(b.last_y0) as i64;
                let shorter = (y1 - y0).min(b.last_y1 - b.last_y0) as i64;
                overlap * 2 >= shorter
            });
            match joined {
                Some(b) => {
                    b.x1 = x;
                    b.y0 = b.y0.min(y0);
                    b.y1 = b.y1.max(y1);
                    b.last_y0 = y0;
                    b.last_y1 = y1;
                }
                None => open.push(Block { x0: x, x1: x, y0, y1, last_y0: y0, last_y1: y1 }),
            }
        }
        let mut i = 0;
        while i < open.len() {
            if open[i].x1 < x {
                done.push(open.swap_remove(i));
            } else {
                i += 1;
            }
        }
    }
    done.append(&mut open);

    let pw = feed.portrait_width * s;
    let row_end = w as f32 - feed.death_cap_right_offset * s;
    for b in done {
        let width = (b.x1 - b.x0 + 1) as f32;
        let height = (b.y1 - b.y0) as f32;
        if width < min_w || width > max_w || height > max_run || height < feed.cap_min_height * rh {
            continue;
        }
        let cy = (b.y0 + b.y1) as f32 / 2.0;
        let (ya, yb) = (cy - 0.3 * rh, cy + 0.3 * rh);
        let right_gap = (w - 1 - b.x1) as f32 / s;
        let anchored = (right_gap - feed.death_cap_right_offset).abs() <= feed.death_cap_right_tolerance;
        // The stretch of the row between the portraits, seen from the cap.
        let (kind, xa, xb) = if anchored {
            let end = b.x0 as f32 - pw;
            (OwnRowKind::Death, end - feed.row_span * s, end)
        } else {
            let start = b.x1 as f32 + 1.0 + pw;
            let end = row_end - pw;
            if end - start < feed.kill_min_span * s {
                continue;
            }
            (OwnRowKind::Kill, start, end)
        };
        let frac = fraction(cls, w, h, xa, xb, ya, yb, C_ALLY) + fraction(cls, w, h, xa, xb, ya, yb, C_ENEMY);
        if frac < feed.row_min_fraction {
            continue;
        }
        let span = (0.8 - feed.row_min_fraction).max(0.05);
        let obs = FeedObservation {
            kind,
            x: (w as f32 - (b.x0 + b.x1 + 1) as f32 / 2.0) / s,
            y: cy / s,
            score: ((frac - feed.row_min_fraction) / span).clamp(0.0, 1.0),
            anchored,
        };
        // One cap can fall apart into two blocks. Keep one observation per row end.
        let twin = found.iter_mut().find(|o| {
            o.kind == obs.kind && (o.x - obs.x).abs() < feed.cap_max_width && (o.y - obs.y).abs() < feed.row_height * 0.5
        });
        match twin {
            Some(o) => {
                if obs.score > o.score {
                    *o = obs;
                }
            }
            None => found.push(obs),
        }
    }
    found.sort_by(|a, b| a.y.total_cmp(&b.y));
    found
}

/// What the round banner region looks like in one sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RoundBannerFeatures {
    pub white: f32,
    /// Share of the non white pixels in the ally and in the enemy colour.
    pub ally: f32,
    pub enemy: f32,
    /// Number of the four corner marks found.
    pub corners: u32,
}

impl RoundBannerFeatures {
    /// `Some(true)` for a won round, `Some(false)` for a lost round.
    pub(crate) fn outcome(&self, p: &RoundBannerProfile) -> Option<bool> {
        if self.corners < p.corners_required || self.white < p.white_min || self.white > p.white_max {
            return None;
        }
        if self.ally >= p.color_min_fraction && self.ally > self.enemy {
            Some(true)
        } else if self.enemy >= p.color_min_fraction && self.enemy > self.ally {
            Some(false)
        } else {
            None
        }
    }
}

/// Measures the round banner box: a box in the ally colour (won) or the enemy
/// colour (lost) with a large white word and four white corner marks.
pub(crate) fn round_banner_features(cls: &[u8], w: usize, h: usize, px_per_h: f32, p: &RoundBannerProfile) -> RoundBannerFeatures {
    if w == 0 || h == 0 || cls.len() < w * h {
        return RoundBannerFeatures::default();
    }
    let mut white = 0usize;
    let mut ally = 0usize;
    let mut enemy = 0usize;
    for &c in &cls[..w * h] {
        match c {
            C_WHITE => white += 1,
            C_ALLY => ally += 1,
            C_ENEMY => enemy += 1,
            _ => {}
        }
    }
    let total = w * h;
    let coloured = (total - white).max(1) as f32;
    let cw = (p.corner_width * px_per_h).max(2.0);
    let ch = (p.corner_height * px_per_h).max(1.0);
    let (wf, hf) = (w as f32, h as f32);
    let corners = [
        fraction(cls, w, h, 0.0, cw, 0.0, ch, C_WHITE),
        fraction(cls, w, h, wf - cw, wf, 0.0, ch, C_WHITE),
        fraction(cls, w, h, 0.0, cw, hf - ch, hf, C_WHITE),
        fraction(cls, w, h, wf - cw, wf, hf - ch, hf, C_WHITE),
    ]
    .iter()
    .filter(|&&f| f >= p.corner_white_min)
    .count() as u32;
    RoundBannerFeatures {
        white: white as f32 / total as f32,
        ally: ally as f32 / coloured,
        enemy: enemy as f32 / coloured,
        corners,
    }
}

/// Share of white pixels, used for the kill banner region.
pub(crate) fn white_fraction(cls: &[u8]) -> f32 {
    if cls.is_empty() {
        return 0.0;
    }
    cls.iter().filter(|&&c| c == C_WHITE).count() as f32 / cls.len() as f32
}
