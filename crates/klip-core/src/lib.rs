//! Types shared by the recorder and the app: clip metadata and default paths.
//! File formats are defined here and nowhere else.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version of the `.klip.json` sidecar format. Bump on breaking changes and migrate on read.
pub const SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    Clip,
    Recording,
    Screenshot,
    AutoClip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

/// A point in time inside a clip, in milliseconds from the first frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipEvent {
    pub at_ms: u64,
    /// Stable identifier such as "marker", "kill", "ace". Never shown to the user directly.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Sidecar written next to every clip as `<name>.klip.json`. This is the source of truth,
/// the library index is only a cache built from these files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipMeta {
    pub version: u32,
    pub kind: ClipKind,
    /// Local time of the last frame, RFC 3339.
    pub created: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    /// Why an automatic clip was saved: the identifier of its most important event,
    /// such as "ace". Absent for everything the user saved by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: VideoCodec,
    pub encoder: String,
    #[serde(default)]
    pub audio_tracks: Vec<String>,
    #[serde(default)]
    pub events: Vec<ClipEvent>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    /// The webcam, recorded as a file of its own next to the clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraTrack>,
    /// Where the clip is in Klip Cloud, once it was uploaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudRef>,
}

/// A clip that is in Klip Cloud: the server and its id there.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudRef {
    pub server: String,
    pub id: String,
    /// Local time of the upload, RFC 3339.
    pub uploaded: String,
}

/// Where the camera of a clip is and how it lines up with the clip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraTrack {
    /// File name next to the clip, `<clip>.cam.mp4`.
    pub file: String,
    /// Time of the first camera frame on the timeline of the clip, in milliseconds. Below
    /// zero when the camera file starts a little earlier than the clip.
    pub offset_ms: i64,
}

impl ClipMeta {
    /// Path of the camera file that belongs to `video`.
    pub fn camera_path(video: &std::path::Path) -> PathBuf {
        video.with_extension("cam.mp4")
    }

    pub fn sidecar_path(video: &std::path::Path) -> PathBuf {
        video.with_extension("klip.json")
    }

    pub fn write_next_to(&self, video: &std::path::Path) -> std::io::Result<()> {
        let path = Self::sidecar_path(video);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        // Rename is atomic on the same volume, a crash never leaves a half written sidecar.
        std::fs::rename(tmp, path)
    }
}

/// `Klip` inside the user's video folder, the default clip folder. Windows is asked where
/// that folder is, because many people move it to another drive.
pub fn default_clip_dir() -> PathBuf {
    let videos = videos_dir().unwrap_or_else(|| {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        home.join("Videos")
    });
    videos.join("Klip")
}

#[cfg(windows)]
fn videos_dir() -> Option<PathBuf> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    #[link(name = "shell32")]
    extern "system" {
        fn SHGetKnownFolderPath(id: *const Guid, flags: u32, token: *mut c_void, path: *mut *mut u16) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoTaskMemFree(p: *mut c_void);
    }
    // FOLDERID_Videos
    let id = Guid(0x18989B1D, 0x99B5, 0x455B, [0x84, 0x1C, 0xAB, 0x7C, 0x74, 0xE4, 0xDD, 0xFC]);
    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: the call fills `raw` with a NUL terminated string that we free below,
    // also when it fails, as the documentation asks.
    unsafe {
        let hr = SHGetKnownFolderPath(&id, 0, std::ptr::null_mut(), &mut raw);
        let path = (hr >= 0 && !raw.is_null()).then(|| {
            let len = (0..).take_while(|&i| *raw.add(i) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(raw, len)))
        });
        CoTaskMemFree(raw.cast());
        path.filter(|p| !p.as_os_str().is_empty())
    }
}

#[cfg(not(windows))]
fn videos_dir() -> Option<PathBuf> {
    None
}

