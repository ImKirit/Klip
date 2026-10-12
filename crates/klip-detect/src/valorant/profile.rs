//! Detection parameters for Valorant as data. A HUD patch or another colour mode
//! is answered with a new profile file, not with new code.

use serde::{Deserialize, Serialize};

use crate::frame::{HsvRange, RelRect};

/// The profile compiled into the crate.
pub const DEFAULT_PROFILE_JSON: &str = include_str!("../../profiles/valorant_default.json");

/// Every length is a fraction of the frame height unless stated otherwise.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ValorantProfile {
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub notes: String,
    /// Samples per second the detector is tuned for.
    pub sample_rate_hz: f32,
    /// Frames taller than this are scaled down to this height before analysis.
    pub work_ref_height: u32,
    /// Horizontal stretch of the HUD in the captured frame relative to the
    /// measured 16:9 layout. 1.0 means not stretched.
    pub hud_x_stretch: f32,
    pub colors: Colors,
    pub feed: FeedProfile,
    pub round_banner: RoundBannerProfile,
    pub kill_banner: KillBannerProfile,
    pub tracking: TrackingProfile,
    pub round: RoundProfile,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Colors {
    /// Yellow frame around the own player in the kill feed.
    pub own_highlight: HsvRange,
    /// Row background of allies.
    pub ally: HsvRange,
    /// Row background of enemies. Follows the colour blind setting.
    pub enemy: HsvRange,
    pub white_v_min: f32,
    pub white_s_max: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FeedProfile {
    pub roi: RelRect,
    pub row_height: f32,
    pub row_pitch: f32,
    /// Shortest and longest vertical yellow run that can belong to the end cap,
    /// as a fraction of the row height.
    pub cap_min_run: f32,
    pub cap_max_run: f32,
    /// Width limits of the solid part of the end cap.
    pub cap_min_width: f32,
    pub cap_max_width: f32,
    /// Smallest height of the whole cap as a fraction of the row height.
    pub cap_min_height: f32,
    /// Width of the portrait between the cap and the coloured segments.
    pub portrait_width: f32,
    /// Length of the stretch left of a death cap in which row colours are checked.
    pub row_span: f32,
    /// A kill row must be at least this long between cap and right row end.
    pub kill_min_span: f32,
    /// Share of ally or enemy coloured pixels the row stretch needs.
    pub row_min_fraction: f32,
    /// Where the right edge of the cap of an own death sits, measured from the
    /// right frame edge, and how far it may be off. Every row ends there, so a
    /// cap at this place means "own player is the victim", a cap anywhere else
    /// means "own player is the killer".
    pub death_cap_right_offset: f32,
    pub death_cap_right_tolerance: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RoundBannerProfile {
    pub roi: RelRect,
    /// Size of the white corner marks of the banner box.
    pub corner_width: f32,
    pub corner_height: f32,
    pub corner_white_min: f32,
    pub corners_required: u32,
    /// Allowed share of white pixels (the big word) inside the box.
    pub white_min: f32,
    pub white_max: f32,
    /// Share of the non white pixels that must carry the ally or enemy colour.
    pub color_min_fraction: f32,
    pub confirm_samples: u32,
    /// No second round end within this time.
    pub cooldown_s: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KillBannerProfile {
    pub roi: RelRect,
    /// Share of white pixels that counts as "banner visible".
    pub white_min_fraction: f32,
    /// A kill waits at most this long for the banner before it is reported.
    pub wait_s: f32,
    /// The banner may already be visible this long before the feed row.
    pub lookback_s: f32,
    pub confidence_bonus: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TrackingProfile {
    /// Sightings a row needs before it counts.
    pub confirm_samples: u32,
    /// A row not seen for this long is forgotten.
    pub lost_after_s: f32,
    /// How far a tracked row may move between sightings.
    pub match_dx: f32,
    pub match_dy_up: f32,
    pub match_dy_down: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RoundProfile {
    /// A kill this long after the own death must belong to a new round.
    pub new_round_after_death_s: f32,
    /// Without any own kill for this long the kill counter starts over.
    pub idle_reset_s: f32,
}

impl ValorantProfile {
    /// The embedded default profile (16:9, default colours).
    pub fn builtin() -> ValorantProfile {
        serde_json::from_str(DEFAULT_PROFILE_JSON).expect("embedded Valorant profile is valid")
    }

    pub fn from_json(text: &str) -> Result<ValorantProfile, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("profile serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_profile_parses_and_roundtrips() {
        let p = ValorantProfile::builtin();
        assert_eq!(p.version, 1);
        assert!(p.sample_rate_hz >= 5.0 && p.sample_rate_hz <= 10.0);
        assert_eq!(ValorantProfile::from_json(&p.to_json()).unwrap(), p);
    }

    #[test]
    fn measured_colours_fall_into_the_ranges() {
        use crate::frame::rgb_to_hsv;
        let p = ValorantProfile::builtin();
        let hit = |r: &HsvRange, rgb: [u8; 3]| {
            let (h, s, v) = rgb_to_hsv(rgb);
            r.contains(h, s, v)
        };
        assert!(hit(&p.colors.own_highlight, [231, 228, 112]));
        assert!(hit(&p.colors.ally, [94, 177, 164]));
        assert!(hit(&p.colors.ally, [87, 146, 135]));
        assert!(hit(&p.colors.enemy, [237, 104, 84]));
        assert!(hit(&p.colors.enemy, [176, 85, 71]));
        assert!(!hit(&p.colors.own_highlight, [94, 177, 164]));
        assert!(!hit(&p.colors.own_highlight, [255, 200, 0]));
        assert!(!hit(&p.colors.ally, [237, 104, 84]));
    }
}
