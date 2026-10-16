//! From the caller's state to what one element shows at one moment.
//!
//! A scene is plain data with all texts resolved, so two scenes compare equal exactly
//! when the picture is the same. The window thread redraws only when a scene changed.

use crate::state::{
    fmt_buffer, fmt_clock, Action, ClipEntry, Element, Lang, OverlayState, PanelHints, Placement, StatusKind, ToastKind,
};
use crate::text::{tr, T};
use crate::theme::Rgb;
use klip_input::{Held, OverlayLayout};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    Buffering,
    Recording,
    Paused,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StatusScene {
    pub look: Look,
    pub label: String,
    pub value: String,
    pub accent: Rgb,
}

/// One notice of the stack as it looks right now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToastItem {
    pub title: String,
    pub detail: String,
    pub kind: ToastKind,
    /// Its own fade, 0 to 1.
    pub alpha: f32,
}

/// The stack of notices, oldest first, drawn top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToastScene {
    pub items: Vec<ToastItem>,
    pub accent: Rgb,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InputScene {
    pub layout: OverlayLayout,
    pub held: Held,
    pub wheel: i32,
    pub motion: Option<u8>,
    pub kps: Option<u32>,
    pub cps: Option<u32>,
    pub accent: Rgb,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PanelScene {
    /// `None` while Klip does nothing.
    pub status: Option<StatusScene>,
    pub recording: bool,
    pub mic_muted: bool,
    pub clips: Vec<ClipEntry>,
    pub hints: PanelHints,
    pub sources: Vec<String>,
    pub source: usize,
    pub hover: Option<Action>,
    pub accent: Rgb,
    pub lang: Lang,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Scene {
    Status(StatusScene),
    Toast(ToastScene),
    Input(Box<InputScene>),
    Panel(Box<PanelScene>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Place {
    Anchored(Placement),
    /// Top left corner as a share of the monitor, as the input layouts store it.
    Fraction { x: f32, y: f32 },
    /// Top left corner in pixels relative to the monitor: where the user dragged the panel.
    Fixed { x: i32, y: i32 },
}

impl Place {
    pub fn origin(&self, area: (f32, f32), size: (f32, f32), k: f32) -> (f32, f32) {
        match self {
            Place::Anchored(p) => p.origin(area, size, k),
            Place::Fraction { x, y } => {
                let clean = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
                (
                    (clean(*x) * area.0).min(area.0 - size.0).max(0.0).round(),
                    (clean(*y) * area.1).min(area.1 - size.1).max(0.0).round(),
                )
            }
            Place::Fixed { x, y } => clamp_to_area((*x as f32, *y as f32), area, size),
        }
    }
}

/// Keeps an element of `size` completely on an `area`. An element larger than the area
/// sticks to the top left.
pub(crate) fn clamp_to_area(at: (f32, f32), area: (f32, f32), size: (f32, f32)) -> (f32, f32) {
    (at.0.min(area.0 - size.0).max(0.0).round(), at.1.min(area.1 - size.1).max(0.0).round())
}

/// One element, ready to measure and draw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Resolved {
    pub scene: Scene,
    /// Pixels per design pixel. For the input overlay: pixels per key unit.
    pub scale: f32,
    pub opacity: f32,
    pub place: Place,
}

fn clean_scale(v: f32) -> f32 {
    if v.is_nan() {
        1.0
    } else {
        v.clamp(0.25, 4.0)
    }
}

fn clean_opacity(v: f32) -> f32 {
    if v.is_nan() {
        1.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

fn status_scene(state: &OverlayState, now: Instant) -> Option<StatusScene> {
    let (look, label, value) = match state.status {
        StatusKind::Off => return None,
        StatusKind::Buffering { seconds } => (Look::Buffering, T::BufferActive, fmt_buffer(seconds)),
        StatusKind::Recording { since } => {
            (Look::Recording, T::Recording, fmt_clock(now.saturating_duration_since(since).as_secs()))
        }
        StatusKind::Paused { elapsed } => (Look::Paused, T::Paused, fmt_clock(elapsed.as_secs())),
    };
    Some(StatusScene { look, label: tr(state.language, label).into(), value, accent: state.accent })
}

/// What `element` shows right now, or `None` when it should not be visible.
/// `toasts` are the notices that are on screen right now, oldest first. `k` is monitor
/// height / 1080.
pub(crate) fn resolve(
    state: &OverlayState,
    element: Element,
    toasts: &[ToastItem],
    hover: Option<Action>,
    now: Instant,
    k: f32,
) -> Option<Resolved> {
    let anchored = |p: &Placement, scene: Scene| {
        p.visible.then(|| Resolved {
            scene,
            scale: clean_scale(p.scale) * k,
            opacity: clean_opacity(p.opacity),
            place: Place::Anchored(*p),
        })
    };
    match element {
        Element::Status => {
            // The owner wants no notice for the running buffer at all: the indicator
            // only ever shows a recording.
            if matches!(state.status, StatusKind::Buffering { .. }) {
                return None;
            }
            anchored(&state.indicator, Scene::Status(status_scene(state, now)?))
        }
        Element::Toast => {
            if toasts.is_empty() {
                return None;
            }
            anchored(&state.toast, Scene::Toast(ToastScene { items: toasts.to_vec(), accent: state.accent }))
        }
        Element::Input => {
            let input = &state.input;
            if !input.visible {
                return None;
            }
            let look = &input.layout.appearance;
            let widgets = &input.layout.widgets;
            let unit = if look.unit_px.is_nan() { 44.0 } else { look.unit_px.clamp(8.0, 200.0) };
            let rate = |on: bool, v: Option<f32>| v.filter(|v| on && v.is_finite()).map(|v| v.max(0.0).round() as u32);
            Some(Resolved {
                scene: Scene::Input(Box::new(InputScene {
                    layout: input.layout.clone(),
                    held: input.held.clone(),
                    wheel: input.wheel.signum(),
                    motion: input.motion.filter(|_| input.layout.mouse.movement).map(|d| d % 8),
                    kps: rate(widgets.kps, input.kps),
                    cps: rate(widgets.cps, input.cps),
                    accent: state.accent,
                })),
                scale: unit * clean_scale(input.scale) * k,
                opacity: clean_opacity(look.opacity),
                place: Place::Fraction { x: look.x, y: look.y },
            })
        }
        Element::Panel => {
            let mut resolved = anchored(
                &state.panel.placement,
                Scene::Panel(Box::new(PanelScene {
                    status: status_scene(state, now),
                    recording: matches!(state.status, StatusKind::Recording { .. } | StatusKind::Paused { .. }),
                    mic_muted: state.mic_muted,
                    clips: state.panel.clips.iter().take(5).cloned().collect(),
                    hints: state.panel.hints.clone(),
                    sources: state.panel.sources.iter().take(6).cloned().collect(),
                    source: state.panel.source,
                    hover,
                    accent: state.accent,
                    lang: state.language,
                })),
            )?;
            // A dragged panel stays where the user put it, the anchor no longer counts.
            if let Some((x, y)) = state.panel.position {
                resolved.place = Place::Fixed { x, y };
            }
            Some(resolved)
        }
    }
}

/// True when the picture changes by itself once per second.
pub(crate) fn ticks(state: &OverlayState) -> Option<Instant> {
    match state.status {
        StatusKind::Recording { since } if state.indicator.visible || state.panel.placement.visible => Some(since),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Toast;
    use std::time::Duration;

    fn item(t: &Toast) -> ToastItem {
        ToastItem { title: t.title.clone(), detail: t.detail.clone(), kind: t.kind, alpha: 1.0 }
    }

    #[test]
    fn nothing_is_wanted_by_default() {
        let state = OverlayState::default();
        let now = Instant::now();
        for e in Element::ALL {
            assert!(resolve(&state, e, &[], None, now, 1.0).is_none(), "{e:?}");
        }
    }

    #[test]
    fn status_texts() {
        let now = Instant::now();
        let mut state = OverlayState { status: StatusKind::Buffering { seconds: 30 }, ..Default::default() };
        state.indicator.visible = true;
        // The running buffer never gets an indicator, only the panel names it.
        assert!(resolve(&state, Element::Status, &[], None, now, 1.0).is_none());
        assert_eq!(status_scene(&state, now).unwrap().label, "Buffer active");
        let text = |s: &OverlayState, at: Instant| match resolve(s, Element::Status, &[], None, at, 1.0).unwrap().scene {
            Scene::Status(s) => (s.look, s.label, s.value),
            _ => unreachable!(),
        };
        state.language = Lang::De;
        assert_eq!(status_scene(&state, now).unwrap().label, "Puffer aktiv");
        state.language = Lang::En;
        state.status = StatusKind::Recording { since: now };
        assert_eq!(text(&state, now + Duration::from_secs(75)).2, "01:15");
        // Within one second the scene is the same, so nothing is redrawn.
        let a = resolve(&state, Element::Status, &[], None, now + Duration::from_millis(100), 1.0);
        let b = resolve(&state, Element::Status, &[], None, now + Duration::from_millis(900), 1.0);
        assert_eq!(a, b);
        state.status = StatusKind::Paused { elapsed: Duration::from_secs(9) };
        assert_eq!(text(&state, now), (Look::Paused, "Paused".into(), "00:09".into()));
        // Hidden means not wanted, whatever the status.
        state.indicator.visible = false;
        assert!(resolve(&state, Element::Status, &[], None, now, 1.0).is_none());
    }

    #[test]
    fn toast_needs_a_toast_and_scales_with_the_monitor() {
        let state = OverlayState::default();
        let now = Instant::now();
        assert!(resolve(&state, Element::Toast, &[], None, now, 1.0).is_none());
        let stack = [item(&Toast::clip_saved("Last 30 s")), item(&Toast::recording("Recording started", ""))];
        let r = resolve(&state, Element::Toast, &stack, None, now, 2.0).unwrap();
        assert_eq!(r.scale, 2.0);
        match r.scene {
            Scene::Toast(t) => assert_eq!(t.items.iter().map(|i| i.kind).collect::<Vec<_>>(), [ToastKind::Normal, ToastKind::Recording]),
            _ => unreachable!(),
        }
    }

    #[test]
    fn panel_shows_at_most_five_clips() {
        let mut state = OverlayState::default();
        state.panel.placement.visible = true;
        state.panel.clips =
            (0..9).map(|i| ClipEntry { title: format!("Clip {i}"), time: "21:14".into(), duration_secs: 30 }).collect();
        match resolve(&state, Element::Panel, &[], None, Instant::now(), 1.0).unwrap().scene {
            Scene::Panel(p) => assert_eq!(p.clips.len(), 5),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_dragged_panel_ignores_its_anchor_and_stays_on_the_monitor() {
        let mut state = OverlayState::default();
        state.panel.placement.visible = true;
        let (area, size) = ((1920.0, 1080.0), (380.0, 500.0));
        let place = |s: &OverlayState| resolve(s, Element::Panel, &[], None, Instant::now(), 1.0).unwrap().place;
        assert!(matches!(place(&state), Place::Anchored(_)));
        state.panel.position = Some((700, 120));
        assert_eq!(place(&state).origin(area, size, 1.0), (700.0, 120.0));
        // Caught at every edge.
        state.panel.position = Some((5000, -40));
        assert_eq!(place(&state).origin(area, size, 1.0), (1540.0, 0.0));
        state.panel.position = Some((-3, 9000));
        assert_eq!(place(&state).origin(area, size, 1.0), (0.0, 580.0));
        // The position is in pixels, a larger monitor does not scale it.
        state.panel.position = Some((700, 120));
        assert_eq!(place(&state).origin((3840.0, 2160.0), size, 2.0), (700.0, 120.0));
    }

    #[test]
    fn fraction_place_stays_on_the_monitor() {
        let area = (1920.0, 1080.0);
        assert_eq!(Place::Fraction { x: 0.5, y: 0.5 }.origin(area, (100.0, 100.0), 1.0), (960.0, 540.0));
        assert_eq!(Place::Fraction { x: 2.0, y: f32::NAN }.origin(area, (100.0, 100.0), 1.0), (1820.0, 0.0));
    }
}
