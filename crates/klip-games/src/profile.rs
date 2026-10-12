//! Per game profiles: optional overrides on top of the global settings.
//!
//! A profile only stores what differs. `resolve` lays it over the global `Config` from
//! `klip-core` and returns what the recorder should really use while that game runs.

use klip_core::{Config, HotkeyAction, HotkeyBinding, VideoCodec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What goes into one audio track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioTrackKind {
    /// The game itself (its process tree).
    Game,
    Microphone,
    /// The programs named in `processes`, for example Discord or Spotify.
    Programs,
    /// Everything that has no track of its own.
    Rest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrack {
    pub name: String,
    pub kind: AudioTrackKind,
    /// Exe names, only used for `Programs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<String>,
}

/// One automatic clip rule: which event, and how much before and after it is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoClipRule {
    /// Stable event id of a detector, the same ids as `ClipEvent::kind` ("kill", "ace").
    pub event: String,
    pub enabled: bool,
    pub seconds_before: u32,
    pub seconds_after: u32,
}

/// Overrides for one game. Every field is optional, absent means "as in the settings".
/// No `PartialEq`: `HotkeyBinding` from `klip-core` does not have it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GameProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<VideoCodec>,
    /// Output height. The value 0 means "source size", which the global setting spells
    /// as an absent height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Clip length: how many seconds the replay buffer holds for this game.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_seconds: Option<u32>,
    /// Replaces the whole hotkey list for this game.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkeys: Option<Vec<HotkeyBinding>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tracks: Option<Vec<AudioTrack>>,
    /// Id of an overlay layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_clip_rules: Option<Vec<AutoClipRule>>,
}

impl GameProfile {
    pub fn is_empty(&self) -> bool {
        self.fps.is_none()
            && self.bitrate_kbps.is_none()
            && self.codec.is_none()
            && self.height.is_none()
            && self.clip_seconds.is_none()
            && self.hotkeys.is_none()
            && self.audio_tracks.is_none()
            && self.overlay_layout.is_none()
            && self.auto_clip_rules.is_none()
    }
}

/// The settings that are not part of `Config` yet and therefore have their defaults here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileDefaults {
    pub audio_tracks: Vec<AudioTrack>,
    pub overlay_layout: Option<String>,
    pub auto_clip_rules: Vec<AutoClipRule>,
}

/// Default plus profile: what applies while a game runs.
#[derive(Debug, Clone)]
pub struct EffectiveSettings {
    /// The global settings with the overrides for quality, clip length and hotkeys applied.
    pub config: Config,
    pub audio_tracks: Vec<AudioTrack>,
    pub overlay_layout: Option<String>,
    pub auto_clip_rules: Vec<AutoClipRule>,
}

/// Lays a profile over the defaults. Without a profile the defaults come back unchanged.
pub fn resolve(base: &Config, defaults: &ProfileDefaults, profile: Option<&GameProfile>) -> EffectiveSettings {
    let mut config = base.clone();
    let empty = GameProfile::default();
    let p = profile.unwrap_or(&empty);

    if let Some(fps) = p.fps {
        config.fps = fps;
    }
    if let Some(bitrate) = p.bitrate_kbps {
        config.bitrate_kbps = bitrate;
    }
    if let Some(codec) = p.codec {
        config.codec = codec;
    }
    if let Some(height) = p.height {
        config.height = (height != 0).then_some(height);
    }
    if let Some(hotkeys) = &p.hotkeys {
        config.hotkeys = hotkeys.clone();
    }
    if let Some(seconds) = p.clip_seconds {
        config.buffer_seconds = seconds;
        // A clip key cannot save more than the buffer holds. Clamping here keeps the
        // effective settings honest instead of promising 60 seconds from a 30 second buffer.
        for binding in &mut config.hotkeys {
            if let HotkeyAction::Clip { seconds: clip } = &mut binding.action {
                *clip = (*clip).min(seconds);
            }
        }
    }

    EffectiveSettings {
        config,
        audio_tracks: p.audio_tracks.clone().unwrap_or_else(|| defaults.audio_tracks.clone()),
        overlay_layout: p.overlay_layout.clone().or_else(|| defaults.overlay_layout.clone()),
        auto_clip_rules: p.auto_clip_rules.clone().unwrap_or_else(|| defaults.auto_clip_rules.clone()),
    }
}

