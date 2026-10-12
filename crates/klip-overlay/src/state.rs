//! The state the caller owns. The overlay draws exactly this, nothing else.

use crate::sound::Cue;
use crate::text::{tr, T};
use crate::theme::{Rgb, ACCENT};
use klip_input::{Held, LayoutKind, OverlayLayout, Preset};
use std::time::{Duration, Instant};

/// The four parts of the overlay. Each one is a window of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    Status = 0,
    Toast = 1,
    Input = 2,
    Panel = 3,
}

impl Element {
    pub const ALL: [Element; 4] = [Element::Status, Element::Toast, Element::Input, Element::Panel];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

/// Position, size, opacity and visibility of one element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub anchor: Anchor,
    /// Distance from the anchored edges in pixels on a 1080 line monitor. On a centered
    /// axis it is an offset from the middle.
    pub margin_x: f32,
    pub margin_y: f32,
    /// 1.0 is the designed size on a 1080 line monitor. Larger monitors scale on top.
    pub scale: f32,
    pub opacity: f32,
    pub visible: bool,
}

impl Placement {
    pub fn new(anchor: Anchor, margin_x: f32, margin_y: f32) -> Self {
        Placement { anchor, margin_x, margin_y, scale: 1.0, opacity: 1.0, visible: true }
    }

    /// Top left corner of an element of `size` on an `area` (both in pixels). `k` is the
    /// monitor factor, height / 1080. The result always lies inside the area.
    pub fn origin(&self, area: (f32, f32), size: (f32, f32), k: f32) -> (f32, f32) {
        use Anchor::*;
        let (mx, my) = (self.margin_x * k, self.margin_y * k);
        let x = match self.anchor {
            TopLeft | Left | BottomLeft => mx,
            Top | Center | Bottom => (area.0 - size.0) / 2.0 + mx,
            TopRight | Right | BottomRight => area.0 - size.0 - mx,
        };
        let y = match self.anchor {
            TopLeft | Top | TopRight => my,
            Left | Center | Right => (area.1 - size.1) / 2.0 + my,
            BottomLeft | Bottom | BottomRight => area.1 - size.1 - my,
        };
        (x.min(area.0 - size.0).max(0.0).round(), y.min(area.1 - size.1).max(0.0).round())
    }
}

/// Which monitor the overlay lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorTarget {
    Primary,
    /// Index into [`crate::list_monitors`]. Falls back to the primary monitor.
    Index(usize),
    /// The monitor that contains this point of the virtual desktop (physical pixels).
    At { x: i32, y: i32 },
}

/// What Klip is doing. Shown by the indicator and by the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// Nothing to show, the indicator is hidden.
    Off,
    /// The replay buffer runs and holds this many seconds.
    Buffering { seconds: u32 },
    /// A manual recording runs since this moment. The overlay counts by itself, one
    /// redraw per second.
    Recording { since: Instant },
    /// A manual recording is paused at this length.
    Paused { elapsed: Duration },
}

/// Language of the texts the overlay brings along itself. English is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lang {
    #[default]
    En,
    De,
}

/// Which stripe a notice carries on its left edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ToastKind {
    /// Accent stripe.
    #[default]
    Normal,
    /// Red stripe. Only for "a recording is running".
    Recording,
}

/// The short notice. Content rule: only Klip's own events, never anything read from the
/// game picture beyond "a clip was saved". Up to [`MAX_TOASTS`] are stacked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toast {
    /// Finished text from the caller. Drawn in capitals.
    pub title: String,
    /// Finished text from the caller, drawn as a small label. Empty leaves the line out.
    pub detail: String,
    pub kind: ToastKind,
    /// The sound that goes with it.
    pub cue: Cue,
    /// How long this notice stays fully visible. `None` takes `OverlayState::toast_hold`.
    pub hold: Option<Duration>,
}

/// More notices than this are never on screen at once, the oldest one drops out.
pub const MAX_TOASTS: usize = 4;