/// `%APPDATA%\Klip`, settings, logs and the library cache.
pub fn data_dir() -> PathBuf {
    // Tests point this elsewhere so they never touch the real settings.
    if let Some(dir) = std::env::var_os("KLIP_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    base.join("Klip")
}

/// Version of `config.json`. Bump on breaking changes and migrate on read.
pub const CONFIG_VERSION: u32 = 4;

/// Language of everything the user reads: app, tray menu, overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    En,
    De,
}

/// Which programs get an audio track of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    /// Every program that plays sound, found by itself. Nothing is ever missing from a clip.
    All,
    /// Only the programs listed in `apps`.
    Selected,
}

/// Which monitor is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MonitorChoice {
    /// The main monitor of Windows, whatever else is connected.
    #[default]
    Primary,
    /// The monitor the game in the foreground is on, the main monitor without a game.
    Game,
    /// A chosen monitor: its place in the list of the recorder, the main monitor is 0.
    /// Falls back to the main monitor when it is not connected.
    Index { index: usize },
    /// Every monitor side by side in one wide picture, as Windows arranges them.
    All,
}

/// Default accent colour. Flat, never part of a gradient.
pub const DEFAULT_ACCENT: &str = "#8B5CFF";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HotkeyAction {
    /// Save the last `seconds` from the replay buffer.
    Clip { seconds: u32 },
    RecordToggle,
    RecordPause,
    Marker,
    Screenshot,
    /// Open or close the overlay panel.
    Overlay,
    /// Pause or resume the capture of keys for the input overlay.
    InputPause,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyBinding {
    /// Modifiers and one key joined by "+", for example "Alt+F10" or "Ctrl+Shift+S".
    pub keys: String,
    pub action: HotkeyAction,
}

/// A program that gets its own audio track.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppTrack {
    /// Track name shown in the editor.
    pub name: String,
    /// Exe names, compared without regard to case. All running matches are summed.
    pub exe: Vec<String>,
}

/// Which audio tracks a clip gets. Track 1 is always the mix and track 2 the whole
/// system sound, then microphone, game and the programs listed here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub enabled: bool,
    pub microphone: bool,
    pub mode: AudioMode,
    /// In mode `selected` the only programs with a track. In mode `all` these entries
    /// group and name programs (Discord and TeamSpeak become "Voice chat"), every other
    /// program gets a track named after its exe.
    pub apps: Vec<AppTrack>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        let app = |name: &str, exe: &[&str]| AppTrack {
            name: name.to_string(),
            exe: exe.iter().map(|e| e.to_string()).collect(),
        };
        Self {
            enabled: true,
            microphone: true,
            mode: AudioMode::All,
            apps: vec![
                app("Voice chat", &["discord.exe", "teamspeak.exe", "ts3client_win64.exe"]),
                app("Music", &["spotify.exe"]),
                app("Browser", &["brave.exe", "chrome.exe", "firefox.exe", "msedge.exe", "opera.exe"]),
            ],
        }
    }
}

/// When the replay buffer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferMode {
    /// From the start of the recorder on, whatever is on screen.
    Always,
    /// Only while a recognised game runs. Nothing of the desktop ends up in the buffer.
    GameOnly,
}

/// The input overlay: which keys are captured next to the video.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// Capture key and mouse button states while a recognised game has the focus.
    pub enabled: bool,
    /// Name of a layout in `layouts.json`. The built in left hand layout if absent.
    pub layout: Option<String>,
    /// Show the pressed keys live on top of the game. Off by default: the input overlay
    /// is meant to be seen in the clip, not on the screen while playing.
    pub live: bool,
    /// Show the input overlay in clips unless the editor says otherwise for a clip.
    pub show_in_clips: bool,
    /// Corner of the picture the input overlay sits in by default.
    pub corner: Corner,
}