/// All profiles, keyed by game id (`InstalledGame::id`, `GameMatch::id`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileStore {
    pub defaults: ProfileDefaults,
    pub profiles: BTreeMap<String, GameProfile>,
}

impl ProfileStore {
    pub fn get(&self, game_id: &str) -> Option<&GameProfile> {
        self.profiles.get(game_id)
    }

    /// Stores a profile. An empty profile removes the entry, so the file stays small.
    pub fn set(&mut self, game_id: &str, profile: GameProfile) {
        if profile.is_empty() {
            self.profiles.remove(game_id);
        } else {
            self.profiles.insert(game_id.to_string(), profile);
        }
    }

    /// Effective settings for a game, or the plain defaults for `None` (no game running).
    pub fn resolve(&self, base: &Config, game_id: Option<&str>) -> EffectiveSettings {
        resolve(base, &self.defaults, game_id.and_then(|id| self.get(id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_profile_means_defaults() {
        let base = Config::default();
        let e = resolve(&base, &ProfileDefaults::default(), None);
        assert_eq!(e.config.fps, base.fps);
        assert_eq!(e.config.buffer_seconds, base.buffer_seconds);
        assert_eq!(e.config.hotkeys.len(), base.hotkeys.len());
        assert!(e.audio_tracks.is_empty());
    }

    #[test]
    fn overrides_apply_and_the_rest_stays() {
        let base = Config::default();
        let profile = GameProfile {
            fps: Some(120),
            codec: Some(VideoCodec::Av1),
            height: Some(1080),
            clip_seconds: Some(30),
            overlay_layout: Some("minimal".into()),
            ..Default::default()
        };
        let e = resolve(&base, &ProfileDefaults::default(), Some(&profile));
        assert_eq!(e.config.fps, 120);
        assert_eq!(e.config.codec, VideoCodec::Av1);
        assert_eq!(e.config.height, Some(1080));
        assert_eq!(e.config.bitrate_kbps, base.bitrate_kbps);
        assert_eq!(e.config.buffer_seconds, 30);
        assert_eq!(e.overlay_layout.as_deref(), Some("minimal"));
        // The default 60 second clip key is cut down to what the buffer holds.
        for binding in &e.config.hotkeys {
            if let HotkeyAction::Clip { seconds } = binding.action {
                assert!(seconds <= 30);
            }
        }
    }

    #[test]
    fn height_zero_means_source_size() {
        let base = Config { height: Some(720), ..Config::default() };
        let profile = GameProfile { height: Some(0), ..Default::default() };
        assert_eq!(resolve(&base, &ProfileDefaults::default(), Some(&profile)).config.height, None);
        assert_eq!(resolve(&base, &ProfileDefaults::default(), None).config.height, Some(720));
    }

    #[test]
    fn store_round_trips_and_drops_empty_profiles() {
        let mut store = ProfileStore::default();
        store.defaults.auto_clip_rules.push(AutoClipRule { event: "kill".into(), enabled: true, seconds_before: 8, seconds_after: 4 });
        store.set("riot:valorant.live", GameProfile { bitrate_kbps: Some(30_000), auto_clip_rules: Some(Vec::new()), ..Default::default() });
        store.set("steam:400", GameProfile::default());
        assert!(store.get("steam:400").is_none());

        let json = serde_json::to_string_pretty(&store).unwrap();
        // Absent overrides are not written at all.
        assert!(!json.contains("\"fps\""));
        let back: ProfileStore = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string_pretty(&back).unwrap(), json);

        let base = Config::default();
        let valorant = back.resolve(&base, Some("riot:valorant.live"));
        assert_eq!(valorant.config.bitrate_kbps, 30_000);
        assert!(valorant.auto_clip_rules.is_empty(), "the profile switches the default rules off");
        assert_eq!(back.resolve(&base, Some("steam:400")).auto_clip_rules.len(), 1);
        assert_eq!(back.resolve(&base, None).auto_clip_rules.len(), 1);
    }
}
