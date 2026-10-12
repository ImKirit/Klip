//! The four pictures. Every function measures and, when the `Gfx` has a target, draws.
//!
//! Design rules that hold everywhere (design note, version 2): flat fills, no gradient,
//! no glow, no shadow, no rounded corner. The mark of the design is the corner cut off at
//! the top right. One accent. Red only for recording.

use crate::fonts::Role;
use crate::gfx::Gfx;
use crate::scene::{InputScene, Look, PanelScene, Scene, StatusScene, ToastScene};
use crate::state::{fmt_duration, Action, ToastKind};
use crate::text::{tr, T};
use crate::theme::*;
use klip_input::{bounds, Corner, MouseButton};

/// A clickable area of the panel in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hit {
    pub action: Action,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Hit {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

impl Scene {
    /// Size in pixels at scale `s`. Draws when `g` has a target. `hits` receives the
    /// clickable areas of the panel.
    pub fn view(&self, g: &Gfx, s: f32, hits: &mut Vec<Hit>) -> (f32, f32) {
        let (w, h) = match self {
            Scene::Status(sc) => status(g, sc, s),
            Scene::Toast(sc) => toast(g, sc, s),
            Scene::Input(sc) => input(g, sc, s),
            Scene::Panel(sc) => panel(g, sc, s, hits),
        };
        (w.ceil().max(1.0), h.ceil().max(1.0))
    }
}

/// Height of the panel's grip at scale `s`. The window thread uses it to tell a drag on
/// the grip from a click on a button.
pub(crate) fn grip_height(s: f32) -> f32 {
    (34.0 * s).round()
}

/// Key names the way the HUD writes them: "Ctrl+Shift+F9" becomes "CTRL SHIFT F9".
fn keys(hint: &str) -> String {
    hint.split('+').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The status mark: a square like the dot of the wordmark. Accent while buffering, red
/// while recording, two bars while paused. `x`, `y` is the top left corner, `d` the size.
fn status_mark(g: &Gfx, sc: &StatusScene, x: f32, y: f32, d: f32) {
    match sc.look {
        Look::Buffering => g.rect(x, y, d, d, 0.0, sc.accent, 1.0),
        Look::Recording => g.rect(x, y, d, d, 0.0, RECORDING, 1.0),
        Look::Paused => {
            let bar = (d * 0.35).round().max(1.0);
            g.rect(x, y, bar, d, 0.0, TEXT_MUTED, 1.0);
            g.rect(x + d - bar, y, bar, d, 0.0, TEXT_MUTED, 1.0);
        }
    }
}

fn status(g: &Gfx, sc: &StatusScene, s: f32) -> (f32, f32) {
    let px = |v: f32| (v * s).round();
    let (h, pad, dot, gap) = (px(28.0), px(12.0), px(8.0), px(8.0));
    let label = g.caps(&sc.label, Role::Display, 700, 12.0 * s, 0.06);
    let value = g.caps(&sc.value, Role::Mono, 700, 11.0 * s, 0.04);
    let mut w = pad + dot + gap + label.w;
    if !sc.value.is_empty() {
        w += px(10.0) + value.w;
    }
    // A little more room on the right, the cut corner takes some of it.
    w = (w + pad + px(4.0)).ceil();
    g.cut(0.0, 0.0, w, h, px(6.0), SURFACE_1, 0.96);
    status_mark(g, sc, pad, ((h - dot) / 2.0).round(), dot);
    let x = pad + dot + gap;
    g.put(&label, x, (h - label.h) / 2.0, TEXT, 1.0);
    g.put(&value, x + label.w + px(10.0), (h - value.h) / 2.0, TEXT_MUTED, 1.0);
    (w, h)
}

/// The stack of notices: fixed width, a four pixel stripe on the left, the cut corner
/// top right. Each one carries its own fade.
fn toast(g: &Gfx, sc: &ToastScene, s: f32) -> (f32, f32) {
    let px = |v: f32| (v * s).round();
    let (w, stripe, gap) = (px(330.0), px(4.0), px(10.0));
    let (left, top, line1, line2, bottom) = (stripe + px(18.0), px(10.0), px(22.0), px(16.0), px(11.0));
    let room = w - left - px(16.0);
    let mut y = 0.0;
    for (i, item) in sc.items.iter().enumerate() {
        if i > 0 {
            y += gap;
        }
        let a = if item.alpha.is_nan() { 1.0 } else { item.alpha.clamp(0.0, 1.0) };
        let title = g.caps_in(&item.title, Role::Display, 700, 15.0 * s, 0.04, room);
        let detail = g.caps_in(&item.detail, Role::Mono, 500, 10.0 * s, 0.12, room);
        let h = top + line1 + if item.detail.is_empty() { 0.0 } else { line2 } + bottom;
        let color = match item.kind {
            ToastKind::Normal => sc.accent,
            ToastKind::Recording => RECORDING,
        };
        g.cut(0.0, y, w, h, px(14.0), SURFACE_1, 0.96 * a);
        g.rect(0.0, y, stripe, h, 0.0, color, a);
        g.put(&title, left, y + top + (line1 - title.h) / 2.0, TEXT, a);
        g.put(&detail, left, y + top + line1 + (line2 - detail.h) / 2.0, TEXT_FAINT, a);
        y += h;
    }
    (w, y)
}

fn panel(g: &Gfx, sc: &PanelScene, s: f32, hits: &mut Vec<Hit>) -> (f32, f32) {
    let px = |v: f32| (v * s).round();
    // The background needs the height, which only the layout knows: measure first.
    if g.drawing() {
        let (w, h) = panel(&g.measuring(), sc, s, &mut Vec::new());
        g.cut(0.0, 0.0, w, h, px(20.0), SURFACE_1, 0.97);
    }
    let (w, pad) = (px(380.0), px(20.0));
    let inner = w - pad * 2.0;
    let t = |key: T| tr(sc.lang, key);
    let hot = |a: Action| sc.hover == Some(a);
    let mut hit = |action: Action, x: f32, y: f32, w: f32, h: f32| hits.push(Hit { action, x, y, w, h });
    let look = sc.status.as_ref().map(|st| st.look);

    // Grip: dot grid, wordmark with its status square, the hint, the close button. The
    // window thread treats everything in this bar except the close button as the handle.
    let grip = grip_height(s);
    g.cut(0.0, 0.0, w, grip, px(20.0), SURFACE_2, 1.0);
    let gx = px(14.0);
    for row in 0..2 {
        for col in 0..3 {
            let (cx, cy) = (gx + (col as f32 * 6.0 + 2.0) * s, grip / 2.0 + (row as f32 * 6.0 - 3.0) * s);
            g.circle(cx, cy, 1.4 * s, TEXT_FAINT, 1.0);
        }
    }
    let mark = g.caps("Klip", Role::Display, 700, 16.0 * s, 0.08);
    let mx = gx + px(25.0);
    let my = ((grip - mark.h) / 2.0).round();
    g.put(&mark, mx, my, TEXT, 1.0);
    let dot = px(5.0);
    let dot_color = match look {
        Some(Look::Buffering) => sc.accent,
        Some(Look::Recording) => RECORDING,
        _ => TEXT_FAINT,
    };
    g.rect(mx + mark.w + px(4.0), (my + mark.h * 0.76).round() - dot, dot, dot, 0.0, dot_color, 1.0);
    // The close button sits left of the cut corner, so the cut never touches it.
    let close = px(22.0);
    let (cx, cy) = (w - px(18.0) - close, ((grip - close) / 2.0).round());
    if hot(Action::ClosePanel) {
        g.rect(cx, cy, close, close, 0.0, SURFACE_3, 1.0);
    }
    let (m, c) = (close / 2.0, 4.5 * s);
    let cross = if hot(Action::ClosePanel) { TEXT } else { TEXT_MUTED };
    g.line(cx + m - c, cy + m - c, cx + m + c, cy + m + c, 1.5 * s, cross, 1.0);
    g.line(cx + m + c, cy + m - c, cx + m - c, cy + m + c, 1.5 * s, cross, 1.0);
    hit(Action::ClosePanel, cx, cy, close, close);
    let hint = g.caps(t(T::DragToMove), Role::Mono, 500, 10.0 * s, 0.12);
    let hx = cx - px(10.0) - hint.w;
    if hx > mx + mark.w + dot + px(16.0) {
        g.put(&hint, hx, (grip - hint.h) / 2.0, TEXT_FAINT, 1.0);
    }
    let mut y = grip + px(14.0);

    // Status line: mark, label, value on the right.
    let (row, sq) = (px(20.0), px(8.0));
    let sq_y = |y: f32, h: f32| y + ((h - sq) / 2.0).round();
    match &sc.status {
        Some(st) => {
            status_mark(g, st, pad, sq_y(y, row), sq);
            let label = g.caps(&st.label, Role::Display, 700, 14.0 * s, 0.06);
            let value = g.caps(&st.value, Role::Mono, 700, 12.0 * s, 0.0);
            g.put(&label, pad + sq + px(10.0), y + (row - label.h) / 2.0, TEXT, 1.0);
            g.put(&value, pad + inner - value.w, y + (row - value.h) / 2.0, TEXT_MUTED, 1.0);
        }
        None => {
            g.rect(pad, sq_y(y, row), sq, sq, 0.0, TEXT_FAINT, 1.0);
            let label = g.caps(t(T::Ready), Role::Display, 700, 14.0 * s, 0.06);
            g.put(&label, pad + sq + px(10.0), y + (row - label.h) / 2.0, TEXT_MUTED, 1.0);
        }
    }
    y += row + px(10.0);

    // Segment bar. It is a state light, not a fill level: lit while Klip works, dark
    // while it does nothing. The overlay does not know how full the buffer is.
    let (seg, seg_gap) = (px(6.0), px(2.0));
    let count = ((inner + seg_gap) / (seg + seg_gap)).floor().max(0.0) as usize;
    let (on, lit) = match look {
        Some(Look::Buffering) => (count, sc.accent),
        Some(Look::Recording) => (count, RECORDING),
        Some(Look::Paused) => (count, TEXT_FAINT),
        None => (0, SURFACE_3),
    };
    g.meter(pad, y, count, on, seg, seg_gap, lit, SURFACE_3);
    y += seg;

    // Buttons: one look, the first one filled with the accent.
    let bh = px(46.0);
    let button = |g: &Gfx, action: Action, y: f32, text: &str, hint: &str, mark: Option<Rgb>, primary: bool| {
        let over = sc.hover == Some(action);
        let (fill, a) = match (primary, over) {
            (true, true) => (sc.accent, 0.86),
            (true, false) => (sc.accent, 1.0),
            (false, true) => (SURFACE_3, 1.0),
            (false, false) => (SURFACE_2, 1.0),
        };
        g.cut(pad, y, inner, bh, px(10.0), fill, a);
        let mut tx = pad + px(16.0);
        if let Some(c) = mark {
            g.rect(tx, sq_y(y, bh), sq, sq, 0.0, c, 1.0);
            tx += sq + px(10.0);
        }
        let hint = g.caps(&keys(hint), Role::Mono, 700, 10.0 * s, 0.06);
        let right = pad + inner - px(16.0);
        let room = right - tx - if hint.w > 0.0 { hint.w + px(10.0) } else { 0.0 };
        let label = g.caps_in(text, Role::Display, 600, 14.0 * s, 0.04, room);
        g.put(&label, tx, y + (bh - label.h) / 2.0, if primary { ON_ACCENT } else { TEXT }, 1.0);
        g.put(&hint, right - hint.w, y + (bh - hint.h) / 2.0, if primary { ON_ACCENT } else { TEXT_MUTED }, 1.0);
    };
    let record = if sc.recording { T::StopRecording } else { T::StartRecording };
    let (mic, mic_mark) = if sc.mic_muted { (T::MicOff, WARNING) } else { (T::MicOn, OK) };
    let rows = [
        (Action::SaveClip, T::SaveClip, sc.hints.save_clip.as_str(), None, true),
        (Action::ToggleRecording, record, sc.hints.record.as_str(), Some(RECORDING), false),
        (Action::Screenshot, T::Screenshot, sc.hints.screenshot.as_str(), None, false),
        (Action::ToggleMic, mic, sc.hints.mic.as_str(), Some(mic_mark), false),
    ];
    for (action, text, hint, mark, primary) in rows {
        y += px(8.0);
        button(g, action, y, t(text), hint, mark, primary);
        hit(action, pad, y, inner, bh);
        y += bh;
    }

    // Recent clips.
    y += px(16.0);
    let heading = g.caps(t(T::Recent), Role::Mono, 500, 10.0 * s, 0.12);
    g.put(&heading, pad, y, TEXT_FAINT, 1.0);
    y += heading.h + px(6.0);
    let rh = px(29.0);
    if sc.clips.is_empty() {
        let none = g.text(t(T::NoClips), Role::Ui, 400, 14.0 * s);
        g.put(&none, pad, y + (rh - none.h) / 2.0, TEXT_FAINT, 1.0);
        y += rh;
    }
    for (i, clip) in sc.clips.iter().enumerate() {
        let bleed = px(8.0);
        if hot(Action::OpenClip(i)) {
            g.rect(pad - bleed, y, inner + bleed * 2.0, rh, 0.0, SURFACE_2, 1.0);
        }
        let length = g.text(&fmt_duration(clip.duration_secs), Role::Mono, 700, 11.0 * s);
        let time = g.text(&clip.time, Role::Mono, 400, 10.0 * s);
        // The length column is fixed so the times line up whatever the length is.
        let column = length.w.max(px(36.0));
        let time_x = pad + inner - column - px(10.0) - time.w;
        g.put(&length, pad + inner - length.w, y + (rh - length.h) / 2.0, TEXT, 1.0);
        g.put(&time, time_x, y + (rh - time.h) / 2.0, TEXT_FAINT, 1.0);
        let title = g.text_in(&clip.title, Role::Ui, 500, 14.0 * s, (time_x - px(10.0) - pad).max(1.0));
        g.put(&title, pad, y + (rh - title.h) / 2.0, TEXT, 1.0);
        hit(Action::OpenClip(i), pad - bleed, y, inner + bleed * 2.0, rh);
        y += rh;
    }

    // Two small buttons.
    y += px(14.0);
    let fh = px(34.0);
    let half = ((inner - px(10.0)) / 2.0).floor();
    for (action, x, text) in
        [(Action::OpenLibrary, pad, T::OpenLibrary), (Action::OpenSettings, pad + inner - half, T::Settings)]
    {
        g.cut(x, y, half, fh, px(8.0), if hot(action) { SURFACE_3 } else { SURFACE_2 }, 1.0);
        let label = g.caps_in(t(text), Role::Mono, 700, 10.0 * s, 0.08, half - px(16.0));
        g.put_centered(&label, x, y, half, fh, TEXT, 1.0);
        hit(action, x, y, half, fh);
    }
    y += fh;
    if !sc.hints.toggle.is_empty() {
        y += px(12.0);
        let key = g.caps(&keys(&sc.hints.toggle), Role::Mono, 700, 10.0 * s, 0.06);
        let rest = g.caps(t(T::ClosesOverlay), Role::Mono, 500, 10.0 * s, 0.12);
        let x = pad + ((inner - key.w - px(8.0) - rest.w) / 2.0).round();
        g.put(&key, x, y, TEXT_MUTED, 1.0);
        g.put(&rest, x + key.w + px(8.0), y, TEXT_FAINT, 1.0);
        y += key.h.max(rest.h);
    }
    (w, y + px(18.0))
}

/// `s` is pixels per key unit here.
fn input(g: &Gfx, sc: &InputScene, u: f32) -> (f32, f32) {
    let look = &sc.layout.appearance;
    // The stock appearance of klip-input still carries the colors and the rounded keys of
    // design version 1. An appearance nobody touched follows the overlay's own theme and
    // accent instead. Anything the user set is taken as it is.
    let c = &look.colors;
    let same = |text: &str, old: &str| text.eq_ignore_ascii_case(old);
    let stock = same(&c.key, "#2B2B27")
        && same(&c.key_pressed, "#C8F03C")
        && same(&c.text, "#F2F1EA")
        && same(&c.text_pressed, "#10120A")
        && same(&c.border, "#2E2E2A");
    let pick = |text: &str, theme: Rgb| if stock { theme } else { Rgb::parse(text).unwrap_or(theme) };
    let key_c = pick(&c.key, SURFACE_3);
    let down_c = pick(&c.key_pressed, sc.accent);
    let text_c = pick(&c.text, TEXT);
    let text_down_c = pick(&c.text_pressed, ON_ACCENT);
    let border_c = pick(&c.border, LINE);
    let square = match look.corner {
        Corner::Square => true,
        Corner::Rounded { radius } => stock && radius == 6.0,
        Corner::Pill => false,
    };
    let gap = if look.gap.is_nan() { 0.08 } else { look.gap.clamp(0.0, 0.4) } * u;
    let k = u / 44.0;
    let radius = |w: f32, h: f32| match look.corner {
        _ if square => 0.0,
        Corner::Square => 0.0,
        Corner::Rounded { radius } => (if radius.is_nan() { 6.0 } else { radius.max(0.0) }) * k,
        Corner::Pill => w.min(h) / 2.0,
    };
    // One key cap: border fill, then the cap inset by one pixel.
    let cap = |x: f32, y: f32, w: f32, h: f32, down: bool| {
        let r = radius(w, h);
        if down {
            g.rect(x, y, w, h, r, down_c, 1.0);
        } else {
            g.frame(x, y, w, h, r, key_c, border_c);
        }
    };
    let caption = |text: &str, x: f32, y: f32, w: f32, h: f32, down: bool| {
        if text.is_empty() {
            return;
        }
        let mut size = u * if text.chars().count() > 3 { 0.22 } else { 0.30 };
        let mut t = g.text(text, Role::Mono, 500, size);
        let room = w - 6.0 * k;
        if t.w > room && t.w > 0.0 {
            size = (size * room / t.w).max(6.0);
            t = g.text(text, Role::Mono, 500, size);
        }
        g.put_centered(&t, x, y, w, h, if down { text_down_c } else { text_c }, 1.0);
    };

    let keys = sc.layout.visible_keys();
    let (mut w, mut h) = (0.0f32, 0.0f32);
    if let Some(b) = bounds(&keys) {
        let px = |v: f32| (v * u).round();
        for key in &keys {
            let down = sc.held.key(key.id);
            let r = key.rect;
            let (x, y) = (px(r.x - b.x) + gap / 2.0, px(r.y - b.y) + gap / 2.0);
            let (kw, kh) = (px(r.right() - b.x) - px(r.x - b.x) - gap, px(r.bottom() - b.y) - px(r.y - b.y) - gap);
            match key.extra {
                // The ISO Enter key: the stem reaches up into the top part so both
                // pieces read as one shape.
                Some(e) => {
                    let ex = px(e.x - b.x) + gap / 2.0;
                    let ew = px(e.right() - b.x) - px(e.x - b.x) - gap;
                    let eh = px(e.bottom() - b.y) - gap / 2.0 - y;
                    let rr = radius(kw, kh);
                    let (fill, edge) = if down { (down_c, down_c) } else { (key_c, border_c) };
                    g.rect(x, y, kw, kh, rr, edge, 1.0);
                    g.rect(ex, y, ew, eh, rr, edge, 1.0);
                    g.rect(x + 1.0, y + 1.0, kw - 2.0, kh - 2.0, (rr - 1.0).max(0.0), fill, 1.0);
                    g.rect(ex + 1.0, y + 1.0, ew - 2.0, eh - 2.0, (rr - 1.0).max(0.0), fill, 1.0);
                }
                None => cap(x, y, kw, kh, down),
            }
            caption(&key.label, x, y, kw, kh, down);
        }
        w = px(b.w);
        h = px(b.h);
    }

    // The mouse, built from the same caps: two buttons, the wheel between them, the
    // body below with the side buttons and the movement mark.
    let m = sc.layout.mouse;
    if m.any() {
        let mx = if w > 0.0 { w + (0.4 * u).round() } else { 0.0 };
        let (mw, top_h, body_h) = ((2.2 * u).round(), (1.3 * u).round(), (1.9 * u).round());
        let mh = top_h + body_h;
        let my = (h - mh).max(0.0);
        let g2 = gap / 2.0;
        let wheel_w = (0.46 * u).round();
        let side = ((mw - wheel_w) / 2.0).floor();
        let down = |b: MouseButton| sc.held.button(b);
        if m.left {
            cap(mx + g2, my + g2, side - gap, top_h - gap, down(MouseButton::Left));
            caption("L", mx + g2, my + g2, side - gap, top_h - gap, down(MouseButton::Left));
        }
        if m.right {
            let x = mx + mw - side;
            cap(x + g2, my + g2, side - gap, top_h - gap, down(MouseButton::Right));
            caption("R", x + g2, my + g2, side - gap, top_h - gap, down(MouseButton::Right));
        }
        if m.middle || m.wheel {
            let (x, y, ww, wh) = (mx + side + g2, my + g2 + 0.12 * u, wheel_w - gap, top_h - gap - 0.24 * u);
            cap(x, y, ww, wh, m.middle && down(MouseButton::Middle));
            // A wheel step lights the half it turns toward.
            if m.wheel && sc.wheel != 0 {
                let half = (wh / 2.0).floor();
                let hy = if sc.wheel > 0 { y } else { y + wh - half };
                g.rect(x, hy, ww, half, radius(ww, wh), down_c, 1.0);
            }
        }
        let (bx, by, bw, bh) = (mx + g2, my + top_h + g2, mw - gap, body_h - gap);
        g.frame(bx, by, bw, bh, if square { 0.0 } else { radius(bw, bh).max(0.3 * u) }, key_c, border_c);
        let (sw, sh) = ((0.34 * u).round(), (0.56 * u).round());
        for (on, button, row) in [(m.side2, MouseButton::Side2, 0.0), (m.side1, MouseButton::Side1, 1.0)] {
            if on {
                let y = by + 0.22 * u + row * (sh + 0.1 * u);
                let r = radius(sw, sh).min(0.1 * u);
                if down(button) {
                    g.rect(bx + 0.16 * u, y, sw, sh, r, down_c, 1.0);
                } else {
                    g.rect(bx + 0.16 * u, y, sw, sh, r, text_c, 0.16);
                }
            }
        }
        if m.movement {
            let (cx, cy, ring) = (bx + bw / 2.0 + 0.12 * u, by + bh / 2.0, 0.42 * u);
            g.ring(cx, cy, ring, 1.0, text_c, 0.22);
            match sc.motion {
                Some(dir) => {
                    let a = dir as f32 * std::f32::consts::FRAC_PI_4;
                    g.circle(cx + a.cos() * ring * 0.62, cy + a.sin() * ring * 0.62, 0.13 * u, down_c, 1.0);
                }
                None => g.circle(cx, cy, 0.1 * u, text_c, 0.45),
            }
        }
        w = mx + mw;
        h = h.max(mh);
    }

    // Rate widgets as small chips below.
    let chips: Vec<String> = [("KPS", sc.kps), ("CPS", sc.cps)]
        .into_iter()
        .filter_map(|(name, v)| v.map(|v| format!("{name} {v}")))
        .collect();
    if !chips.is_empty() {
        let (ch, mut x, y) = ((0.62 * u).round(), gap / 2.0, h + (0.14 * u).round());
        for chip in &chips {
            let t = g.text(chip, Role::Mono, 500, 0.26 * u);
            let cw = t.w + (0.36 * u).round();
            g.frame(x, y, cw, ch, radius(cw, ch), key_c, border_c);
            g.put_centered(&t, x, y, cw, ch, text_c, 1.0);
            x += cw + gap.max(0.08 * u);
        }
        w = w.max(x);
        h = y + ch + gap / 2.0;
    }
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::Fonts;
    use crate::scene::{resolve, ToastItem};
    use crate::state::{ClipEntry, Element, Lang, OverlayState, StatusKind, Toast};
    use klip_input::{LayoutKind, OverlayLayout, Preset, Scope};
    use std::time::Instant;

    fn stack(toasts: &[Toast]) -> Vec<ToastItem> {
        toasts
            .iter()
            .map(|t| ToastItem { title: t.title.clone(), detail: t.detail.clone(), kind: t.kind, alpha: 1.0 })
            .collect()
    }

    fn size_with(state: &OverlayState, e: Element, toasts: &[Toast], hits: &mut Vec<Hit>) -> (f32, f32) {
        let fonts = Fonts::new(&[]).unwrap();
        let r = resolve(state, e, &stack(toasts), None, Instant::now(), 1.0).unwrap();
        r.scene.view(&Gfx::dry(&fonts), r.scale, hits)
    }

    fn size_of(state: &OverlayState, e: Element, hits: &mut Vec<Hit>) -> (f32, f32) {
        size_with(state, e, &[Toast::clip_saved("Last 30 s")], hits)
    }

    #[test]
    fn toasts_stack_downwards_at_one_width() {
        let state = OverlayState::default();
        let one = size_with(&state, Element::Toast, &[Toast::clip_saved("Last 30 s")], &mut Vec::new());
        assert_eq!(one, (330.0, 59.0));
        let bare = size_with(&state, Element::Toast, &[Toast::new("Screenshot saved", "")], &mut Vec::new());
        assert_eq!(bare, (330.0, 43.0));
        let three = [Toast::clip_saved("a"), Toast::recording("Recording started", "b"), Toast::new("Auto clip saved", "c")];
        // Three notices and two gaps of ten pixels.
        assert_eq!(size_with(&state, Element::Toast, &three, &mut Vec::new()), (330.0, 59.0 * 3.0 + 20.0));
    }

    #[test]
    fn grip_is_free_of_buttons_except_close() {
        let mut state = OverlayState::default();
        state.panel.placement.visible = true;
        for lang in [Lang::En, Lang::De] {
            state.language = lang;
            let mut hits = Vec::new();
            let (w, _) = size_of(&state, Element::Panel, &mut hits);
            let grip = grip_height(1.0);
            let in_grip: Vec<Action> = hits.iter().filter(|h| h.y < grip).map(|h| h.action).collect();
            assert_eq!(in_grip, [Action::ClosePanel]);
            // The middle of the grip is a handle, not a button.
            assert!(!hits.iter().any(|h| h.contains(w / 2.0, grip / 2.0)));
            // The close button stays clear of the corner that is cut off (20 pixels).
            let close = hits[0];
            assert!(close.x + close.w - (w - 20.0) < close.y, "{close:?}");
        }
    }

    #[test]
    fn small_elements_stay_small() {
        let state = OverlayState { status: StatusKind::Recording { since: Instant::now() }, ..Default::default() };
        let (w, h) = size_of(&state, Element::Status, &mut Vec::new());
        assert!(h == 28.0 && w > 100.0 && w < 240.0, "{w}x{h}");
        let (w, h) = size_of(&state, Element::Toast, &mut Vec::new());
        assert!(h < 80.0 && w == 330.0, "{w}x{h}");
    }

    #[test]
    fn panel_has_a_hit_area_for_every_action_and_none_overlap() {
        let mut state = OverlayState::default();
        state.panel.placement.visible = true;
        state.panel.clips =
            (0..3).map(|i| ClipEntry { title: format!("Clip {i}"), time: "21:14".into(), duration_secs: 30 }).collect();
        let mut hits = Vec::new();
        let (w, h) = size_of(&state, Element::Panel, &mut hits);
        assert_eq!(w, 380.0);
        let wanted = [
            Action::ClosePanel,
            Action::SaveClip,
            Action::ToggleRecording,
            Action::Screenshot,
            Action::ToggleMic,
            Action::OpenClip(0),
            Action::OpenClip(1),
            Action::OpenClip(2),
            Action::OpenLibrary,
            Action::OpenSettings,
        ];
        assert_eq!(hits.iter().map(|h| h.action).collect::<Vec<_>>(), wanted);
        for (i, a) in hits.iter().enumerate() {
            assert!(a.x >= 0.0 && a.y >= 0.0 && a.x + a.w <= w && a.y + a.h <= h, "{:?} leaves the panel", a.action);
            for b in &hits[i + 1..] {
                let apart = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
                assert!(apart, "{:?} overlaps {:?}", a.action, b.action);
            }
        }
    }

    #[test]
    fn input_size_follows_the_layout() {
        let mut state = OverlayState::default();
        state.input.visible = true;
        state.input.layout = OverlayLayout::new("wasd", LayoutKind::IsoQwertz, Scope::Preset { preset: Preset::Wasd });
        // W over A S D: three units wide, two high, 44 pixels per unit.
        assert_eq!(size_of(&state, Element::Input, &mut Vec::new()), (132.0, 88.0));
        state.input.scale = 2.0;
        assert_eq!(size_of(&state, Element::Input, &mut Vec::new()), (264.0, 176.0));
        // An empty scope without a mouse has nothing to show and still has a size.
        state.input.layout.scope = Scope::Keys { keys: vec![] };
        assert_eq!(size_of(&state, Element::Input, &mut Vec::new()), (1.0, 1.0));
    }
}