/// The webcam. Recorded as a picture of its own next to each clip, so the editor can
/// place it, resize it or leave it out at export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraConfig {
    pub enabled: bool,
    /// Part of the name of the camera to use. The first camera Windows lists if absent.
    pub device: Option<String>,
    /// Corner the camera sits in by default in the editor and the export.
    pub corner: Corner,
    /// Width of the camera picture in the export, in percent of the clip width.
    pub size_percent: u32,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self { enabled: false, device: None, corner: Corner::BottomRight, size_percent: 25 }
    }
}

/// Which clips go to Klip Cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudMode {
    /// Nothing leaves the PC.
    Off,
    /// Only the clips the user uploads in the app.
    Selected,
    /// Every new clip, automatic clip and recording, right after it is saved.
    All,
}

/// Klip Cloud: a server of the user's choice, reached with a Discord sign in. Where the
/// server keeps the files (its own disk or the user's Google Drive) is set on the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudConfig {
    pub mode: CloudMode,
    /// Address of the server, for example `https://imkirit.dev/klip/cloud`.
    pub server: String,
    /// Keep the clip on this PC after the upload. False moves it to the recycle bin once
    /// the server confirmed it, so the clip lives in the cloud only.
    pub keep_local: bool,
    /// Automatic uploads only take clips saved after this moment (RFC 3339), so turning
    /// the mode on does not send the whole library.
    pub since: Option<String>,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self { mode: CloudMode::Off, server: DEFAULT_CLOUD_SERVER.to_string(), keep_local: true, since: None }
    }
}

/// The server of the Klip project. Anyone can run their own and enter it instead.
pub const DEFAULT_CLOUD_SERVER: &str = "https://imkirit.dev/klip/cloud";

/// A corner of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self { enabled: true, layout: None, live: false, show_in_clips: true, corner: Corner::TopRight }
    }
}

/// The overlay on top of the game. It is a window of its own, nothing is loaded into the game.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    pub enabled: bool,
    /// Small permanent dot with the buffer or recording state. Off by default: a window
    /// that stays on top of a game can cost it its lowest latency presentation mode.
    pub indicator: bool,
    /// Short notice when a clip was saved.
    pub toasts: bool,
    pub sound: bool,
    /// 0.0 to 1.0.
    pub sound_volume: f32,
    /// Top left corner of the panel after the user dragged it, in pixels of the monitor.
    /// Absent means the default place.
    pub panel_pos: Option<[i32; 2]>,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self { enabled: true, indicator: false, toasts: true, sound: true, sound_volume: 0.5, panel_pos: None }
    }
}

