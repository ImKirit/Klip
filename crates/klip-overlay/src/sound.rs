//! The short sounds that go with a notice. The built in ones come from Kenney's
//! "Interface Sounds" (CC0, see `assets/sounds/SOURCES.md`) and are part of the program.
//! A WAV file with the name of a cue in the sound folder replaces the built in one.

use std::path::PathBuf;
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FLAGS, SND_MEMORY, SND_NODEFAULT};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundConfig {
    pub enabled: bool,
    /// 0.0 to 1.0.
    pub volume: f32,
}

impl Default for SoundConfig {
    fn default() -> Self {
        SoundConfig { enabled: true, volume: 0.5 }
    }
}

/// Which sound a notice plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Cue {
    /// A clip was saved by hand: two notes, rising.
    #[default]
    Clip,
    /// A clip was saved by itself: three notes, rising.
    AutoClip,
    /// A recording started: one note that slides up.
    RecordStart,
    /// A recording was saved: two notes, falling.
    RecordStop,
    /// A screenshot: a shutter.
    Screenshot,
    /// Everything small, a marker or a switch: one short note.
    Tick,
    /// No sound.
    Silent,
}

impl Cue {
    pub const ALL: [Cue; 7] =
        [Cue::Clip, Cue::AutoClip, Cue::RecordStart, Cue::RecordStop, Cue::Screenshot, Cue::Tick, Cue::Silent];

    /// File name without extension, in `assets/sounds` and in the sound folder.
    pub fn name(self) -> &'static str {
        match self {
            Cue::Clip => "clip",
            Cue::AutoClip => "auto",
            Cue::RecordStart => "rec_start",
            Cue::RecordStop => "rec_stop",
            Cue::Screenshot => "shot",
            Cue::Tick => "tick",
            Cue::Silent => "",
        }
    }

    /// The built in WAV file: 16 bit mono, 44.1 kHz, a plain 44 byte header.
    fn file(self) -> &'static [u8] {
        match self {
            Cue::Clip => include_bytes!("../../../assets/sounds/clip.wav"),
            Cue::AutoClip => include_bytes!("../../../assets/sounds/auto.wav"),
            Cue::RecordStart => include_bytes!("../../../assets/sounds/rec_start.wav"),
            Cue::RecordStop => include_bytes!("../../../assets/sounds/rec_stop.wav"),
            Cue::Screenshot => include_bytes!("../../../assets/sounds/shot.wav"),
            Cue::Tick => include_bytes!("../../../assets/sounds/tick.wav"),
            Cue::Silent => &[],
        }
    }
}

/// Length of the header of the built in files.
#[cfg(test)]
const HEADER: usize = 44;

/// Folder with the user's own sounds, set once by the program that owns the overlay.
static SOUND_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sounds larger than this are ignored, a notice is no place for a song.
const MAX_OWN_BYTES: u64 = 2 * 1024 * 1024;

/// Names the folder in which `clip.wav`, `auto.wav`, `rec_start.wav`, `rec_stop.wav`,
/// `shot.wav` and `tick.wav` replace the built in sounds.
pub fn set_sound_dir(dir: Option<PathBuf>) {
    *SOUND_DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// The user's own file for `cue`, if there is a usable one.
fn own_file(cue: Cue) -> Option<Vec<u8>> {
    let dir = SOUND_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let path = dir.join(format!("{}.wav", cue.name()));
    if std::fs::metadata(&path).ok()?.len() > MAX_OWN_BYTES {
        return None;
    }
    let wav = std::fs::read(path).ok()?;
    (wav.len() > 12 && &wav[0..4] == b"RIFF" && &wav[8..12] == b"WAVE").then_some(wav)
}

/// Where the samples of a 16 bit PCM file start and end. `None` for every other format,
/// which is then played as it is, without the volume setting.
fn pcm16_data(wav: &[u8]) -> Option<(usize, usize)> {
    let mut at = 12;
    let mut pcm16 = false;
    while at + 8 <= wav.len() {
        let size = u32::from_le_bytes(wav[at + 4..at + 8].try_into().ok()?) as usize;
        let body = at + 8;
        match &wav[at..at + 4] {
            b"fmt " if body + 16 <= wav.len() => {
                let format = u16::from_le_bytes([wav[body], wav[body + 1]]);
                let bits = u16::from_le_bytes([wav[body + 14], wav[body + 15]]);
                pcm16 = format == 1 && bits == 16;
            }
            b"data" => return pcm16.then(|| (body, (body + size).min(wav.len()))),
            _ => {}
        }
        at = body + size + (size & 1);
    }
    None
}

/// A complete WAV file for `cue` at `volume` (0.0 to 1.0). The files peak at half of full
/// scale, so even at 1.0 the sound stays well below the level of a game. Empty for
/// [`Cue::Silent`].
pub fn cue_wav(cue: Cue, volume: f32) -> Vec<u8> {
    let volume = if volume.is_nan() { 0.0 } else { volume.clamp(0.0, 1.0) };
    if cue == Cue::Silent {
        return Vec::new();
    }
    let mut wav = own_file(cue).unwrap_or_else(|| cue.file().to_vec());
    let Some((from, to)) = pcm16_data(&wav) else { return wav };
    for sample in wav[from..to].chunks_exact_mut(2) {
        let v = i16::from_le_bytes([sample[0], sample[1]]) as f32 * volume;
        sample.copy_from_slice(&(v as i16).to_le_bytes());
    }
    wav
}

/// The sound for a saved clip. Kept for callers that only know this one.
pub fn clip_sound_wav(volume: f32) -> Vec<u8> {
    cue_wav(Cue::Clip, volume)
}

/// The buffer an asynchronous `PlaySoundW` reads from. It must outlive the playback.
static PLAYING: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Plays `cue` on the default output without blocking. Returns false when Windows
/// refuses, the cue is silent or the volume is zero.
pub fn play_cue(cue: Cue, volume: f32) -> bool {
    if volume.is_nan() || volume <= 0.0 || cue == Cue::Silent {
        return false;
    }
    let mut buffer = PLAYING.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        // Stop a sound that still reads the old buffer before replacing it.
        let _ = PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0));
        *buffer = cue_wav(cue, volume);
        PlaySoundW(PCWSTR(buffer.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT).as_bool()
    }
}