impl Toast {
    /// A notice with the accent stripe.
    pub fn new(title: &str, detail: &str) -> Self {
        Toast { title: title.into(), detail: detail.into(), ..Default::default() }
    }

    /// A notice with the red stripe.
    pub fn recording(title: &str, detail: &str) -> Self {
        Toast { kind: ToastKind::Recording, cue: Cue::RecordStart, ..Toast::new(title, detail) }
    }

    /// The same notice with another sound.
    pub fn with_cue(self, cue: Cue) -> Self {
        Toast { cue, ..self }
    }

    /// "Clip saved" in English.
    pub fn clip_saved(detail: &str) -> Self {
        Toast::clip_saved_in(Lang::En, detail)
    }

    /// "Clip saved" in the given language.
    pub fn clip_saved_in(lang: Lang, detail: &str) -> Self {
        Toast::new(tr(lang, T::ClipSaved), detail)
    }

    pub fn with_hold(mut self, hold: Duration) -> Self {
        self.hold = Some(hold);
        self
    }
}

/// The live input overlay. Position and opacity come from `layout.appearance`
/// (`x` and `y` as a share of the monitor, `unit_px`, `opacity`), `scale` multiplies.
#[derive(Clone, Debug, PartialEq)]
pub struct InputElement {
    pub visible: bool,
    pub scale: f32,
    pub layout: OverlayLayout,
    pub held: Held,
    /// Sign of the current wheel movement: above 0 away from the user, 0 still.
    pub wheel: i32,
    /// Direction of the mouse movement in 45 degree steps, 0 right, 2 down, 4 left,
    /// 6 up. `None` while the mouse rests.
    pub motion: Option<u8>,
    /// Shown only when the layout switches the widget on.
    pub kps: Option<f32>,
    pub cps: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipEntry {
    pub title: String,
    /// Time of day as the caller wants it shown, for example "21:14".
    pub time: String,
    pub duration_secs: u32,
}

/// Key names shown next to the panel buttons, for example "Alt+F10". Empty hides one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelHints {
    pub save_clip: String,
    pub record: String,
    pub screenshot: String,
    pub mic: String,
    /// The key that opens and closes the panel.
    pub toggle: String,
}

/// The panel. `placement.visible` is "open".
#[derive(Clone, Debug, PartialEq)]
pub struct PanelElement {
    pub placement: Placement,
    /// Top left corner in pixels relative to the monitor. When set it replaces the
    /// anchor of `placement`. The overlay writes it while the user drags the grip and
    /// keeps the panel on the monitor.
    pub position: Option<(i32, i32)>,
    /// Newest first. The panel shows at most five.
    pub clips: Vec<ClipEntry>,
    pub hints: PanelHints,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverlayState {
    pub monitor: MonitorTarget,
    /// Everything that is accent hangs on this one value.
    pub accent: Rgb,
    /// Language of the overlay's own texts.
    pub language: Lang,
    pub status: StatusKind,
    pub mic_muted: bool,
    pub indicator: Placement,
    pub toast: Placement,
    /// How long a toast stays fully visible, fades not counted. A toast can bring its
    /// own time in `Toast::hold`.
    pub toast_hold: Duration,
    pub input: InputElement,
    pub panel: PanelElement,
}

impl Default for OverlayState {
    /// Nothing visible. The corners are chosen so that nothing sits in the middle of the
    /// screen and nothing covers Valorant's minimap in the top left.
    fn default() -> Self {
        let mut panel = Placement::new(Anchor::Left, 32.0, 0.0);
        panel.visible = false;
        OverlayState {
            monitor: MonitorTarget::Primary,
            accent: ACCENT,
            language: Lang::En,
            status: StatusKind::Off,
            mic_muted: false,
            indicator: Placement::new(Anchor::TopRight, 24.0, 24.0),
            toast: Placement::new(Anchor::TopRight, 24.0, 64.0),
            toast_hold: Duration::from_millis(2500),
            input: InputElement {
                visible: false,
                scale: 1.0,
                layout: OverlayLayout::preset(LayoutKind::IsoQwertz, Preset::LeftHandShooter),
                held: Held::default(),
                wheel: 0,
                motion: None,
                kps: None,
                cps: None,
            },
            panel: PanelElement { placement: panel, position: None, clips: Vec::new(), hints: PanelHints::default() },
        }
    }
}

/// A click in the panel, or the end of a drag. The overlay only reports, the caller acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    SaveClip,
    ToggleRecording,
    Screenshot,
    ToggleMic,
    /// Index into `PanelElement::clips`.
    OpenClip(usize),
    OpenLibrary,
    OpenSettings,
    /// The close button. The overlay has already closed the panel when this arrives.
    ClosePanel,
    /// The user let go of the grip after moving the panel. Top left corner in pixels
    /// relative to the monitor, the same value that now stands in `PanelElement::position`.
    PanelMoved { x: i32, y: i32 },
}