/// Settings of the recorder, stored as `%APPDATA%\Klip\config.json`. The app writes the
/// file, the recorder reads it. Unknown fields are ignored and missing ones take their
/// default, so older and newer files both load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub buffer_seconds: u32,
    /// Keep the replay buffer in files instead of RAM. Meant for buffers of several minutes.
    pub buffer_on_disk: bool,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub codec: VideoCodec,
    /// Output height, the width follows the aspect ratio. Source size if absent.
    pub height: Option<u32>,
    pub capture_cursor: bool,
    /// Clip folder, `default_clip_dir()` if absent.
    pub clip_dir: Option<PathBuf>,
    /// Longest manual recording in minutes, a guard against a forgotten recording. 0 = no limit.
    pub max_recording_minutes: u32,
    pub hotkeys: Vec<HotkeyBinding>,
    pub audio: AudioConfig,
    pub buffer_mode: BufferMode,
    pub input: InputConfig,
    pub overlay: OverlaySettings,
    /// Save clips by itself when a detector reports an event in the game.
    pub auto_clips: bool,
    /// Record only this part of the source: left, top, width, height in pixels.
    pub region: Option<[u32; 4]>,
    /// Which monitor is recorded.
    pub monitor: MonitorChoice,
    /// The webcam, off by default.
    pub camera: CameraConfig,
    /// Klip Cloud, off by default.
    pub cloud: CloudConfig,
    pub language: Language,
    /// Accent colour as "#RRGGBB", shared by app, tray and overlay.
    pub accent: String,
    /// Folder for temporary files: replay buffer on disk, thumbnails, extracted audio.
    pub cache_dir: Option<PathBuf>,
    /// Only a drive like "E:". The default cache path is then created on that drive
    /// instead. Ignored when `cache_dir` is set.
    pub cache_drive: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let bind = |keys: &str, action| HotkeyBinding { keys: keys.to_string(), action };
        Self {
            version: CONFIG_VERSION,
            buffer_seconds: 60,
            buffer_on_disk: false,
            fps: 60,
            bitrate_kbps: 20_000,
            codec: VideoCodec::H264,
            height: None,
            capture_cursor: false,
            clip_dir: None,
            max_recording_minutes: 240,
            audio: AudioConfig::default(),
            // Clipping works everywhere, on the desktop too. "Only in games" is a choice.
            buffer_mode: BufferMode::Always,
            input: InputConfig::default(),
            overlay: OverlaySettings::default(),
            auto_clips: true,
            region: None,
            monitor: MonitorChoice::Primary,
            camera: CameraConfig::default(),
            cloud: CloudConfig::default(),
            language: Language::En,
            accent: DEFAULT_ACCENT.to_string(),
            cache_dir: None,
            cache_drive: None,
            hotkeys: vec![
                // Alt+F9, Alt+F1 and Alt+Shift+F10 belong to the NVIDIA overlay on many
                // machines, so only the clip key follows the NVIDIA habit.
                bind("Alt+F10", HotkeyAction::Clip { seconds: 60 }),
                bind("Alt+S", HotkeyAction::Clip { seconds: 60 }),
                bind("Ctrl+Shift+F10", HotkeyAction::Clip { seconds: 30 }),
                bind("Ctrl+Shift+F9", HotkeyAction::RecordToggle),
                bind("Ctrl+Shift+F8", HotkeyAction::RecordPause),
                bind("Ctrl+Shift+F7", HotkeyAction::Marker),
                bind("Ctrl+Shift+F6", HotkeyAction::Screenshot),
                bind("Ctrl+Shift+F5", HotkeyAction::Overlay),
                bind("Ctrl+Shift+F4", HotkeyAction::InputPause),
            ],
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        data_dir().join("config.json")
    }

    /// Loads the settings. A missing file means defaults, a broken file is an error so
    /// that a typo does not silently reset everything.
    pub fn load() -> Result<Self, String> {
        let path = Self::path();
        match std::fs::read(&path) {
            Ok(bytes) => {
                let read = serde_json::from_slice::<Self>(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                let was = read.version;
                let now = read.migrated();
                if now.version != was {
                    // Written back once, so the app shows what the recorder really does.
                    // If that fails the next start migrates again, nothing is lost.
                    let _ = now.save();
                }
                Ok(now)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the settings through a temporary file, so a crash never leaves half a file.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }

    /// Brings a file written by an older version up to date. Fields that did not exist
    /// yet already carry their defaults, this adds what a default alone cannot express.
    fn migrated(mut self) -> Self {
        if self.version < 2 {
            // Version 2 introduced Alt+S as a second clip key. Added once, and only if
            // the user has not given that combination to something else.
            let taken = self.hotkeys.iter().any(|b| b.keys.replace(' ', "").eq_ignore_ascii_case("alt+s"));
            if !taken {
                let seconds = self
                    .hotkeys
                    .iter()
                    .find_map(|b| match b.action {
                        HotkeyAction::Clip { seconds } => Some(seconds),
                        _ => None,
                    })
                    .unwrap_or(60);
                let at = self.hotkeys.iter().position(|b| !matches!(b.action, HotkeyAction::Clip { .. })).unwrap_or(self.hotkeys.len());
                self.hotkeys.insert(at.min(1), HotkeyBinding { keys: "Alt+S".into(), action: HotkeyAction::Clip { seconds } });
            }
            // The old accent was a setting of the app alone and started out green.
            self.accent = DEFAULT_ACCENT.to_string();
            self.version = 2;
        }
        if self.version < 3 {
            // The owner never wants to see the keys or a status mark while playing. Older
            // files carried both switched on, from the time they were the default.
            self.overlay.indicator = false;
            self.version = 3;
        }
        if self.version < 4 {
            // The owner wants to clip on the desktop as well: the buffer runs always
            // unless someone switches it back to "only in games" after this.
            self.buffer_mode = BufferMode::Always;
            self.version = 4;
        }
        // The pressed keys are only ever shown in the clip. The live display over the game
        // was removed, the field stays so that old files still load.
        self.input.live = false;
        self
    }

    /// Where temporary files go. A chosen folder wins. A chosen drive keeps the default
    /// path and only swaps the drive letter, so `C:\Users\me\AppData\Local\Klip\cache`
    /// becomes `E:\Users\me\AppData\Local\Klip\cache`.
    pub fn cache_dir(&self) -> PathBuf {
        if let Some(dir) = self.cache_dir.as_ref().filter(|d| !d.as_os_str().is_empty()) {
            return dir.clone();
        }
        let default = default_cache_dir();
        let drive = self.cache_drive.as_deref().map(str::trim).and_then(|d| d.chars().next()).filter(char::is_ascii_alphabetic);
        match (drive, default.to_str()) {
            (Some(letter), Some(text)) if text.len() > 2 && text.as_bytes()[1] == b':' => {
                PathBuf::from(format!("{}{}", letter.to_ascii_uppercase(), &text[1..]))
            }
            _ => default,
        }
    }

    pub fn clip_dir(&self) -> PathBuf {
        self.clip_dir.clone().unwrap_or_else(default_clip_dir)
    }
}

/// `%LOCALAPPDATA%\Klip\cache`, the default place for temporary files. Kept apart from
/// the clips on purpose: everything in here can be deleted at any time.
pub fn default_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KLIP_CACHE_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    base.join("Klip").join("cache")
}

/// Name of the pipe the recorder listens on. Local only, remote clients are rejected.
pub const PIPE_NAME: &str = r"\\.\pipe\klip-recorder";

/// One request from the app to the recorder, sent as a single line of JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    Status,
    /// Run an action exactly as its hotkey would.
    Action { action: HotkeyAction },
    /// Read `config.json` again. Hotkeys change at once, picture and audio settings need
    /// a restart of the recorder and are reported back as such.
    ReloadConfig,
    Quit,
}

