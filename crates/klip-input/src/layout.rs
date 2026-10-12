//! Keyboard geometry and the user's overlay layout: which keys are shown, the mouse
//! parts, the widgets and the look.
//!
//! All sizes are in key units (one letter key is 1 x 1). Every coordinate is a multiple of
//! 0.25, which is exact in binary, so the geometry tests can compare without tolerance
//! games and the exported JSON reads back to the same numbers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Physical key, identified by its scancode (set 1). Keys with the E0 prefix carry it in
/// the high byte (right Ctrl is 0xE01D), Pause is 0xE11D. The id names a position on the
/// board, so it means the same key on QWERTZ and QWERTY, and it cannot be turned back
/// into the character the user typed without knowing their system layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyId(pub u16);

impl KeyId {
    /// Builds the id from what raw input reports. Returns `None` for codes that are not a
    /// real key (0, and the overrun marker 0xFF and above).
    pub fn from_scancode(make: u16, e0: bool, e1: bool) -> Option<KeyId> {
        if make == 0 || make >= 0xFF {
            return None;
        }
        Some(KeyId(match (e1, e0) {
            (true, _) => 0xE100 | make,
            (false, true) => 0xE000 | make,
            (false, false) => make,
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// True when the two share area. Touching edges do not count.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right() && other.x < self.right() && self.y < other.bottom() && other.y < self.bottom()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    /// ISO board with German labels.
    IsoQwertz,
    /// ANSI board with US labels.
    AnsiQwerty,
}

/// Part of the board a key belongs to. The 60 % and arrow templates are defined by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    Function,
    Main,
    System,
    Nav,
    Arrows,
    Numpad,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub id: KeyId,
    /// Position name as in the web `KeyboardEvent.code` ("KeyW"), so the app can match
    /// keys without a scancode table of its own.
    pub code: String,
    /// Default label of this layout, or the user's own label after
    /// [`OverlayLayout::visible_keys`].
    pub label: String,
    pub block: Block,
    pub rect: Rect,
    /// Second rectangle of a key that is not a rectangle. Only the ISO Enter key has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Rect>,
}

impl Key {
    fn right(&self) -> f32 {
        self.extra.map_or(self.rect.right(), |e| e.right().max(self.rect.right()))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyboardLayout {
    pub kind: LayoutKind,
    pub width: f32,
    pub height: f32,
    pub keys: Vec<Key>,
}

/// (scancode id, code name, German label, US label, width)
type Row = [(u16, &'static str, &'static str, &'static str, f32)];

struct Builder {
    kind: LayoutKind,
    keys: Vec<Key>,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn key(&mut self, block: Block, id: u16, code: &str, de: &str, us: &str, x: f32, y: f32, w: f32, h: f32) {
        let label = if self.kind == LayoutKind::IsoQwertz { de } else { us };
        self.keys.push(Key {
            id: KeyId(id),
            code: code.to_string(),
            label: label.to_string(),
            block,
            rect: Rect { x, y, w, h },
            extra: None,
        });
    }

    /// Places keys left to right and returns the x after the last one.
    fn row(&mut self, block: Block, mut x: f32, y: f32, row: &Row) -> f32 {
        for &(id, code, de, us, w) in row {
            self.key(block, id, code, de, us, x, y, w, 1.0);
            x += w;
        }
        x
    }
}

impl KeyboardLayout {
    pub fn iso_qwertz() -> Self {
        Self::build(LayoutKind::IsoQwertz)
    }

    pub fn ansi_qwerty() -> Self {
        Self::build(LayoutKind::AnsiQwerty)
    }

    pub fn of(kind: LayoutKind) -> Self {
        Self::build(kind)
    }

    pub fn key(&self, id: KeyId) -> Option<&Key> {
        self.keys.iter().find(|k| k.id == id)
    }

    fn build(kind: LayoutKind) -> Self {
        use Block::*;
        let iso = kind == LayoutKind::IsoQwertz;
        let mut b = Builder { kind, keys: Vec::new() };
        // Row tops. The function row sits half a unit above the rest, as on a real board.
        let (r0, r1, r2, r3, r4, r5) = (0.0, 1.5, 2.5, 3.5, 4.5, 5.5);
        // Left edges of the three right hand groups.
        let (nav, pad) = (15.25, 18.5);

        b.key(Function, 0x01, "Escape", "Esc", "Esc", 0.0, r0, 1.0, 1.0);
        b.row(Function, 2.0, r0, &[
            (0x3B, "F1", "F1", "F1", 1.0),
            (0x3C, "F2", "F2", "F2", 1.0),
            (0x3D, "F3", "F3", "F3", 1.0),
            (0x3E, "F4", "F4", "F4", 1.0),
        ]);
        b.row(Function, 6.5, r0, &[
            (0x3F, "F5", "F5", "F5", 1.0),
            (0x40, "F6", "F6", "F6", 1.0),
            (0x41, "F7", "F7", "F7", 1.0),
            (0x42, "F8", "F8", "F8", 1.0),
        ]);
        b.row(Function, 11.0, r0, &[
            (0x43, "F9", "F9", "F9", 1.0),
            (0x44, "F10", "F10", "F10", 1.0),
            (0x57, "F11", "F11", "F11", 1.0),
            (0x58, "F12", "F12", "F12", 1.0),
        ]);
        b.row(System, nav, r0, &[
            (0xE037, "PrintScreen", "Druck", "PrtSc", 1.0),
            (0x46, "ScrollLock", "Rollen", "ScrLk", 1.0),
            (0xE11D, "Pause", "Pause", "Pause", 1.0),
        ]);

        b.row(Main, 0.0, r1, &[
            (0x29, "Backquote", "^", "`", 1.0),
            (0x02, "Digit1", "1", "1", 1.0),
            (0x03, "Digit2", "2", "2", 1.0),
            (0x04, "Digit3", "3", "3", 1.0),
            (0x05, "Digit4", "4", "4", 1.0),
            (0x06, "Digit5", "5", "5", 1.0),
            (0x07, "Digit6", "6", "6", 1.0),
            (0x08, "Digit7", "7", "7", 1.0),
            (0x09, "Digit8", "8", "8", 1.0),
            (0x0A, "Digit9", "9", "9", 1.0),
            (0x0B, "Digit0", "0", "0", 1.0),
            (0x0C, "Minus", "ß", "-", 1.0),
            (0x0D, "Equal", "´", "=", 1.0),
            (0x0E, "Backspace", "Rück", "Back", 2.0),
        ]);
        b.row(Nav, nav, r1, &[
            (0xE052, "Insert", "Einfg", "Ins", 1.0),
            (0xE047, "Home", "Pos1", "Home", 1.0),
            (0xE049, "PageUp", "Bild auf", "PgUp", 1.0),
        ]);
        b.row(Numpad, pad, r1, &[
            (0x45, "NumLock", "Num", "Num", 1.0),
            (0xE035, "NumpadDivide", "/", "/", 1.0),
            (0x37, "NumpadMultiply", "*", "*", 1.0),
            (0x4A, "NumpadSubtract", "-", "-", 1.0),
        ]);

        let x = b.row(Main, 0.0, r2, &[
            (0x0F, "Tab", "Tab", "Tab", 1.5),
            (0x10, "KeyQ", "Q", "Q", 1.0),
            (0x11, "KeyW", "W", "W", 1.0),
            (0x12, "KeyE", "E", "E", 1.0),
            (0x13, "KeyR", "R", "R", 1.0),
            (0x14, "KeyT", "T", "T", 1.0),
            (0x15, "KeyY", "Z", "Y", 1.0),
            (0x16, "KeyU", "U", "U", 1.0),
            (0x17, "KeyI", "I", "I", 1.0),
            (0x18, "KeyO", "O", "O", 1.0),
            (0x19, "KeyP", "P", "P", 1.0),
            (0x1A, "BracketLeft", "Ü", "[", 1.0),
            (0x1B, "BracketRight", "+", "]", 1.0),
        ]);
        if iso {
            // The ISO Enter key is an upside down L: a wide top and a narrower stem that
            // reaches down into the home row.
            b.key(Main, 0x1C, "Enter", "Enter", "Enter", x, r2, 1.5, 1.0);
            b.keys.last_mut().unwrap().extra = Some(Rect { x: x + 0.25, y: r3, w: 1.25, h: 1.0 });
        } else {
            b.key(Main, 0x2B, "Backslash", "#", "\\", x, r2, 1.5, 1.0);
        }
        b.row(Nav, nav, r2, &[
            (0xE053, "Delete", "Entf", "Del", 1.0),
            (0xE04F, "End", "Ende", "End", 1.0),
            (0xE051, "PageDown", "Bild ab", "PgDn", 1.0),
        ]);
        b.row(Numpad, pad, r2, &[
            (0x47, "Numpad7", "7", "7", 1.0),
            (0x48, "Numpad8", "8", "8", 1.0),
            (0x49, "Numpad9", "9", "9", 1.0),
        ]);
        b.key(Numpad, 0x4E, "NumpadAdd", "+", "+", pad + 3.0, r2, 1.0, 2.0);

        let x = b.row(Main, 0.0, r3, &[
            (0x3A, "CapsLock", "Feststell", "Caps", 1.75),
            (0x1E, "KeyA", "A", "A", 1.0),
            (0x1F, "KeyS", "S", "S", 1.0),
            (0x20, "KeyD", "D", "D", 1.0),
            (0x21, "KeyF", "F", "F", 1.0),
            (0x22, "KeyG", "G", "G", 1.0),
            (0x23, "KeyH", "H", "H", 1.0),
            (0x24, "KeyJ", "J", "J", 1.0),
            (0x25, "KeyK", "K", "K", 1.0),
            (0x26, "KeyL", "L", "L", 1.0),
            (0x27, "Semicolon", "Ö", ";", 1.0),
            (0x28, "Quote", "Ä", "'", 1.0),
        ]);
        if iso {
            b.key(Main, 0x2B, "Backslash", "#", "\\", x, r3, 1.0, 1.0);
        } else {
            b.key(Main, 0x1C, "Enter", "Enter", "Enter", x, r3, 2.25, 1.0);
        }
        b.row(Numpad, pad, r3, &[
            (0x4B, "Numpad4", "4", "4", 1.0),
            (0x4C, "Numpad5", "5", "5", 1.0),
            (0x4D, "Numpad6", "6", "6", 1.0),
        ]);

        let x = if iso {
            b.row(Main, 0.0, r4, &[
                (0x2A, "ShiftLeft", "Umschalt", "Shift", 1.25),
                (0x56, "IntlBackslash", "<", "\\", 1.0),
            ])
        } else {
            b.row(Main, 0.0, r4, &[(0x2A, "ShiftLeft", "Umschalt", "Shift", 2.25)])
        };
        b.row(Main, x, r4, &[
            (0x2C, "KeyZ", "Y", "Z", 1.0),
            (0x2D, "KeyX", "X", "X", 1.0),
            (0x2E, "KeyC", "C", "C", 1.0),
            (0x2F, "KeyV", "V", "V", 1.0),
            (0x30, "KeyB", "B", "B", 1.0),
            (0x31, "KeyN", "N", "N", 1.0),
            (0x32, "KeyM", "M", "M", 1.0),
            (0x33, "Comma", ",", ",", 1.0),
            (0x34, "Period", ".", ".", 1.0),
            (0x35, "Slash", "-", "/", 1.0),
            (0x36, "ShiftRight", "Umschalt", "Shift", 2.75),
        ]);
        b.key(Arrows, 0xE048, "ArrowUp", "Hoch", "Up", nav + 1.0, r4, 1.0, 1.0);
        b.row(Numpad, pad, r4, &[
            (0x4F, "Numpad1", "1", "1", 1.0),
            (0x50, "Numpad2", "2", "2", 1.0),
            (0x51, "Numpad3", "3", "3", 1.0),
        ]);
        b.key(Numpad, 0xE01C, "NumpadEnter", "Enter", "Enter", pad + 3.0, r4, 1.0, 2.0);

        b.row(Main, 0.0, r5, &[
            (0x1D, "ControlLeft", "Strg", "Ctrl", 1.25),
            (0xE05B, "MetaLeft", "Win", "Win", 1.25),
            (0x38, "AltLeft", "Alt", "Alt", 1.25),
            (0x39, "Space", "Leer", "Space", 6.25),
            (0xE038, "AltRight", "Alt Gr", "Alt", 1.25),
            (0xE05C, "MetaRight", "Win", "Win", 1.25),
            (0xE05D, "ContextMenu", "Menü", "Menu", 1.25),
            (0xE01D, "ControlRight", "Strg", "Ctrl", 1.25),
        ]);
        b.row(Arrows, nav, r5, &[
            (0xE04B, "ArrowLeft", "Links", "Left", 1.0),
            (0xE050, "ArrowDown", "Runter", "Down", 1.0),
            (0xE04D, "ArrowRight", "Rechts", "Right", 1.0),
        ]);
        b.key(Numpad, 0x52, "Numpad0", "0", "0", pad, r5, 2.0, 1.0);
        b.key(Numpad, 0x53, "NumpadDecimal", ",", ".", pad + 2.0, r5, 1.0, 1.0);

        KeyboardLayout { kind, width: 22.5, height: 6.5, keys: b.keys }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Wasd,
    LeftHandShooter,
    Sixty,
    Arrows,
}

/// W, A, S, D by position.
const WASD: &[u16] = &[0x11, 0x1E, 0x1F, 0x20];

/// What the left hand reaches in a shooter: 1 to 5, Tab to T, Caps to G, Shift to V
/// (with the extra ISO key next to Shift where the board has one), Ctrl, Alt, Space.
const LEFT_HAND: &[u16] = &[
    0x02, 0x03, 0x04, 0x05, 0x06, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x3A, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x2A, 0x56,
    0x2C, 0x2D, 0x2E, 0x2F, 0x1D, 0x38, 0x39,
];

/// Which keys of a keyboard are part of the overlay. This is also the capture allowlist:
/// a key that is not in the scope is never recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Full,
    /// Everything left of a cut. `fraction` is the cut position as a share of the full
    /// board width, 0 shows nothing and 1 shows all. A key counts only when it lies
    /// completely left of the cut, so the overlay never records a key it shows half of.
    Cut { fraction: f32 },
    Preset { preset: Preset },
    /// Keys picked one by one. Ids that the keyboard does not have are ignored.
    Keys { keys: Vec<KeyId> },
}

/// The keys of `layout` that `scope` selects, in layout order, with their rectangles.
pub fn visible_keys(layout: &KeyboardLayout, scope: &Scope) -> Vec<Key> {
    let keep: Box<dyn Fn(&Key) -> bool> = match scope {
        Scope::Full => Box::new(|_| true),
        Scope::Cut { fraction } => {
            // NaN must not select anything, so it is treated as 0.
            let f = if fraction.is_nan() { 0.0 } else { fraction.clamp(0.0, 1.0) };
            let cut = f * layout.width + 1e-4;
            Box::new(move |k| k.right() <= cut)
        }
        Scope::Preset { preset } => match preset {
            Preset::Wasd => Box::new(|k| WASD.contains(&k.id.0)),
            Preset::LeftHandShooter => Box::new(|k| LEFT_HAND.contains(&k.id.0)),
            Preset::Sixty => Box::new(|k| k.block == Block::Main),
            Preset::Arrows => Box::new(|k| k.block == Block::Arrows),
        },
        Scope::Keys { keys } => Box::new(move |k| keys.contains(&k.id)),
    };
    layout.keys.iter().filter(|k| keep(k)).cloned().collect()
}

/// Smallest rectangle around the given keys, `None` when there are none.
pub fn bounds(keys: &[Key]) -> Option<Rect> {
    let mut rects = keys.iter().flat_map(|k| std::iter::once(k.rect).chain(k.extra));
    let first = rects.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.right(), first.bottom());
    for r in rects {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.right());
        y1 = y1.max(r.bottom());
    }
    Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Side1,
    Side2,
}

impl MouseButton {
    pub const ALL: [MouseButton; 5] =
        [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Side1, MouseButton::Side2];

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Option<MouseButton> {
        Self::ALL.get(i as usize).copied()
    }
}

/// Mouse parts of the overlay. Like the key scope this doubles as the capture allowlist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseConfig {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub side1: bool,
    pub side2: bool,
    pub wheel: bool,
    /// Direction of movement. Off by default: it is the only part that makes the capture
    /// thread wake up for every mouse report.
    pub movement: bool,
}

impl MouseConfig {
    pub fn buttons_and_wheel() -> Self {
        MouseConfig { left: true, right: true, middle: true, side1: true, side2: true, wheel: true, movement: false }
    }

    pub fn button(&self, b: MouseButton) -> bool {
        match b {
            MouseButton::Left => self.left,
            MouseButton::Right => self.right,
            MouseButton::Middle => self.middle,
            MouseButton::Side1 => self.side1,
            MouseButton::Side2 => self.side2,
        }
    }

    pub fn any(&self) -> bool {
        self.left || self.right || self.middle || self.side1 || self.side2 || self.wheel || self.movement
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Widgets {
    /// Key presses per second.
    pub kps: bool,
    /// Clicks per second.
    pub cps: bool,
    /// Length of the averaging window.
    pub window_ms: u32,
}

impl Default for Widgets {
    fn default() -> Self {
        Widgets { kps: false, cps: false, window_ms: 1000 }
    }
}

/// Colors as "#RRGGBB". Defaults follow the dark palette in the design note.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Colors {
    pub key: String,
    pub key_pressed: String,
    pub text: String,
    pub text_pressed: String,
    pub border: String,
}

impl Default for Colors {
    fn default() -> Self {
        Colors {
            key: "#2B2B27".into(),
            key_pressed: "#C8F03C".into(),
            text: "#F2F1EA".into(),
            text_pressed: "#10120A".into(),
            border: "#2E2E2A".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Corner {
    Square,
    /// Radius in pixels at scale 1.
    Rounded { radius: f32 },
    Pill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationKind {
    None,
    Fade,
    Pop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Animation {
    pub kind: AnimationKind,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Top left corner of the overlay as a share of the video frame, 0 to 1 on both axes.
    pub x: f32,
    pub y: f32,
    /// Pixels per key unit on a 1080 line frame. The renderer scales with frame height.
    pub unit_px: f32,
    /// Gap between keys in key units.
    pub gap: f32,
    pub opacity: f32,
    pub colors: Colors,
    pub corner: Corner,
    pub animation: Animation,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            x: 0.02,
            y: 0.72,
            unit_px: 44.0,
            gap: 0.08,
            opacity: 0.9,
            colors: Colors::default(),
            corner: Corner::Rounded { radius: 6.0 },
            animation: Animation { kind: AnimationKind::Fade, duration_ms: 80 },
        }
    }
}

/// One complete, named overlay setup. This is what the user saves as a template and what
/// the capture takes its allowlist from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlayLayout {
    pub name: String,
    pub keyboard: LayoutKind,
    pub scope: Scope,
    /// The user's own labels by key ("Jump" instead of "Space").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<KeyId, String>,
    #[serde(default)]
    pub mouse: MouseConfig,
    #[serde(default)]
    pub widgets: Widgets,
    #[serde(default)]
    pub appearance: Appearance,
}

impl OverlayLayout {
    pub fn new(name: &str, keyboard: LayoutKind, scope: Scope) -> Self {
        OverlayLayout {
            name: name.to_string(),
            keyboard,
            scope,
            labels: BTreeMap::new(),
            mouse: MouseConfig::default(),
            widgets: Widgets::default(),
            appearance: Appearance::default(),
        }
    }

    pub fn preset(keyboard: LayoutKind, preset: Preset) -> Self {
        let name = match preset {
            Preset::Wasd => "WASD",
            Preset::LeftHandShooter => "Left hand (shooter)",
            Preset::Sixty => "60 %",
            Preset::Arrows => "Arrows",
        };
        let mut layout = Self::new(name, keyboard, Scope::Preset { preset });
        if preset == Preset::LeftHandShooter {
            layout.mouse = MouseConfig::buttons_and_wheel();
        }
        layout
    }

    /// The four built in templates for one keyboard.
    pub fn builtin(keyboard: LayoutKind) -> Vec<OverlayLayout> {
        [Preset::Wasd, Preset::LeftHandShooter, Preset::Sixty, Preset::Arrows]
            .into_iter()
            .map(|p| Self::preset(keyboard, p))
            .collect()
    }

    /// Visible keys with the user's labels applied.
    pub fn visible_keys(&self) -> Vec<Key> {
        let mut keys = visible_keys(&KeyboardLayout::of(self.keyboard), &self.scope);
        for key in &mut keys {
            if let Some(label) = self.labels.get(&key.id) {
                key.label = label.clone();
            }
        }
        keys
    }
}

/// The user's saved templates as one JSON file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutLibrary {
    pub version: u32,
    pub layouts: Vec<OverlayLayout>,
}

impl LayoutLibrary {
    pub const VERSION: u32 = 1;

    pub fn new() -> Self {
        LayoutLibrary { version: Self::VERSION, layouts: Vec::new() }
    }

    pub fn get(&self, name: &str) -> Option<&OverlayLayout> {
        self.layouts.iter().find(|l| l.name == name)
    }

    /// Saves under the layout's name, replacing an older one of the same name.
    pub fn upsert(&mut self, layout: OverlayLayout) {
        match self.layouts.iter_mut().find(|l| l.name == layout.name) {
            Some(slot) => *slot = layout,
            None => self.layouts.push(layout),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.layouts.len();
        self.layouts.retain(|l| l.name != name);
        self.layouts.len() != before
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let lib: LayoutLibrary = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if lib.version > Self::VERSION {
            return Err(format!("{}: version {} is newer than this build", path.display(), lib.version));
        }
        Ok(lib)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

impl KeyboardLayout {
    /// JSON with one key per line. Plain `to_string_pretty` would spread a board over
    /// two thousand lines, and the single line form is unreadable in a diff.
    pub fn to_json(&self) -> String {
        let mut out = format!(
            "{{\n  \"kind\": {},\n  \"width\": {},\n  \"height\": {},\n  \"keys\": [\n",
            serde_json::to_string(&self.kind).unwrap(),
            self.width,
            self.height
        );
        for (i, key) in self.keys.iter().enumerate() {
            let comma = if i + 1 < self.keys.len() { "," } else { "" };
            out.push_str(&format!("    {}{comma}\n", serde_json::to_string(key).unwrap()));
        }
        out.push_str("  ]\n}\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn both() -> [KeyboardLayout; 2] {
        [KeyboardLayout::iso_qwertz(), KeyboardLayout::ansi_qwerty()]
    }

    fn codes(keys: &[Key]) -> BTreeSet<&str> {
        keys.iter().map(|k| k.code.as_str()).collect()
    }

    #[test]
    fn key_counts_match_real_boards() {
        assert_eq!(KeyboardLayout::iso_qwertz().keys.len(), 105);
        assert_eq!(KeyboardLayout::ansi_qwerty().keys.len(), 104);
    }

    #[test]
    fn no_key_overlaps_another_and_all_lie_inside_the_board() {
        for layout in both() {
            let rects: Vec<(usize, Rect)> = layout
                .keys
                .iter()
                .enumerate()
                .flat_map(|(i, k)| std::iter::once((i, k.rect)).chain(k.extra.map(|e| (i, e))))
                .collect();
            for (n, (i, a)) in rects.iter().enumerate() {
                assert!(a.w > 0.0 && a.h > 0.0);
                assert!(a.x >= 0.0 && a.y >= 0.0 && a.right() <= layout.width && a.bottom() <= layout.height);
                for (j, b) in &rects[n + 1..] {
                    assert!(
                        i == j || !a.overlaps(b),
                        "{:?}: {} overlaps {}",
                        layout.kind,
                        layout.keys[*i].code,
                        layout.keys[*j].code
                    );
                }
            }
        }
    }

    #[test]
    fn ids_and_codes_are_unique() {
        for layout in both() {
            let ids: BTreeSet<_> = layout.keys.iter().map(|k| k.id).collect();
            let names: BTreeSet<_> = layout.keys.iter().map(|k| k.code.clone()).collect();
            assert_eq!(ids.len(), layout.keys.len());
            assert_eq!(names.len(), layout.keys.len());
        }
    }

    #[test]
    fn same_id_is_the_same_position_name_on_both_boards() {
        let (iso, ansi) = (KeyboardLayout::iso_qwertz(), KeyboardLayout::ansi_qwerty());
        for key in &ansi.keys {
            assert_eq!(iso.key(key.id).map(|k| k.code.as_str()), Some(key.code.as_str()));
        }
        // The id follows the position, the label follows the layout.
        assert_eq!(iso.key(KeyId(0x15)).unwrap().label, "Z");
        assert_eq!(ansi.key(KeyId(0x15)).unwrap().label, "Y");
    }

    #[test]
    fn main_block_rows_are_fifteen_units_wide() {
        for layout in both() {
            for y in [1.5, 2.5, 3.5, 4.5, 5.5] {
                let width: f32 = layout
                    .keys
                    .iter()
                    .filter(|k| k.block == Block::Main)
                    .flat_map(|k| std::iter::once(k.rect).chain(k.extra))
                    .filter(|r| r.y == y)
                    .map(|r| r.w)
                    .sum();
                assert_eq!(width, 15.0, "{:?} row at {y}", layout.kind);
            }
        }
    }

    #[test]
    fn cut_at_half_is_the_left_half() {
        let layout = KeyboardLayout::iso_qwertz();
        let keys = visible_keys(&layout, &Scope::Cut { fraction: 0.5 });
        let half = layout.width / 2.0;
        assert!(keys.iter().all(|k| k.right() <= half));
        // And nothing that fits was left out.
        assert_eq!(keys.len(), layout.keys.iter().filter(|k| k.right() <= half).count());
        let names = codes(&keys);
        for inside in ["Escape", "F8", "Digit0", "Tab", "KeyW", "KeyA", "KeyS", "KeyD", "KeyO", "KeyL", "Period", "Space"] {
            assert!(names.contains(inside), "{inside} should be left of the cut");
        }
        for outside in ["F9", "Minus", "KeyP", "Semicolon", "Slash", "Enter", "ArrowLeft", "Numpad5"] {
            assert!(!names.contains(outside), "{outside} should be right of the cut");
        }
        assert_eq!(keys.len(), 56);
    }

    #[test]
    fn cut_ends_and_bad_values() {
        let layout = KeyboardLayout::ansi_qwerty();
        assert!(visible_keys(&layout, &Scope::Cut { fraction: 0.0 }).is_empty());
        assert!(visible_keys(&layout, &Scope::Cut { fraction: -3.0 }).is_empty());
        assert!(visible_keys(&layout, &Scope::Cut { fraction: f32::NAN }).is_empty());
        assert_eq!(visible_keys(&layout, &Scope::Cut { fraction: 1.0 }).len(), layout.keys.len());
        assert_eq!(visible_keys(&layout, &Scope::Cut { fraction: 9.0 }).len(), layout.keys.len());
        // Moving the cut right never removes a key.
        let mut last = 0;
        for step in 0..=100 {
            let n = visible_keys(&layout, &Scope::Cut { fraction: step as f32 / 100.0 }).len();
            assert!(n >= last);
            last = n;
        }
    }

    #[test]
    fn presets() {
        let iso = KeyboardLayout::iso_qwertz();
        let ansi = KeyboardLayout::ansi_qwerty();
        let pick = |l: &KeyboardLayout, p| visible_keys(l, &Scope::Preset { preset: p });

        assert_eq!(codes(&pick(&iso, Preset::Wasd)), BTreeSet::from(["KeyW", "KeyA", "KeyS", "KeyD"]));
        assert_eq!(
            codes(&pick(&ansi, Preset::Arrows)),
            BTreeSet::from(["ArrowUp", "ArrowLeft", "ArrowDown", "ArrowRight"])
        );
        assert_eq!(pick(&iso, Preset::Sixty).len(), 62);
        assert_eq!(pick(&ansi, Preset::Sixty).len(), 61);
        assert_eq!(pick(&iso, Preset::LeftHandShooter).len(), 26);
        // The ANSI board has no key between Shift and Z.
        assert_eq!(pick(&ansi, Preset::LeftHandShooter).len(), 25);
        let left_keys = pick(&iso, Preset::LeftHandShooter);
        let left = codes(&left_keys);
        assert!(left.contains("Space") && left.contains("ShiftLeft") && left.contains("Digit5"));
        assert!(!left.contains("Enter") && !left.contains("KeyH") && !left.contains("Digit6"));
    }

    #[test]
    fn free_keys_ignore_ids_the_board_does_not_have() {
        let layout = KeyboardLayout::ansi_qwerty();
        let scope = Scope::Keys { keys: vec![KeyId(0x39), KeyId(0x56), KeyId(0x7777)] };
        assert_eq!(codes(&visible_keys(&layout, &scope)), BTreeSet::from(["Space"]));
    }

    #[test]
    fn custom_label_and_bounds() {
        let mut layout = OverlayLayout::preset(LayoutKind::IsoQwertz, Preset::Wasd);
        layout.scope = Scope::Keys { keys: vec![KeyId(0x39), KeyId(0x11)] };
        layout.labels.insert(KeyId(0x39), "Jump".into());
        let keys = layout.visible_keys();
        assert_eq!(keys.iter().find(|k| k.code == "Space").unwrap().label, "Jump");
        assert_eq!(keys.iter().find(|k| k.code == "KeyW").unwrap().label, "W");
        assert_eq!(bounds(&keys), Some(Rect { x: 2.5, y: 2.5, w: 7.5, h: 4.0 }));
        assert_eq!(bounds(&[]), None);
    }

    #[test]
    fn overlay_layout_and_library_survive_json() {
        let mut layout = OverlayLayout::preset(LayoutKind::AnsiQwerty, Preset::LeftHandShooter);
        layout.labels.insert(KeyId(0x39), "Jump".into());
        layout.appearance.corner = Corner::Pill;
        let mut lib = LayoutLibrary::new();
        lib.upsert(layout.clone());
        lib.upsert(OverlayLayout::new("cut", LayoutKind::IsoQwertz, Scope::Cut { fraction: 0.4 }));
        lib.upsert(layout.clone());
        assert_eq!(lib.layouts.len(), 2);
        let text = serde_json::to_string(&lib).unwrap();
        assert_eq!(serde_json::from_str::<LayoutLibrary>(&text).unwrap(), lib);
        // A hand written minimal layout gets defaults for everything optional.
        let small: OverlayLayout =
            serde_json::from_str(r#"{"name":"x","keyboard":"iso_qwertz","scope":{"kind":"full"}}"#).unwrap();
        assert_eq!(small.appearance, Appearance::default());
        assert!(!small.mouse.any());
    }

    #[test]
    fn exported_files_match_the_code() {
        // The app reads these files instead of calling Rust, so they must not drift.
        // Regenerate with: cargo run -p klip-input --example export_layouts
        for (layout, text) in [
            (KeyboardLayout::iso_qwertz(), include_str!("../layouts/qwertz.json")),
            (KeyboardLayout::ansi_qwerty(), include_str!("../layouts/qwerty.json")),
        ] {
            assert_eq!(serde_json::from_str::<KeyboardLayout>(text).unwrap(), layout);
        }
    }
}
