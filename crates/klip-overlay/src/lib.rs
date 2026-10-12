//! Overlay stage 1 for Klip: small topmost windows of our own over the game.
//!
//! Nothing here touches the game. There is no hook, no injection, no call on a foreign
//! window except reading its rectangle in [`diag`], and no generated input. The windows
//! are transparent (DirectComposition, no redirection bitmap), never take focus, and are
//! excluded from screen capture.
//!
//! Each element (status indicator, the stack of toasts, input overlay, panel) is a window
//! of its own, exactly as large as what it draws. The panel can be dragged by its grip,
//! which moves our own window and nothing else. A window exists only while its element is visible.
//! With nothing visible there is no window and the thread sleeps without a timeout.
//!
//! What the overlay shows is limited by the anti cheat rules of the project: Klip's own
//! state and the user's own inputs. Nothing derived from the game picture.

mod engine;
mod fonts;
mod gfx;
mod scene;
mod snapshot;
mod text;
mod views;

pub mod diag;
pub mod sound;
pub mod state;
pub mod theme;

pub use diag::{covers, window_coverage, Coverage, PxRect};
pub use engine::{list_monitors, MonitorInfo, Overlay, OverlayConfig, OverlayStats, WindowInfo};
pub use fonts::{font_files_in, FontReport};
pub use snapshot::{render_png, SnapshotOptions};
pub use sound::{clip_sound_wav, cue_wav, play_clip_sound, play_cue, set_sound_dir, Cue, SoundConfig};
pub use state::{
    fmt_buffer, fmt_clock, fmt_duration, Action, Anchor, ClipEntry, Element, InputElement, Lang, MonitorTarget,
    OverlayState, PanelElement, PanelHints, Placement, StatusKind, Toast, ToastKind, MAX_TOASTS,
};
pub use theme::Rgb;