/// "00:12", from one hour on "1:02:03".
pub fn fmt_clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Length of a clip in a list: "0:30", "12:05", "1:02:03".
pub fn fmt_duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Length of the buffer: "30 s", "2 min", "1 min 30 s".
pub fn fmt_buffer(seconds: u32) -> String {
    match (seconds / 60, seconds % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_clock(0), "00:00");
        assert_eq!(fmt_clock(72), "01:12");
        assert_eq!(fmt_clock(3723), "1:02:03");
        assert_eq!(fmt_duration(30), "0:30");
        assert_eq!(fmt_duration(725), "12:05");
        assert_eq!(fmt_duration(3723), "1:02:03");
        assert_eq!(fmt_buffer(30), "30 s");
        assert_eq!(fmt_buffer(120), "2 min");
        assert_eq!(fmt_buffer(90), "1 min 30 s");
    }

    #[test]
    fn placement_anchors() {
        let area = (1920.0, 1080.0);
        let size = (200.0, 40.0);
        let at = |a| Placement::new(a, 24.0, 10.0).origin(area, size, 1.0);
        assert_eq!(at(Anchor::TopLeft), (24.0, 10.0));
        assert_eq!(at(Anchor::TopRight), (1920.0 - 200.0 - 24.0, 10.0));
        assert_eq!(at(Anchor::BottomRight), (1696.0, 1080.0 - 40.0 - 10.0));
        assert_eq!(at(Anchor::Center), (860.0 + 24.0, 520.0 + 10.0));
        assert_eq!(at(Anchor::Left), (24.0, 530.0));
        // Margins scale with the monitor, the element never leaves it.
        assert_eq!(Placement::new(Anchor::TopLeft, 24.0, 24.0).origin((3840.0, 2160.0), size, 2.0), (48.0, 48.0));
        assert_eq!(Placement::new(Anchor::TopLeft, 5000.0, -50.0).origin(area, size, 1.0), (1720.0, 0.0));
    }

    #[test]
    fn toast_constructors() {
        assert_eq!(Toast::clip_saved("x").title, "Clip saved");
        assert_eq!(Toast::clip_saved_in(Lang::De, "x").title, "Clip gespeichert");
        assert_eq!(Toast::new("a", "b").kind, ToastKind::Normal);
        let t = Toast::recording("a", "b").with_hold(Duration::from_secs(1));
        assert_eq!((t.kind, t.hold), (ToastKind::Recording, Some(Duration::from_secs(1))));
        assert_eq!(Toast::default(), Toast::new("", ""));
    }

    #[test]
    fn default_state_shows_nothing() {
        let s = OverlayState::default();
        assert_eq!(s.status, StatusKind::Off);
        assert!(!s.input.visible && !s.panel.placement.visible);
        assert_eq!((s.language, s.accent, s.panel.position), (Lang::En, Rgb(0x8B, 0x5C, 0xFF), None));
        // Nothing is anchored to the middle of the screen.
        for p in [s.indicator, s.toast, s.panel.placement] {
            assert_ne!(p.anchor, Anchor::Center);
        }
    }
}