/// Answer to one request, a single line of JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RecorderStatus>,
}

/// What the recorder is doing right now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecorderStatus {
    pub version: String,
    pub buffer_seconds: u32,
    /// How much the replay buffer holds at the moment.
    pub buffered_seconds: f64,
    pub recording: bool,
    pub recording_ms: u64,
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio_tracks: Vec<String>,
    /// Path of the file saved last (clip, recording or screenshot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_saved: Option<String>,
    /// Game in the foreground, if one was recognised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    /// The connected monitors in the order `MonitorChoice::Index` counts them, each as
    /// a short description such as "1920x1080 at 0,0".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<String>,
    /// Names of the connected cameras.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cameras: Vec<String>,
    /// The camera that is running, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// Hotkeys that could not be registered, each as one finished sentence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hotkey_problems: Vec<String>,
    /// Why the last automatic upload to Klip Cloud failed. Cleared by the next success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_error: Option<String>,
    /// Clips uploaded automatically since the recorder started.
    #[serde(default)]
    pub cloud_uploaded: u32,
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn default_clip_dir_is_inside_the_video_folder_windows_reports() {
        let dir = default_clip_dir();
        println!("default clip dir: {}", dir.display());
        assert!(dir.is_absolute());
        assert_eq!(dir.file_name().unwrap(), "Klip");
        assert!(dir.parent().unwrap().is_dir());
    }
}