pub fn play_clip_sound(volume: f32) -> bool {
    play_cue(Cue::Clip, volume)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(wav: &[u8]) -> Vec<i16> {
        wav[HEADER..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    /// The sound folder is one setting for the whole process, so the tests that read
    /// sounds take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn turn() -> std::sync::MutexGuard<'static, ()> {
        TURN.lock().unwrap_or_else(|e| e.into_inner())
    }

    // These tests only build the bytes. Nothing is played.
    #[test]
    fn every_cue_is_a_valid_short_wav() {
        let _turn = turn();
        for cue in Cue::ALL {
            let wav = cue_wav(cue, 1.0);
            if cue == Cue::Silent {
                assert!(wav.is_empty());
                continue;
            }
            assert_eq!(&wav[0..4], b"RIFF", "{cue:?}");
            assert_eq!(&wav[8..16], b"WAVEfmt ");
            assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize, wav.len() - 8);
            assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1); // PCM
            assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1); // mono
            assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44_100);
            assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
            assert_eq!(&wav[36..40], b"data");
            assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize, wav.len() - HEADER);
            let s = samples(&wav);
            let ms = s.len() * 1000 / 44_100;
            assert!((60..=700).contains(&ms), "{cue:?}: {ms} ms");
            // Starts and ends without a click, and never comes near full scale.
            assert!(s[0].abs() < 200 && s[s.len() - 1].abs() < 200, "{cue:?}");
            let peak = s.iter().map(|v| v.unsigned_abs()).max().unwrap();
            assert!(peak > 8000 && peak < 17000, "{cue:?}: peak {peak}");
        }
    }

    #[test]
    fn the_cues_differ_from_each_other() {
        let _turn = turn();
        let all: Vec<_> = Cue::ALL.iter().map(|c| cue_wav(*c, 1.0)).collect();
        for a in 0..all.len() {
            for b in a + 1..all.len() {
                assert_ne!(all[a], all[b]);
            }
        }
    }

    #[test]
    fn an_own_file_replaces_the_built_in_sound() {
        let _turn = turn();
        let dir = std::env::temp_dir().join(format!("klip-sound-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The built in tick under the name of the clip sound, with an extra chunk before
        // the samples as other programs write it.
        let tick = Cue::Tick.file();
        let mut own = tick[..36].to_vec();
        own.extend_from_slice(b"LIST   abcd");
        own.extend_from_slice(&tick[36..]);
        std::fs::write(dir.join("clip.wav"), &own).unwrap();
        std::fs::write(dir.join("auto.wav"), b"not a wav file at all").unwrap();
        set_sound_dir(Some(dir.clone()));
        let clip = cue_wav(Cue::Clip, 1.0);
        let auto = cue_wav(Cue::AutoClip, 1.0);
        let quiet = cue_wav(Cue::Clip, 0.0);
        set_sound_dir(None);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(clip, own);
        // Something that is no WAV file is ignored, the built in sound plays.
        assert_eq!(auto, Cue::AutoClip.file());
        // The volume reaches the samples behind the extra chunk and leaves the header alone.
        assert_eq!(quiet[..56], own[..56]);
        assert!(quiet[56..].iter().all(|b| *b == 0));
    }

    #[test]
    fn volume_scales() {
        let _turn = turn();
        let peak = |v: f32| samples(&cue_wav(Cue::Clip, v)).iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert_eq!(peak(0.0), 0);
        assert!(peak(0.5) > 0 && peak(1.0) > peak(0.5));
        assert_eq!(peak(7.0), peak(1.0));
        assert_eq!(peak(f32::NAN), 0);
    }

    #[test]
    fn zero_volume_and_silence_play_nothing() {
        assert!(!play_cue(Cue::Clip, 0.0));
        assert!(!play_cue(Cue::Clip, f32::NAN));
        assert!(!play_cue(Cue::Silent, 1.0));
    }
}
